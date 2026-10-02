//! ES8389 audio codec control over I2C — register-level port of
//! espressif/esp_codec_dev `es8389.c`, in the configuration used by the
//! Korvo BSP: I2C 0x20, I2S slave, no MCLK (clocks derived from SCLK),
//! 48 kHz / 16-bit / I2S-Philips.
//!
//! The sequence below mirrors `es8389_open` followed by `es8389_set_fs`
//! (sample-rate coefficient block, bit-width/format, bias cycle).

use crate::board::{ES8389_I2C_ADDR as ADDR, SharedI2c};

// Data format / bit values (es8389_reg.h)
const S16_LE: u8 = 3 << 5;
const DAIFMT_I2S: u8 = 0 << 2;
const MASK_DATALEN: u8 = 7 << 5;
const MASK_DAIFMT: u8 = 7 << 2;
const MASK_MS_MODE: u8 = 1 << 0;

pub struct Es8389 {
    i2c: &'static SharedI2c,
    pa_pin: Option<esp_hal::gpio::Output<'static>>,
}

pub type CodecResult<T> = Result<T, ()>;

impl Es8389 {
    pub fn new(i2c: &'static SharedI2c, pa_pin: Option<esp_hal::gpio::Output<'static>>) -> Self {
        Self { i2c, pa_pin }
    }

    fn wr(&mut self, reg: u8, val: u8) -> CodecResult<()> {
        self.i2c
            .lock(|i2c| i2c.borrow_mut().write(ADDR, &[reg, val]))
            .map_err(|_| ())
    }

    fn rd(&mut self, reg: u8) -> CodecResult<u8> {
        let mut buf = [0u8; 1];
        match self
            .i2c
            .lock(|i2c| i2c.borrow_mut().write_read(ADDR, &[reg], &mut buf))
        {
            Ok(()) => Ok(buf[0]),
            Err(_) => Err(()),
        }
    }

    fn upd(&mut self, reg: u8, mask: u8, val: u8) -> CodecResult<()> {
        let mut v = self.rd(reg)?;
        v &= !mask;
        v |= val & mask;
        self.wr(reg, v)
    }

    /// Chip responds on the bus?
    pub fn ping(&mut self) -> bool {
        // Register 0x00 resets on write, so only read; a responding device
        // ACKs the address phase even if the value is meaningless.
        self.rd(0x00).is_ok()
    }

    /// Read both chip-ID registers (0xFD/0xFE).
    pub fn chip_id(&mut self) -> CodecResult<(u8, u8)> {
        Ok((self.rd(0xFD)?, self.rd(0xFE)?))
    }

    /// Full power-up: `es8389_open` + `es8389_set_fs(48 kHz, 16 bit, I2S)`.
    pub fn init_48k(&mut self) -> CodecResult<()> {
        // ---- es8389_open ----
        self.wr(0xF3, 0x00)?; // isolation off
        self.wr(0x00, 0x7E)?; // reset
        self.wr(0xF3, 0x38)?;
        self.wr(0x24, 0x64)?; // ADC input
        self.wr(0x25, 0x04)?; // PGA default
        self.wr(0x45, 0x03)?;
        self.wr(0x60, 0x2A)?; // VMID
        self.wr(0x61, 0xC9)?; // analog bias
        self.wr(0x62, 0x4F)?;
        self.wr(0x63, 0x06)?;
        self.wr(0x6B, 0x00)?;
        self.wr(0x6D, 0x16)?;
        self.wr(0x6E, 0xAA)?;
        self.wr(0x6F, 0x66)?;
        self.wr(0x70, 0x99)?;
        self.wr(0x23, 0x00)?; // ALC off
        self.wr(0x72, 1 << 4)?; // PGA1 gain scale
        self.wr(0x73, 1 << 4)?; // PGA2 gain scale
        self.wr(0x10, 0xC4)?; // clock divider
        self.wr(0x01, 0x08)?; // misc
        self.wr(0xF1, 0x00)?; // CSM state
        for reg in [0x12, 0x13, 0x14, 0x15] {
            self.wr(reg, 0x01)?; // power up all blocks
        }
        self.wr(0x16, 0x35)?;
        self.wr(0x17, 0x09)?;
        self.wr(0x18, 0x91)?;
        self.wr(0x19, 0x28)?;
        self.wr(0x1A, 0x01)?;
        self.wr(0x1B, 0x01)?;
        self.wr(0x1C, 0x11)?;
        self.wr(0x2A, 0x00)?;
        self.wr(0x20, S16_LE | DAIFMT_I2S)?; // ADC: 16-bit I2S
        self.wr(0x40, S16_LE | DAIFMT_I2S)?; // DAC: 16-bit I2S
        self.wr(0xF0, 0x01)?;
        self.wr(0x02, 0x00)?; // clock manager: ext MCLK src (overridden below)
        self.wr(0x04, 0x00)?;
        self.wr(0x05, 0x10)?;
        self.wr(0x06, 0x00)?;
        self.wr(0x07, 0xC0)?;
        self.wr(0x08, 0x00)?;
        self.wr(0x09, 0xC0)?;
        self.wr(0x0A, 0x80)?;
        self.wr(0x0B, 4)?; // BCLK divider
        self.wr(0x0C, (256 >> 8) as u8)?; // LRCLK divider
        self.wr(0x0D, (256 & 0xFF) as u8)?;
        self.wr(0x0F, 0x10)?;
        self.wr(0x21, 0x1F)?;
        self.wr(0x22, 0x7F)?;
        self.wr(0x2F, 0xC0)?;
        self.wr(0x30, 0xF4)?; // DAC oversampling
        self.wr(0x31, 0x00)?;
        self.wr(0x44, 0x00)?;
        self.wr(0x41, 0x7F)?; // DAC mixer
        self.wr(0x42, 0x7F)?;
        self.wr(0x43, 0x10)?;
        self.wr(0x49, 0x0F)?;
        self.wr(0x4C, 0xC0)?;
        self.wr(0x00, 0x00)?; // release reset
        self.wr(0x03, 0xC1)?; // chip status: power on clocks
        self.wr(0x00, 0x01)?; // "start" bit
        self.wr(0x4D, 0x02)?;
        // Default gains (0 dB): ADC digital, ADC volumes, DAC volume
        self.wr(0x26, 0xBF)?;
        self.wr(0x27, 0xBF)?;
        self.wr(0x28, 0xBF)?;
        self.wr(0x46, 0xBF)?;
        self.wr(0x47, 0xBF)?;
        self.wr(0x48, 95 << 1)?;

        // ---- configuration tail of es8389_open ----
        self.upd(0x01, MASK_MS_MODE, 0x00)?; // slave mode
        self.upd(0x02, 0xC0, 1 << 6)?; // clock from SCLK (no MCLK)
        self.upd(0x02, 0x02, 0x00)?; // MCLK not inverted
        self.wr(0xF0, 0x12)?; // both ADC microphone channels; no DAC reference
        self.upd(0x02, 0x01, 0x00)?; // SCLK not inverted

        // ---- es8389_set_fs(48000, 16) ----
        // Coefficient row {ratio 32, MCLK 1.536 MHz (=SCLK)}:
        self.wr(0x04, 0x00)?;
        self.wr(0x05, 0x45)?;
        self.wr(0x06, 0xA4)?;
        self.wr(0x07, 0xD0)?;
        self.wr(0x08, 0x10)?;
        self.wr(0x09, 0xD1)?;
        self.wr(0x0A, 0x80)?;
        self.upd(0x0F, 0xC0, 0x00)?;
        self.wr(0x11, 0x00)?;
        self.wr(0x21, 0x1F)?;
        self.wr(0x22, 0x7F)?;
        self.wr(0x26, 0xBF)?;
        self.upd(0x30, 0xC0, 0xC0)?;
        self.wr(0x41, 0x7F)?;
        self.wr(0x42, 0x7F)?;
        self.upd(0x43, 0x81, 0x00)?;
        self.upd(0xF0, 0x73, 0x12)?; // 48 kHz / ratio-32 coefficient row
        self.wr(0xF1, 0x00)?;
        self.wr(0x16, 0x35)?;
        self.wr(0x18, 0x91)?;
        self.wr(0x19, 0x28)?;
        // set_bits(16) + config_fmt(I2S)
        self.upd(0x20, MASK_DATALEN, S16_LE)?;
        self.upd(0x40, MASK_DATALEN, S16_LE)?;
        self.upd(0x0C, 0xE0, 0x00)?;
        self.upd(0x20, MASK_DAIFMT, DAIFMT_I2S)?;
        self.upd(0x40, MASK_DAIFMT, DAIFMT_I2S)?;

        // bias standby -> bias on
        self.upd(0x40, 0x03, 0x03)?;
        self.wr(0x10, 0xD4)?;
        self.delay_ms(70);
        self.wr(0x61, 0x59)?;
        self.wr(0x64, 0x00)?;
        self.wr(0x03, 0x00)?;
        self.wr(0x00, 0x7E)?;
        self.upd(0x40, 0x03, 0x00)?;

        self.wr(0x4D, 0x02)?;
        self.upd(0x69, 0x20, 0x20)?;
        self.wr(0x61, 0xD9)?;
        self.wr(0x64, 0x8F)?;
        self.wr(0x10, 0xE4)?;
        self.wr(0x00, 0x01)?;
        self.wr(0x03, 0xC3)?;
        self.wr(0x24, 0x6A)?; // ADC input (post-bias)
        self.wr(0x25, 0x0A)?; // PGA

        self.set_pa(true);
        Ok(())
    }

    /// Speaker volume in dB, -95.5..=32 (vendor curve: reg = (db+95.5)/127.5*255).
    pub fn set_volume_db(&mut self, db: f32) -> CodecResult<()> {
        let db = db.clamp(-95.5, 32.0);
        let reg = (((db + 95.5) / 127.5 * 255.0) + 0.5) as u8;
        self.wr(0x46, reg)?;
        self.wr(0x47, reg)
    }

    /// ADC (microphone) mute, vendor semantics.
    pub fn set_adc_mute(&mut self, mute: bool) -> CodecResult<()> {
        let mut v = self.rd(0x20)?;
        v &= 0xFC;
        if mute {
            v |= 0x03;
        }
        self.wr(0x20, v)
    }

    /// Microphone PGA gain in dB (vendor quantization, 0..=36.5 dB).
    pub fn set_mic_gain(&mut self, db: u8) -> CodecResult<()> {
        let gain = match db {
            0 => 0x00,       // 0 dB
            1..=5 => 0x01,   // 3.5 dB
            6..=8 => 0x02,   // 6.5 dB
            9..=11 => 0x03,  // 9.5 dB
            12..=14 => 0x04, // 12.5 dB
            15..=17 => 0x05, // 15.5 dB
            18..=20 => 0x06, // 18.5 dB
            21..=23 => 0x07, // 21.5 dB
            24..=26 => 0x08, // 24.5 dB
            27..=29 => 0x09, // 27.5 dB
            30..=32 => 0x0A, // 30.5 dB
            33..=35 => 0x0B, // 33.5 dB
            _ => 0x0C,       // 36.5 dB
        };
        self.wr(0x72, gain | (3 << 4))?;
        self.wr(0x73, gain | (3 << 4))
    }

    /// NS4150B class-D power-amplifier enable (GPIO7, active-high).
    pub fn set_pa(&mut self, on: bool) {
        if let Some(pa) = self.pa_pin.as_mut() {
            pa.set_level(on.into());
        }
    }

    fn delay_ms(&mut self, ms: u64) {
        // A plain spin delay: embassy Timers cannot be used from `block_on`
        // (their waker must come from an embassy executor).
        let start = esp_hal::time::Instant::now();
        while start.elapsed().as_millis() < ms {
            core::hint::spin_loop();
        }
    }
}
