> Updated 2026-10-02: the issue proposals below include historical hypotheses.
> LCD DMA EOF rates were not reliable PCLK evidence; the application now
> counts LCD VSYNC. Enabling the missing RGB transfer buffer stopped LCD
> underruns and user-observed drift. An upstream DPI initialization fix
> should enable that buffer for S31. Claims of consumed EOF flags and
> missing ADC initialization below are not established bugs (raw zero is
> valid button idle). See [current evidence](10-lcd-known-issues.md).

# Upstream Contributions & Issues to File

This document lists everything discovered that should be contributed
back to esp-rs/esp-hal, organized by priority.

## Patches already in the local branch

Branch: `s31-adc-clock-patch` in `/home/nws/w/esp32/esp-rs/esp-hal`

### Patch 1: S31 LCD DPI wire mode + always-out enable

**File**: `esp-hal/src/lcd_cam/lcd/dpi.rs`

**Problem**: esp-hal never sets `lcd_wire_mode` (16-bit bus) or
`lcd_always_out_en` for RGB mode on S31. ESP-IDF's `rgb_panel_init`
programs both. Without them, the FIFO is read with an 8-bit wire and
the panel shows a continuously skewed/rolling image in 16-bit mode.

**Fix**:
```rust
#[cfg(esp32s31)]
self.regs().lcd_misc().modify(|_, w| unsafe {
    w.lcd_wire_mode().bits(if config.format.enable_2byte_mode { 1 } else { 0 })
});
self.regs().lcd_user().modify(|_, w| w.lcd_always_out_en().set_bit());
```

**Ready to PR**: Yes (tested on hardware)

---

### Patch 2: ChannelTx interrupt control (public)

**File**: `esp-hal/src/dma/mod.rs`

**Problem**: `ChannelTx<Blocking, CH>` has interrupt control methods
(`set_interrupt_handler`, `listen_out`, `clear_out`) that are
`pub(crate)`. Applications that need custom DMA interrupt handlers
(like the LCD bounce-buffer driver) can't access them.

**Fix**: Three thin public wrappers:
```rust
pub fn set_interrupt_handler_pub(&mut self, handler: InterruptHandler)
pub fn listen_out_pub(&self, interrupts: impl Into<EnumSet<DmaTxInterrupt>>)
pub fn clear_out_pub(&self, interrupts: impl Into<EnumSet<DmaTxInterrupt>>)
```

**Notes**: Must be inside the `ChannelTx<Blocking, CH>` impl block,
not the `Async` block (they call `self.set_interrupt_handler` which
is only defined for Blocking).

**Ready to PR**: Yes (works, but naming could be improved — perhaps
just make the existing pub(crate) methods pub?)

---

### Patch 3: Dpi::tx_channel_mut

**File**: `esp-hal/src/lcd_cam/lcd/dpi.rs`

**Problem**: `Dpi::send()` consumes the driver, making it impossible
to configure the TX DMA channel (bind interrupt handlers, set burst
mode) before starting the transfer.

**Fix**:
```rust
pub fn tx_channel_mut(&mut self) -> &mut ChannelTx<Blocking, ErasedTxChannel<'d>> {
    &mut self.tx_channel
}
```

**Ready to PR**: Yes (trivial, tested)

---

## Issues to file (bugs found, no fix yet)

### Issue A: S31 ADC always reads 0

**Component**: `esp-hal/src/analog/adc/s31.rs`

**Description**: All ADC conversions complete but the data register
always reads 0. The upstream HIL test (`hil-test/src/bin/adc.rs`) only
asserts `value <= MAX_RAW`, so a constant 0 passes CI.

**Attempted fixes** (all in the local branch, none worked):
- `LP_PERI_CLKRST.adc_ctrl.lp_adc_clk_en = 1` + reset pulse
- `sar1_clk_pos_sel = 1`
- regi2c writes to SAR blocks 0x10/0x11 (CAL_DONE, EN_RAW_DATA)

**Suspected cause**: Bootloader-side analog initialization that
`esp-bootloader-esp-idf` doesn't replicate. The vendor BSP ships its
own runtime SAR weight calibration for this chip.

**Reproduction**: `esp-hal` main @ 0e9fe8d, ESP32-S31-Korvo-1, read
GPIO42 (ADC1_CH0) → always 0.

---

### Issue B: S31 PCLK 2× bug

**Component**: `esp-hal/src/lcd_cam/lcd/mod.rs` (clock configuration)

**Description**: Requesting 18 MHz PCLK produces 36 MHz actual.
The `lcd_clk_equ_sysclk = 1` path doubles the LCD_CLK on S31.

**Evidence**: DMA descriptor rate shows 355 segments/sec where 177 is
expected; frame rate is ~71 Hz where ~35 Hz is expected.

**IDF reference**: `lcd_hal_cal_pclk_freq` in
`components/esp_hal_lcd/lcd_hal.c` — always uses `mo=2` prescaler,
never `equ_sysclk=1`. Comment: "due to some unstable hardware issue".

**Failed fix attempt**: Applying IDF's mo=2 approach (`equ_sysclk=0`,
`lcd_clkcnt_n=1`, doubled LCD_CLK target) made things WORSE —
bidirectional horizontal drift + vertical drift. The S31's
`lcd_clkcnt_n` has different semantics than S3's.

**What's needed**: TRM-level analysis of the S31 LCD_CAM clock chain,
or a register dump from a running IDF build.

---

### Issue C: AXI GDMA suc_eof flags consumed after first use

**Component**: `esp-hal/src/dma/engine/axi_gdma.rs` (or hardware)

**Description**: When using per-descriptor EOF interrupts on the AXI
GDMA, only the first 1-2 descriptors fire after starting a transfer.
Subsequent passes through the ring never re-trigger EOF.

**Evidence**: Descriptor trace `[8, 36, 8, 36, ...]` — only 2 of 6
EOF-flagged descriptors ever fire, alternating.

**Impact**: Makes EOF-driven bounce-buffer architectures impossible
on S31.

---

### Issue D: esp-bootloader-esp-idf may need S31 analog init

**Component**: `esp-bootloader-esp-idf`

**Description**: The vendor BSP includes
`esp32_s31_adc_calibration.c` which performs runtime SAR ADC
calibration via regi2c. This suggests the bootloader (or first-stage
init) is expected to set up analog blocks that neither
`esp-bootloader-esp-idf` nor `esp-hal` currently handle.

**Related**: Issue A (ADC reads 0).

---

## Documentation improvements for esp-rs

1. **FrameTiming field semantics**: `horizontal_blank_front_porch`
   includes the HSYNC pulse width (not documented; source code only)
2. **I2S full-duplex**: Document that TX and RX units run independent
   clock dividers; there is no shared-clock mode for slave codecs
3. **embassy_time::Timer in block_on**: Document that `Timer` panics
   when polled from `embassy_futures::block_on` (needs executor waker);
   use `Instant` spin-delay instead
4. **riscv-rt 0.16 linker**: Document that apps must pass
   `-Tlinkall.x` themselves (CI does this but templates don't)
