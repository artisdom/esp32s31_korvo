# Optional ESP32-S31 radio demos

The S31 silicon supports 2.4 GHz Wi-Fi 6, Bluetooth 5.4 LE, Bluetooth Classic,
Zigbee 3.0 and Thread 1.4. The Rust application now has optional Wi-Fi/BLE and
802.15.4 discovery builds. These are separate from the default media build.
They are **build-tested, not yet validated on this board**. No wireless protocol
is described here as working on hardware until a flash and an over-the-air
check have succeeded.

## What is implemented

| Protocol | Application in this checkout | Remaining validation or implementation |
| --- | --- | --- |
| Wi-Fi | 2.4 GHz B/G/N/**AX explicitly enabled**, scan with channel/RSSI; optional station connection, DHCP and TCP echo on port 2323 | Board scan, association, DHCP and echo test; Wi-Fi 6 negotiation requires a compatible access point |
| Bluetooth LE | Connectable `Korvo-S31` advertising, custom GATT service with readable/notifiable uptime in seconds | Discover and connect with a phone; read and subscribe |
| IEEE 802.15.4 | Channel 11–26 active discovery, MAC beacon requests, received frame channel/RSSI/LQI; identifies Zigbee PRO beacon extended PAN and capacity | Receive beacons from a nearby coordinator; test TX/RX with a second radio |
| Zigbee | Zigbee beacon discovery | Full commissioning, security, joining and application clusters are not implemented |
| Thread | Available 802.15.4 PHY/MAC; no Thread host stack | MLE, 6LoWPAN, IPv6 routing, commissioning and network dataset support are not implemented |
| Bluetooth Classic | Supported by hardware; unavailable in this application's Rust controller interface | The local esp-radio adapter enables BLE only; Classic controller mode and BR/EDR host/profile stack need implementation |

The BLE demo uses legacy advertising and GATT. It does not imply implementation
of every optional Bluetooth 5.4 feature, LE Audio or Classic audio profiles.
The TCP echo service is a connectivity demo, not a media streaming server.

## Build and test

From the repository root:

```sh
# Existing camera/audio/SD application, radios disabled:
cargo build --release

# Scan nearby access points and advertise the BLE GATT service:
cargo build --release --features radio-wifi-ble

# Also connect to an access point (credentials become part of this firmware):
KORVO_WIFI_SSID='your-ssid' KORVO_WIFI_PASSWORD='your-password' \
  cargo build --release --features radio-wifi-ble

# Alternative radio build: discover 802.15.4/Zigbee beacons:
cargo build --release --features radio-802154
```

`KORVO_WIFI_PASSWORD` is never logged. An unset or empty password selects open
network authentication; a nonempty password selects WPA2 Personal. An unset or
empty SSID enables scans only. The station retries connection every five seconds.
Scan-only operation repeats every 30 seconds. Credentials are optional and no
SSID is guessed from the development machine.

Flash only after the board is available for testing:

```sh
espflash flash --port /dev/ttyUSB0 --baud 921600 \
  target/riscv32imafc-unknown-none-elf/release/korvo-demo
```

Follow UART output at 115200. For configured Wi-Fi, read the DHCP address and
connect to port 2323, for example with `nc ADDRESS 2323`: typed bytes should echo
back. The server handles one client at a time and closes idle connections after
30 seconds. It is accessible to devices on the same LAN.

For BLE, use a GATT browser such as nRF Connect, find `Korvo-S31`, connect, and
open service `279df826-8b77-4d9f-93b4-9ec9f1ab3100`. Characteristic
`279df826-8b77-4d9f-93b4-9ec9f1ab3101` is a little-endian `u32` uptime in seconds.
Read it or enable notifications; connected updates arrive once per second.
Disconnecting returns to advertising. The temporary static random BLE address
is generated separately at each boot.

For 802.15.4, nearby Zigbee coordinators should answer the MAC beacon requests.
The scan spends about one second on each channel, then waits 30 seconds before
repeating. It never associates, acknowledges received traffic or writes network
keys. Discovery is not evidence of Zigbee or Thread joining. A coordinator's
beacon capacity flags do not guarantee that it permits commissioning.

Parser checks can run without the board:

```sh
rustc --edition=2024 --test src/radio_beacon.rs -o /tmp/korvo-beacon-tests
/tmp/korvo-beacon-tests
```

## Integration constraints

The current local `esp-radio/build.rs` rejects Wi-Fi and IEEE 802.15.4 enabled
together, so **choose one radio feature**, never both. Wi-Fi and BLE use the
upstream coexistence feature. Radio tasks run on the core 0 Embassy executor;
core 1 remains available for USB and camera compression. Network waits yield,
and scan reporting yields between access points to avoid a long UART print
blocking microphone servicing. Hardware tests must check microphone overrun
counters and display DMA underruns while scanning and transferring data.

The Wi-Fi/BLE build has 112 KiB of internal heap split into a 64 KiB reclaimed
region and the existing 48 KiB region. Splitting regions prevents a 150 KiB
camera frame from consuming the radio's internal heap. A short startup yield
lets radio controller allocations occur before media buffers are allocated.
The build also accounts for the camera worker's 64 KiB core 1 stack. Internal
heap exhaustion and sustained SD/audio/camera load remain hardware checks.

Application tasks and the BLE host are Rust, using `esp-hal`, `esp-rtos`,
`esp-radio`, `embassy-net` and `trouble-host`. Espressif's proprietary Wi-Fi/BT
controller and PHY libraries are binary dependencies of `esp-radio`; this is
not a complete Rust reimplementation of the RF firmware. This demo does not
introduce ESP-IDF, C application code, Bluedroid or C++ OpenThread.

## Primary sources and next steps

- [Espressif S31 datasheet](https://documentation.espressif.com/esp32-s31_datasheet_en.html): silicon capabilities, including Classic BR/EDR.
- [esp-radio source](https://github.com/esp-rs/esp-hal/tree/main/esp-radio): current local target support and API. The local S31 BLE adapter in `src/ble/btdm2/os_adapter_esp32s31.rs` selects BLE mode and leaves BR/EDR configuration empty; `src/ble/btdm2/mod.rs` enables BLE explicitly.
- [TrouBLE](https://github.com/embassy-rs/trouble): Rust BLE host and GATT implementation.
- [zigbee-rs](https://github.com/zigbee-rs/zigbee-rs): an actual Rust Zigbee stack with discovery and end-device steering examples. Its current ESP adapter targets C6/C5/H2 and older esp-hal/esp-sync APIs; porting it to S31 and verifying secure commissioning is a separate step, not assumed complete by our beacon parser.
- [OpenThread](https://github.com/openthread/openthread): a full Thread implementation in C/C++. Wrapping it in Rust would not meet a strict Rust host-stack requirement.
- [Remade-With-Rust rusty_esp_signal](https://github.com/Remade-With-Rust/rusty_esp_signal): Rust radio application framing/telemetry on existing driver stacks; it does not provide missing Classic or Thread host stacks.

Keep the working default media firmware installed until the optional radio
build has a coordinated hardware test window. Then validate Wi-Fi/BLE first,
restore or reflash the desired build, and test 802.15.4 with a coordinator/peer.
