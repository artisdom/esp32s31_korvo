//! Zigbee end-device commissioning, secure key exchange, interview and parent
//! maintenance using zigbee-rs. Persistent state is restricted to an explicitly
//! reserved partition; no guessed flash address is erased.
use super::status;
use core::sync::atomic::Ordering;
use embassy_embedded_hal::adapter::BlockingAsync;
use embassy_futures::join::join;
use embassy_time::{Delay, Timer};
use esp_hal::peripherals::{FLASH, IEEE802154};
use esp_println::println;
use esp_radio::ieee802154::Ieee802154;
use esp_storage::FlashStorage;
use static_cell::StaticCell;
use zigbee::zcl::{
    clusters::general::{basic::BasicServer, identify::IdentifyServer},
    server::UnsupportedClusterResponder,
};
use zigbee::zdo::{
    config::DiscoveryType,
    descriptor::{
        DeviceDescriptorConfig, EndpointDescriptor, NodeDescriptorConfig, PowerDescriptorConfig,
    },
};
use zigbee::{
    CurrentPowerMode, CurrentPowerSourceLevel, DeviceConfig, LogicalType, NetworkConfig,
    PowerSource, TimingConfig, config::StackConfig,
};
use zigbee_mac::esp::EspMlme;

const RANGE: core::ops::Range<u32> = 0xff0000..0x1000000;
const CLUSTERS: [u16; 2] = [0, 3]; // Basic and Identify
const ENDPOINTS: [EndpointDescriptor; 1] = [EndpointDescriptor {
    endpoint: 1,
    profile_id: 0x0104,
    device_id: 0x0007,
    device_version: 1,
    input_clusters: &CLUSTERS,
    output_clusters: &[],
}];
const BASIC: BasicServer = BasicServer {
    zcl_version: 8,
    application_version: 1,
    stack_version: 0,
    hw_version: 1,
    manufacturer_name: "Rust Korvo",
    model_identifier: "korvo-s31.demo",
    power_source: 0x04, // DC source: USB-powered board.
};
static IDENTIFY: IdentifyServer = IdentifyServer::new();
type Storage = zigbee::storage::FlashStorage<BlockingAsync<FlashStorage<'static>>>;
type Handler = (
    BasicServer<'static>,
    &'static IdentifyServer,
    UnsupportedClusterResponder<'static>,
);
type Stack = zigbee::Stack<'static, EspMlme<'static>, Handler, Storage>;
static STACK: StaticCell<Stack> = StaticCell::new();

#[embassy_executor::task]
pub async fn zigbee_task(radio: IEEE802154<'static>, flash: FLASH<'static>) {
    status::ZIGBEE.store(1, Ordering::Relaxed);
    let Some(epid) = option_env!("KORVO_ZIGBEE_EPID") else {
        println!("Zigbee: set KORVO_ZIGBEE_EPID to your coordinator extended PAN in hexadecimal");
        return;
    };
    let Ok(epid) = u64::from_str_radix(epid.trim_start_matches("0x"), 16) else {
        status::ZIGBEE.store(5, Ordering::Relaxed);
        println!("Zigbee: invalid extended PAN ID");
        return;
    };
    if epid == 0 || epid == u64::MAX {
        status::ZIGBEE.store(5, Ordering::Relaxed);
        println!("Zigbee: extended PAN must identify a real coordinator");
        return;
    }
    let channel = option_env!("KORVO_ZIGBEE_CHANNEL")
        .unwrap_or("11")
        .parse::<u8>();
    let Ok(channel @ 11..=26) = channel else {
        status::ZIGBEE.store(5, Ordering::Relaxed);
        println!("Zigbee: channel must be 11 through 26");
        return;
    };
    let mut flash = FlashStorage::new(flash);
    let mut partitions = [0; 0xc00];
    if flash.capacity() < RANGE.end as usize
        || flash.read(0x8000, &mut partitions).is_err()
        || !super::partition::zigbee_partition_reserved(&partitions)
    {
        status::ZIGBEE.store(2, Ordering::Relaxed);
        println!(
            "Zigbee: refusing persistence writes; flash partitions-radio.csv first (16 MiB flash required)"
        );
        return;
    }
    status::ZIGBEE.store(3, Ordering::Relaxed);
    let storage = zigbee::init_with_flash(BlockingAsync::new(flash), RANGE).await;
    let config = StackConfig::new(
        NetworkConfig {
            extended_pan_id: zigbee::types::IeeeAddress(epid),
            channels: channel..channel + 1,
            scan_duration: 5,
        },
        DeviceConfig {
            logical_type: LogicalType::EndDevice,
            // Polling end device, allocate address. Some coordinators deliver
            // association/key responses only through indirect data requests.
            capability_information: zigbee::nwk::nib::CapabilityInformation(0x80),
            discovery_type: DiscoveryType::default(),
            tc_link_key_exchange: true,
        },
        TimingConfig {
            poll_interval_ms: 500,
            ..Default::default()
        },
        DeviceDescriptorConfig {
            node: NodeDescriptorConfig {
                frequency_band: 0x08,
                manufacturer_code: 0x1037,
                maximum_buffer_size: 80,
                maximum_incoming_transfer_size: 128,
                maximum_outgoing_transfer_size: 128,
                ..Default::default()
            },
            power: PowerDescriptorConfig {
                current_power_mode: CurrentPowerMode::Stimulated,
                available_power_sources: &[PowerSource::ConstantMainPower],
                current_power_source: PowerSource::ConstantMainPower,
                current_power_source_level: CurrentPowerSourceLevel::Full,
            },
            endpoints: &ENDPOINTS,
        },
    );
    let mac = EspMlme::new(Ieee802154::new(radio), Default::default());
    println!(
        "Zigbee: IEEE {:016x}; commission channel {} PAN {:016x}",
        mac.ieee_address(),
        channel,
        epid
    );
    let stack: &'static Stack = STACK.init(Stack::new(
        mac,
        config,
        (
            BASIC,
            &IDENTIFY,
            UnsupportedClusterResponder::new(&CLUSTERS),
        ),
        storage,
    ));
    // Both futures must run: commissioning consumes received association/key
    // responses, then the stack serves ZDP/Basic/Identify and keeps polling.
    join(
        async {
            let outcome = stack.run(Delay).await;
            status::ZIGBEE.store(5, Ordering::Relaxed);
            println!(
                "Zigbee stack stopped: {:?}; retained state for next reboot",
                outcome
            );
        },
        async {
            stack.wait_until_joined().await;
            status::ZIGBEE.store(4, Ordering::Relaxed);
            let nib = zigbee::nwk::nib::get_ref();
            println!(
                "Zigbee joined: address {:04x}, PAN {:04x}, channel {}",
                *nib.network_address(),
                *nib.panid(),
                stack.config().channel()
            );
            // Keys never appear in serial logs.
            loop {
                Timer::after_secs(1).await;
                if IDENTIFY.is_identifying() {
                    println!("Zigbee identifying: {} seconds", IDENTIFY.tick(1));
                }
            }
        },
    )
    .await;
}
