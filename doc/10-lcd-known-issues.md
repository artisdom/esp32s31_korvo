# LCD: Known Issues & Evidence

Updated 2026-10-02. Earlier documents overclaimed stability and treated
several hypotheses as established root causes.

## Horizontal drift: missing RGB transfer buffer — corrected and board-checked

The user reported continuous left-to-right drift after the PCLK change.
The runtime still updated dynamic widgets every 50 ms; it was not static
as previously documented.

The Rust driver left `LCD_TRANS_BUFF_CFG.LCD_TRANS_BUFFER_ENA` disabled.
ESP-IDF's `components/esp_lcd/rgb/esp_lcd_panel_rgb.c`, in
`rgb_panel_create()`, explicitly enables it to improve performance and
prevent underrun. `Display::new` now does the same in Rust before DMA starts.

The comparison kept the local esp-hal clock configuration and live UI:

| Observation | Buffer disabled | Buffer enabled |
|---|---|---|
| UART underrun count | 588 → 2,105 over 30 seconds | 0 through >1,800 VSYNC events |
| LCD VSYNC rate | Approximately 35.4 Hz | Approximately 35.4 Hz |
| User visual check | Continuous horizontal drift | Stationary with live updates and tab changes |

The enabled-buffer capture ran for about one minute. This supports the
fix for the observed workload; it is not a long-duration or camera-load
validation. Local logs are under `target/lcd-debug/` (ignored build artifacts).

## PCLK: timing inferred from VSYNC, not DMA EOF

The local esp-hal patch uses a 36 MHz module clock with an LCD prescaler
of two for the requested 18 MHz PCLK. Actual LCD VSYNC events give about
35.4 Hz, consistent with the configured 928×548 total frame timing.
The original DMA-descriptor counter was unreliable and cannot substantiate
a physical PCLK measurement. Use a scope or logic analyzer for that.
The application now counts LCD VSYNC directly. Heartbeat count differences
over elapsed time can be used to infer refresh rate and PCLK.

## Split title: legacy rendering rotation removed

After drift stopped, the user reported half the title at the bottom of the
screen. The last boot calibration recorded touch (404,257) and offsets
(796,463). The vertical offset wraps rendering upward by 17 rows, splitting
the title drawn at y=12 between the top and bottom. Earlier buffer-enabled
boots similarly recorded y offsets of 460 and 463.

Removed the boot crosshair calibration, offset atomics, modulo rendering
rotation and touch-coordinate transformation. Screen coordinates now map
directly to framebuffer coordinates and drawing clips at the edges. The
existing transfer-buffer fix remains enabled. After flashing, the user
confirmed that the full title and touch tabs align correctly. UART logs
reported zero underruns through more than 1,400 VSYNC events.

Two host rendering regression checks pass: the title remains within its
header rows, and edge drawing clips without wrapping to the opposite side.
Run them with:

```sh
rustc --edition=2024 --test tests/canvas.rs -o target/canvas-tests
./target/canvas-tests
```

Repeated SoC resets and any delayed panel recovery also need validation;
the previously asserted panel PLL explanation is unverified.

## Other board features

Buttons currently use the differential ADC channel's vendor raw-to-mV
mapping; raw zero is valid idle on this board. The older claim that zero
proved an ADC initialization failure was superseded by that correction.
SC101IOT detection works; camera capture remains future work. Consult the
README and the feature-specific documents for current status.

The earlier bounce-buffer EOF observations do not prove that hardware
consumes descriptor flags. Descriptor contents, DMA status and interrupt
routing would need a controlled test before drawing that conclusion.
