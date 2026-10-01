# LCD: Current Architecture

## Diagram

```
┌─────────────────────────────────────────────────────────┐
│                    PSRAM (16 MB hex)                     │
│                                                         │
│  ┌─────────────────────────────────────────────────┐    │
│  │         Framebuffer (768 KB = 800×480×2)         │    │
│  │  Written by CPU through cache (never cleaned)    │    │
│  │  Read by AXI GDMA via descriptor ring            │    │
│  └─────────────────────────────────────────────────┘    │
│                                                         │
│  ┌─────────────────────────────────────────────────┐    │
│  │    DMA Descriptor Ring (191 × 4032-byte chunks)  │    │
│  │    Self-linked (last→first), in PSRAM             │    │
│  └─────────────────────────────────────────────────┘    │
└───────────────────────┬─────────────────────────────────┘
                        │ AXI GDMA (channel 0)
                        ▼
┌─────────────────────────────────────────────────────────┐
│                 LCD_CAM (DPI mode)                       │
│                                                         │
│  Timing generator:                                      │
│    PCLK: 36 MHz actual (18 req × 2 hw bug)              │
│    HSYNC: 40/40/48 @ active-low                         │
│    VSYNC: 23/32/13 @ active-low                         │
│    DE: active during 800 px × 480 lines                 │
│    lcd_next_frame_en = 1 (continuous rescan)            │
│                                                         │
│  Output: 16-bit bus + PCLK + HSYNC + VSYNC + DE         │
└───────────────────────┬─────────────────────────────────┘
                        │ GPIO (matrix-routed)
                        ▼
┌─────────────────────────────────────────────────────────┐
│              ST7262E43 Panel (4.3″ 800×480)              │
│                                                         │
│  Locks to stream with random phase each boot            │
│  (calibrated at boot via crosshair touch)               │
│  Data latched on FALLING PCLK edge                      │
└─────────────────────────────────────────────────────────┘
```

## Key design decisions

### 1. Full-frame ring (not bounce buffers)

The ring contains **exactly one frame** (480 lines × 1600 bytes/line =
768 KB). This ensures the ring-to-screen-row mapping is deterministic:
ring position 0 always corresponds to screen row 0, regardless of when
the DMA starts relative to the panel's frame boundary.

A shorter ring (e.g., 96 lines) would cycle multiple times per frame,
and which ring-cycle maps to which screen rows would be random — causing
vertical flickering (proven experimentally, see 08-lcd-debug-journey.md).

### 2. PSRAM (not SRAM)

The ring must be 768 KB — larger than the S31's 512 KB internal SRAM.
PSRAM is the only option for a full-frame ring. The tradeoff is that
PSRAM writes disturb the LCD DMA stream (see below).

### 3. Static UI (no dynamic framebuffer writes)

**Any PSRAM write to the framebuffer region disturbs the LCD DMA stream**
on this chip — whether from explicit cache maintenance (writeback) or
from natural dirty-line eviction. The disturbances cause the panel to
re-lock at a new phase, shifting the image horizontally.

The demo therefore uses a **fully static UI**: pages are drawn once when
entered, with a single cache-clean. Live values (fps, uptime, mic level)
print to the UART console instead of updating on-screen.

This is the **only** architecture that produces a stable display on this
hardware with the current esp-hal S31 support.

### 4. Boot calibration for panel phase

```
fn lcd_calibrate(display, touch, st) {
    // Draw crosshair at FB (400, 240) — appears at glass (400+Sx, 240+Sy)
    // Wait for user touch (5 min timeout)
    // Sx = (400 - touch_x) mod 800
    // Sy = (240 - touch_y) mod 480
    // Store in atomics CAL_OFFSET_X / CAL_OFFSET_Y
}
```

All rendering rotates by `(offset_x, offset_y)` with wrap-around:
```rust
Canvas::set(x, y, c) {
    let xx = (x + self.offset_x) % self.width;
    let yy = (y + self.offset_y) % self.height;
    self.pixels[yy * self.width + xx] = c;
}
```

Touch coordinates map back from glass to framebuffer space:
```rust
fn touch_to_fb(x: u16, y: u16) -> (u16, u16) {
    let fx = (x as i32 - Sx).rem_euclid(800) as u16;
    let fy = (y as i32 - Sy).rem_euclid(480) as u16;
    (fx, fy)
}
```

### 5. Cache maintenance strategy

| Operation | Cache action | Display impact |
|---|---|---|
| Initial splash | One full flush | Brief shift, then stable |
| Calibration screen | One full flush | Brief shift, then stable |
| Page change | One full flush (chunked 4 KB) | Brief shift, then stable |
| Dynamic widgets | **NONE** — console-only | None (no framebuffer writes) |

The full flush is chunked into 4 KB pieces to reduce the burst duration,
though any flush causes a brief phase shift. The shift is not cumulative
if the display is otherwise left alone.

## Page-change procedure

```rust
if shown_page != st.page {
    shown_page = st.page;
    ui::draw(&mut display.canvas(), &st);  // draw full page into PSRAM
    display.flush();                        // one clean (chunked)
    // Display may shift briefly, then holds at a new (possibly different)
    // phase. The calibration offsets remain valid — the content rotates
    // correctly regardless of the underlying phase.
}
```

## What the touch cursor looks like

The cursor is drawn with a self-inverse XOR ring so it can be erased
without repainting the underlying content:

```rust
pub fn xor_ring(&mut self, cx: usize, cy: usize, r: usize) {
    // XOR the pixels in the ring pattern
    // Drawing again at the same position erases it
}
```

⚠ With the fully-static UI, the cursor is drawn only during page
transitions (when the full page is redrawn). Continuous cursor tracking
would require framebuffer writes that disturb the display.
