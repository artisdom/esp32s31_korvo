//! BR/EDR inquiry through the experimental S31 controller port. No pairing,
//! connections or audio profiles are enabled by this discovery application.
use super::status;
use core::sync::atomic::Ordering;
use embassy_time::{Duration, Timer, with_timeout};
use esp_hal::peripherals::BT;
use esp_println::println;
use esp_radio::ble::controller::BleConnector;

#[embassy_executor::task]
pub async fn classic_task(bt: BT<'static>) {
    let Ok(mut controller) = BleConnector::new(bt, Default::default()) else {
        status::CLASSIC.store(3, Ordering::Relaxed);
        println!("Classic controller configuration failed");
        return;
    };
    // Raw H4 packets: Reset, Write Inquiry Mode (RSSI), then GIAC inquiry.
    // Wait for each command's completion/status before issuing another.
    for packet in [&[1, 3, 12, 0][..], &[1, 0x45, 12, 1, 1][..]] {
        if !command(&mut controller, packet).await {
            return;
        }
    }
    loop {
        status::CLASSIC.store(1, Ordering::Relaxed);
        println!("Classic: inquiry for 5.12 seconds; put peer into discoverable mode");
        if !command(&mut controller, &[1, 1, 4, 5, 0x33, 0x8b, 0x9e, 4, 0]).await {
            return;
        }
        loop {
            let Some((event, bytes, size)) = event(&mut controller, Duration::from_secs(8)).await
            else {
                status::CLASSIC.store(3, Ordering::Relaxed);
                println!("Classic inquiry timed out");
                return;
            };
            match event {
                1 if size >= 1 => {
                    println!("Classic inquiry complete, status {}", bytes[0]);
                    if bytes[0] != 0 {
                        status::CLASSIC.store(3, Ordering::Relaxed);
                        return;
                    }
                    break;
                }
                2 | 0x22 | 0x2f => {
                    if size > 0 {
                        status::CLASSIC_REPORTS.fetch_add(bytes[0] as u32, Ordering::Relaxed);
                    }
                    println!(
                        "Classic inquiry result: event {:02x}, {} bytes {:02x?}",
                        event,
                        size,
                        &bytes[..size]
                    );
                }
                _ => {}
            }
        }
        status::CLASSIC.store(2, Ordering::Relaxed);
        Timer::after_secs(30).await;
    }
}
async fn command(controller: &mut BleConnector<'_>, packet: &[u8]) -> bool {
    if controller.write_async(packet).await.ok() != Some(packet.len()) {
        status::CLASSIC.store(3, Ordering::Relaxed);
        return false;
    }
    let opcode = u16::from_le_bytes([packet[1], packet[2]]);
    loop {
        let Some((code, payload, size)) = event(controller, Duration::from_secs(5)).await else {
            status::CLASSIC.store(3, Ordering::Relaxed);
            println!("Classic command {:04x} timed out", opcode);
            return false;
        };
        let status = match code {
            0x0e if size >= 4 && u16::from_le_bytes([payload[1], payload[2]]) == opcode => {
                Some(payload[3])
            }
            0x0f if size >= 4 && u16::from_le_bytes([payload[2], payload[3]]) == opcode => {
                Some(payload[0])
            }
            _ => None,
        };
        if let Some(status) = status {
            println!("Classic HCI command {:04x} status {}", opcode, status);
            if status != 0 {
                super::status::CLASSIC.store(3, Ordering::Relaxed);
            }
            return status == 0;
        }
    }
}
async fn event(
    controller: &mut BleConnector<'_>,
    timeout: Duration,
) -> Option<(u8, [u8; 255], usize)> {
    with_timeout(timeout, async {
        let mut header = [0; 3];
        read_exact(controller, &mut header).await?;
        if header[0] != 4 {
            println!("Classic unexpected H4 packet {}", header[0]);
            return None;
        }
        let size = header[2] as usize;
        let mut payload = [0; 255];
        read_exact(controller, &mut payload[..size]).await?;
        Some((header[1], payload, size))
    })
    .await
    .ok()
    .flatten()
}
async fn read_exact(controller: &mut BleConnector<'_>, mut buf: &mut [u8]) -> Option<()> {
    while !buf.is_empty() {
        let n = controller.read_async(buf).await.ok()?;
        if n == 0 {
            return None;
        }
        buf = &mut buf[n..];
    }
    Some(())
}
