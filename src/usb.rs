//! Native Type-A USB role and synchronized cross-core status.
use core::cell::RefCell;
use core::sync::atomic::{AtomicBool, AtomicU32};
use embassy_sync::blocking_mutex::{Mutex, raw::CriticalSectionRawMutex};
#[cfg(all(feature = "usb-host", feature = "usb-device"))]
compile_error!("Select usb-host OR usb-device for the Type-A port.");
#[cfg(feature = "usb-device")]
#[path = "usb_device.rs"]
mod device;
#[cfg(feature = "usb-device")]
pub use device::usb_task;
#[cfg(feature = "usb-host")]
#[path = "usb_host.rs"]
mod host;
#[cfg(feature = "usb-host")]
pub use host::usb_task;
#[cfg(not(any(feature = "usb-host", feature = "usb-device")))]
#[embassy_executor::task]
pub async fn usb_task(_usb: esp_hal::peripherals::USB_HS<'static>) {}
pub static CONNECTED: AtomicBool = AtomicBool::new(false);
pub static RX_BYTES: AtomicU32 = AtomicU32::new(0);
pub static TX_BYTES: AtomicU32 = AtomicU32::new(0);
pub static USB_REPORT: TextBuffer = TextBuffer::new();
pub static USB_LAST_RX: TextBuffer = TextBuffer::new();
pub static INPUT: Mutex<CriticalSectionRawMutex, RefCell<crate::usb_input::Input>> =
    Mutex::new(RefCell::new(crate::usb_input::Input::new()));
pub struct TextBuffer(Mutex<CriticalSectionRawMutex, RefCell<heapless::String<64>>>);
impl TextBuffer {
    const fn new() -> Self {
        Self(Mutex::new(RefCell::new(heapless::String::new())))
    }
    pub fn write(&self, s: &str) {
        self.0.lock(|b| {
            let mut b = b.borrow_mut();
            b.clear();
            for c in s
                .chars()
                .filter(|c| c.is_ascii_graphic() || *c == ' ')
                .take(64)
            {
                let _ = b.push(c);
            }
        });
    }
    pub fn read(&self) -> heapless::String<64> {
        self.0.lock(|b| b.borrow().clone())
    }
}
