//! DVP camera sensor probe.
//!
//! The board's camera is an OV3660 or SC101IOT behind the LCD_CAM DVP port.
//! Neither answers SCCB until the SoC drives the sensor's master clock: the
//! vendor BSP outputs **20 MHz on GPIO55 from the DVP controller's own clock
//! generator** (not LEDC), waits for it to settle, and only then probes SCCB
//! (`esp_video_init.c` -> `esp_cam_ctlr_dvp_output_clock()`, and
//! `BSP_CAMERA_DEFAULT_XCLK_FREQ_HZ` in the board package).
//!
//! Capture itself needs a sensor register driver with the vendor init tables;
//! that is still future work, so the demo identifies the sensor and reports it.

use esp_hal::{
    lcd_cam::cam::{Camera, Cam},
    peripherals::GPIO55,
    time::Rate,
};

use esp_println::println;

use crate::board::{I2c0, Sccb, SharedI2c, CAM_SENSORS};

/// Master clock the sensors expect (160 MHz PLL / 8, exact integer divider).
pub const XCLK_HZ: u32 = 20_000_000;

#[derive(Clone, Copy)]
pub struct CameraProbe {
    /// (name, PID) on an exact match, or ("unknown", PID) if some chip
    /// answered with an unexpected id.
    pub sensor: Option<(&'static str, u16)>,
    /// How many candidate addresses acknowledged.
    pub acked: u8,
    /// Whether the 20 MHz master clock is running.
    pub xclk: bool,
}

/// Starts the 20 MHz master clock on GPIO55.
///
/// Returns the camera driver (keeping the clock running) or an error string.
///
/// Note on the DMA channel: only `DMA_AXI_CH0` is wired to LCD_CAM on this
/// chip, and its TX half already streams the LCD. `Camera::new` never touches
/// the DMA hardware until `receive()` is called, so the (stolen) RX handle is
/// only used to satisfy the API - no transfer is ever started on it.
pub fn start_xclk(cam: Cam<'static>) -> Result<Camera<'static>, &'static str> {
    let dma = unsafe { esp_hal::peripherals::DMA_AXI_CH0::steal() };

    Camera::new(
        cam,
        dma,
        esp_hal::lcd_cam::cam::Config::default().with_frequency(Rate::from_hz(XCLK_HZ)),
    )
    .map(|cam| cam.with_master_clock(unsafe { GPIO55::steal() }))
    .map_err(|_| "camera clock config rejected")
}

/// SCCB speed the vendor board package uses for the camera (`camera.h`).
/// The sensor is clocked from the SoC and starts up at a slow bus speed.
const SCCB_HZ: u32 = 100_000;

/// One register read, honouring the sensor's addressing mode.
fn read_reg(bus: &mut I2c0, sccb: u8, mode: Sccb, reg: u16) -> Option<u8> {
    let mut value = [0u8; 1];
    match mode {
        Sccb::Addr16 => bus.write_read(sccb, &reg.to_be_bytes(), &mut value).ok()?,
        Sccb::Paged => {
            bus.write(sccb, &[0xf0, (reg >> 8) as u8]).ok()?;
            bus.write_read(sccb, &[reg as u8], &mut value).ok()?;
        }
    }
    Some(value[0])
}

pub fn probe(i2c: &'static SharedI2c) -> CameraProbe {
    let mut out = CameraProbe {
        sensor: None,
        acked: 0,
        xclk: false,
    };
    let mut unknown: Option<u16> = None;

    i2c.lock(|bus| {
        let mut bus = bus.borrow_mut();
        // The sensors are read one byte at a time (16-bit register address,
        // one data byte) - they do not auto-increment like the touch
        // controller does, so a 2-byte burst read returns garbage.
        let _ = bus.apply_config(
            &esp_hal::i2c::master::Config::default().with_frequency(Rate::from_hz(SCCB_HZ)),
        );

        for (name, sccb, id_reg, expect, mode) in CAM_SENSORS {
            let Some(pid_h) = read_reg(&mut bus, sccb, mode, id_reg) else {
                println!("camera: 0x{:02x} did not answer", sccb);
                continue;
            };
            out.acked += 1;
            let pid_l = read_reg(&mut bus, sccb, mode, id_reg + 1).unwrap_or(0);
            let pid = ((pid_h as u16) << 8) | pid_l as u16;
            println!(
                "camera: 0x{:02x} answered, ID 0x{:04x} = 0x{:02x}{:02x} (expect 0x{:04x})",
                sccb, id_reg, pid_h, pid_l, expect
            );
            if pid == expect {
                out.sensor = Some((name, pid));
                break;
            }
            if pid != 0 && pid != 0xffff {
                unknown = Some(pid);
            }
        }

        // Restore the bus speed for the codec and the touch controller.
        let _ = bus.apply_config(
            &esp_hal::i2c::master::Config::default().with_frequency(Rate::from_hz(400_000)),
        );
    });

    if out.sensor.is_none() {
        if let Some(pid) = unknown {
            out.sensor = Some(("unknown", pid));
        }
    }
    out
}
