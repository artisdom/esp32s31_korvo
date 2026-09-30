//! ESP32-S31-Korvo-1 board definition.
//!
//! Pin map and analog constants transcribed from the official BSP
//! (`esp-dev-kits/examples/esp32-s31-korvo/.../common_components/esp32_s31_korvo`)
//! and the board user guide (revision V1.1).

#![allow(dead_code)]

/// The single I2C0 master, shared by codec, touch controller and camera
/// SCCB (all on GPIO0/GPIO1).
pub type I2c0 = esp_hal::i2c::master::I2c<'static, esp_hal::Blocking>;
pub type SharedI2c = embassy_sync::blocking_mutex::Mutex<
    embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
    core::cell::RefCell<I2c0>,
>;

// --- Shared I2C bus (codec, touch, camera SCCB) --------------------------
pub const I2C_SDA: u8 = 0;
pub const I2C_SCL: u8 = 1;

// --- ES8389 audio codec ---------------------------------------------------
pub const I2S_MCLK: u8 = 2;
pub const I2S_SCLK: u8 = 3; // BCLK
pub const I2S_LRCLK: u8 = 4; // WS
pub const I2S_DOUT: u8 = 5; // SoC -> codec (playback)
pub const I2S_DSIN: u8 = 6; // codec -> SoC (mic capture)
pub const PA_CTRL: u8 = 7; // NS4150B enable, active-high

/// ES8389 7-bit I2C address. The vendor macro `ES8389_CODEC_DEFAULT_ADDR`
/// says 0x20, which is the 8-bit write form; on the wire the chip ACKs at
/// 0x10 (verified with a bus scan on this board).
pub const ES8389_I2C_ADDR: u8 = 0x10;
pub const AUDIO_SAMPLE_RATE: u32 = 48_000;

// --- WS2812 status LED -----------------------------------------------------
pub const LED_WS2812: u8 = 37;

// --- ADC button ladder on GPIO42 (ADC1) -------------------------------------
/// Idle voltage of the ladder when no key is pressed.
pub const BUTTON_IDLE_MV: u16 = 2000;
/// Voltage at the centre of each key's window (from the BSP).
pub const BUTTON_CENTER_MV: [u16; 4] = [380, 820, 1340, 1870];
/// Index into [`BUTTON_CENTER_MV`].
pub const BTN_VOLUP: usize = 0;
pub const BTN_VOLDOWN: usize = 1;
pub const BTN_MODE: usize = 2;
pub const BTN_SET: usize = 3;

// --- microSD card (SDMMC 4-bit) ----------------------------------------------
pub const SD_CLK: u8 = 24;
pub const SD_CMD: u8 = 25;
pub const SD_D0: u8 = 20;
pub const SD_D1: u8 = 21;
pub const SD_D2: u8 = 22;
pub const SD_D3: u8 = 23;
/// Card power/switch control, active-low.
pub const SD_CTRL: u8 = 39;

// --- 4.3" 800x480 RGB LCD sub-board (ESP32-S3-LCD-EV-Board-SUB3) -------------
pub const LCD_DATA: [u8; 16] = [8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 33, 34, 35, 36];
pub const LCD_PCLK: u8 = 40;
pub const LCD_DE: u8 = 43;
pub const LCD_HSYNC: u8 = 44;
pub const LCD_VSYNC: u8 = 45;

pub const LCD_H_RES: usize = 800;
pub const LCD_V_RES: usize = 480;
// Panel: ST7262E43 (4.3" 800x480 RGB, same as ESP32-S3-LCD-EV-Board SUB3).
// Timing from Espressif's `SUB_BOARD3_800_480_PANEL_35HZ_RGB_TIMING`:
// 18 MHz PCLK (~35 Hz refresh), data latched on the falling PCLK edge.
// NOTE: the Korvo S31 BSP's display.h carries different numbers
// (26 MHz, 1/40/20, 1/10/5) which do NOT lock this panel - the image
// rolls vertically with a 1-line VSYNC the ST7262E43 can't see.
pub const LCD_PIXEL_CLOCK_HZ: u32 = 18_000_000;
pub const LCD_HSYNC_PULSE_WIDTH: usize = 40;
pub const LCD_HSYNC_BACK_PORCH: usize = 40;
pub const LCD_HSYNC_FRONT_PORCH: usize = 48;
pub const LCD_VSYNC_PULSE_WIDTH: usize = 23;
pub const LCD_VSYNC_BACK_PORCH: usize = 32;
pub const LCD_VSYNC_FRONT_PORCH: usize = 13;

/// GT1151 capacitive touch controller, 7-bit I2C addresses to probe.
pub const GT1151_I2C_ADDRS: [u8; 2] = [0x14, 0x5d];

// --- DVP camera (sensor SCCB on the shared I2C bus) ---------------------------
pub const CAM_DATA: [u8; 8] = [46, 47, 48, 49, 50, 51, 52, 53];
pub const CAM_PCLK: u8 = 54;
pub const CAM_XCLK: u8 = 55;
pub const CAM_VSYNC: u8 = 56;
pub const CAM_HSYNC: u8 = 57;

/// Candidate sensors supported by the stock firmware: (name, SCCB addr,
/// ID register, expected ID). ID registers are 16-bit addressed.
pub const CAM_SENSORS: [(&str, u8, u16, u16); 2] = [
    ("OV3660", 0x3c, 0x300a, 0x3660),
    ("SC101IOT", 0x68, 0x31f7, 0xda4a),
];
