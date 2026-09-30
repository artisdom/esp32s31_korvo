# ESP32-S31-Korvo-1 — all-features Rust demo

A 100% Rust firmware for the [ESP32-S31-Korvo-1](https://docs.espressif.com/projects/esp-dev-kits/en/latest/esp32s31/esp32-s31-korvo-1/index.html)
development board (ESP32-S31-WROOM-3 module: Wi-Fi 6 / BT 5.4 / 802.15.4,
16 MB flash, 16 MB PSRAM) that drives every peripheral the board offers.

Built on **esp-hal** (git `main`, pre-1.3), **esp-rtos** + **embassy**, and a
register-level Rust port of the vendor's **ES8389** codec driver. No ESP-IDF
components, no C — the only non-Rust binary is the ESP-IDF-compatible second
stage bootloader stub (`esp-bootloader-esp-idf`).

## What it does

On boot the firmware brings up the whole board, prints a per-subsystem log on
the USB-C serial console (115200 8N1), and then runs a 800x480 UI on the LCD:

| Feature | Hardware | Driver | Status on this board |
|---|---|---|---|
| RGB LCD 800x480@60Hz | 16-bit bus, 26 MHz PCLK, framebuffer in PSRAM | `esp_hal::lcd_cam::lcd::dpi` + custom descriptor-ring DMA buffer | **works** — continuous scan-out from PSRAM, ~18 fps full-frame redraws |
| Capacitive touch | GT1151 @ I2C 0x14 | this repo (`touch.rs`, 16-bit regs, checksummed reports) | **works** — polled, drives page navigation + cursor |
| Audio playback | ES8389 codec @ I2C **0x10** + 2x NS4150B 3 W PAs | this repo (`es8389.rs`, full vendor init sequence ported) + `esp_hal::i2s` DMA streaming | **works** — on-chip synthesized chime/test tone, 48 kHz/16-bit |
| Mic capture | 2 analog mics -> ES8389 ADC -> I2S0 RX | `esp_hal::i2s` DMA + RMS meter | **partial** — DMA delivers samples; full-duplex clocking of the slave codec is not phase-locked in esp-hal yet, so sample quality is not guaranteed |
| microSD | SDMMC 4-bit @ 20 MHz, power switch GPIO39 | `esp_hal::sdmmc` + `sdio` + hand-rolled read-only FAT inspector | **works** (verified with a 128 GB card) — card info, partition/FAT type, volume label, root dir listing, first `.TXT` preview. Never writes. |
| WS2812 status LED | GPIO37 | `esp_hal::rmt` | **works** — colour-cycle breathing, orange flash on key press |
| Buttons | 4-key resistor ladder on GPIO42 (ADC1_CH0) | `esp_hal::analog::adc` | **blocked upstream** — see "ADC status" below |
| USB 2.0 HS device | Type-A port, native USB_HS pins | `esp_hal::usb` (synopsys-OTG via embassy-usb) | **works** — CDC-ACM on core 1, echoes upper-cased, `?` prints a report |
| DVP camera | OV3660 / SC101IOT (SCCB on shared I2C) | probe only (`camera.rs`) | **detected?** — see "Camera status" below |
| PSRAM | 16 MB hex @ 250 MHz | `esp_hal::psram` + `esp-alloc` | **works** — heap region, framebuffer lives here |
| Dual core | 2x RISC-V | `esp_rtos::start_second_core` | **works** — core 0: UI/audio/input; core 1: USB task |
| Wi-Fi 6 / BT 5.4 / 802.15.4 | modem | `esp-radio` | **not in this demo** — esp-radio support for the S31 is unreleased/experimental upstream |

## Console

- **USB-C (FT232R)**: boot log + heartbeat, 115200 8N1.
- **Type-A (USB HS)**: CDC-ACM port (e.g. `/dev/ttyACM0`), connect with any
  terminal. Type text - it echoes in upper case. Send `?` for a status line.

## Controls

- **Touch** the tabs to switch pages (HOME / AUDIO / SD CARD / CAMERA / ABOUT).
- **MODE** cycles pages and toggles a 440 Hz test tone.
- **SET** replays the startup chime.
- **VOL+ / VOL-** change the ES8389 DAC volume in 3 dB steps.
  (Buttons require the ADC - see below.)

## ADC status (buttons) — upstream gap

`esp_hal`'s S31 ADC driver (as of esp-hal `main` @ `0e9fe8d`) programs the same
APB_SARADC registers ESP-IDF does, conversions "complete", but the data
register always reads **0**. The upstream HIL test only asserts
`value <= MAX_RAW`, so a constant 0 passes CI.

What was tried locally (branch `s31-adc-clock-patch` in the esp-hal checkout):

1. `LP_PERI_CLKRST.adc_ctrl.lp_adc_clk_en = 1` + `lp_adc_rst_en` reset pulse
   (= IDF `adc_ll_enable_bus_clock` / `adc_ll_reset_register`),
2. `sar1_clk_pos_sel = 1` (IDF sets it in `adc_ll_digi_set_convert_mode`),
3. regi2c writes to the SAR blocks (0x10/0x11) per the Korvo BSP's
   `esp32_s31_adc_calibration.c` (CAL_DONE / raw-output enable).

None made data flow. Prime remaining suspect: analog-domain init performed by
the ESP-IDF bootloader/startup path that neither `esp-bootloader-esp-idf` nor
esp-hal replicate (the vendor BSP ships its own runtime SAR weight calibration
for exactly this reason). The IDF S31 SOC caps have no
`SOC_ADC_CALIBRATION_V1_SUPPORTED`, so IDF's own adc_common.c calibration
constructor is compiled out there too.

Until this is fixed upstream (or in the local patch), the four onboard buttons
read 0 mV and the UI degrades gracefully.

## Camera status

The stock firmware supports OV3660 and SC101IOT sensors behind the DVP port.
This demo probes both SCCB addresses with the sensor ID registers; on this
board neither acknowledged (sensors likely need their power/XCLK bring-up
sequence before SCCB answers). Full DVP capture additionally needs a Rust
port of the vendor sensor init tables (~2k lines of registers) — tracked as
future work; the esp-hal `lcd_cam::cam` DVP driver itself is available.

## Layout

```
src/
├── main.rs      — bring-up, event loop, dual-core split
├── board.rs     — pin map + analog constants (transcribed from the vendor BSP)
├── display.rs   — RGB panel: DPI config + PSRAM framebuffer w/ self-linked
│                  DMA descriptor ring (esp-hal's DmaLoopBuf caps at one
│                  4092-byte descriptor; a framebuffer needs 188)
├── gfx.rs       — RGB565 drawing primitives
├── font.rs      — public-domain 8x8 font
├── ui.rs        — pages + tab navigation
├── es8389.rs    — ES8389 register driver (port of esp_codec_dev es8389.c:
│                  slave mode, SCLK-derived clocks, 48 kHz coeff table)
├── audio.rs     — I2S0 TX+RX DMA streaming, tone/chime generator, mic RMS
├── touch.rs     — GT1151 polling driver
├── sdcard.rs    — SDMMC host + sdio card stack + read-only FAT16/32/exFAT detector
├── camera.rs    — DVP sensor SCCB probe
├── usb.rs       — embassy-usb CDC-ACM task (runs on core 1)
├── buttons.rs   — ADC ladder decode (blocked, see above)
└── led.rs       — WS2812 via RMT
```

### The framebuffer trick

esp-hal's public DMA buffer types don't cover "endlessly repeat a 768 KB
buffer": `DmaTxBuf` stops at the last descriptor and `DmaLoopBuf` links a
single <=4092-byte descriptor to itself. `display.rs` therefore implements the
public-but-unsafe `DmaTxBuffer` trait with a ring of 188 descriptors over a
PSRAM slice, and combines it with the LCD controller's "next frame" mode.
CPU writes are made visible to the DMA by cleaning the cache through
`DmaAlignedMut::writeback` each frame.

## Building & flashing

```sh
rustup target add riscv32imafc-unknown-none-elf

# The esp-rs crates point at a local checkout of esp-hal @ 0e9fe8d with the
# branch `s31-adc-clock-patch` checked out (see Cargo.toml path deps).
cargo run --release          # builds, flashes, and opens the monitor
```

The runner flashes `/dev/ttyUSB0` (the board's FT232R port) — adjust
`.cargo/config.toml` if yours differs. Use `espflash monitor` or any serial
terminal at 115200.

Pinning note: esp-hal 1.2.x (stable) does **not** contain ESP32-S31 support.
I2S, LCD_CAM (RGB + DVP) and USB-HS drivers for this chip only exist on
`main` (expected in the next minor release), which is why all esp-rs deps
point at a fixed local checkout of that revision.

## Honest limitations

- Mic capture quality: the codec is a slave on shared BCLK/LRCLK; esp-hal's
  I2S full-duplex has no shared-clock mode, so RX timing is nominally-identical
  but not phase-locked to TX. Playback is unaffected.
- The UI redraws the full frame every tick (~18 fps); no dirty-rect tracking.
- Camera: probe only (see above). Radio: not attempted.
- exFAT cards are detected but not listed (FAT16/FAT32 only).
