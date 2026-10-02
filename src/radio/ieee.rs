//! 802.15.4 active discovery on channels 11..26. A MAC Beacon Request discovers
//! Zigbee PANs. Thread needs a full MLE/6LoWPAN stack, not just this PHY/MAC.
use super::status;
use core::sync::atomic::Ordering;
use embassy_time::Timer;
use esp_hal::peripherals::IEEE802154;
use esp_println::println;
use esp_radio::ieee802154::{Config, Ieee802154};

#[embassy_executor::task]
pub async fn ieee_task(peripheral: IEEE802154<'static>) {
    let mut radio = Ieee802154::new(peripheral);
    let mut sequence = 0u8;
    loop {
        for channel in 11..=26 {
            status::CHANNEL.store(channel as u32, Ordering::Relaxed);
            radio.set_config(Config {
                channel,
                promiscuous: true,
                rx_when_idle: true,
                // Discovery never acknowledges or associates with other devices.
                auto_ack_rx: false,
                auto_ack_tx: false,
                txpower: 0,
                ..Default::default()
            });
            radio.start_receive();
            // Legacy MAC Beacon Request: command FCF, sequence, broadcast
            // destination PAN/address, command ID 7, hardware-generated FCS.
            let request = [0x03, 0x08, sequence, 0xff, 0xff, 0xff, 0xff, 0x07, 0, 0];
            sequence = sequence.wrapping_add(1);
            match radio.transmit_raw(&request, true) {
                Ok(()) => println!("802.15.4 scan: channel {} beacon request queued", channel),
                Err(error) => println!("802.15.4 TX failed: {:?}", error),
            }
            // Drain promptly and bound work each pass so audio polling gets CPU.
            for _ in 0..100 {
                for _ in 0..4 {
                    match radio.received() {
                        Some(Ok(frame)) => {
                            status::RECEIVED.fetch_add(1, Ordering::Relaxed);
                            if matches!(
                                frame.frame.content,
                                ieee802154::mac::FrameContent::Beacon(_)
                            ) {
                                if let Some(beacon) =
                                    super::beacon::ZigbeeBeacon::parse(&frame.frame.payload)
                                {
                                    status::ZIGBEE_BEACONS.fetch_add(1, Ordering::Relaxed);
                                    println!(
                                        "Zigbee PAN: ch {} source {:?} extended PAN {:016x} version {} depth {} router capacity {} end-device capacity {}",
                                        frame.channel,
                                        frame.frame.header.source,
                                        beacon.extended_pan,
                                        beacon.version,
                                        beacon.depth,
                                        beacon.router_capacity,
                                        beacon.end_device_capacity
                                    );
                                }
                            }
                            println!(
                                "802.15.4 RX: ch {} RSSI {} LQI {} payload {} bytes",
                                frame.channel,
                                frame.rssi,
                                frame.lqi,
                                frame.frame.payload.len()
                            );
                        }
                        Some(Err(error)) => println!("802.15.4 decode: {:?}", error),
                        None => break,
                    }
                }
                Timer::after_millis(10).await;
            }
        }
        println!("802.15.4 discovery complete; this firmware has not joined Zigbee/Thread");
        Timer::after_secs(30).await;
    }
}
