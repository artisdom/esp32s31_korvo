//! 800x480 RGB (DPI) panel driver: full-frame PSRAM ring.
//!
//! The ST7262E43 panel locks to the DPI stream with a random phase each
//! boot; a touch on the boot crosshair measures it (main.rs). A DMA
//! descriptor ring over the whole 768 KB PSRAM framebuffer scans
//! continuously. CPU drawing goes through the cache and reaches the panel
//! via natural eviction — updates take effect within a few seconds, but
//! the display is always stable (any explicit cache maintenance or
//! PSRAM write burst disturbs the LCD DMA stream on this chip and makes
//! the panel re-lock at a new phase).

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
use crate::gfx::{Canvas, Color};

const FB_LEN: usize = b::LCD_H_RES * b::LCD_V_RES * 2;
const MAX_CHUNK: usize = 4032;
const DESC_COUNT: usize = (FB_LEN + MAX_CHUNK - 1) / MAX_CHUNK;

static mut DESCRIPTORS: [DmaDescriptor; DESC_COUNT] = [DmaDescriptor::EMPTY; DESC_COUNT];
static FB_PTR: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

static mut TRANSFER: Option<DpiTransfer> = None;

pub struct Display;

impl Display {
    pub fn new(
        lcd_cam: LcdCam<'static, esp_hal::Blocking>,
        channel: impl esp_hal::lcd_cam::LcdDmaTxChannel<'static>,
        fb: &'static mut [u8],
        pins: LcdPins,
    ) -> Result<Self, &'static str> {
        if fb.len() != FB_LEN || fb.as_ptr() as usize % 64 != 0 {
            return Err("framebuffer must be 64-byte aligned and 768000 bytes");
        }

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
            .map_err(|_| "DPI config rejected")?;

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

        FB_PTR.store(fb.as_ptr() as usize, core::sync::atomic::Ordering::Relaxed);

        // Bind the EOF handler for wire mode (esp-hal local patch).
        {
            use esp_hal::dma::DmaTxInterrupt;
            let tx = dpi.tx_channel_mut();
            tx.set_interrupt_handler_pub(LCD_EOF_HANDLER);
            tx.listen_out_pub(DmaTxInterrupt::Eof);
        }

        let ring = match unsafe { FrameRing::build((&raw mut DESCRIPTORS).cast(), fb) } {
            Ok(r) => r,
            Err(_) => return Err("descriptor ring build failed"),
        };
        let transfer = dpi.send(true, ring).map_err(|_| "DPI DMA start failed")?;
        unsafe { TRANSFER = Some(transfer); }

        Ok(Self)
    }

    pub fn canvas(&mut self) -> Canvas<'_> {
        let ptr = FB_PTR.load(core::sync::atomic::Ordering::Relaxed) as *mut Color;
        let px: &mut [Color] = unsafe { core::slice::from_raw_parts_mut(ptr, FB_LEN / 2) };
        let mut c = Canvas::new(px, b::LCD_H_RES, b::LCD_V_RES);
        c.offset_x = crate::CAL_OFFSET_X.load(core::sync::atomic::Ordering::Relaxed);
        c.offset_y = crate::CAL_OFFSET_Y.load(core::sync::atomic::Ordering::Relaxed);
        c
    }

    /// Clean the whole framebuffer in small chunks. Only for page changes
    /// and rare full redraws; frequent use disturbs the LCD DMA stream.
    pub fn flush(&mut self) {
        const CHUNK: usize = 4096;
        let ptr = FB_PTR.load(core::sync::atomic::Ordering::Relaxed) as *mut u8;
        let mut off = 0;
        while off < FB_LEN {
            let len = CHUNK.min(FB_LEN - off);
            let mut region: DmaAlignedMut<'_, [u8]> = unsafe {
                DmaAlignedMut::new_unchecked(core::slice::from_raw_parts_mut(
                    ptr.add(off), len))
            };
            region.writeback();
            off += len;
        }
    }

    pub fn flush_rects(&mut self, rects: &[crate::gfx::Rect]) {
        const LINE: usize = b::LCD_H_RES * 2;
        for r in rects {
            let x0 = (r.x * 2) & !63;
            let x1 = ((r.x + r.w) * 2 + 63) & !63;
            let start = r.y * LINE + x0;
            let end = ((r.y + r.h) * LINE + x1).min(FB_LEN);
            if end <= start { continue; }
            let ptr = FB_PTR.load(core::sync::atomic::Ordering::Relaxed) as *mut u8;
            let mut region: DmaAlignedMut<'_, [u8]> = unsafe {
                DmaAlignedMut::new_unchecked(core::slice::from_raw_parts_mut(
                    ptr.add(start), end - start))
            };
            region.writeback();
        }
    }

    pub fn service_bounce(&mut self) { /* no-op with full-frame ring */ }
}

pub fn bounce_stats() -> (usize, usize) { (0, 0) }
pub fn eof_desc_trace() -> [usize; 32] { [usize::MAX; 32] }
pub fn desc_base() -> usize { 0 }

extern "C" fn lcd_dma_eof_isr() {
    // Clear the EOF flag (W1C).
    esp_hal::peripherals::AXI_GDMA::regs()
        .out_ch(0)
        .out_int()
        .clr()
        .write(|w| w.out_eof().clear_bit_by_one());
}

const LCD_EOF_HANDLER: esp_hal::interrupt::InterruptHandler =
    esp_hal::interrupt::InterruptHandler::new(lcd_dma_eof_isr, esp_hal::interrupt::Priority::Priority2);

type DpiTransfer =
    esp_hal::lcd_cam::lcd::dpi::DpiTransfer<'static, FrameRing, esp_hal::Blocking>;

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

/// Self-linked descriptor ring over the whole PSRAM framebuffer.
struct FrameRing {
    descriptors: DmaAlignedMut<'static, [DmaDescriptor]>,
}

impl FrameRing {
    unsafe fn build(
        descriptors: *mut [DmaDescriptor; DESC_COUNT],
        fb: &'static mut [u8],
    ) -> Result<Self, &'static mut [u8]> {
        let descriptors: &'static mut [DmaDescriptor] = unsafe {
            core::slice::from_raw_parts_mut(descriptors as *mut DmaDescriptor, DESC_COUNT)
        };
        let mut desc = unsafe { DmaAlignedMut::new_unchecked(descriptors) };
        let ring_base = desc.as_mut_ptr();
        let mut offset = 0;
        let mut idx = 0;
        while offset < fb.len() {
            let chunk = MAX_CHUNK.min(fb.len() - offset);
            let d = &mut desc[idx];
            d.set_size(chunk);
            d.set_length(chunk);
            d.set_suc_eof(false);
            d.set_owner(esp_hal::dma::Owner::Dma);
            d.buffer = unsafe { fb.as_mut_ptr().add(offset) };
            d.next = unsafe { ring_base.add(idx + 1) };
            offset += chunk;
            idx += 1;
        }
        unsafe {
            (*ring_base.add(idx - 1)).next = ring_base;
        }
        Ok(Self { descriptors: desc })
    }
}

unsafe impl DmaTxBuffer for FrameRing {
    type View = ();
    type Final = ();

    fn prepare(&mut self) -> Preparation {
        let fb = FB_PTR.load(core::sync::atomic::Ordering::Relaxed) as *mut u8;
        let mut region: DmaAlignedMut<'_, [u8]> = unsafe {
            DmaAlignedMut::new_unchecked(
                core::slice::from_raw_parts_mut(fb, FB_LEN))
        };
        region.writeback();
        Preparation {
            start: self.descriptors.as_mut_ptr(),
            accesses_psram: true,
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
