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
        .with_channels(Channels::STEREO)
        .with_signal_loopback(true),
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

### Shared full-duplex clocks

The codec shares one BCLK/LRCLK pair. `TdmConfig::with_signal_loopback(true)`
connects TX's clocks to RX internally and sets RX slave mode. The GPIO matrix
also routes the wire BCLK/WS to RX. Both directions run at 48 kHz, 16-bit stereo.

The vendor ratio-32 coefficient row requires register `0xF0` masked with
`0x73` to receive **0x12**. The previous port wrote zero there; this is now
corrected. Both microphone PGAs use 9.5 dB gain (the requested value 9 uses the
vendor quantisation), and the ADC is explicitly unmuted. DAC-reference
routing is disabled so both channels carry microphone input. The codec is
initialised after I2S starts, because its clocks are derived from SCLK.
RX EOF uses an aligned period of 1024, replacing the HAL streaming default
0xfffe; the fixed-size ring requires complete descriptor payloads. Speaker output starts silent at -30 dB after user feedback that the
old startup chime was too loud.

## Continuous DMA

`audio_ring.rs` implements `DmaTxBuffer` / `DmaRxBuffer` with permanent
64 KiB descriptor rings. The DMA completed-descriptor addresses identify
1024-byte blocks; the main loop reads capture and refills playback in batches
up to 8192 bytes. Cache maintenance uses HAL DMA-aligned wrappers.

The previous `DmaTxStreamBuf` and `DmaRxStreamBuf` are linked streams rather
than permanent rings: exhausting them can stop DMA. They stalled during
boot/filesystem work. The ring implementation leaves clocks running, clears
played blocks to silence, and counts producer underruns and capture overruns.
Elapsed time resolves completed full ring wraps during long filesystem calls.

A board test recorded 1,940,480 PCM bytes over approximately ten seconds and
replayed the resulting WAV to completion, with zero RX overruns. Audible
speech quality requires a separate user check. See
[SD audio and recording](14-sd-audio-recording.md).

## Tone generation

Sine table (256 entries, Bhaskara approximation ~1.6% error — fine for
a demo). Sources:
- `Source::Chime` — 4-note arpeggio (C-E-G-C) with attack/release envelope
- `Source::Tone(hz)` — steady sine at given frequency
- `Source::Silence` — zero output
- `Source::File` — PCM supplied by the SD playback/decoder path

## Volume control

```rust
codec.set_volume_db(-30.0)?;  // -95.5 to +32 dB, mapped to reg 0x46/0x47
codec.set_mic_gain(9)?;     // 0-36 dB, mapped to both 0x72 and 0x73
```
