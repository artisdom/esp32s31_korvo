//! Optional radio tasks. Wi-Fi/BLE coexist; IEEE 802.15.4 is a separate build
//! because the current esp-radio driver rejects Wi-Fi + 802.15.4 together.
#[cfg(any(
    all(feature = "radio-wifi-ble", feature = "radio-802154"),
    all(feature = "radio-wifi-ble", feature = "radio-zigbee"),
    all(feature = "radio-802154", feature = "radio-zigbee"),
    all(feature = "radio-classic", feature = "radio-wifi-ble"),
    all(feature = "radio-classic", feature = "radio-802154"),
    all(feature = "radio-classic", feature = "radio-zigbee")
))]
compile_error!("Select exactly one radio demo feature");
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
#[cfg(feature = "radio-zigbee")]
mod zigbee_demo;
#[cfg(feature = "radio-zigbee")]
pub use zigbee_demo::zigbee_task;
#[cfg(feature = "radio-zigbee")]
#[path = "zigbee_partition.rs"]
mod partition;

#[cfg(feature = "radio-classic")]
mod classic_demo;
#[cfg(feature = "radio-classic")]
pub use classic_demo::classic_task;
