# Audio: ES8389 Codec + I2S DMA

## Overview

The board has an ES8389 stereo codec connected via I2C (control) and I2S
(data). Two NS4150B 3 W class-D amplifiers drive the speakers (enabled by
GPIO7). Two analog microphones feed the codec's ADCs.

## I2C control interface

- **7-bit address: 0x10** (not 0x20 — the vendor macro
  `ES8389_CODEC_DEFAULT_ADDR` is the 8-bit write form)
- Register auto-increment for multi-byte reads
- Verified working with `write_read()` at 400 kHz

## Codec initialization (48 kHz, slave mode, SCLK-derived clocks)

The codec is an **I2S slave** — the SoC is the clock master on BCLK/LRCLK.
MCLK is not needed (the codec derives internal clocks from SCLK). The full
init sequence is a register-level port of `esp_codec_dev`'s `es8389.c`:

```
Register map highlights (full sequence in src/es8389.rs):
0x00 = reset register (write 0x7E to reset, 0x01 to start)
0x01 = misc control (slave mode bit 0, clock source bits 6-7)
0x02 = clock manager (SCLK-derived when bit 6 set)
0x0C = I2S format select (bits 5-7: 0=I2S Philips)
0x20 = ADC serial port (bits 5-7: data length; bits 2-4: format)
0x40 = DAC serial port (same layout as 0x20)
0x46/0x47 = DAC L/R volume (0xBF ≈ 0 dB)
0x72/0x73 = PGA gain (mic input level)
0x7D = PA power control
```

### Sample-rate coefficient block

For 48 kHz / 16-bit / SCLK-derived (no MCLK), the vendor coefficient
table selects ratio 32:

```
0x04..0x0A = clock dividers (0x00, 0x45, 0xA4, 0xD0, 0x10, 0xD1, 0x80)
0x0B..0x0D = BCLK/LRCLK dividers
0x0F..0x11 = ADC/DAC clock config
0x16..0x19 = chip font / MCLK / DAC / ADC clocks
0x21..0x22 = ADC HPF / ALC
0x30, 0x41..0x43 = DAC oversampling / mixer
0xF0..0xF1 = chip misc / CSM state
```

### Bias cycle

After the coefficient block, the codec needs a bias standby → bias on
cycle:
```
bias_standby: 0x40[0:1]=3, 0x10=0xD4, wait 70ms, 0x61=0x59, 0x64=0x00,
              0x03=0x00, 0x00=0x7E, 0x40[0:1]=0
bias_on:      0x4D=0x02, 0x69[5]=1, 0x61=0xD9, 0x64=0x8F, 0x10=0xE4,
              0x00=0x01, 0x03=0xC3, 0x24=0x6A, 0x25=0x0A
```

## I2S configuration (esp-hal)

```rust
let i2s = I2s::new(
    peripherals.I2S0,
    peripherals.DMA_CH0,     // AHB GDMA channel
    TdmConfig::new_tdm_philips()
        .with_sample_rate(Rate::from_hz(48_000))
        .with_data_format(DataFormat::Data16Channel16)
        .with_channels(Channels::STEREO),
)?;
let i2s = i2s.with_mclk(peripherals.GPIO2);

let tx = i2s.i2s_tx
    .with_bclk(peripherals.GPIO3)
    .with_ws(peripherals.GPIO4)
    .with_dout(peripherals.GPIO5)
    .build();

let rx = i2s.i2s_rx
    .with_din(peripherals.GPIO6)
    .build();
```

### Full-duplex clock caveat

The codec is a slave sharing one BCLK/LRCLK pair for both directions.
The TX unit generates those clocks; the RX unit runs as master off its
own divider (same root clock, same ratio). The timings are nominally
identical but not phase-locked — mic capture quality is not guaranteed.
Playback is unaffected.

### Loopback for RX timing

```rust
InputSignal::I2S0I_BCK.connect_to(&pins.bclk);
InputSignal::I2S0I_WS.connect_to(&pins.ws);
```

This routes the TX-generated BCLK/WS into the RX timing inputs so the
RX unit sees the same clocks the codec is timed by. Call these BEFORE
the TX builder consumes the pins.

## DMA streaming

Use `dma_tx_stream_buffer!` and `dma_rx_stream_buffer!` macros for
statically-allocated circular buffers:

```rust
// ~100 ms of stereo 16-bit @ 48 kHz
dma_tx_stream_buffer!(48000 / 10 * 4, 1024);
```

The TX transfer's `push_with()` closure generates samples; the RX
transfer's `pop()` drains mic data. Both are polled from the main loop.

## Tone generation

Sine table (256 entries, Bhaskara approximation ~1.6% error — fine for
a demo). Sources:
- `Source::Chime` — 4-note arpeggio (C-E-G-C) with attack/release envelope
- `Source::Tone(hz)` — steady sine at given frequency
- `Source::Silence` — zero output

## Volume control

```rust
codec.set_volume_db(-6.0)?;  // -95.5 to +32 dB, mapped to reg 0x46/0x47
codec.set_mic_gain(21)?;     // 0-36 dB, mapped to reg 0x72
```
