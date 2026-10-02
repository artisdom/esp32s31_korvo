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
| Audio playback | ES8389 codec @ I2C **0x10** + 2x NS4150B 3 W PAs | this repo (`es8389.rs`, full vendor init sequence ported) + `esp_hal::i2s` DMA streaming | SD MP3 and PCM WAV playback implemented; DMA completion verified on this board, audible chime confirmed |
| Mic capture | 2 analog mics -> ES8389 ADC -> I2S0 RX | shared-clock I2S rings + RMS meter + FAT WAV recorder | 48 kHz stereo WAV recording; clean new recording playback confirmed by user |
| microSD | SDMMC 4-bit @ 20 MHz, power switch GPIO39 | `esp_hal::sdmmc` + `sdio` + `embedded-sdmmc` | **works** on the 128 GB FAT32 card: file playback, new numbered WAV recordings, and read-only boot inspection |
| WS2812 status LED | GPIO37 | `esp_hal::rmt` | **disabled at user request** — black latched once at startup; no animation or button feedback |
| Buttons | 4-key resistor ladder on GPIO42 (ADC1_CH0**_N**) | `esp_hal::analog::adc` + vendor raw→mV mapping (`buttons.rs`) | **works** — VOL+/VOL-/MODE/SET; direct weighted-code conversion and 20 ms debouncing |
| USB hub / keyboard / mouse | Type-A, native USB_HS PHY | Rust Embassy USB host, FS/LS bus | **implemented; peripheral testing pending** — USB page, typing, mouse cursor/clicks, hub hotplug; see [USB host](doc/17-usb-host-input.md) |
| DVP camera | SC101IOT (SCCB 0x68 on the shared I2C) | `camera.rs`: paged SCCB + DVP RX, Rust JPEG and AVI | live 320x240 preview and MJPEG + stereo microphone AVI recording; on-board AVI playback; SD file also decoded on host |
| PSRAM | 16 MB hex @ 250 MHz | `esp_hal::psram` + `esp-alloc` | **works** — heap region, framebuffer lives here |
| Dual core | 2x RISC-V | `esp_rtos::start_second_core` | **works** — core 0: UI/audio/input; core 1: JPEG + USB tasks |
| Wi-Fi 6 / BLE / Classic / 802.15.4 | modem | `esp-radio` | **optional builds** — station/DHCP/TCP echo, BLE uptime GATT, Classic inquiry, 802.15.4/Zigbee discovery and experimental commissioning; [status and limits](doc/16-radio-support.md) |

## Console

- **USB-C (FT232R)**: boot log + heartbeat, 115200 8N1.
- **Type-A**: USB host by default; attach a hub, keyboard and mouse. The optional
  `usb-device` build retains the earlier CDC implementation.

## Controls

USB keyboard and mouse shortcuts are described in [USB host input](doc/17-usb-host-input.md).

- The title stays at the top; startup no longer asks for display-phase calibration.
- **Touch** the tabs to switch pages (HOME / AUDIO / SD CARD / CAMERA / ABOUT).
- **MODE** cycles pages.
- On **AUDIO**, **SET** starts recording or saves the active recording. On **CAMERA**, SET starts or saves video with microphone audio. On remaining pages, SET stops media and plays the test chime.
- **AUDIO** controls: PREVIOUS / NEXT select a track; PLAY starts it; STOP ends playback or saves a recording; RECORD / SAVE toggles recording; REPLAY LAST plays the latest saved recording; RESCAN SD refreshes the list.
- The LED stays off. Speaker audio starts silent, with default volume **-30 dB**.
- **VOL+ / VOL-** change the ES8389 DAC volume in 3 dB steps.
  (Buttons require the ADC - see below.)

Put MP3 or integer PCM WAV files in the FAT16/FAT32 card root before boot.
The browser shows up to 64 files using their FAT 8.3 names/aliases. Recordings
use new `REC00001.WAV`, `REC00002.WAV`, etc. files; existing files are preserved.
Tap STOP to save before power-off or card removal. See [SD audio and recording](doc/14-sd-audio-recording.md) for formats, console commands, and validation.

## Buttons: why raw 0 means "idle"

The four keys are a resistor ladder on **GPIO42 = `ADC1_CH0_N`** — the
*negative* input of the differential SAR channel. The ladder idles at 2 V,
which is the **bottom** of the code range, so an untouched board legitimately
reads raw `0`. Pressing a key lowers the ladder voltage and *raises* the code.

The HAL returns the SAR's already weighted code (0..4393). Convert it
**directly** with `mv = 2000 - 4000 * code / 4393`; applying the comparator
weights again misclassifies keys. `button_logic.rs` supplies midpoint
voltage windows and 20 ms press/release debouncing. The UI updates the
indicator beside each label and logs stable state changes over UART.

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
populated on this unit). CAMERA now offers a 320x240 live preview and records
MJPEG video plus 48 kHz stereo microphone audio together in numbered AVI files.
Tap REC / SAVE or press SET; STOP saves before card removal. AVI files can
be played with CAMERA PLAY / REPLAY LAST, opened with SD CARD PLAY FILE,
deleted in SD CARD and played on a computer. See
[camera video recording](doc/15-camera-video-recording.md) for controls,
implementation, validation and limits.

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
├── audio.rs     — shared-clock I2S0 TX+RX, synth and microphone meter
├── audio_ring.rs — continuous DMA rings and completed-descriptor polling
├── media.rs     — MP3/WAV playback, WAV recording and video/audio AVI muxing
├── storage.rs   — writable FAT adapter over native SDMMC
├── pcm.rs       — WAV format and streaming sample-rate conversion
├── touch.rs     — GT1151 polling driver
├── sdcard.rs    — SDMMC host + sdio card stack + read-only FAT16/32/exFAT detector
├── camera.rs    — DVP sensor SCCB probe
├── usb.rs       — USB role selection and synchronized input/status
├── usb_host.rs  — USB hub/HID host tasks on core 1
├── usb_hid.rs   — report descriptor decoding
├── usb_input.rs — key/click edges, mouse position and bounded text field
├── usb_device.rs — optional legacy CDC task
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
scripts/setup-usb-host.sh    # pinned Embassy host + HAL compatibility patches
cargo run --release          # builds USB host + media, flashes and opens monitor
```

The runner flashes `/dev/ttyUSB0` (the board's FT232R port) — adjust
`.cargo/config.toml` if yours differs. Use `espflash monitor` or any serial
terminal at 115200.

Pinning note: esp-hal 1.2.x (stable) does **not** contain ESP32-S31 support.
I2S, LCD_CAM (RGB + DVP) and USB-HS drivers for this chip only exist on
`main` (expected in the next minor release), which is why all esp-rs deps
point at a fixed local checkout of that revision.

## Current limitations

- Root-directory browser, up to 64 MP3/WAV files; long names appear as FAT 8.3 aliases.
- FAT16/FAT32 only for media; exFAT is detected by boot inspection but cannot play or record.
- WAV: integer PCM, 8/16/24/32-bit, mono/stereo, 8–96 kHz. MP3: MPEG Layer III. AAC, Ogg, FLAC and float/extensible WAV are unsupported.
- Playback converts to 48 kHz stereo with nearest-neighbour rate conversion. Gapless MP3 playback and higher-quality resampling are future improvements.
- Recording uses 48 kHz stereo PCM16 WAV (~11.5 MB/minute). Save with STOP; there is no power-loss recovery or hot removal support.
- FAT timestamps use a fixed date (2026-10-02); there is no real-time clock synchronisation.
- Camera: probe only (see above). Radio: not attempted.
