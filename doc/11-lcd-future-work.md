# LCD: Future Work & Improvement Roadmap

Ordered by impact and feasibility. Each item includes the approach,
estimated difficulty, and what it unlocks.

## 1. Fix the PCLK 2× bug at the register level ⭐ Highest impact

**Difficulty**: Medium (needs TRM study + register comparison with IDF)

**Approach**: 
- Boot ESP-IDF's factory demo on the same board
- Dump `HP_SYS_CLKRST.lcdcam_lcd_ctrl0` register (clock divider + source)
- Dump `LCD_CAM.lcd_clock` register (PCLK config)
- Compare with what esp-hal writes
- Fix the discrepancy in `esp-hal/src/soc/esp32s31/clocks.rs` or
  `esp-hal/src/lcd_cam/lcd/mod.rs`

**Unlocks**: Correct 18 MHz PCLK / 35 Hz refresh, halved PSRAM read
bandwidth (36 MB/s instead of 72 MB/s). This alone might eliminate the
horizontal drift by giving the LCD DMA FIFO enough headroom to absorb
small cache write evictions.

**Notes**: The `equ_sysclk` and `lcd_clkcnt_n` fields likely have
different meanings on S31 vs S3. The S3 values are documented; the S31
TRM chapter on LCD_CAM clocking is the authoritative source.

---

## 2. Uncacheable PSRAM writes for the framebuffer

**Difficulty**: Medium (MMU page attribute manipulation)

**Approach**:
- Remap the framebuffer's PSRAM pages as uncacheable (write-through)
  using the S31's MMU page attributes
- CPU writes go directly to PSRAM in small beats (no cache lines to
  clean or evict)
- No cache maintenance needed at all
- The LCD DMA reads the same PSRAM — writes interleave at the bus
  transaction level (32-64 bytes), which the FIFO can absorb

**Unlocks**: Dynamic UI updates without display disturbance. The
framebuffer becomes writable at any time.

**Implementation sketch**:
```rust
// After allocating the framebuffer, change its MMU page attributes
// to disable caching. On S31 this means modifying the page table
// entries that map the PSRAM virtual address range.
unsafe {
    let page_attrs = mmu_get_page_attributes(fb_start);
    mmu_set_page_attributes(fb_start, fb_len, page_attrs & !CACHEABLE);
}
```

The S31's MMU (same family as P4) supports per-page cacheability
attributes. The exact register interface is in the TRM's MMU chapter.

---

## 3. LVGL with dirty-region flushing

**Difficulty**: Medium (once issues 1-2 are resolved)

**Approach**: 
- Integrate LVGL (via `lvgl` Rust crate or the esp-lvgl port)
- LVGL's built-in dirty-region tracking means only changed areas get
  redrawn
- Each frame flush only the dirty rects to PSRAM + clean those rects
- With uncacheable writes (item 2), no cleaning needed at all

**Unlocks**: Full GUI with buttons, sliders, animations — a proper
demo experience.

---

## 4. Camera streaming (DVP → LCD)

**Difficulty**: High (needs sensor driver port)

**Approach**:
- Port the OV3660 or SC101IOT register init table (~2000 lines of
  vendor C code converted to Rust)
- Use `esp_hal::lcd_cam::cam` (the DVP receive driver, already available
  on `main`)
- Display via direct DMA write to the LCD framebuffer or through a
  format converter

**Prerequisites**: PCLK fix (item 1) so the PSRAM has bandwidth for
both the LCD stream and the camera DMA.

---

## 5. Wi-Fi / BLE / 802.15.4

**Difficulty**: Low (just dependency update) once esp-radio releases

**Approach**: Wait for `esp-radio` crate to publish esp32s31 support
(already on their main branch). Then:
```toml
esp-radio = { version = "1.0", features = ["esp32s31", "wifi", "ble"] }
```

Standard embassy-network or BLE examples should work with minimal
changes.

---

## 6. Continuous touch cursor tracking

**Difficulty**: Low (once framebuffer writes are safe)

**Approach**: Poll GT1151 at 20 Hz; draw XOR-ring cursor at each
position. Requires the framebuffer to be writable without display
disturbance (items 1-2).

---

## 7. ADC fix (buttons)

**Difficulty**: Medium (upstream contribution)

**Approach**: Compare IDF's `esp_adc` component initialization with
esp-hal's S31 ADC driver at the register level. The vendor BSP's
`esp32_s31_adc_calibration.c` may hold clues about what's missing.

**File to study**: `esp-dev-kits/examples/esp32-s31-korvo/examples/
common_components/esp32_s31_korvo/esp32_s31_adc_calibration.c`

---

## Priority recommendation

```
1. PCLK fix          — foundational, fixes bandwidth for everything
2. Uncacheable PSRAM — unlocks dynamic UI
3. LVGL              — polish (depends on 1+2)
4. Camera            — flashy demo feature (depends on 1)
5. Wi-Fi             — easy once esp-radio ships
6. Touch cursor      — polish (depends on 2)
7. ADC / buttons     — upstream contribution
```
