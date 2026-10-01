//! 800x480 RGB (DPI) panel driver with SRAM bounce buffers.
//!
//! Architecture (the reason this file is unusual):
//!
//! The LCD DMA cannot stream the PSRAM framebuffer directly on this chip:
//! any PSRAM write traffic (cache maintenance or dirty-line eviction) into
//! the framebuffer region disturbs the LCD's read stream, its FIFO
//! underruns mid-frame, and the panel re-locks at a new random phase (the
//! image "jumps"; established experimentally - pacing and chunking the
//! writes do not help). ESP-IDF solves this with bounce-buffer mode; so do
//! we:
//!
//! - The DMA streams an endless ring of SRAM segments (6 x 16-line
//!   segments = 96 lines, ~154 KB internal RAM, an exact divisor of the
//!   480-line frame so ring cycles align with frames deterministically).
//! - Each segment's last descriptor raises a DMA EOF interrupt when it has
//!   been fully consumed. The ISR only counts.
//! - `Display::service_bounce` copies the frame lines each segment will
//!   display on its next cycle from the PSRAM framebuffer into the SRAM
//!   segment. PSRAM is only ever *read* for streaming, so CPU drawing
//!   (through the cache, never cleaned) cannot disturb the panel at all.
//!
//! The panel phase is therefore stable for the whole session, and the boot
//! crosshair calibration (main.rs) stays valid.

use esp_hal::{
    dma::{DmaDescriptor, DmaTxBuffer, Preparation, aligned::DmaAlignedMut},
    gpio::Level,
    lcd_cam::{
        LcdCam,
        lcd::{
            ClockMode,
            DelayMode,
            Phase,
            Polarity,
            dpi::{self, Dpi, FrameTiming},
        },
    },
    peripherals::{
        GPIO10, GPIO11, GPIO12, GPIO13, GPIO14, GPIO15, GPIO16, GPIO17, GPIO18, GPIO19, GPIO33,
        GPIO34, GPIO35, GPIO36, GPIO40, GPIO43, GPIO44, GPIO45, GPIO8, GPIO9,
    },
    time::Rate,
};

use crate::board as b;
use core::sync::atomic::{AtomicUsize, Ordering as AtOrdering};

use crate::gfx::{Canvas, Color};

const FB_LEN: usize = b::LCD_H_RES * b::LCD_V_RES * 2;

// --- bounce ring geometry ---------------------------------------------------
// SEGMENTS * SEG_LINES must divide the 480-line frame exactly.
const SEG_LINES: usize = 16;
const SEGMENTS: usize = 6; // ring = 96 lines
const RING_LINES: usize = SEG_LINES * SEGMENTS;
const FRAME_CYCLES: usize = b::LCD_V_RES / RING_LINES; // 480 / 96 = 5

const LINE_BYTES: usize = b::LCD_H_RES * 2;
const SEG_BYTES: usize = LINE_BYTES * SEG_LINES; // 25 600
const MAX_CHUNK: usize = 4032; // multiple of 64, below the 4095 limit
const CHUNKS_PER_SEG: usize = (SEG_BYTES + MAX_CHUNK - 1) / MAX_CHUNK; // 7
const DESC_COUNT: usize = SEGMENTS * CHUNKS_PER_SEG;

// SRAM bounce ring (two copies: the DMA streams one while the refill task
// prepares the other; an atomic descriptor-pointer swap switches them) +
// descriptor storage.
#[repr(align(64))]
struct Segments([[u8; SEG_BYTES]; SEGMENTS]);
static mut SEGMENTS_MEM: [Segments; 2] =
    [Segments([[0; SEG_BYTES]; SEGMENTS]), Segments([[0; SEG_BYTES]; SEGMENTS])];
/// Which copy each segment's descriptors currently point at.
static ACTIVE: [AtomicUsize; SEGMENTS] = {
    // const-init via array repeat on stable for AtomicUsize::new(0)
    #[allow(clippy::declare_interior_mutable_const)]
    const Z: AtomicUsize = AtomicUsize::new(0);
    [Z; SEGMENTS]
};
static mut DESCRIPTORS: [DmaDescriptor; DESC_COUNT] = [DmaDescriptor::EMPTY; DESC_COUNT];

fn seg_ptr(copy: usize, seg: usize) -> *mut u8 {
    unsafe {
        (&raw mut SEGMENTS_MEM)
            .cast::<u8>()
            .add(copy * core::mem::size_of::<Segments>())
            .add(seg * SEG_BYTES)
    }
}

/// Framebuffer base (PSRAM); all drawing happens here.
static FB_PTR: AtomicUsize = AtomicUsize::new(0);

/// ISR -> refill task: number of segments fully streamed so far.
static STREAMED: AtomicUsize = AtomicUsize::new(0);

/// Diagnostics: EOF events observed (expected ~177/s: 5 ring cycles per
/// frame at 35.4 Hz).
pub fn bounce_stats() -> (usize, usize) {
    (
        STREAMED.load(AtOrdering::Relaxed),
        SERVICED.load(AtOrdering::Relaxed),
    )
}
/// Refill task: number of segment completions already handled.
static SERVICED: AtomicUsize = AtomicUsize::new(0);

extern "C" fn lcd_dma_eof_isr() {
    // Clear the Eof flag for the LCD's AXI GDMA channel (DMA_AXI_CH0) and
    // count the finished segment. The 1W write-clear register makes this a
    // single store; safe from an ISR.
    esp_hal::peripherals::AXI_GDMA::regs()
        .out_ch(0)
        .out_int()
        .clr()
        .write(|w| w.out_eof().clear_bit_by_one());
    STREAMED.fetch_add(1, AtOrdering::Relaxed);
}

const LCD_EOF_HANDLER: esp_hal::interrupt::InterruptHandler =
    esp_hal::interrupt::InterruptHandler::new(
        lcd_dma_eof_isr,
        esp_hal::interrupt::Priority::Priority2,
    );

/// Keeps the DMA transfer (and thus the stream) alive for the program's
/// lifetime once started.
static mut TRANSFER: Option<DpiTransfer> = None;

pub struct Display;

impl Display {
    /// `fb` must be a 64-byte aligned PSRAM slice of exactly
    /// `LCD_H_RES * LCD_V_RES * 2` bytes.
    pub fn new(
        lcd_cam: LcdCam<'static, esp_hal::Blocking>,
        channel: impl esp_hal::lcd_cam::LcdDmaTxChannel<'static>,
        fb: &'static mut [u8],
        pins: LcdPins,
    ) -> Result<Self, &'static str> {
        if fb.len() != FB_LEN || fb.as_ptr() as usize % 64 != 0 {
            return Err("framebuffer must be 64-byte aligned and 768000 bytes");
        }

        // HSYNC/VSYNC idle high (active-low pulses). The panel latches data
        // on the falling PCLK edge (BSP `pclk_active_neg = true`).
        let timing = FrameTiming {
            horizontal_total_width: b::LCD_H_RES
                + b::LCD_HSYNC_BACK_PORCH
                + b::LCD_HSYNC_FRONT_PORCH
                + b::LCD_HSYNC_PULSE_WIDTH,
            horizontal_blank_front_porch: b::LCD_HSYNC_BACK_PORCH + b::LCD_HSYNC_PULSE_WIDTH,
            horizontal_active_width: b::LCD_H_RES,
            vertical_total_height: b::LCD_V_RES
                + b::LCD_VSYNC_BACK_PORCH
                + b::LCD_VSYNC_FRONT_PORCH
                + b::LCD_VSYNC_PULSE_WIDTH,
            vertical_blank_front_porch: b::LCD_VSYNC_BACK_PORCH + b::LCD_VSYNC_PULSE_WIDTH,
            vertical_active_height: b::LCD_V_RES,
            vsync_width: b::LCD_VSYNC_PULSE_WIDTH,
            hsync_width: b::LCD_HSYNC_PULSE_WIDTH,
            hsync_position: 0,
        };

        let config = dpi::Config::default()
            .with_frequency(Rate::from_hz(b::LCD_PIXEL_CLOCK_HZ))
            .with_clock_mode(ClockMode {
                polarity: Polarity::IdleLow,
                phase: Phase::ShiftHigh,
            })
            .with_timing(timing)
            .with_format(dpi::Format {
                enable_2byte_mode: true,
                ..Default::default()
            })
            .with_vsync_idle_level(Level::High)
            .with_hsync_idle_level(Level::High)
            .with_de_idle_level(Level::Low)
            .with_disable_black_region(false)
            .with_hs_blank_en(true)
            .with_de_mode(DelayMode::None)
            .with_hsync_mode(DelayMode::None)
            .with_vsync_mode(DelayMode::None)
            .with_output_bit_mode(DelayMode::None);

        let mut dpi = Dpi::new(lcd_cam.lcd, channel, config)
            .map_err(|_| "DPI config rejected (PCLK out of range)")?;

        let mut dpi = dpi
            .with_vsync(pins.vsync)
            .with_hsync(pins.hsync)
            .with_de(pins.de)
            .with_pclk(pins.pclk)
            .with_data0(pins.d0)
            .with_data1(pins.d1)
            .with_data2(pins.d2)
            .with_data3(pins.d3)
            .with_data4(pins.d4)
            .with_data5(pins.d5)
            .with_data6(pins.d6)
            .with_data7(pins.d7)
            .with_data8(pins.d8)
            .with_data9(pins.d9)
            .with_data10(pins.d10)
            .with_data11(pins.d11)
            .with_data12(pins.d12)
            .with_data13(pins.d13)
            .with_data14(pins.d14)
            .with_data15(pins.d15);

        FB_PTR.store(fb.as_ptr() as usize, AtOrdering::Relaxed);

        // Prime the ring with the first frame's content so the panel sees a
        // clean image immediately.
        fill_initial_ring();

        // Bind the EOF handler before the stream starts (local esp-hal
        // patch: set_interrupt_handler_pub / listen_out_pub on ChannelTx).
        #[allow(unused_mut)]
        {
            use esp_hal::dma::DmaTxInterrupt;
            let tx = dpi.tx_channel_mut();
            tx.set_interrupt_handler_pub(LCD_EOF_HANDLER);
            tx.listen_out_pub(DmaTxInterrupt::Eof);
        }

        let ring = unsafe { BounceRing::build() };
        let transfer = dpi.send(true, ring).map_err(|_| "DPI DMA start failed")?;
        unsafe {
            TRANSFER = Some(transfer);
        }

        Ok(Self)
    }

    /// Borrow the framebuffer as a drawing canvas, pre-rotated by the boot
    /// calibration offsets. Drawing becomes visible within ~1-2 frames via
    /// the refill task; no cache maintenance is needed or wanted.
    pub fn canvas(&mut self) -> Canvas<'_> {
        let ptr = FB_PTR.load(AtOrdering::Relaxed) as *mut Color;
        let px: &mut [Color] =
            unsafe { core::slice::from_raw_parts_mut(ptr, FB_LEN / 2) };
        let mut c = Canvas::new(px, b::LCD_H_RES, b::LCD_V_RES);
        c.offset_x = crate::CAL_OFFSET_X.load(core::sync::atomic::Ordering::Relaxed);
        c.offset_y = crate::CAL_OFFSET_Y.load(core::sync::atomic::Ordering::Relaxed);
        c
    }

    /// No-op: retained for call-site compatibility. The bounce ring never
    /// needs cache maintenance.
    pub fn flush(&mut self) {}

    /// No-op, see [`Self::flush`].
    pub fn flush_rects(&mut self, _rects: &[crate::gfx::Rect]) {}

    /// Deprecated handle method: see [`service_bounce`].
    pub fn service_bounce(&mut self) {
        service_bounce()
    }
}

/// Refill any bounce segments that finished streaming since the last call.
///
/// MUST be called at a cadence well under one ring cycle (~2.7 ms); a
/// dedicated task does so (see main.rs). Writing a segment that is
/// mid-stream corrupts the panel rows it carries.
pub fn service_bounce() {
    // Segment EOF sequence drives everything. At each EOF(seg):
    //   1. swap seg's descriptors to the copy pre-filled earlier, and
    //   2. pre-fill its OFF copy for the cycle after next (deadline-free).
    // Copies never touch streaming memory; only the ~7 pointer stores in
    // swap_segment are timing-sensitive, with a ~1.1 ms window.
    const REL: AtOrdering = AtOrdering::Relaxed;
    loop {
        let streamed = STREAMED.load(REL);
        let serviced = SERVICED.load(REL);
        if serviced == streamed {
            return;
        }
        let next = serviced.wrapping_add(1);
        let seg = (next - 1) % SEGMENTS;
        let cycle_after_next = ((next - 1) / SEGMENTS + 2) % FRAME_CYCLES.max(1);
        swap_segment(seg);
        fill_off_segment(seg, cycle_after_next);
        SERVICED.store(next, REL);
    }
}

/// Copy the framebuffer rows a segment displays on `cycle` into its OFF
/// buffer (never the one the DMA is streaming). Deadline-free.
fn fill_off_segment(seg: usize, cycle: usize) {
    // The ring is 96 lines and the frame 480 = 5 cycles: segment s covers
    // rows [s*16 .. s*16+16) shifted by 96*cycle.
    let row = seg * SEG_LINES + cycle * RING_LINES;
    let fb_ptr = FB_PTR.load(AtOrdering::Relaxed) as *const u8;
    if fb_ptr.is_null() {
        return;
    }
    let src: &[u8] =
        unsafe { core::slice::from_raw_parts(fb_ptr.add(row * LINE_BYTES), SEG_BYTES) };
    let off = 1 - ACTIVE[seg].load(AtOrdering::Relaxed);
    // SAFETY: dedicated static segment copy; the DMA reads the other one.
    let dst: &mut [u8] =
        unsafe { core::slice::from_raw_parts_mut(seg_ptr(off, seg), SEG_BYTES) };
    dst.copy_from_slice(src);
}

/// Point a segment's descriptors at its OFF copy (the one just filled).
/// Seven pointer stores; must run between the segment's EOF and its next
/// stream pass (~1.1 ms at the measured 71 Hz scan). If a deadline is in
/// doubt the caller simply skips a cycle - the panel shows stale content
/// for one cycle instead of torn data.
fn swap_segment(seg: usize) {
    let from = ACTIVE[seg].load(AtOrdering::Relaxed);
    let to = 1 - from;
    let ring_base: *mut DmaDescriptor = unsafe { (&raw mut DESCRIPTORS).cast() };
    for c in 0..CHUNKS_PER_SEG {
        // SAFETY: descriptor fields for this segment are only touched here,
        // inside the segment's post-EOF window.
        let d = unsafe { &mut *ring_base.add(seg * CHUNKS_PER_SEG + c) };
        d.buffer = unsafe { seg_ptr(to, seg).add(c * MAX_CHUNK) };
    }
    ACTIVE[seg].store(to, AtOrdering::Relaxed);
}

/// Fill all segments with the frame content they show on cycle 0.
fn fill_initial_ring() {
    for seg in 0..SEGMENTS {
        let off = 1 - ACTIVE[seg].load(AtOrdering::Relaxed);
        let row = seg * SEG_LINES;
        let fb_ptr = FB_PTR.load(AtOrdering::Relaxed) as *const u8;
        if fb_ptr.is_null() {
            return;
        }
        let src: &[u8] = unsafe {
            core::slice::from_raw_parts(fb_ptr.add(row * LINE_BYTES), SEG_BYTES)
        };
        let dst: &mut [u8] =
            unsafe { core::slice::from_raw_parts_mut(seg_ptr(off, seg), SEG_BYTES) };
        dst.copy_from_slice(src);
        swap_segment(seg);
    }
}

type DpiTransfer =
    esp_hal::lcd_cam::lcd::dpi::DpiTransfer<'static, BounceRing, esp_hal::Blocking>;

pub struct LcdPins {
    pub vsync: GPIO45<'static>,
    pub hsync: GPIO44<'static>,
    pub de: GPIO43<'static>,
    pub pclk: GPIO40<'static>,
    pub d0: GPIO8<'static>,
    pub d1: GPIO9<'static>,
    pub d2: GPIO10<'static>,
    pub d3: GPIO11<'static>,
    pub d4: GPIO12<'static>,
    pub d5: GPIO13<'static>,
    pub d6: GPIO14<'static>,
    pub d7: GPIO15<'static>,
    pub d8: GPIO16<'static>,
    pub d9: GPIO17<'static>,
    pub d10: GPIO18<'static>,
    pub d11: GPIO19<'static>,
    pub d12: GPIO33<'static>,
    pub d13: GPIO34<'static>,
    pub d14: GPIO35<'static>,
    pub d15: GPIO36<'static>,
}

/// Endless ring over the SRAM segments; each segment's final descriptor
/// carries `suc_eof` so finishing it raises the DMA EOF interrupt.
struct BounceRing {
    descriptors: DmaAlignedMut<'static, [DmaDescriptor]>,
}

impl BounceRing {
    /// # Safety
    /// Requires exclusive access to the static descriptor storage; rebuilds
    /// the ring verbatim.
    unsafe fn build() -> Self {
        let descriptors: &'static mut [DmaDescriptor] = unsafe {
            core::slice::from_raw_parts_mut(
                (&raw mut DESCRIPTORS).cast::<DmaDescriptor>(),
                DESC_COUNT,
            )
        };
        let mut desc = unsafe { DmaAlignedMut::new_unchecked(descriptors) };

        let ring_base: *mut DmaDescriptor = (&raw mut DESCRIPTORS).cast();
        for seg in 0..SEGMENTS {
            let seg_ptr: *mut u8 = seg_ptr(0, seg);
            for c in 0..CHUNKS_PER_SEG {
                let idx = seg * CHUNKS_PER_SEG + c;
                let offset = c * MAX_CHUNK;
                let chunk = MAX_CHUNK.min(SEG_BYTES - offset);
                let d = &mut desc[idx];
                d.set_size(chunk);
                d.set_length(chunk);
                // EOF on the last descriptor of every segment drives the
                // refill interrupt.
                d.set_suc_eof(c + 1 == CHUNKS_PER_SEG);
                d.set_owner(esp_hal::dma::Owner::Dma);
                d.buffer = unsafe { seg_ptr.add(offset) };
                d.next = unsafe { ring_base.add(idx + 1) };
            }
        }
        // Close the ring.
        unsafe {
            (*ring_base.add(DESC_COUNT - 1)).next = ring_base;
        }

        Self { descriptors: desc }
    }
}

unsafe impl DmaTxBuffer for BounceRing {
    type View = ();
    type Final = ();

    fn prepare(&mut self) -> Preparation {
        Preparation {
            start: self.descriptors.as_mut_ptr(),
            accesses_psram: false,
            burst_transfer: esp_hal::dma::BurstConfig {
                external_memory: esp_hal::dma::ExternalBurstConfig::Size64,
                internal_memory: esp_hal::dma::InternalBurstConfig::Enabled,
            },
            check_owner: Some(false),
            auto_write_back: false,
        }
    }

    fn into_view(self) {}
    fn from_view((): ()) {}
}
