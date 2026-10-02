use super::status;
use core::sync::atomic::Ordering;
use embassy_futures::{join::join, select::select};
use embassy_time::Timer;
use esp_hal::{peripherals::BT, rng::Rng};
use esp_println::println;
use esp_radio::ble::controller::BleConnector;
use trouble_host::prelude::*;

#[gatt_server]
struct Server {
    demo: DemoService,
}
#[gatt_service(uuid = "279df826-8b77-4d9f-93b4-9ec9f1ab3100")]
struct DemoService {
    /// Little-endian seconds since boot, readable and notifiable.
    #[characteristic(uuid = "279df826-8b77-4d9f-93b4-9ec9f1ab3101", read, notify)]
    uptime: u32,
}

#[embassy_executor::task]
pub async fn ble_task(bt: BT<'static>) {
    let connector = match BleConnector::new(bt, Default::default()) {
        Ok(connector) => connector,
        Err(error) => {
            println!("BLE init failed: {:?}", error);
            return;
        }
    };
    let controller: ExternalController<_, 1> = ExternalController::new(connector);
    let mut resources: HostResources<_, DefaultPacketPool, 1, 2> = HostResources::new();
    let rng = Rng::new();
    let mut addr = [0; 6];
    addr[..4].copy_from_slice(&rng.random().to_le_bytes());
    addr[4..].copy_from_slice(&rng.random().to_le_bytes()[..2]);
    addr[5] |= 0xc0; // Bluetooth static random address, generated afresh per boot.
    let stack = trouble_host::new(controller, &mut resources)
        .set_random_address(Address::random(addr))
        .build();
    let mut runner = stack.runner();
    let mut peripheral = stack.peripheral();
    let server = match Server::new_with_config(GapConfig::Peripheral(PeripheralConfig {
        name: "Korvo-S31",
        appearance: &appearance::UNKNOWN,
    })) {
        Ok(server) => server,
        Err(error) => {
            println!("BLE GATT init failed: {:?}", error);
            return;
        }
    };
    let data = [
        2,
        1,
        LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED,
        10,
        9,
        b'K',
        b'o',
        b'r',
        b'v',
        b'o',
        b'-',
        b'S',
        b'3',
        b'1',
    ];
    // BR_EDR_NOT_SUPPORTED describes this firmware's BLE host, not S31 silicon.
    join(
        async {
            loop {
                if let Err(error) = runner.run().await {
                    println!("BLE runner: {:?}", error);
                    Timer::after_secs(1).await;
                }
            }
        },
        async {
            loop {
                let advertiser = match peripheral
                    .advertise(
                        &Default::default(),
                        Advertisement::ConnectableScannableUndirected {
                            adv_data: &data,
                            scan_data: &[],
                        },
                    )
                    .await
                {
                    Ok(advertiser) => advertiser,
                    Err(error) => {
                        println!("BLE advertise: {:?}", error);
                        Timer::after_secs(1).await;
                        continue;
                    }
                };
                status::BLE.store(1, Ordering::Relaxed);
                println!("BLE advertising: Korvo-S31");
                let connection = match advertiser.accept().await {
                    Ok(connection) => connection,
                    Err(error) => {
                        println!("BLE accept: {:?}", error);
                        continue;
                    }
                };
                let connection = match connection.with_attribute_server(&server) {
                    Ok(connection) => connection,
                    Err(error) => {
                        println!("BLE attach GATT: {:?}", error);
                        continue;
                    }
                };
                status::BLE.store(2, Ordering::Relaxed);
                println!("BLE connected");
                select(
                    async {
                        loop {
                            match connection.next().await {
                                GattConnectionEvent::Disconnected { .. } => break,
                                GattConnectionEvent::Gatt { event } => {
                                    if let Ok(reply) = event.accept() {
                                        reply.send().await;
                                    }
                                }
                                _ => {}
                            }
                        }
                    },
                    async {
                        loop {
                            let value = esp_hal::time::Instant::now()
                                .duration_since_epoch()
                                .as_secs() as u32;
                            let _ = server.set(&server.demo.uptime, &value);
                            // Notify subscribers; an unsubscribed phone can still read the value.
                            let _ = server.demo.uptime.notify(&connection, &value, false).await;
                            Timer::after_secs(1).await;
                        }
                    },
                )
                .await;
                status::BLE.store(0, Ordering::Relaxed);
                println!("BLE disconnected");
            }
        },
    )
    .await;
}
