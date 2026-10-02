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
| RGB LCD 800x480 | 16-bit bus, 18 MHz PCLK (35 Hz), framebuffer in PSRAM | `esp_hal::lcd_cam::lcd::dpi` + custom descriptor-ring DMA buffer | **works** — RGB transfer buffer enabled; horizontal drift stopped in the 2026-10-02 board check, with zero reported underruns during live updates |
| Capacitive touch | GT1151 @ I2C 0x14 | this repo (`touch.rs`, 16-bit regs, checksummed reports) | **works** — polled, drives page navigation + cursor |
| Audio playback | ES8389 codec @ I2C **0x10** + 2x NS4150B 3 W PAs | this repo (`es8389.rs`, full vendor init sequence ported) + `esp_hal::i2s` DMA streaming | **works** — on-chip synthesized chime/test tone, 48 kHz/16-bit |
| Mic capture | 2 analog mics -> ES8389 ADC -> I2S0 RX | `esp_hal::i2s` DMA + RMS meter | **partial** — DMA delivers samples; full-duplex clocking of the slave codec is not phase-locked in esp-hal yet, so sample quality is not guaranteed |
| microSD | SDMMC 4-bit @ 20 MHz, power switch GPIO39 | `esp_hal::sdmmc` + `sdio` + hand-rolled read-only FAT inspector | **works** (verified with a 128 GB card) — card info, partition/FAT type, volume label, root dir listing, first `.TXT` preview. Never writes. |
| WS2812 status LED | GPIO37 | `esp_hal::rmt` | **works** — colour-cycle breathing, orange flash on key press |
| Buttons | 4-key resistor ladder on GPIO42 (ADC1_CH0**_N**) | `esp_hal::analog::adc` + vendor raw→mV mapping (`buttons.rs`) | **works** — VOL+/VOL-/MODE/SET, idle 2000 mV |
| USB 2.0 HS device | Type-A port, native USB_HS pins | `esp_hal::usb` (synopsys-OTG via embassy-usb) | **works** — CDC-ACM on core 1, echoes upper-cased, `?` prints a report |
| DVP camera | SC101IOT (SCCB 0x68 on the shared I2C) | `camera.rs`: 20 MHz XCLK from LCD_CAM + paged SCCB | **detected** — PID 0xda4a; capture is future work |
| PSRAM | 16 MB hex @ 250 MHz | `esp_hal::psram` + `esp-alloc` | **works** — heap region, framebuffer lives here |
| Dual core | 2x RISC-V | `esp_rtos::start_second_core` | **works** — core 0: UI/audio/input; core 1: USB task |
| Wi-Fi 6 / BT 5.4 / 802.15.4 | modem | `esp-radio` | **not in this demo** — esp-radio support for the S31 is unreleased/experimental upstream |

## Console

- **USB-C (FT232R)**: boot log + heartbeat, 115200 8N1.
- **Type-A (USB HS)**: CDC-ACM port (e.g. `/dev/ttyACM0`), connect with any
  terminal. Type text - it echoes in upper case. Send `?` for a status line.

## Controls

- The title stays at the top; startup no longer asks for display-phase calibration.
- **Touch** the tabs to switch pages (HOME / AUDIO / SD CARD / CAMERA / ABOUT).
- **MODE** cycles pages and toggles a 440 Hz test tone.
- **SET** replays the startup chime.
- **VOL+ / VOL-** change the ES8389 DAC volume in 3 dB steps.
  (Buttons require the ADC - see below.)

## Buttons: why raw 0 means "idle"

The four keys are a resistor ladder on **GPIO42 = `ADC1_CH0_N`** — the
*negative* input of the differential SAR channel. The ladder idles at 2 V,
which is the **bottom** of the code range, so an untouched board legitimately
reads raw `0`. Pressing a key lowers the ladder voltage and *raises* the code.

The SAR's 17 comparator bits have non-uniform weights, so the code is a
weighted sum, not the integer value. `buttons.rs` ports both the weight table
and the code→voltage mapping from the vendor BSP's
`esp32_s31_adc_calibration.c`, giving 2000 mV at idle and 380/820/1340/1870 mV
for VOL+/VOL-/MODE/SET — the same thresholds the stock firmware uses.

This was verified against ESP-IDF itself: an IDF `adc_oneshot` app built from
the local checkout reads raw 0 on the same channel, so the hardware and the
esp-hal driver were both correct — only the interpretation was wrong.

Local esp-hal patch (`s31-adc-clock-patch` branch): the driver now also
enables the SAR's raw-data output and `cal_done` through regi2c (blocks 0x10 /
0x11), exactly as the vendor BSP does before using the ADC.

## Camera status

The onboard camera answers SCCB once the SoC drives its master clock:

1. **20 MHz XCLK on GPIO55** from the DVP controller's own clock generator
   (160 MHz PLL / 8) — *not* LEDC/PWM. The vendor BSP derives it the same way
   (`esp_cam_ctlr_dvp_output_clock`).
2. ~100 ms for the clock to settle (the vendor waits 20 ms for OV3660, 100 ms
   for SC101IOT).
3. **Paged SCCB**: the SC101IOT does not take 16-bit register addresses. The
   high byte goes into register `0xf0` and every access uses the 8-bit low
   byte (`sc101iot_read_a16v8` in the vendor driver).

With that in place the board reports **SC101IOT, PID 0xda4a** (OV3660 is not
populated on this unit). Live capture still needs a port of the vendor's
~190-entry init table plus DVP DMA reception and YUV→RGB565 conversion; the
esp-hal `lcd_cam::cam` DVP driver is available for it.

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
