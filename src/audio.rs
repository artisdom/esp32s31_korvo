//! I2S audio path: ES8389 playback (I2S0 TX, DMA streaming) and dual-mic
//! capture (I2S0 RX), 48 kHz / PCM16 stereo in 32-bit wire slots, SoC as clock master.
//!
//! The codec is an I2S slave sharing one BCLK/LRCLK pair for both
//! directions. The TX unit generates those clocks; the RX unit runs as
//! slave from the TX clock signals through the hardware clock loopback.

extern crate alloc;
use alloc::boxed::Box;

use esp_hal::{
    gpio::InputSignal,
    i2s::master::{Channels, DataFormat, I2s, TdmConfig},
    peripherals::{GPIO2, GPIO3, GPIO4, GPIO5, GPIO6},
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
        .with_channels(Channels::STEREO)
        .with_signal_loopback(true);
    let i2s = I2s::new(i2s0, dma, config).map_err(|_| "I2S config")?;
    // Keep PCM16 in DMA but use 32-bit wire slots: BCLK = 64 * 48 kHz.
    // The current S31 HAL derives BCLK and WS from data width, and swaps
    // data/slot register widths for mixed formats. Apply the standard-slot
    // settings from IDF's S31 i2s_ll_* helpers explicitly before DMA starts.
    // MCLK remains 256 * Fs; BCLK divides it by four (register stores N-1).
    let regs = esp_hal::peripherals::I2S0::regs();
    regs.tx_conf1().modify(|_, w| unsafe {
        w.tx_bits_mod().bits(15);
        w.tx_tdm_chan_bits().bits(31);
        w.tx_half_sample_bits().bits(31);
        w.tx_tdm_ws_width().bits(31)
    });
    regs.rx_conf1().modify(|_, w| unsafe {
        w.rx_bits_mod().bits(15);
        w.rx_tdm_chan_bits().bits(31);
        w.rx_half_sample_bits().bits(31);
        w.rx_tdm_ws_width().bits(31)
    });
    regs.tx_conf().modify(|_, w| unsafe {
        w.tx_chan_equal().clear_bit();
        w.tx_bck_div_num().bits(3);
        w.tx_update().set_bit()
    });
    while regs.tx_conf().read().tx_update().bit_is_set() {}
    regs.rx_conf()
        .modify(|_, w| w.rx_mono_fst_vld().clear_bit().rx_update().set_bit());
    while regs.rx_conf().read().rx_update().bit_is_set() {}
    Ok(i2s.with_mclk(mclk))
}

/// ~100 ms of stereo 16-bit @ 48 kHz.

const TX_CHUNK: usize = 8192;

// Drain up to half the ring each pass so SD/radio load cannot limit capture
// throughput to 8192 bytes times the UI loop frequency.
const RX_CHUNK: usize = 32768;

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
    File,
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
    mic_scratch: Box<[u8]>,
}

type TxTransfer = esp_hal::i2s::master::I2sTxDmaTransfer<
    'static,
    esp_hal::Blocking,
    crate::audio_ring::Ring<false>,
>;
type RxTransfer = esp_hal::i2s::master::I2sRxDmaTransfer<
    'static,
    esp_hal::Blocking,
    crate::audio_ring::Ring<true>,
>;

pub struct AudioPins {
    pub bclk: GPIO3<'static>,
    pub ws: GPIO4<'static>,
    pub dout: GPIO5<'static>,
    pub din: GPIO6<'static>,
}

impl Audio {
    pub fn new(
        i2s: I2s<'static, esp_hal::Blocking>,
        pins: AudioPins,
    ) -> Result<Self, &'static str> {
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

        let (rx_bytes, rx_desc, tx_bytes, tx_desc) = esp_hal::dma_buffers_chunk_size!(
            crate::audio_ring::LEN,
            crate::audio_ring::LEN,
            crate::audio_ring::CHUNK
        );
        let tx_buf = crate::audio_ring::Ring::<false>::new(tx_bytes, tx_desc);
        let rx_buf = crate::audio_ring::Ring::<true>::new(rx_bytes, rx_desc);
        let tx_transfer = tx.write(tx_buf).map_err(|_| "I2S TX start")?;
        let rx_transfer = rx.read(rx_buf).map_err(|_| "I2S RX start")?;
        // HAL starts generic RX with 0xfffe (usize::MAX - 1 truncated).
        // Keep the EOF period a multiple of complete stereo frames and DMA
        // blocks. This avoids partial descriptor payloads in a fixed-size ring.
        // 1024 is aligned whether the EOF unit is bytes (IDF helper docs) or
        // sample words (PAC field docs).
        let regs = esp_hal::peripherals::I2S0::regs();
        regs.rx_conf().modify(|_, w| w.rx_start().clear_bit());
        regs.rxeof_num()
            .write(|w| unsafe { w.rx_eof_num().bits(crate::audio_ring::CHUNK as u16) });
        regs.rx_conf().modify(|_, w| w.rx_update().set_bit());
        while regs.rx_conf().read().rx_update().bit_is_set() {}
        regs.rx_conf().modify(|_, w| w.rx_start().set_bit());

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
            mic_scratch: crate::psram_buffer::zeroed(RX_CHUNK),
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
            Source::Silence | Source::File => {
                self.phase_inc = 0.0;
                self.chime_left = 0;
            }
        }
    }

    pub fn source(&self) -> Source {
        self.source
    }

    /// Call every loop tick: keeps the DMA fed and the mic meter fresh.
    pub fn free(&mut self) -> usize {
        self.tx_transfer.available()
    }
    pub fn queued(&mut self) -> usize {
        self.tx_transfer.queued()
    }
    pub fn queue(&mut self, bytes: &[u8]) -> usize {
        self.tx_transfer.push(bytes)
    }

    pub fn debug_samples(&self) {
        esp_println::println!("mic samples (left/right):");
        for frame in self.mic_scratch[..128].chunks_exact(4) {
            esp_println::println!(
                "{}/{}",
                i16::from_le_bytes([frame[0], frame[1]]),
                i16::from_le_bytes([frame[2], frame[3]])
            );
        }
    }
    pub fn log_stats(&self) {
        esp_println::println!(
            "I2S: TX blocks={} RX blocks={} underruns={} overruns={}",
            crate::audio_ring::TX_DONE.load(core::sync::atomic::Ordering::Relaxed),
            crate::audio_ring::RX_DONE.load(core::sync::atomic::Ordering::Relaxed),
            self.tx_transfer.lost,
            self.rx_transfer.lost
        );
    }

    pub fn poll(&mut self) -> &[u8] {
        crate::audio_ring::poll_completions();
        self.tx_transfer.clear_played();
        let mut captured = 0;
        let avail = self.rx_transfer.available();
        if avail > 0 {
            let n = self.rx_transfer.pop(&mut self.mic_scratch);
            captured = n;
            if n >= 8 {
                let samples = &self.mic_scratch[..n];
                let mut acc: u64 = 0;
                let mut count = 0u32;
                let mut mn = i16::MAX;
                let mut mx = i16::MIN;
                let mut i = 0;
                while i + 1 < samples.len() {
                    let s = i16::from_le_bytes([samples[i], samples[i + 1]]);
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

        let free = self.tx_transfer.available();
        for _ in 0..if self.source != Source::File {
            free / TX_CHUNK
        } else {
            0
        } {
            let mut sample_gen = SampleGen {
                phase: self.phase,
                phase_inc: self.phase_inc,
                source: self.source,
                chime_left: self.chime_left,
            };
            let mut chunk = [0u8; TX_CHUNK];
            sample_gen.fill(&mut chunk);
            self.tx_transfer.push(&chunk);
            self.phase = sample_gen.phase;
            self.chime_left = sample_gen.chime_left;
            if self.source == Source::Chime && self.chime_left == 0 {
                self.source = Source::Silence;
            }
        }
        &self.mic_scratch[..captured]
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
        let mut written = 0;

        if self.chime_left > 0 {
            let per_note = b::AUDIO_SAMPLE_RATE;
            let note = 3 - (self.chime_left / per_note).min(3);
            let base = [261.63, 329.63, 392.00, 523.25][note as usize];
            self.phase_inc = base / b::AUDIO_SAMPLE_RATE as f32;
        }

        for frame in chunk.chunks_exact_mut(4) {
            let s = if matches!(self.source, Source::Tone(_)) || self.chime_left > 0 {
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
            frame[..2].copy_from_slice(&s.to_le_bytes());
            frame[2..].copy_from_slice(&s.to_le_bytes());
            written += 4;
            if self.chime_left > 0 {
                self.chime_left -= 1;
            }
        }
        written
    }
}
