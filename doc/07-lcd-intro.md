# LCD: ST7262E43 Panel + DPI Interface

## Panel identification

The 4.3″ sub-board is an **ST7262E43** — the same panel as the
ESP32-S3-LCD-EV-Board SUB3. This was identified by cross-referencing
the S3 EV-board BSP (`smart_panel/components/esp32_s3_lcd_ev_board`)
which lists the SUB3 panel and its timing.

⚠ The Korvo S31 BSP's `display.h` has **wrong timing values** (26 MHz,
1/40/20 H, 1/10/5 V). Using them produces a vertically rolling image.
The correct values come from the S3 EV-board BSP.

## Correct timing parameters

```rust
// From SUB_BOARD3_800_480_PANEL_35HZ_RGB_TIMING
const LCD_PIXEL_CLOCK_HZ:  u32 = 18_000_000;  // (see PCLK 2× bug)
const LCD_HSYNC_PULSE_WIDTH: usize = 40;
const LCD_HSYNC_BACK_PORCH:  usize = 40;
const LCD_HSYNC_FRONT_PORCH: usize = 48;
const LCD_VSYNC_PULSE_WIDTH: usize = 23;
const LCD_VSYNC_BACK_PORCH:  usize = 32;
const LCD_VSYNC_FRONT_PORCH: usize = 13;
```

### esp-hal FrameTiming mapping

esp-hal's `FrameTiming` fields have non-obvious semantics:
- `horizontal_blank_front_porch` = HSYNC pulse + back porch (NOT just
  the front porch)
- `vertical_blank_front_porch` = VSYNC pulse + back porch
- Totals include pulse widths

```rust
FrameTiming {
    horizontal_total_width: 800 + 40 + 48 + 40,  // = 928
    horizontal_blank_front_porch: 40 + 40,         // = 80
    horizontal_active_width: 800,
    vertical_total_height: 480 + 23 + 32 + 13,    // = 548
    vertical_blank_front_porch: 23 + 32,           // = 55
    vertical_active_height: 480,
    vsync_width: 23,
    hsync_width: 40,
    hsync_position: 0,
}
```

## Clock mode

```rust
ClockMode {
    polarity: Polarity::IdleLow,   // PCLK idles low
    phase: Phase::ShiftHigh,       // data latched on FALLING edge
                                    // (= BSP's pclk_active_neg = true)
}
```

⚠ `Phase::ShiftLow` samples on the RISING edge (same edge data changes)
→ garbled output. This must be `ShiftHigh`.

## Sync/DE idle levels

```rust
vsync_idle_level: Level::High,   // active-low pulse
hsync_idle_level: Level::High,   // active-low pulse
de_idle_level:    Level::Low,
```

## Wire mode (esp-hal patch required)

The S31 LCD_CAM needs `lcd_wire_mode = 1` (16-bit) and
`lcd_always_out_en = 1` for RGB mode — both missing from stock esp-hal
(see 02-build-setup.md, patch #1). Without them the FIFO is read with
an 8-bit wire and the image is continuously skewed.

## DMA buffer requirements

The framebuffer is 800 × 480 × 2 = **768,000 bytes** in PSRAM.
Descriptor chunks must be ≤4095 bytes, 64-byte aligned, and multiples
of the burst length (64 bytes). The demo uses 4032-byte chunks
(63 × 64), requiring 191 descriptors.

### DmaTxBuffer implementation

esp-hal's built-in `DmaLoopBuf` links a single ≤4092-byte descriptor to
itself — unsuitable for a 768 KB framebuffer. The demo implements the
public `DmaTxBuffer` trait with a self-linked descriptor chain:

```rust
unsafe impl DmaTxBuffer for FrameRing {
    type View = ();
    type Final = ();

    fn prepare(&mut self) -> Preparation {
        Preparation {
            start: self.descriptors.as_mut_ptr(),
            accesses_psram: true,
            burst_transfer: BurstConfig {
                external_memory: ExternalBurstConfig::Size64,
                internal_memory: InternalBurstConfig::Enabled,
            },
            check_owner: Some(false),
            auto_write_back: false,
        }
    }
    fn into_view(self) {}
    fn from_view((): ()) {}
}
```

The last descriptor's `next` pointer wraps to the first, creating an
endless ring. `Dpi::send(true, ring)` starts the continuous scan with
`lcd_next_frame_en = 1`.

## Alignment observations

The previous driver exhibited shifting and per-boot offsets with the RGB
transfer buffer disabled. Those observations do not establish an inherent
random phase in the ST7262E43. Enabling the buffer stopped the reported
drift; see [current evidence](10-lcd-known-issues.md). Boot calibration
remains pending alignment validation.

### Minutes-slow re-lock after SoC reset

If the SoC resets while the panel stays powered, the panel can take
**minutes** to re-lock to the stream. A cold power-on locks in ~1 second.
The cause of this earlier observation is unverified; retest after the transfer-buffer fix.

### Boot calibration

```
1. Draw a crosshair at framebuffer (400, 240)
2. Wait for user to touch the visible crosshair (up to 30 seconds)
3. Compute offsets: Sx = (touch_x - 400) mod 800
                    Sy = (touch_y - 240) mod 480
4. All rendering rotates by (-Sx, -Sy) — Canvas.offset_x/y
5. Touch coordinates map back with the same offsets
```
