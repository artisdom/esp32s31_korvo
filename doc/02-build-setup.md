# Build & Flash Setup

## Toolchain

```bash
rustup target add riscv32imafc-unknown-none-elf
```

The S31 is a RISC-V core with FPU — mainline Rust target, no esp-specific
toolchain needed for compilation (only for flashing).

## Cargo.toml dependency pinning

**Critical**: esp-hal 1.2.x (stable) does NOT contain ESP32-S31 support.
The I2S, LCD_CAM (RGB + DVP), and USB-HS drivers for this chip only
exist on the `main` branch (expected in the next minor release). All
esp-rs crates must come from the same revision:

```toml
esp-hal = { git = "https://github.com/esp-rs/esp-hal",
            rev = "0e9fe8d87f2ce90208bf824e1202158b1bd67d5d",
            features = ["esp32s31", "unstable"] }
# ... same rev for esp-rtos, esp-println, esp-backtrace, esp-alloc,
#     esp-bootloader-esp-idf
```

In this project, they point at a local checkout:
`/home/nws/w/esp32/esp-rs/esp-hal` (branch `s31-adc-clock-patch`).

## .cargo/config.toml — the linker flags that make it work

```toml
[target.riscv32imafc-unknown-none-elf]
runner = "espflash flash --monitor --baud 921600 --port /dev/ttyUSB0"
rustflags = [
    # Matches upstream CI: linkall.x pulls in memory.x, the chip script,
    # hal-defaults.x (and the PAC's device.x through it).
    "-C", "link-arg=-Tlinkall.x",
    # readable backtraces via esp-backtrace
    "-C", "force-frame-pointers",
]

[build]
target = "riscv32imafc-unknown-none-elf"

[env]
ESP_LOG = "info"   # NOT ESP_LOGLEVEL (renamed in newer esp-println)
```

**Why `-Tlinkall.x` is required**: riscv-rt 0.16 does not link its own
script; the app must pass it. Without this flag, you get undefined
symbols for every interrupt handler (`AXI_PERF_MON`, `LCD_CAM`, etc.)
because the PAC's `device.x` PROVIDE statements are never included.

**Why NOT also `-Tlink.x`**: adding it after `-Tlinkall.x` causes the
riscv-rt `.data` LMA alignment assert to fire ("BUG(riscv-rt): the LMA
of .data is not 32-byte aligned"). Use `linkall.x` alone.

## esp-hal local patches (branch `s31-adc-clock-patch`)

Three patches are needed beyond stock `main`:

### 1. LCD DPI: wire mode + always-out enable

`esp-hal/src/lcd_cam/lcd/dpi.rs` — ESP-IDF's `rgb_panel_init` programs
`lcd_wire_mode` (16-bit bus) and `lcd_always_out_en` for RGB mode.
Without these, the FIFO is read with the wrong wire width and the panel
shows a continuously skewed/rolling image in 16-bit mode.

### 2. ChannelTx interrupt control (public)

`esp-hal/src/dma/mod.rs` — `set_interrupt_handler_pub()`,
`listen_out_pub()`, `clear_out_pub()` on `ChannelTx<Blocking, CH>`.
Needed for the LCD bounce-buffer driver to bind its own EOF handler.
⚠ These must be inside the `ChannelTx<Blocking, CH>` impl block, not
the `Async` block.

### 3. Dpi::tx_channel_mut

`esp-hal/src/lcd_cam/lcd/dpi.rs` — mutable access to the TX DMA channel
before `send()` consumes the driver.

Apply with:
```bash
cd /path/to/esp-hal
git checkout s31-adc-clock-patch
```

## Flashing

```bash
cargo run --release
```

Or manually:
```bash
cargo build --release
espflash flash --port /dev/ttyUSB0 --baud 921600 \
    target/riscv32imafc-unknown-none-elf/release/korvo-demo
```

**Common pitfall**: kill any pyserial monitor processes before flashing
(`fuser -k /dev/ttyUSB0`) — they hold the port and espflash fails with
"Device or resource busy".

## Boot sequence

The board uses the ESP-IDF second-stage bootloader (via
`esp-bootloader-esp-idf`). Boot log looks like:

```
ESP-ROM:esp32s31-20251218
I (28) boot: ESP-IDF v6.2-dev 2nd stage bootloader
...
== ESP32-S31-Korvo-1 Rust demo ==
```

## Program structure

```
src/
├── main.rs       — bring-up, event loop, dual-core split
├── board.rs      — pin map + timing constants
├── display.rs    — LCD DPI driver (full-frame PSRAM ring)
├── gfx.rs        — RGB565 drawing primitives
├── font.rs       — public-domain 8×8 font
├── ui.rs         — pages + navigation
├── es8389.rs     — ES8389 register-level codec driver
├── audio.rs      — I2S DMA streaming, tone generator, mic RMS
├── touch.rs      — GT1151 polling driver
├── sdcard.rs     — SDMMC host + read-only FAT inspector
├── camera.rs     — DVP sensor SCCB probe
├── usb.rs        — embassy-usb CDC-ACM task (core 1)
├── buttons.rs    — ADC ladder decode
└── led.rs        — WS2812 via RMT
```

## BTDM internal-memory follow-up

Optional Wi-Fi/BLE media operation also requires the S31 controller allocator
fix in local `esp-hal` commit `d5313cb1d`. See
[radio memory and validation](16-radio-support.md#internal-ram-and-media-coexistence).
For a compatible fresh HAL checkout, run
`scripts/apply-btdm-memory-fix.sh /path/to/esp-hal` from this app repository.
The app disables esp-alloc's default global allocator and supplies one that
places JPEG worker allocations on core 1 in PSRAM; the HAL heap still provides
explicit internal controller/DMA allocations and owns all deallocations.
