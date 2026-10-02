# Optional ESP32-S31 radio demos

The S31 silicon supports 2.4 GHz Wi-Fi 6, Bluetooth 5.4 LE, Bluetooth Classic,
Zigbee 3.0 and Thread 1.4. The Rust application now has optional Wi-Fi/BLE,
Classic inquiry, 802.15.4 discovery and experimental Zigbee commissioning
builds. These are separate from the default media build.
Wi-Fi scan/association/DHCP/TCP echo and BLE GATT have now passed hardware
checks on this board. DHCP is intermittent across resets and remains under
investigation; Wi-Fi 6 negotiation has not been verified with an AX access point.
Classic inquiry has discovered the laptop over the air. 802.15.4 discovery
scanned all channels and received a Zigbee beacon on channel 20. Zigbee
commissioning remains build-tested only.

## What is implemented

| Protocol | Application in this checkout | Remaining validation or implementation |
| --- | --- | --- |
| Wi-Fi | 2.4 GHz B/G/N/**AX explicitly enabled**, scan with channel/RSSI; optional station connection, DHCP and TCP echo on port 2323 | Hardware scan/association/DHCP/echo passed; intermittent DHCP across resets; AX negotiation needs a compatible access point |
| Bluetooth LE | Connectable `Korvo-S31` advertising, custom GATT service with readable/notifiable uptime in seconds | Host adapter discovered and connected, read uptime and received five notifications; phone testing remains |
| IEEE 802.15.4 | Channel 11–26 active discovery, MAC beacon requests, received frame channel/RSSI/LQI; identifies Zigbee PRO beacon extended PAN and capacity | Board scan completed and decoded a coordinator beacon on channel 20; controlled peer TX/RX and network joining still need validation |
| Zigbee | Beacon discovery; separate experimental end-device commissioning build with security, persistence, Basic/Identify interview and parent maintenance | Joining/interview/reset must be tested with a real coordinator |
| Thread | Available 802.15.4 PHY/MAC; no Thread host stack | MLE, 6LoWPAN, IPv6 routing, commissioning and network dataset support are not implemented |
| Bluetooth Classic | Experimental `radio-classic` build: dual-mode controller plus Rust HCI GIAC inquiry with RSSI; HOME scan/report status | Board discovered laptop and completed inquiry with status zero; pairing, connections, SPP/A2DP/HFP and other host profiles remain unimplemented |

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

# Alternative radio build: Bluetooth Classic inquiry (no pairing):
cargo build --release --features radio-classic

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
together, so **choose one radio feature**, never combine them. Classic inquiry
also has its own build, separate from Wi-Fi/BLE and both 802.15.4 applications. Wi-Fi and BLE use the
upstream coexistence feature. Radio tasks run on the core 0 Embassy executor;
core 1 remains available for USB and camera compression. Network waits yield,
and scan reporting yields between access points to avoid a long UART print
blocking microphone servicing. Hardware tests must check microphone overrun
counters and display DMA underruns while scanning and transferring data.

The Wi-Fi/BLE and Classic builds have 112 KiB of internal heap split into a 64 KiB reclaimed
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
- [esp-radio source](https://github.com/esp-rs/esp-hal/tree/main/esp-radio): current local target support and API. The local S31 adapter selects BLE mode by default; our optional `classic` driver patch configures and initializes dual-mode BR/EDR discovery.
- [TrouBLE](https://github.com/embassy-rs/trouble): Rust BLE host and GATT implementation.
- [zigbee-rs](https://github.com/zigbee-rs/zigbee-rs): an actual Rust Zigbee stack with discovery and end-device steering examples. Its upstream ESP adapter targets C6/C5/H2; our small S31 manifest port and commissioning demo are described below. Secure commissioning still needs board/coordinator validation.
- [OpenThread](https://github.com/openthread/openthread): a full Thread implementation in C/C++. Wrapping it in Rust would not meet a strict Rust host-stack requirement.
- [Remade-With-Rust rusty_esp_signal](https://github.com/Remade-With-Rust/rusty_esp_signal): Rust radio application framing/telemetry on existing driver stacks; it does not provide missing Classic or Thread host stacks.

## Hardware checks, 2026-10-03

The Wi-Fi/BLE firmware scanned 9–15 access points, associated with the user's
WPA2 network, obtained a DHCP lease and echoed ten TCP payloads on port 2323.
Some subsequent boots associated but failed to obtain a lease within 80 seconds;
that reliability issue is unresolved. A host BlueZ/Bleak client discovered
`Korvo-S31`, connected, read the uptime characteristic and received five
notifications at one-second intervals. Credentials remain in an ignored local
build file and are not committed or logged.

Two 33-second camera + microphone recordings completed with radios running;
LCD underruns stayed zero and no new RX overruns accumulated while recording.
The build's one RX overrun was already present before recording. These tests
required increasing the microphone drain to 32 KiB per UI loop. Home now shows
radio task snapshots (Wi-Fi phase/AP count/BLE connection or discovery channel
and RX/beacon count) instead of a static radio claim.

No Zigbee coordinator or Thread border router is available, so commissioning
and real mesh interoperability require a later peer-based hardware check.

## Zigbee commissioning build

`radio-zigbee` adds a full Rust end-device stack via a pinned `zigbee-rs`
checkout. It runs network steering, receives the network key, requests Trust
Center link-key exchange, answers ZDP interview requests, serves Basic and
Identify clusters on endpoint 1, polls its parent every 500 ms and maintains
its link/rejoins. It resumes the stored network across resets. Keys and frame
counters are persisted in a reserved flash partition, and keys are never
printed. This commissioning path is **build-tested, not yet joined to a real
coordinator on this board**. It is an experimental stack, not a Zigbee-certified
product; interoperability must be checked with your coordinator.

The tiny S31 adapter port changes only the upstream MAC manifest: adds the
`esp32s31` feature and matches the local esp-hal/esp-radio/esp-sync/esp-alloc
versions. The checkout is `/home/nws/w/esp32/zigbee-rs-s31`, upstream revision
`3c2d51c5ee18893e0e5bc4201e70a6358961708f`, local port commit `d912ad8`.
`patches/zigbee-rs-s31.patch` records the exact port, and
`scripts/setup-zigbee-s31.sh` reproduces it in a fresh environment. No shared
esp-hal checkout is modified by this port.

Use the coordinator's **extended PAN ID**, not its short 16-bit PAN ID. Set the
actual channel, enable joining on the coordinator, then build:

```sh
KORVO_ZIGBEE_EPID='your-16-hex-digit-extended-pan' KORVO_ZIGBEE_CHANNEL=15 \
  cargo build --release --features radio-zigbee
```

An absent/invalid extended PAN ID or a channel outside 11–26 leaves the stack
off and logs the configuration error. `radio-zigbee` cannot be combined with
`radio-wifi-ble` or `radio-802154` because they would own the same modem.

The supplied `partitions-radio.csv` reserves the last 64 KiB of the board's
16 MiB flash for Zigbee state (`zigbee`, data subtype 0x40, offset `0xff0000`).
The factory image is restricted below that region. Flash that table together
with the configured image:

```sh
espflash flash --port /dev/ttyUSB0 --baud 921600 --flash-size 16mb \
  --partition-table partitions-radio.csv \
  target/riscv32imafc-unknown-none-elf/release/korvo-demo
```

Before any persistence access, firmware checks detected flash capacity and
the actual partition table at `0x8000`. It requires the exact dedicated
partition and rejects other partitions overlapping it. An ordinary default
partition table therefore cannot cause writes to an assumed spare address.
Subsequent media builds may keep this table when flashing to preserve state.

The endpoint advertises a Home Automation combined interface with model
`korvo-s31.demo` and manufacturer `Rust Korvo`. It serves Basic and Identify,
not a fabricated temperature measurement or a physical light. Identify is
reported on UART; the status LED remains disabled. Coordinators such as
Zigbee2MQTT may need a custom converter to expose an unfamiliar model even
after successful commissioning/interview.

Test commissioning, complete interview, Identify, a reset/rejoin, and loss /
restoration of the parent. Check microphone overruns during key persistence
and under simultaneous camera/SD recording. The upstream flash driver can
pause execution while erasing/writing, so uninterrupted capture during those
operations remains a hardware validation requirement.

Flash partition validation checks:

```sh
rustc --edition=2024 --test src/zigbee_partition.rs -o /tmp/korvo-zigbee-partition-tests
/tmp/korvo-zigbee-partition-tests
```

Thread research found maintained Rust OpenThread bindings in
[esp-rs/openthread](https://github.com/esp-rs/openthread), which still link the
C++ OpenThread host, and a Rust commissioner in
[meshcop-rs](https://github.com/mtilchen/meshcop-rs), which configures networks
but is not an on-chip MLE/6LoWPAN/IPv6 Thread node stack. Neither supplies a
complete pure Rust Thread stack for this board. This remains unimplemented.

Validation of the commissioning integration: configured and unconfigured S31
release builds compile/link; all 344 upstream Zigbee workspace host tests pass;
two dedicated partition guard tests reject overlaps, missing/duplicate entries,
incorrect partition types and overflowing ranges. These checks do not replace
a coordinator commissioning/interview test.

## Internal RAM and media coexistence

The S31 controller uses compressed pointers relative to internal SRAM. Its
BTDM2 generic `malloc` callback could spill into PSRAM under media load and
produced an invalid-memory-access crash during a BLE connection. The local
`esp-hal` dependency commit `d5313cb1d` routes controller/OSAL allocations to
internal RAM. The reproducible patch is
`patches/esp-radio-btdm-internal-memory.patch`; apply it to a compatible checkout
with `scripts/apply-btdm-memory-fix.sh /path/to/esp-hal`. That script checks for
an already-applied fix before making changes.

The application supplies its own global allocator: core 1's private JPEG
allocations use PSRAM, while core 0 retains the usual allocator and explicit
controller/DMA allocations stay internal. Media staging, samples and MP3
state are explicitly external. Without that policy, JPEG decoding left only
about 1 KiB internal heap and TCP timed out; the revised test kept 21–24 KiB
free. `zigbee-mac`'s optional allocator dependency has default features disabled
so it cannot install a conflicting global allocator. The updated port patch
includes that manifest change (local dependency commit `9d7d377`).

Buttons use bounded, nonblocking SAR conversion polling. A pending ADC
conversion previously held the main loop and prevented media commands and
network futures from running during some radio boots. Four-sample averaging
and the existing 20 ms debounce are retained. SAR ownership/arbitration needs
further target-driver investigation; the UI loop now yields when conversion
has not completed.

The final playback/coexistence check connected to Wi-Fi, acquired DHCP, echoed
ten TCP payloads and delivered five BLE GATT notifications **during** replay
of a 33-second AVI. All 165 images decoded without skips; LCD underruns stayed
zero and audio DMA counters stayed at their startup baselines during playback.
A new 55-frame camera/microphone recording then saved and replayed cleanly in
this build, without additional recording/playback DMA faults. Earlier DHCP
and CPU-lockup observations are retained as experimental-driver limitations;
these successful checks do not establish long-duration reliability.

## Experimental Bluetooth Classic inquiry

This is a Rust discovery application, not a Classic connection/profile stack.
It issues HCI Reset, Write Inquiry Mode (RSSI), then a 5.12-second GIAC inquiry,
waiting for each command's completion/status. It reports inquiry payloads on
UART, scan state/report count on HOME, and repeats after 30 seconds. Put a
peer into discoverable mode. Reports may repeat for the same address; the
counter is not a count of unique devices. No ACL/SCO connections or pairing
are initiated. ECDH callbacks explicitly fail; A2DP, HFP and SPP are absent.

The published `esp-wifi-sys-esp32s31` **0.3.0** crate already ships
`libbredr_app.a`, but its build script omits the library. Our local SDK copy
links it alongside that same package's BLE/common/PHY archives. The controller
expects the older `0x20250327` configuration layout and event `tx_done/free`
ABI. Do not mix newer ESP-IDF controller libraries into this package. The
local SDK dependency is `/home/nws/w/esp32/esp-rs/esp-wifi-sys-s31-classic`,
commit `10ffa39`; the shared HAL driver patch is commit `233478051`.

Rust SHA256/HMAC and RustCrypto P-192/P-256 initialization callbacks replace
the vendor C crypto adapter. RF entropy supplies secret scalars; zero/out-of-
range values are retried with a bound and temporary secrets are zeroized.
Public coordinates and private scalars use the controller's little-endian ABI.
P-192 is used only to satisfy legacy controller initialization; this does not
enable secure pairing. Packet allocations must stay inside the controller's
reserved SRAM pool. A coalescing Rust allocator provides that pool; allocating
from the global heap causes the controller's ownership assertion to fail.

Reproduce these dependency changes before resolving/building this checkout:

```sh
scripts/apply-btdm-memory-fix.sh /home/nws/w/esp32/esp-rs/esp-hal
scripts/setup-classic-s31.sh
cargo build --release --features radio-classic
```

The setup script downloads the SDK crate only when its destination is absent,
checks the package checksum pinned in the original Cargo.lock, then applies
`patches/esp-wifi-sys-s31-classic.patch` and
`patches/esp-radio-classic-s31.patch`. Already-applied patches are detected.
It checks patches before applying and never substitutes newer binary archives.
Commit the applied patches in those dependency checkouts. Optional arguments
select other HAL/SDK paths; Cargo.toml paths must then be updated to match.

The standalone production-callback checks run without the board:

```sh
cargo test --manifest-path /home/nws/w/esp32/esp-rs/esp-hal/esp-radio/tests/classic-host/Cargo.toml \
  --target x86_64-unknown-linux-gnu
```

Three tests cover published curve generator vectors in controller byte order,
bounded invalid-entropy rejection, and 256 mixed packet allocation/free cycles
with alignment, reserved-pool bounds, exhaustion and coalescing checks. The
inquiry hardware check received RSSI events for laptop `00:1A:7D:DA:71:15`
and completed with status zero. These checks do not validate pairing or profiles.

The integrated Classic build also replayed `VID00013.AVI` (55 frames) with
all frames decoded, none skipped, zero LCD underruns and no additional I2S
faults during playback. It completed two inquiry cycles. The dependency
setup script was checked against a fresh checksum-verified SDK package and
reconstructed pre-patch HAL files, then run again to verify idempotence.
Default, Wi-Fi/BLE, Classic, 802.15.4 and configured Zigbee release builds
compile/link with the shared driver changes; all 18 media host tests still pass.

## 802.15.4 discovery hardware check

The board queued MAC beacon requests across channels 11–26 and completed the
scan. On channel 20 it decoded a Zigbee PRO coordinator beacon, including
extended PAN, capacity flags and receive RSSI/LQI (the observed frame was
-79 dBm, LQI 5). Two other received frames produced bounded parser errors
(`BadInput`, `Incomplete`) and scanning continued. No controller panic occurred;
LCD underruns stayed zero and the I2S counters stayed at their startup baseline.
This demonstrates discovery reception, not successful Zigbee/Thread joining or
controlled bidirectional interoperability. A coordinator owned/configured for
this project is still needed to validate commissioning and persistence.

After integrating the shared Classic driver/SDK patch, the restored Wi-Fi/BLE
firmware repeated the 33-second `VID00012.AVI` coexistence check: all 165 frames
decoded, zero skipped images and zero LCD underruns. Ten TCP echo payloads
and a GATT read plus five notifications passed during playback. I2S TX/RX
fault counters remained at their startup baseline through completion. This
Wi-Fi/BLE camera-playback build is installed on `/dev/ttyUSB0`; Classic and
802.15.4 remain separate selectable firmware builds.
