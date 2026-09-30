//! USB 2.0 High-Speed device on the Type-A port (native USB_HS pins).
//!
//! Enumerates as a CDC-ACM serial port. Anything typed into the port is
//! echoed back upper-cased; sending `?` returns a one-line system report.
//! Live counters are exported through statics so the LCD can show them.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use embassy_futures::join::join;
use embassy_usb::{
    Builder,
    class::cdc_acm::{CdcAcmClass, State},
    driver::EndpointError,
};
use esp_hal::usb::otg::{
    Usb,
    embassy_usb_device::{Config, Driver},
};

pub static CONNECTED: AtomicBool = AtomicBool::new(false);
pub static RX_BYTES: AtomicU32 = AtomicU32::new(0);
pub static TX_BYTES: AtomicU32 = AtomicU32::new(0);

/// One-line status printed into the USB CDC port (also shown on the LCD).
pub static USB_REPORT: SpinBuffer = SpinBuffer::new();

/// Single-word slot for the last line received over USB.
pub static USB_LAST_RX: SpinBuffer = SpinBuffer::new();

/// Tiny fixed-size string shared between cores without locking (one writer
/// at a time by protocol: USB task writes, UI reads).
pub struct SpinBuffer {
    buf: [u8; 64],
    len: AtomicU32,
}

impl SpinBuffer {
    const fn new() -> Self {
        Self {
            buf: [0; 64],
            len: AtomicU32::new(0),
        }
    }

    pub fn write(&self, s: &str) {
        let bytes = s.as_bytes();
        let n = bytes.len().min(self.buf.len());
        // SAFETY: USB task is the only writer; readers copy under the same
        // protocol (see `read`).
        let dst = unsafe { &mut *(self.buf.as_ptr() as *mut [u8; 64]) };
        dst[..n].copy_from_slice(&bytes[..n]);
        self.len.store(n as u32, Ordering::Release);
    }

    pub fn read(&self) -> heapless::String<64> {
        let n = self.len.load(Ordering::Acquire) as usize;
        let mut out = heapless::String::new();
        for i in 0..n.min(self.buf.len()) {
            let c = self.buf[i];
            if c.is_ascii_graphic() || c == b' ' {
                let _ = out.push(c as char);
            }
        }
        out
    }
}

#[embassy_executor::task]
pub async fn usb_task(usb_hs: esp_hal::peripherals::USB_HS<'static>) {
    let usb = Usb::new_hs(usb_hs);

    static mut EP_OUT: [u8; 1024] = [0; 1024];
    // SAFETY: `EP_OUT` is only used here, on this task, forever.
    let ep_out: &'static mut [u8; 1024] = unsafe { &mut *(&raw mut EP_OUT) };
    let driver = Driver::new(usb, ep_out, Config::default());

    let mut config = embassy_usb::Config::new(0x16c0, 0x27dd); // misc device
    config.manufacturer = Some("esp-rs");
    config.product = Some("ESP32-S31-Korvo demo");
    config.serial_number = Some("S31KORVO1");
    config.max_packet_size_0 = 64;

    static mut CONFIG_DESC: [u8; 256] = [0; 256];
    static mut BOS_DESC: [u8; 256] = [0; 256];
    static mut CONTROL_BUF: [u8; 64] = [0; 64];
    static mut STATE: State = State::new();

    // SAFETY: all statics above are owned exclusively by this task.
    let (config_desc, bos_desc, control_buf, state) = unsafe {
        (
            &mut *(&raw mut CONFIG_DESC),
            &mut *(&raw mut BOS_DESC),
            &mut *(&raw mut CONTROL_BUF),
            &mut *(&raw mut STATE),
        )
    };

    let mut builder = Builder::new(
        driver,
        config,
        config_desc,
        bos_desc,
        &mut [], // no MS OS descriptors
        control_buf,
    );

    let mut class = CdcAcmClass::new(&mut builder, state, 512);

    let mut usb_dev = builder.build();

    let usb_fut = usb_dev.run();
    let echo_fut = async {
        loop {
            class.wait_connection().await;
            CONNECTED.store(true, Ordering::Relaxed);
            USB_REPORT.write("ESP32-S31-Korvo Rust demo. Send ? for info.");
            let _ = echo(&mut class).await;
            CONNECTED.store(false, Ordering::Relaxed);
        }
    };

    join(usb_fut, echo_fut).await;
}

struct Disconnected {}

impl From<EndpointError> for Disconnected {
    fn from(_: EndpointError) -> Self {
        Disconnected {}
    }
}

async fn echo<'d>(class: &mut CdcAcmClass<'d, Driver<'d>>) -> Result<(), Disconnected> {
    let mut buf = [0; 64];
    loop {
        let n = class.read_packet(&mut buf).await?;
        RX_BYTES.fetch_add(n as u32, Ordering::Relaxed);

        if buf[..n].contains(&b'?') {
            use core::fmt::Write;
            let mut msg: heapless::String<192> = heapless::String::new();
            let _ = write!(
                msg,
                "\r\nESP32-S31-Korvo-1 Rust demo\r\nCDC rx={}B tx={}B loop={}\r\n",
                RX_BYTES.load(Ordering::Relaxed),
                TX_BYTES.load(Ordering::Relaxed),
                crate::LOOP_COUNT.load(Ordering::Relaxed),
            );
            class.write_packet(msg.as_bytes()).await?;
            TX_BYTES.fetch_add(msg.len() as u32, Ordering::Relaxed);
            continue;
        }

        for c in buf[0..n].iter_mut() {
            if c.is_ascii_lowercase() {
                *c &= !0x20;
            }
        }
        let mut line = [0u8; 64];
        let mut line_len = 0;
        for &c in &buf[..n] {
            if c == b'\r' || c == b'\n' {
                break;
            }
            line[line_len] = c;
            line_len += 1;
        }
        if line_len > 0 {
            if let Ok(s) = core::str::from_utf8(&line[..line_len]) {
                USB_LAST_RX.write(s);
            }
        }

        let data = &buf[..n];
        class.write_packet(data).await?;
        TX_BYTES.fetch_add(n as u32, Ordering::Relaxed);
    }
}
