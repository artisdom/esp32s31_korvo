//! SC101IOT sensor initialization and frame-boundary DVP capture.
//!
//! The board's camera is an OV3660 or SC101IOT behind the LCD_CAM DVP port.
//! Neither answers SCCB until the SoC drives the sensor's master clock: the
//! vendor BSP outputs **20 MHz on GPIO55 from the DVP controller's own clock
//! generator** (not LEDC), waits for it to settle, and only then probes SCCB
//! (`esp_video_init.c` -> `esp_cam_ctlr_dvp_output_clock()`, and
//! `BSP_CAMERA_DEFAULT_XCLK_FREQ_HZ` in the board package).
//!
//! Captures a centered 320x240 UYVY window using the vendor register table.
//! DMA frames with missing bytes are discarded before preview or encoding.

use esp_hal::{
    lcd_cam::cam::{Cam, Camera},
    peripherals::GPIO55,
    time::Rate,
};

use esp_println::println;

use crate::board::{CAM_SENSORS, I2c0, Sccb, SharedI2c};

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
/// the DMA hardware until `receive()` is called. `Stream` exclusively owns
/// that RX half; the display owns the separate TX half.
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

extern crate alloc;
use alloc::boxed::Box;
use esp_hal::{
    dma::{DmaDescriptor, DmaRxBuf, aligned::DmaAlignedMut},
    lcd_cam::cam::CameraTransfer,
    time::Instant,
};
const NATIVE: usize = FRAME_BYTES;
const CAPACITY: usize = NATIVE * 3 + 4032 * 4;
const DESCS: usize = CAPACITY.div_ceil(4032).next_multiple_of(4);
#[repr(C, align(64))]
struct FrameDescriptors([DmaDescriptor; DESCS]);
static CAPTURE_TAKEN: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
static mut FRAME_DESCRIPTORS: FrameDescriptors = FrameDescriptors([DmaDescriptor::EMPTY; DESCS]);
pub const FRAME_BYTES: usize = 320 * 240 * 2;
pub struct Stream {
    camera: Option<Camera<'static>>,
    buffer: Option<DmaRxBuf>,
    transfer: Option<CameraTransfer<'static, DmaRxBuf>>,
    started: Instant,
    native: Box<[u8]>,
    pub frame: Box<[u8]>,
    pub count: u32,
    pub errors: u32,
}
impl Stream {
    pub fn new(
        camera: Camera<'static>,
        i2c: &'static SharedI2c,
        probe: CameraProbe,
    ) -> Result<Self, &'static str> {
        if !matches!(probe.sensor, Some(("SC101IOT", _))) {
            return Err("SC101IOT sensor required");
        }
        if CAPTURE_TAKEN.swap(true, core::sync::atomic::Ordering::AcqRel) {
            return Err("Camera DMA descriptors already owned");
        }
        i2c.lock(|bus| {
            let mut bus = bus.borrow_mut();
            for &(reg, val) in crate::sc101iot_regs::VGA_UYVY {
                bus.write(0x68, &[reg, val])
                    .map_err(|_| "SC101IOT setup failed")?;
            }
            // Vendor window layout: low start/end bytes, then packed
            // high nibbles (end << 4 | start). Center 320x240 in 1280x720.
            for (reg, val) in [
                (0xf0, 0x01),
                (0x70, 0xe0),
                (0x71, 0x20),
                (0x72, 0x31),
                (0x73, 0xf0),
                (0x74, 0xe0),
                (0x75, 0x10),
            ] {
                bus.write(0x68, &[reg, val])
                    .map_err(|_| "Camera window failed")?;
            }
            bus.write(0x68, &[0xf0, 0x31])
                .map_err(|_| "Camera page failed")?;
            bus.write(0x68, &[0x00, 0x01])
                .map_err(|_| "Camera stream failed")?;
            Ok::<_, &'static str>(())
        })?;
        let mut camera = camera;
        camera
            .apply_config(
                &esp_hal::lcd_cam::cam::Config::default().with_frequency(Rate::from_hz(XCLK_HZ)),
            )
            .map_err(|_| "Camera timing failed")?;
        let camera = camera
            .with_pixel_clock(unsafe { esp_hal::peripherals::GPIO54::steal() })
            .with_vsync(
                esp_hal::gpio::interconnect::InputSignal::from(unsafe {
                    esp_hal::peripherals::GPIO56::steal()
                })
                .with_input_inverter(true),
            )
            .with_h_enable(unsafe { esp_hal::peripherals::GPIO57::steal() })
            .with_data0(unsafe { esp_hal::peripherals::GPIO46::steal() })
            .with_data1(unsafe { esp_hal::peripherals::GPIO47::steal() })
            .with_data2(unsafe { esp_hal::peripherals::GPIO48::steal() })
            .with_data3(unsafe { esp_hal::peripherals::GPIO49::steal() })
            .with_data4(unsafe { esp_hal::peripherals::GPIO50::steal() })
            .with_data5(unsafe { esp_hal::peripherals::GPIO51::steal() })
            .with_data6(unsafe { esp_hal::peripherals::GPIO52::steal() })
            .with_data7(unsafe { esp_hal::peripherals::GPIO53::steal() });
        let layout =
            alloc::alloc::Layout::from_size_align(CAPACITY, 64).map_err(|_| "Camera layout")?;
        let ptr = unsafe {
            esp_alloc::HEAP.alloc_caps(esp_alloc::MemoryCapability::External.into(), layout)
        };
        if ptr.is_null() {
            return Err("Camera frame allocation failed");
        }
        unsafe {
            ptr.write_bytes(0, CAPACITY);
        }
        let data = unsafe { core::slice::from_raw_parts_mut(ptr, CAPACITY) };
        let descriptors = unsafe { &mut *core::ptr::addr_of_mut!(FRAME_DESCRIPTORS) };
        let buffer = DmaRxBuf::new_with_config(
            unsafe { DmaAlignedMut::new_unchecked(&mut descriptors.0[..]) },
            unsafe { DmaAlignedMut::new_unchecked(data) },
            esp_hal::dma::ExternalBurstConfig::Size64,
        )
        .map_err(|_| "Camera DMA buffer rejected")?;
        println!("SC101IOT streaming: 320x240 UYVY centered crop, 20 MHz XCLK");
        Ok(Self {
            camera: Some(camera),
            buffer: Some(buffer),
            transfer: None,
            started: Instant::now(),
            native: crate::psram_buffer::zeroed(NATIVE),
            frame: crate::psram_buffer::zeroed(FRAME_BYTES),
            count: 0,
            errors: 0,
        })
    }
    /// Suspend RX while a full LCD page is repainted; resume on the next poll.
    pub fn pause(&mut self) {
        if let Some(transfer) = self.transfer.take() {
            let (camera, buffer) = transfer.stop();
            self.camera = Some(camera);
            self.buffer = Some(buffer);
        }
    }
    pub fn poll(&mut self, enabled: bool) -> bool {
        if self.transfer.as_ref().map(|t| t.is_done()).unwrap_or(false)
            || (self.transfer.is_some() && self.started.elapsed().as_millis() >= 220)
        {
            let t = self.transfer.take().unwrap();
            let (camera, buffer) = t.stop();
            let (descriptors, data) = buffer.split();
            descriptors.invalidate();
            data.invalidate();
            let mut eof = 0;
            let mut len = 0;
            let mut valid = false;
            for desc in descriptors.iter() {
                let n = desc.len();
                if n == 0 {
                    break;
                }
                if eof >= 1 && len + n <= NATIVE {
                    let offset = (desc.buffer as usize).checked_sub(data.as_ptr() as usize);
                    if let Some(offset) = offset.filter(|o| *o + n <= data.len()) {
                        self.native[len..len + n].copy_from_slice(&data[offset..offset + n]);
                    } else {
                        break;
                    }
                }
                if eof >= 1 {
                    len += n;
                }
                if desc.flags.suc_eof() {
                    if eof >= 1 && len == NATIVE {
                        valid = true;
                        break;
                    }
                    eof += 1;
                    len = 0;
                }
            }
            if valid {
                self.frame.copy_from_slice(&self.native);
                self.count += 1;
            } else {
                self.errors += 1;
            }
            self.buffer = DmaRxBuf::new_with_config(
                descriptors,
                data,
                esp_hal::dma::ExternalBurstConfig::Size64,
            )
            .ok();
            self.camera = Some(camera);
            return valid;
        }
        if enabled && self.transfer.is_none() && self.started.elapsed().as_millis() >= 200 {
            if let (Some(camera), Some(buffer)) = (self.camera.take(), self.buffer.take()) {
                self.started = Instant::now();
                let regs = esp_hal::peripherals::LCD_CAM::regs();
                match camera.receive(buffer) {
                    Ok(t) => {
                        // LCD and camera share AXI bandwidth. Unlike HAL's finite
                        // receive helper, the vendor DVP driver leaves STOP_EN
                        // clear so a momentary full RX FIFO does not abort capture.
                        regs.cam_ctrl()
                            .modify(|_, w| w.cam_stop_en().clear_bit().cam_update().set_bit());
                        self.transfer = Some(t);
                    }
                    Err((e, camera, buffer)) => {
                        println!("camera receive: {:?}", e);
                        self.errors += 1;
                        self.camera = Some(camera);
                        self.buffer = Some(buffer);
                    }
                }
            }
        }
        false
    }
}
