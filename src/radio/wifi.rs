use super::status;
use core::sync::atomic::Ordering;
use embassy_futures::join::join3;
use embassy_net::{StackResources, tcp::TcpSocket};
use embassy_time::{Duration, Timer};
use esp_hal::{peripherals::WIFI, rng::Rng};
use esp_println::println;
use esp_radio::wifi::{
    AuthenticationMethodConfig, Config, ControllerConfig, Interface, Protocol, Protocols,
    WifiController, scan::ScanConfig, sta::StationConfig,
};
use static_cell::StaticCell;

// Never print the password. With no credentials, only scan nearby access points.
const SSID: Option<&str> = option_env!("KORVO_WIFI_SSID");
const PASSWORD: Option<&str> = option_env!("KORVO_WIFI_PASSWORD");

#[embassy_executor::task]
pub async fn wifi_task(wifi: WIFI<'static>) {
    let mut station = StationConfig::default().with_protocols(
        Protocols::default().with_2_4(Protocol::B | Protocol::G | Protocol::N | Protocol::AX),
    );
    if let Some(ssid) = SSID.filter(|s| !s.is_empty()) {
        let Ok(ssid) = ssid.try_into() else {
            println!("Wi-Fi: SSID exceeds 32 bytes");
            return;
        };
        station = station.with_ssid(ssid);
        if let Some(password) = PASSWORD.filter(|p| !p.is_empty()) {
            let Ok(password) = password.try_into() else {
                println!("Wi-Fi: password is too long");
                return;
            };
            station =
                station.with_authentication(AuthenticationMethodConfig::Wpa2Personal(password));
        } else {
            station = station.with_authentication(AuthenticationMethodConfig::Open);
        }
    }
    let Ok(mut controller) = WifiController::new(
        wifi,
        ControllerConfig::default().with_initial_config(Config::Station(station)),
    ) else {
        status::WIFI.store(6, Ordering::Relaxed);
        println!("Wi-Fi: controller initialization failed");
        return;
    };
    println!("Wi-Fi: 2.4 GHz B/G/N/AX enabled; starting scan");
    let scan = ScanConfig::default().with_max(16);
    if SSID.is_none_or(str::is_empty) {
        loop {
            print_scan(&mut controller, &scan).await;
            Timer::after_secs(30).await;
        }
    }
    print_scan(&mut controller, &scan).await;
    static RESOURCES: StaticCell<StackResources<3>> = StaticCell::new();
    let rng = Rng::new();
    let seed = (u64::from(rng.random()) << 32) | u64::from(rng.random());
    let (stack, mut runner) = embassy_net::new(
        Interface::station(),
        embassy_net::Config::dhcpv4(Default::default()),
        RESOURCES.init(StackResources::new()),
        seed,
    );
    // All three futures yield; no synchronous network wait blocks microphone polling.
    join3(
        runner.run(),
        async {
            loop {
                status::WIFI.store(3, Ordering::Relaxed);
                match controller.connect_async().await {
                    Ok(info) => {
                        status::WIFI.store(4, Ordering::Relaxed);
                        println!("Wi-Fi connected: {:?}", info);
                        let _ = controller.wait_for_disconnect_async().await;
                        status::WIFI.store(3, Ordering::Relaxed);
                        println!("Wi-Fi disconnected; retrying");
                    }
                    Err(error) => {
                        status::WIFI.store(6, Ordering::Relaxed);
                        println!("Wi-Fi connect failed: {:?}", error);
                    }
                }
                Timer::after_secs(5).await;
            }
        },
        async {
            let mut rx = [0; 1024];
            let mut tx = [0; 1024];
            let mut data = [0; 512];
            loop {
                stack.wait_config_up().await;
                if let Some(config) = stack.config_v4() {
                    status::WIFI.store(5, Ordering::Relaxed);
                    println!("Wi-Fi DHCP: {}; TCP echo port 2323", config.address);
                }
                let mut socket = TcpSocket::new(stack, &mut rx, &mut tx);
                socket.set_timeout(Some(Duration::from_secs(30)));
                if socket.accept(2323).await.is_err() {
                    continue;
                }
                loop {
                    match socket.read(&mut data).await {
                        Ok(0) | Err(_) => break,
                        Ok(size) => {
                            let mut sent = 0;
                            while sent < size {
                                match socket.write(&data[sent..size]).await {
                                    Ok(0) | Err(_) => break,
                                    Ok(n) => sent += n,
                                }
                            }
                            if sent != size {
                                break;
                            }
                        }
                    }
                }
                socket.close();
                let _ = socket.flush().await;
            }
        },
    )
    .await;
}
async fn print_scan(controller: &mut WifiController<'_>, scan: &ScanConfig) {
    status::WIFI.store(1, Ordering::Relaxed);
    match controller.scan_async(scan).await {
        Ok(aps) => {
            status::WIFI.store(2, Ordering::Relaxed);
            status::APS.store(aps.len() as u32, Ordering::Relaxed);
            println!("Wi-Fi scan: {} networks", aps.len());
            for ap in aps {
                println!(
                    "Wi-Fi AP: {:?} channel {} RSSI {}",
                    ap.ssid, ap.channel, ap.signal_strength
                );
                Timer::after_millis(10).await;
            }
        }
        Err(error) => println!("Wi-Fi scan failed: {:?}", error),
    }
}
