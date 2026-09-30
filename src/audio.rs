//! I2S audio path: ES8389 playback (I2S0 TX, DMA streaming) and dual-mic
//! capture (I2S0 RX), 48 kHz / 16-bit / stereo, SoC as clock master.
//!
//! The codec is an I2S slave sharing one BCLK/LRCLK pair for both
//! directions. The TX unit generates those clocks; the RX unit runs as
//! master off its own divider (same root clock, same ratio), mirroring
//! what the stock driver achieves with a shared-clock full-duplex channel.

use esp_hal::{
    gpio::InputSignal,
    peripherals::{GPIO2, GPIO3, GPIO4, GPIO5, GPIO6},
    i2s::master::{Channels, DataFormat, I2s, TdmConfig},
    time::Rate,
};

use crate::board as b;

/// Convenience for constructing the I2S0 master from main (the DMA channel
/// trait bound is not nameable outside esp-hal).
pub fn build_i2s(
    i2s0: esp_hal::peripherals::I2S0<'static>,
    dma: esp_hal::peripherals::DMA_CH0<'static>,
    mclk: GPIO2<'static>,
) -> Result<I2s<'static, esp_hal::Blocking>, &'static str> {
    let config = TdmConfig::new_tdm_philips()
        .with_sample_rate(Rate::from_hz(b::AUDIO_SAMPLE_RATE))
        .with_data_format(DataFormat::Data16Channel16)
        .with_channels(Channels::STEREO);
    let i2s = I2s::new(i2s0, dma, config).map_err(|_| "I2S config")?;
    Ok(i2s.with_mclk(mclk))
}

/// ~100 ms of stereo 16-bit @ 48 kHz.
const TX_STREAM_LEN: usize = 48000 / 10 * 4;
const TX_CHUNK: usize = 1024;
const RX_STREAM_LEN: usize = 48000 / 10 * 4;
const RX_CHUNK: usize = 1024;

/// 256-entry sine table (Bhaskara approximation, ~1.6% max error — fine
/// for a demonstration tone).
static SINE: [i16; 256] = build_sine();

const fn build_sine() -> [i16; 256] {
    let mut t = [0i16; 256];
    let mut i = 0;
    while i < 256 {
        let x = i as f32 / 256.0;
        let v = if x < 0.5 {
            let y = x * 2.0;
            16.0 * y * (1.0 - y) / (5.0 - 4.0 * y * (1.0 - y))
        } else {
            let y = (x - 0.5) * 2.0;
            -(16.0 * y * (1.0 - y) / (5.0 - 4.0 * y * (1.0 - y)))
        };
        t[i] = (v * 32000.0) as i16;
        i += 1;
    }
    t
}

/// What the generator is currently producing.
#[derive(Clone, Copy, PartialEq)]
pub enum Source {
    Silence,
    /// Steady test tone at the given frequency (Hz), both channels.
    Tone(f32),
    /// Short startup arpeggio; falls back to [`Source::Silence`].
    Chime,
}

pub struct Audio {
    tx_transfer: TxTransfer,
    rx_transfer: RxTransfer,
    phase: f32,
    phase_inc: f32,
    source: Source,
    chime_left: u32,
    /// RMS level of the recent mic audio, 0.0..=1.0.
    pub mic_level: f32,
    /// Extremes of the last mic block (diagnostics).
    pub mic_min: i16,
    pub mic_max: i16,
    mic_scratch: [u8; RX_CHUNK],
}

type TxTransfer =
    esp_hal::i2s::master::I2sTxDmaTransfer<'static, esp_hal::Blocking, esp_hal::dma::DmaTxStreamBuf>;
type RxTransfer =
    esp_hal::i2s::master::I2sRxDmaTransfer<'static, esp_hal::Blocking, esp_hal::dma::DmaRxStreamBuf>;

pub struct AudioPins {
    pub bclk: GPIO3<'static>,
    pub ws: GPIO4<'static>,
    pub dout: GPIO5<'static>,
    pub din: GPIO6<'static>,
}

impl Audio {
    pub fn new(i2s: I2s<'static, esp_hal::Blocking>, pins: AudioPins) -> Result<Self, &'static str> {
        // Loop the wire clocks back into the RX timing inputs so the RX
        // unit can see the same BCLK/WS the codec is timed by. This only
        // touches the input matrix; the TX builder below then drives the
        // same pads as outputs.
        InputSignal::I2S0I_BCK.connect_to(&pins.bclk);
        InputSignal::I2S0I_WS.connect_to(&pins.ws);

        let tx = i2s
            .i2s_tx
            .with_bclk(pins.bclk)
            .with_ws(pins.ws)
            .with_dout(pins.dout)
            .build();
        let rx = i2s.i2s_rx.with_din(pins.din).build();

        let mut tx_buf = esp_hal::dma_tx_stream_buffer!(TX_STREAM_LEN, TX_CHUNK);
        // Half-fill so the stream never underruns at the start.
        tx_buf.push(&[0u8; TX_STREAM_LEN / 2]);
        let tx_transfer = tx.write(tx_buf).map_err(|_| "I2S TX start")?;
        let rx_buf = esp_hal::dma_rx_stream_buffer!(RX_STREAM_LEN, RX_CHUNK);
        let rx_transfer = rx.read(rx_buf).map_err(|_| "I2S RX start")?;

        Ok(Self {
            tx_transfer,
            rx_transfer,
            phase: 0.0,
            phase_inc: 0.0,
            source: Source::Silence,
            chime_left: 0,
            mic_level: 0.0,
            mic_min: 0,
            mic_max: 0,
            mic_scratch: [0; RX_CHUNK],
        })
    }

    pub fn set_source(&mut self, source: Source) {
        self.source = source;
        match source {
            Source::Tone(hz) => {
                self.phase_inc = hz / b::AUDIO_SAMPLE_RATE as f32;
                self.chime_left = 0;
            }
            Source::Chime => {
                self.chime_left = 4 * b::AUDIO_SAMPLE_RATE;
                self.phase_inc = 261.63 / b::AUDIO_SAMPLE_RATE as f32;
            }
            Source::Silence => {
                self.phase_inc = 0.0;
                self.chime_left = 0;
            }
        }
    }

    pub fn source(&self) -> Source {
        self.source
    }

    /// Call every loop tick: keeps the DMA fed and the mic meter fresh.
    pub fn poll(&mut self) {
        let avail = self.rx_transfer.available_bytes();
        if avail > 0 {
            let n = self.rx_transfer.pop(&mut self.mic_scratch);
            if n >= 8 {
                let samples = i16_view(&self.mic_scratch[..n]);
                let mut acc: u64 = 0;
                let mut count = 0u32;
                let mut mn = i16::MAX;
                let mut mx = i16::MIN;
                let mut i = 0;
                while i + 1 < samples.len() {
                    let s = samples[i];
                    acc += (s as i32 * s as i32) as u64;
                    count += 1;
                    mn = mn.min(s);
                    mx = mx.max(s);
                    i += 2;
                }
                self.mic_min = mn;
                self.mic_max = mx;
                if count > 0 {
                    let rms = libm::sqrtf((acc / count as u64) as f32) / 32768.0;
                    let rms = rms.clamp(0.0, 1.0);
                    self.mic_level = self.mic_level * 0.7 + rms * 0.3;
                }
            }
        }

        let free = self.tx_transfer.available_bytes();
        if free >= TX_CHUNK {
            let mut sample_gen = SampleGen {
                phase: self.phase,
                phase_inc: self.phase_inc,
                source: self.source,
                chime_left: self.chime_left,
            };
            let _ = self.tx_transfer.push_with(|chunk| sample_gen.fill(chunk));
            self.phase = sample_gen.phase;
            self.chime_left = sample_gen.chime_left;
        }
    }
}

/// Stateless-per-call sample generator (kept out of `Audio` so the DMA
/// push closure can borrow it independently of the transfer handle).
struct SampleGen {
    phase: f32,
    phase_inc: f32,
    source: Source,
    chime_left: u32,
}

impl SampleGen {
    /// Generate stereo samples into `chunk`; returns bytes written.
    fn fill(&mut self, chunk: &mut [u8]) -> usize {
        let samples = i16_view_mut(chunk);
        let mut written = 0;

        if self.chime_left > 0 {
            let per_note = b::AUDIO_SAMPLE_RATE;
            let note = 3 - (self.chime_left / per_note).min(3);
            let base = [261.63, 329.63, 392.00, 523.25][note as usize];
            self.phase_inc = base / b::AUDIO_SAMPLE_RATE as f32;
        }

        let active = self.source != Source::Silence || self.chime_left > 0;
        let mut i = 0;
        while i + 1 < samples.len() {
            let s = if active {
                let idx = (self.phase * 256.0) as usize & 0xff;
                let raw = SINE[idx] as f32;
                let v = if self.chime_left > 0 {
                    // ~20 ms attack/release per note.
                    let t_in_note = self.chime_left % b::AUDIO_SAMPLE_RATE;
                    let edge = t_in_note.min(b::AUDIO_SAMPLE_RATE - t_in_note);
                    let g = (edge as f32 / 960.0).min(1.0);
                    raw * g
                } else {
                    raw * 0.5
                };
                self.phase += self.phase_inc;
                if self.phase >= 1.0 {
                    self.phase -= 1.0;
                }
                v as i16
            } else {
                0
            };
            samples[i] = s;
            samples[i + 1] = s;
            i += 2;
            written += 4;
            if self.chime_left > 0 {
                self.chime_left -= 1;
            }
        }
        written
    }
}

#[inline]
fn i16_view(bytes: &[u8]) -> &[i16] {
    let n = bytes.len() / 2;
    // SAFETY: DMA buffers are at least 4-byte aligned and lengths are even.
    unsafe { core::slice::from_raw_parts(bytes.as_ptr() as *const i16, n) }
}

#[inline]
fn i16_view_mut(bytes: &mut [u8]) -> &mut [i16] {
    let n = bytes.len() / 2;
    // SAFETY: see i16_view.
    unsafe { core::slice::from_raw_parts_mut(bytes.as_mut_ptr() as *mut i16, n) }
}
