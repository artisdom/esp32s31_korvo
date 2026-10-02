//! Optional radio tasks. Wi-Fi/BLE coexist; IEEE 802.15.4 is a separate build
//! because the current esp-radio driver rejects Wi-Fi + 802.15.4 together.
#[cfg(all(feature = "radio-wifi-ble", feature = "radio-802154"))]
compile_error!("Select radio-wifi-ble OR radio-802154, not both");
#[cfg(feature = "radio-802154")]
#[path = "radio_beacon.rs"]
mod beacon;
#[cfg(feature = "radio-wifi-ble")]
mod ble;
#[cfg(feature = "radio-802154")]
mod ieee;
#[cfg(feature = "radio-wifi-ble")]
mod wifi;
#[cfg(feature = "radio-wifi-ble")]
pub use ble::ble_task;
#[cfg(feature = "radio-802154")]
pub use ieee::ieee_task;
#[cfg(feature = "radio-wifi-ble")]
pub use wifi::wifi_task;

pub mod status;
