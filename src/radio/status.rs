//! Small atomic snapshots for the LCD; protocol tasks own their state transitions.
use core::sync::atomic::AtomicU32;
#[cfg(feature = "radio-wifi-ble")]
pub static WIFI: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "radio-wifi-ble")]
pub static APS: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "radio-wifi-ble")]
pub static BLE: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "radio-802154")]
pub static CHANNEL: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "radio-802154")]
pub static RECEIVED: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "radio-802154")]
pub static ZIGBEE_BEACONS: AtomicU32 = AtomicU32::new(0);

#[cfg(feature = "radio-zigbee")]
pub static ZIGBEE: AtomicU32 = AtomicU32::new(0);
