# ESP32-S31-Korvo-1 — Implementation Guide

This folder documents everything learned while building and debugging the
all-features Rust demo for this board, with special depth on the LCD (the
hardest subsystem by far). Use it as a reference for improving the
implementation, porting to other boards, or contributing fixes upstream.

## Document index

| File | Subject |
|---|---|
| [01-board.md](01-board.md) | Hardware pin map, verified chip addresses, board quirks |
| [02-build-setup.md](02-build-setup.md) | Toolchain, Cargo configuration, esp-hal patching, flashing |
| [14-sd-audio-recording.md](14-sd-audio-recording.md) | SD MP3/WAV player, numbered microphone recordings, controls and validation |
| [03-audio.md](03-audio.md) | ES8389 codec register driver, I2S DMA streaming, mic capture |
| [04-sdcard.md](04-sdcard.md) | SDMMC host, enumeration, FAT read/write and superfloppy mapping |
| [05-touch.md](05-touch.md) | GT1151 touch protocol, polling driver, coordinate mapping |
| [06-usb.md](06-usb.md) | USB 2.0 HS CDC-ACM on core 1 |
| [07-lcd-intro.md](07-lcd-intro.md) | LCD panel specs, DPI interface, timing parameters |
| [08-lcd-debug-journey.md](08-lcd-debug-journey.md) | Chronological debugging log: every experiment and its result |
| [09-lcd-architecture.md](09-lcd-architecture.md) | Current working architecture + why alternatives failed |
| [10-lcd-known-issues.md](10-lcd-known-issues.md) | PCLK 2× bug, PSRAM/DMA contention, upstream gaps |
| [11-lcd-future-work.md](11-lcd-future-work.md) | LCD validation and feature roadmap |
| [12-upstream-contributions.md](12-upstream-contributions.md) | esp-hal patches made, issues to file, what to send upstream |

## Quick summary of board features

| Feature | Hardware | Status in this demo |
|---|---|---|
| RGB LCD 800×480 | ST7262E43, 16-bit bus, 4.3″ | Working — RGB transfer buffer enabled; drift stopped in board check (see 10 for evidence) |
| Capacitive touch | GT1151 @ I2C 0x14 | Working |
| Audio playback | ES8389 codec + 2× NS4150B 3 W PAs | SD MP3/WAV playback and requested test tone |
| Mic capture | 2× analog → ES8389 ADC → I2S RX | WAV recording/reopen verified; speech quality check pending |
| microSD | SDMMC 4-bit @ 20 MHz | Working (128 GB card verified) |
| WS2812 LED | GPIO37, RMT-driven | Disabled at user request |
| Buttons | 4-key ADC ladder on GPIO42 (ADC1_CH0_N) | Working — see 13-buttons-camera.md for the inverted-code gotcha |
| USB 2.0 HS | Type-A port, native USB_HS | Working (CDC-ACM on core 1) |
| DVP camera | SC101IOT on SCCB | Detected (PID 0xda4a) — needs XCLK + paged SCCB, see 13 |
| PSRAM | 16 MB hex @ 250 MHz | Working (heap + framebuffer) |
| Wi-Fi/BT/802.15.4 | modem | Not attempted (upstream esp-radio) |
