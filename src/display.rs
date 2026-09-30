//! 800x480 RGB (DPI) panel driver with a PSRAM framebuffer refreshed by a
//! self-linked DMA descriptor ring.
//!
//! The stock ESP-IDF driver refreshes the panel from PSRAM continuously
//! (`fb_in_psram`). esp-hal's generic `DmaTxBuf` ends after the last
//! descriptor, so we implement [`DmaTxBuffer`] ourselves with a ring:
//! the last descriptor points back to the first and the LCD controller's
//! "next frame" mode keeps scanning forever.

use esp_hal::{
    dma::{DmaDescriptor, DmaTxBuffer, Preparation, aligned::DmaAlignedMut},
    gpio::Level,
    peripherals::{
        GPIO10, GPIO11, GPIO12, GPIO13, GPIO14, GPIO15, GPIO16, GPIO17, GPIO18, GPIO19, GPIO33,
        GPIO34, GPIO35, GPIO36, GPIO40, GPIO43, GPIO44, GPIO45, GPIO8, GPIO9,
    },

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
    time::Rate,
};

use crate::board as b;
use crate::gfx::{Canvas, Color};

/// 800*480*2 bytes; split into <=4092-byte chunks => 188 descriptors.
const FB_LEN: usize = b::LCD_H_RES * b::LCD_V_RES * 2;
const MAX_CHUNK: usize = 4092;
const DESC_COUNT: usize = (FB_LEN + MAX_CHUNK - 1) / MAX_CHUNK;

pub struct Display {
    fb: &'static mut [u8], // owned while no transfer is running
}

impl Display {
    /// `fb` must be a 64-byte aligned PSRAM slice of exactly
    /// `LCD_H_RES * LCD_V_RES * 2` bytes.
    pub fn new(
        lcd_cam: LcdCam<'static, esp_hal::Blocking>,
        channel: impl esp_hal::lcd_cam::LcdDmaTxChannel<'static>,
        fb: &'static mut [u8],
        pins: LcdPins,
    ) -> Result<(Self, DpiTransferHandle), &'static str> {
        if fb.len() != FB_LEN || fb.as_ptr() as usize % 64 != 0 {
            return Err("framebuffer must be 64-byte aligned and 768000 bytes");
        }

        // HSYNC/VSYNC active-low pulses, data clocked on the falling PCLK
        // edge (matches `pclk_active_neg = true` in the stock BSP).
        let timing = FrameTiming {
            horizontal_total_width: b::LCD_H_RES
                + b::LCD_HSYNC_BACK_PORCH
                + b::LCD_HSYNC_FRONT_PORCH,
            horizontal_blank_front_porch: b::LCD_HSYNC_BACK_PORCH + b::LCD_HSYNC_PULSE_WIDTH,
            horizontal_active_width: b::LCD_H_RES,
            vertical_total_height: b::LCD_V_RES + b::LCD_VSYNC_BACK_PORCH + b::LCD_VSYNC_FRONT_PORCH,
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
                phase: Phase::ShiftLow,
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

        let dpi = Dpi::new(lcd_cam.lcd, channel, config)
            .map_err(|_| "DPI config rejected (PCLK out of range)")?;

        let dpi = dpi
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

        static mut DESCRIPTORS: [DmaDescriptor; DESC_COUNT] =
            [DmaDescriptor::EMPTY; DESC_COUNT];

        // SAFETY: `DESCRIPTORS` is a dedicated static, only ever used by this
        // call path, and `fb` is an owned 'static slice handed in by the caller.
        let descriptors: &'static mut [DmaDescriptor] = unsafe {
            core::slice::from_raw_parts_mut(
                (&raw mut DESCRIPTORS) as *mut DmaDescriptor,
                DESC_COUNT,
            )
        };
        let ring = unsafe { FrameRing::build(descriptors, fb) }
            .map_err(|_| "descriptor ring build failed")?;

        // Paint a black frame before the panel starts scanning.
        ring.fb.fill(0);

        // Keep the ring buffer's data pointer for later drawing; the ring
        // takes ownership of the slice.
        let fb_ptr: *mut u8 = ring.fb.as_mut_ptr();

        let transfer = dpi.send(true, ring).map_err(|_| "DPI DMA start failed")?;

        Ok((
            Self {
                fb: unsafe { core::slice::from_raw_parts_mut(fb_ptr, FB_LEN) },
            },
            DpiTransferHandle { transfer },
        ))
    }

    /// Borrow the visible framebuffer as a pixel canvas.
    ///
    /// The panel scans this memory concurrently; call [`Self::flush`] after
    /// drawing to push CPU writes out of the data cache.
    pub fn canvas(&mut self) -> Canvas<'_> {
        let px: &mut [Color] =
            unsafe { core::slice::from_raw_parts_mut(self.fb.as_mut_ptr() as *mut Color, FB_LEN / 2) };
        Canvas::new(px, b::LCD_H_RES, b::LCD_V_RES)
    }

    /// Clean the data cache covering the framebuffer so the DMA engine
    /// observes the latest CPU writes.
    pub fn flush(&mut self) {
        // SAFETY: the wrapper only observes the exclusively-owned
        // framebuffer region; writeback is a pure cache clean.
        let mut aligned: DmaAlignedMut<'_, [u8]> = unsafe {
            DmaAlignedMut::new_unchecked(&mut *self.fb)
        };
        aligned.writeback();
    }
}

/// Transparent handle keeping the continuous DMA transfer alive.
pub struct DpiTransferHandle {
    #[allow(dead_code)]
    transfer: DpiTransfer,
}

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

/// A descriptor ring over a large buffer. Unlike [`esp_hal::dma::buffers::
/// DmaLoopBuf`] (single descriptor, <=4092 bytes) this chains N descriptors
/// and links the last back to the first, giving an endlessly repeating
/// transfer suitable for a framebuffer.
struct FrameRing {
    descriptors: DmaAlignedMut<'static, [DmaDescriptor]>,
    fb: &'static mut [u8],
}

impl FrameRing {
    /// # Safety
    /// `descriptors` must be a mutable, exclusively-owned static of at least
    /// `fb.len().div_ceil(4092)` entries in internal RAM, and `fb` must stay
    /// valid and unmodified-in-ownership for the lifetime of the transfer.
    unsafe fn build(
        descriptors: &'static mut [DmaDescriptor],
        fb: &'static mut [u8],
    ) -> Result<Self, &'static mut [u8]> {
        if descriptors.len() < fb.len().div_ceil(MAX_CHUNK) {
            return Err(fb);
        }
        // SAFETY: caller guarantees the static descriptor array's alignment
        // (DmaDescriptor's own alignment satisfies the DMA requirement).
        let mut desc = unsafe { DmaAlignedMut::new_unchecked(descriptors) };
        let mut offset = 0;
        let mut idx = 0;
        let base = desc.as_mut_ptr();
        while offset < fb.len() {
            let chunk = MAX_CHUNK.min(fb.len() - offset);
            let d = &mut desc[idx];
            d.set_size(chunk);
            d.set_length(chunk);
            d.set_suc_eof(false);
            // Owner stays "DMA" forever; check_owner is disabled below.
            d.set_owner(esp_hal::dma::Owner::Dma);
            d.buffer = unsafe { fb.as_mut_ptr().add(offset) };
            d.next = unsafe { base.add(idx + 1) };
            offset += chunk;
            idx += 1;
        }
        // Close the ring.
        unsafe {
            (*base.add(idx - 1)).next = base;
        }
        Ok(Self {
            descriptors: desc,
            fb,
        })
    }
}

unsafe impl DmaTxBuffer for FrameRing {
    type View = ();
    type Final = ();

    fn prepare(&mut self) -> Preparation {
        // The framebuffer lives in PSRAM: flush it so the first frame the DMA
        // reads matches what the CPU wrote.
        let mut aligned: DmaAlignedMut<'_, [u8]> = unsafe {
            DmaAlignedMut::new_unchecked(&mut *self.fb)
        };
        aligned.writeback();
        Preparation {
            start: self.descriptors.as_mut_ptr(),
            // The framebuffer lives in PSRAM and the LCD_CAM uses the
            // AXI GDMA engine, which can access it.
            accesses_psram: true,
            burst_transfer: Default::default(),
            check_owner: Some(false),
            auto_write_back: false,
        }
    }

    fn into_view(self) {}
    fn from_view((): ()) {}
}
