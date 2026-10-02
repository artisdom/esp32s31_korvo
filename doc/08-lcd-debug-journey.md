# LCD Debug Journey — Chronological Log

> Historical observations and hypotheses below are preserved, not current
> conclusions. On 2026-10-02 the user confirmed drift remained. Enabling the
> previously disabled S31 RGB transfer buffer stopped reported underruns,
> and the user confirmed stationary live updates and tab changes. See
> [current evidence](10-lcd-known-issues.md). Earlier claims that PSRAM
> writes necessarily cause drift or that short bounce rings can never work
> were too broad; the previous experiments did not establish them.

This is the full debugging history, preserved so future work doesn't
repeat dead ends. Each entry lists what was tried, the observed result,
and the conclusion.

## Phase 1: Wrong timing (BSP values)

**Setup**: 26 MHz PCLK, HSYNC 1/40/20, VSYNC 1/10/5 (from Korvo S31 BSP).
**Result**: Image rolls vertically, completely unstable.
**Cause**: The BSP's `display.h` has wrong timing for this panel. The
ST7262E43 needs 18 MHz, HSYNC 40/40/48, VSYNC 23/32/13 (from the S3
EV-board BSP for the same panel).

## Phase 2: Corrected timing + PCLK edge

**Setup**: 18 MHz, correct porches, `Phase::ShiftLow` (rising edge).
**Result**: Image still rolling, but now horizontally instead of
vertically.
**Cause**: `Phase::ShiftLow` samples on the rising edge — the same edge
data changes. Changed to `Phase::ShiftHigh` (falling edge = the BSP's
`pclk_active_neg`).

## Phase 3: Wire mode fix

**Setup**: Correct timing + edge, added `lcd_wire_mode=16` and
`lcd_always_out_en=1` (esp-hal patch).
**Result**: Much better — the image "kind of" shows but keeps rolling
horizontally.
**Conclusion**: These are necessary but not sufficient. The remaining
issue is PSRAM/DMA contention (see below).

## Phase 4: Color bar diagnostic

**Setup**: Static color bars drawn once, no further framebuffer writes.
**Result**: **Rock solid.** The bars never moved.
**Conclusion**: The timing is correct for static content. The problem
is exclusively caused by CPU writes to the framebuffer while the DMA
is streaming.

## Phase 5: Cache maintenance experiments

### Full-frame flush every tick
**Result**: Image rolls (flush bursts monopolize PSRAM, LCD DMA starves).

### Chunked flush (4 KB pieces)
**Result**: Still rolls (chunks still too large; PSRAM controller
doesn't interleave).

### Paced flush (8-line slices, 1 ms gaps)
**Result**: Still rolls (even small paced writes disturb the stream).

### No flush at all (natural eviction)
**Result**: Stable static content, but updates appear after **seconds**
(flushes arrive when cache lines evict naturally). Dynamic widgets like
the fps counter eventually update but with huge latency.

**Conclusion**: ANY PSRAM write to the framebuffer region — whether from
explicit cache maintenance or from natural dirty-line eviction — disturbs
the LCD DMA stream. There is no "safe" write pattern.

## Phase 6: SRAM bounce buffer ring

**Theory**: Move the DMA to read from internal SRAM only; copy data from
PSRAM framebuffer to SRAM in a background task.

### 96-line ring (6 × 16-line segments, single-buffered)
**Result**: **Vertical flickering** — the ring is shorter than one frame
(480 lines), so the ring-to-row mapping drifts.
**Cause**: The DMA wraps the 96-line ring ~5 times per frame; which ring
cycle maps to which screen rows is non-deterministic.

### Per-segment EOF interrupt
**Result**: Hang at boot — the EOF flag was never cleared in the ISR,
causing an interrupt storm.
**Fix**: Clear the `out_eof` bit in the ISR (W1C register).

### 96-line ring with EOF-driven refills
**Result**: Vertical flickering continues. The EOF counter advances at
exactly 2× the expected rate.
**Diagnosis**: The DMA's `suc_eof` descriptor flags are consumed by
hardware after first use — only 2 of 6 segments ever fire EOF again.
Descriptor trace confirmed: `[8, 36, 8, 36, ...]` alternating between
two descriptors.

### Timer-driven round-robin refills
**Result**: Vertical flickering unchanged (the ring is still shorter
than one frame — the refill mechanism doesn't matter).

### Double-buffered segments (pointer swap)
**Result**: Vertical flickering unchanged (same root cause).

**Conclusion**: A bounce ring shorter than one frame can NEVER work —
the ring-to-frame mapping is inherently non-deterministic.

## Phase 7: Full-frame PSRAM ring (final architecture)

**Setup**: 768 KB PSRAM descriptor ring (one frame = one ring),
correct ST7262E43 timing, static UI with no dynamic updates.
**Result**: **Stable crosshair, no vertical drift.** Residual slow
horizontal drift (~5 s per screen width) from occasional PSRAM writes.

## Phase 8: PCLK 2× bug investigation

**Measurement**: The bounce ring EOF rate showed the actual PCLK is
**36 MHz** (2× the requested 18 MHz), giving ~71 Hz scan and 72 MB/s
PSRAM reads.

**Root cause in IDF source** (`lcd_hal.c`): IDF always uses `mo=2`
(prescaler) — "due to some unstable hardware issue, we prefer to start
with mo=2 first". The `equ_sysclk=1` bypass path on S31 produces 2×
the expected PCLK.

### Fix attempt: IDF-style mo=2 prescaler
**Setup**: `equ_sysclk=0`, `lcd_clkcnt_n=1`, doubled LCD_CLK target.
**Result**: **WORSE** — bidirectional horizontal oscillation + vertical
drift. The S31's `lcd_clkcnt_n` field has different semantics than
the S3's.
**Conclusion**: The IDF mo=2 approach does not translate to S31. The
`equ_sysclk=1` path (2× too fast but stable) is the best known-working
configuration.

### Fix attempt: Request 9 MHz (for 18 via 2× bug)
**Result**: Panel completely lost sync — cycling test colors (white,
orange, green, blue, black). Touch stopped responding.
**Conclusion**: The 2× bug only produces a stable clock at the 18 MHz
request point. The fractional divider can't reach 9 MHz reliably.

## Summary of what works and what doesn't

| Approach | Result |
|---|---|
| Static PSRAM content, no writes | ✅ Rock solid |
| PSRAM + full-frame flush | ❌ Phase jumps (burst starves DMA) |
| PSRAM + chunked flush | ❌ Phase jumps |
| PSRAM + paced flush | ❌ Phase jumps |
| PSRAM + no flush (eviction) | ⚠ Slow updates, stable while idle |
| SRAM bounce (96 lines) | ❌ Vertical flickering (ring < frame) |
| SRAM bounce + EOF tracking | ❌ EOF flags consumed by hardware |
| SRAM bounce + timer refills | ❌ Still vertical flickering |
| Full-frame PSRAM + static UI | ✅ **Best known** (slow h-drift from residual PSRAM activity) |
