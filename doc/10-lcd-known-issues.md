# LCD: Known Issues & Root Causes

## Issue 1: PCLK 2× bug — **RESOLVED**

**Resolution (2026-10-02)**: the pixel clock is `lcd_clk / mo`, and ESP-IDF
never uses `mo = 1` — `esp_hal_lcd/lcd_hal.c` starts with `mo = 2` "due to
some unstable hardware issue" and targets twice the pixel clock for the module
clock. With `equ_sysclk = 1` (esp-hal's old S31 path) the panel was driven at
double speed. The local esp-hal patch now requests `2 × pclk` from the clock
tree and sets `equ_sysclk = 0, clkcnt_n = 1`, i.e. `mo = 2`.

Measured on hardware: 18 MHz PCLK / ~35 Hz refresh (was ~36 MHz / ~71 Hz),
matching `SUB_BOARD3_800_480_PANEL_35HZ_RGB_TIMING`. `display::frame_count()`
+ `display::pixels_per_frame()` give the refresh rate and derived PCLK on the
console, so this can be re-checked after any clock change.

---

### Issue 1 (original report, kept for reference)

**Symptom**: Requesting 18 MHz PCLK produces 36 MHz actual (measured via
DMA descriptor rate and refresh-rate calculation). Frame rate is ~71 Hz
instead of ~35 Hz.

**Root cause**: The `lcd_clk_equ_sysclk = 1` path on S31 doubles the
LCD_CLK output. ESP-IDF avoids this by always using a prescaler (`mo=2`),
but that path has different semantics on S31 than S3.

**Attempted fixes**:
| Approach | Result |
|---|---|
| `equ_sysclk=0, lcd_clkcnt_n=1`, doubled LCD_CLK target (IDF mo=2) | Worse: bidirectional horizontal oscillation + vertical drift |
| Request 9 MHz (expecting 18 via 2× bug) | Panel lost sync entirely (cycling test colors) |
| `equ_sysclk=1` (current, 2× too fast) | **Stable**, best known |

**Hypothesis**: The S31's `lcd_clkcnt_n` field means something different
than the S3's. IDF's `mo=2` code was written for S3 and ported to S31
without hardware validation of this specific path.

**Fix path**: Dump HP_SYS_CLKRST register values from a running IDF
build on the same hardware and compare. The answer is in the TRM's
LCD_CAM clock chapter.

**Impact of the 2× PCLK**: PSRAM reads at 72 MB/s instead of 36 MB/s.
This doubles the pressure on the PSRAM controller and makes the DMA
more susceptible to contention from even tiny CPU writes (contributing
to Issue 2).

---

## Issue 2: PSRAM writes disturb LCD DMA stream

**Symptom**: Any CPU write to the framebuffer region in PSRAM — whether
from explicit cache maintenance (writeback) or from natural dirty-line
eviction — causes the LCD DMA to underrun briefly, and the panel
re-locks at a new phase (image shifts horizontally).

**Evidence**:
- Static color bars (zero writes): rock solid indefinitely
- Full-frame flush: immediate phase jump
- 4 KB chunked flush: still jumps
- 8-line paced flush (1 ms gaps): still jumps
- No flush at all: updates arrive via eviction after seconds (during
  which the display is stable; the moment they arrive, it shifts)
- SRAM bounce ring (DMA never touches PSRAM): stable but wrong geometry

**Root cause**: The AXI GDMA reading PSRAM at 72 MB/s (due to PCLK 2×)
has no priority over CPU writes in the PSRAM controller's arbitration.
A single cache line write (32-64 bytes) with read-write turnaround
overhead blocks the LCD DMA long enough to underrun its shallow FIFO.

**Contributing factor**: The 2× PCLK doubling the read bandwidth makes
this much worse. At the correct 18 MHz / 36 MB/s, the FIFO would have
more headroom and small writes might not cause underruns.

**Current mitigation**: Fully static UI — no framebuffer writes during
normal operation. Live data goes to the UART console.

**Fix paths** (see 11-lcd-future-work.md):
1. Fix PCLK to 18 MHz → halve PSRAM read rate → more headroom
2. Use uncacheable PSRAM writes (MMU page attribute) → no cache
   maintenance needed, writes go directly to PSRAM in small beats
3. Bounce buffers with full-frame SRAM ring (needs more SRAM or
   resolution reduction)

---

## Issue 3: Panel random per-boot phase

**Symptom**: The image appears at a random (x, y) offset each boot.

**Root cause**: The ST7262E43 is a "dumb" TTL panel with no reset
register. It locks to the DPI stream wherever its internal counters
happen to be when the stream starts. The offset varies per boot.

**Fix**: Boot calibration — draw a crosshair, user touches it, measure
the offset, rotate all rendering accordingly. Already implemented and
working.

---

## Issue 4: Minutes-slow panel re-lock after SoC reset

**Symptom**: After a SoC reset (RST button) while the panel stays
powered, the panel can take 2-5 minutes to show content again.

**Root cause**: The panel's internal PLL/lock detector gives up when
the stream vanishes (SoC reset) and retries very slowly. A cold
power-on (power switch) locks in ~1 second.

**Mitigation**: None implemented. Use the power switch, not RST, when
possible.

---

## Issue 5: ADC reads zero (affects buttons)

**Symptom**: All ADC conversions on S31 return 0.

**Root cause**: Upstream esp-hal bug — the S31 ADC driver programs the
same APB_SARADC registers as ESP-IDF but something in the analog
domain isn't initialized. The upstream HIL test only asserts
`value <= MAX_RAW`, so a constant 0 passes CI.

**Attempted fixes**: LP ADC clock enable, reset pulse, SAR clock
polarity, regi2c SAR writes (blocks 0x10/0x11) — none worked.

**Suspected root cause**: Bootloader-side analog initialization that
neither `esp-bootloader-esp-idf` nor esp-hal replicate. The vendor BSP
ships its own runtime SAR calibration (`esp32_s31_adc_calibration.c`).

**Status**: Buttons non-functional; the UI degrades gracefully.

---

## Issue 6: `suc_eof` descriptor flags consumed by hardware

**Symptom**: When using per-segment EOF interrupts on the AXI GDMA,
only the first 1-2 descriptors fire after boot. Subsequent passes
through the ring never re-trigger.

**Root cause**: The AXI GDMA appears to clear or ignore `suc_eof` flags
after first use. This makes EOF-based bounce-buffer tracking impossible
on S31.

**Workaround**: Timer-driven refills (not dependent on EOF).

**Status**: Not blocking — the full-frame ring doesn't need per-segment
EOF. But this blocks any future bounce-buffer approach that relies on
descriptor EOF interrupts.
