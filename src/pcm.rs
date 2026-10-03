//! Byte-level WAV helpers and streaming conversion to 48 kHz stereo PCM.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Format {
    pub rate: u32,
    pub channels: usize,
    pub bits: usize,
}
impl Format {
    pub fn parse(bytes: &[u8]) -> Result<Self, &'static str> {
        if bytes.len() < 16 || u16::from_le_bytes([bytes[0], bytes[1]]) != 1 {
            return Err("WAV must be PCM integer");
        }
        let format = Self {
            channels: u16::from_le_bytes([bytes[2], bytes[3]]) as usize,
            rate: u32::from_le_bytes(bytes[4..8].try_into().unwrap()),
            bits: u16::from_le_bytes([bytes[14], bytes[15]]) as usize,
        };
        if !matches!(format.channels, 1 | 2)
            || !matches!(format.bits, 8 | 16 | 24 | 32)
            || !(8000..=96000).contains(&format.rate)
        {
            return Err("unsupported WAV format");
        }
        if u16::from_le_bytes([bytes[12], bytes[13]]) as usize != format.frame_bytes() {
            return Err("invalid WAV alignment");
        }
        Ok(format)
    }
    pub fn frame_bytes(self) -> usize {
        self.channels * self.bits / 8
    }
    pub fn sample(self, b: &[u8]) -> i16 {
        match self.bits {
            8 => (b[0] as i16 - 128) << 8,
            16 => i16::from_le_bytes([b[0], b[1]]),
            24 => i16::from_le_bytes([b[1], b[2]]),
            32 => i16::from_le_bytes([b[2], b[3]]),
            _ => 0,
        }
    }
}
pub fn wav_header(bytes: u32) -> [u8; 44] {
    let mut h = [0u8; 44];
    h[..4].copy_from_slice(b"RIFF");
    h[4..8].copy_from_slice(&(bytes + 36).to_le_bytes());
    h[8..16].copy_from_slice(b"WAVEfmt ");
    h[16..20].copy_from_slice(&16u32.to_le_bytes());
    h[20..24].copy_from_slice(&[1, 0, 2, 0]);
    h[24..28].copy_from_slice(&48000u32.to_le_bytes());
    h[28..32].copy_from_slice(&192000u32.to_le_bytes());
    h[32..36].copy_from_slice(&[4, 0, 16, 0]);
    h[36..40].copy_from_slice(b"data");
    h[40..44].copy_from_slice(&bytes.to_le_bytes());
    h
}
extern crate alloc;
use alloc::boxed::Box;
#[path = "resampler_coeffs.rs"]
mod coefficients;
const TAPS: usize = 64;
const PHASES: usize = 64;
const LATENCY: usize = 31;

#[inline(always)]
fn multiply_add(a: f32, b: f32, c: f32) -> f32 {
    #[cfg(target_arch = "riscv32")]
    {
        let result;
        // The firmware target has F. Saving fcsr and all FP registers at
        // RTOS switches is required; the project setup applies that fix.
        unsafe {
            core::arch::asm!("fmadd.s {result}, {a}, {b}, {c}",
                result = lateout(freg) result,
                a = in(freg) a, b = in(freg) b, c = in(freg) c,
                options(nomem, nostack));
        }
        result
    }
    #[cfg(not(target_arch = "riscv32"))]
    {
        libm::fmaf(a, b, c)
    }
}

pub struct Resampler {
    phase: u32,
    rate: u32,
    history: [(f32, f32); TAPS],
    head: usize,
    warmup: usize,
    downsample: Option<Box<[i16]>>,
    prepared: Option<Box<[f32]>>,
    phase_step: u32,
    finished: bool,
}
impl Default for Resampler {
    fn default() -> Self {
        Self {
            phase: 0,
            rate: 0,
            history: [(0.0, 0.0); TAPS],
            head: 0,
            warmup: LATENCY,
            downsample: None,
            prepared: None,
            phase_step: 1,
            finished: false,
        }
    }
}
impl Resampler {
    /// Configure once before clocks consume the file. Rates above 48 kHz
    /// need a narrower anti-alias filter; MP3 uses the constant flash table.
    pub fn configure(&mut self, rate: u32) {
        assert!((8000..=96000).contains(&rate));
        if self.rate == rate {
            return;
        }
        *self = Self::default();
        self.rate = rate;
        if rate > 48000 {
            let mut table = crate::psram_buffer::samples((PHASES + 1) * TAPS);
            let cutoff = 0.47 * 48000.0 / rate as f64;
            for phase in 0..=PHASES {
                let mut values = [0.0f64; TAPS];
                let mut sum = 0.0;
                for (tap, value) in values.iter_mut().enumerate() {
                    let x = tap as f64 - (31.0 + phase as f64 / PHASES as f64);
                    let p = core::f64::consts::PI * x;
                    let window = 0.42 + 0.5 * libm::cos(p / 32.0) + 0.08 * libm::cos(p / 16.0);
                    *value = window
                        * if x == 0.0 {
                            2.0 * cutoff
                        } else {
                            libm::sin(2.0 * cutoff * p) / p
                        };
                    sum += *value;
                }
                let mut total = 0i32;
                for (tap, value) in values.iter().enumerate() {
                    let q = libm::round(value / sum * 32768.0) as i16;
                    table[phase * TAPS + tap] = q;
                    total += q as i32;
                }
                table[phase * TAPS + 31] += (32768 - total) as i16;
            }
            self.downsample = Some(table);
        }
        if rate != 48000 {
            let (mut a, mut b) = (rate, 48000);
            while b != 0 {
                (a, b) = (b, a % b);
            }
            self.phase_step = a;
            let count = 48000 / a;
            // All nine MP3 rates use at most 640 phases. Unusual WAV rates
            // retain the interpolated kernel rather than allocating megabytes.
            if count <= 1024 {
                let mut prepared = crate::psram_buffer::floats(count as usize * TAPS);
                for phase in 0..count {
                    let position = phase * a * PHASES as u32;
                    let index = (position / 48000) as usize;
                    let weight = (position % 48000) as f32 / 48000.0;
                    let (first, next) = self.coefficients(index);
                    for tap in 0..TAPS {
                        prepared[phase as usize * TAPS + tap] = multiply_add(
                            next[tap] as f32 - first[tap] as f32,
                            weight,
                            first[tap] as f32,
                        ) * (1.0 / 32768.0);
                    }
                }
                self.prepared = Some(prepared);
            }
        }
    }
    fn coefficients(&self, index: usize) -> (&[i16], &[i16]) {
        if let Some(table) = &self.downsample {
            (
                &table[index * TAPS..(index + 1) * TAPS],
                &table[(index + 1) * TAPS..(index + 2) * TAPS],
            )
        } else {
            (
                &coefficients::COEFFICIENTS[index],
                &coefficients::COEFFICIENTS[index + 1],
            )
        }
    }
    /// Streaming 64-tap, interpolated polyphase sinc conversion. Channels and
    /// fractional phase stay independent across decoder and filesystem blocks.
    /// 48 kHz passes through unchanged. `out` needs at least 24 bytes.
    #[cfg_attr(target_arch = "riscv32", esp_hal::ram)]
    pub fn frame(&mut self, left: i16, right: i16, rate: u32, out: &mut [u8]) -> usize {
        self.configure(rate);
        if rate == 48000 {
            out[..2].copy_from_slice(&left.to_le_bytes());
            out[2..4].copy_from_slice(&right.to_le_bytes());
            return 4;
        }
        self.head = (self.head + TAPS - 1) % TAPS;
        self.history[self.head] = (left as f32, right as f32);
        if self.warmup > 0 {
            self.warmup -= 1;
            return 0;
        }
        self.phase += 48000;
        let mut n = 0;
        while self.phase >= rate {
            self.phase -= rate;
            let mut l = 0.0f32;
            let mut r = 0.0f32;
            if let Some(prepared) = &self.prepared {
                let start = (self.phase / self.phase_step) as usize * TAPS;
                // Independent accumulators keep the FPU pipeline busy instead
                // of waiting for one sum after every tap. Each channel remains
                // separate; tests check the pairwise sum against an f64 reference.
                let mut left_sum = [0.0; 4];
                let mut right_sum = [0.0; 4];
                for (group, weights) in prepared[start..start + TAPS].chunks_exact(4).enumerate() {
                    for lane in 0..4 {
                        let (left, right) = self.history[(self.head + group * 4 + lane) % TAPS];
                        left_sum[lane] = multiply_add(left, weights[lane], left_sum[lane]);
                        right_sum[lane] = multiply_add(right, weights[lane], right_sum[lane]);
                    }
                }
                l = (left_sum[0] + left_sum[1]) + (left_sum[2] + left_sum[3]);
                r = (right_sum[0] + right_sum[1]) + (right_sum[2] + right_sum[3]);
            } else {
                let position = self.phase * PHASES as u32;
                let index = (position / 48000) as usize;
                let weight = (position % 48000) as f32 / 48000.0;
                let (a, b) = self.coefficients(index);
                for tap in 0..TAPS {
                    let c = multiply_add(b[tap] as f32 - a[tap] as f32, weight, a[tap] as f32)
                        * (1.0 / 32768.0);
                    let (left, right) = self.history[(self.head + tap) % TAPS];
                    l = multiply_add(left, c, l);
                    r = multiply_add(right, c, r);
                }
            }
            let left = libm::floorf(l + 0.5).clamp(i16::MIN as f32, i16::MAX as f32) as i16;
            let right = libm::floorf(r + 0.5).clamp(i16::MIN as f32, i16::MAX as f32) as i16;
            out[n..n + 2].copy_from_slice(&left.to_le_bytes());
            out[n + 2..n + 4].copy_from_slice(&right.to_le_bytes());
            n += 4;
        }
        n
    }
    /// Supply zero lookahead and remove the filter's startup latency. This
    /// preserves floor(input_frames * 48000 / rate), including very short files.
    pub fn finish(&mut self, out: &mut [u8]) -> usize {
        if self.finished || self.rate == 0 || self.rate == 48000 {
            return 0;
        }
        self.finished = true;
        let mut n = 0;
        for _ in 0..LATENCY {
            n += self.frame(0, 0, self.rate, &mut out[n..]);
        }
        n
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hardware_float_filter_stays_within_one_pcm_step_of_f64_reference() {
        let mut seed = 0x12345678u32;
        for rate in [
            8000, 11025, 11717, 12000, 16000, 22050, 24000, 32000, 44100, 48777, 64000, 96000,
        ] {
            let mut resampler = Resampler::default();
            let mut out = [0; 24];
            for _ in 0..1000 {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                let left = (seed >> 16) as i16;
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                let right = (seed >> 16) as i16;
                let n = resampler.frame(left, right, rate, &mut out);
                if n == 0 {
                    continue;
                }
                let pos = resampler.phase * PHASES as u32;
                let index = (pos / 48000) as usize;
                let weight = (pos % 48000) as f64 / 48000.0;
                let (a, b) = if let Some(table) = &resampler.downsample {
                    (
                        &table[index * TAPS..(index + 1) * TAPS],
                        &table[(index + 1) * TAPS..(index + 2) * TAPS],
                    )
                } else {
                    (
                        &coefficients::COEFFICIENTS[index][..],
                        &coefficients::COEFFICIENTS[index + 1][..],
                    )
                };
                let mut reference = [0.0f64; 2];
                for tap in 0..TAPS {
                    let c = (a[tap] as f64 + (b[tap] as f64 - a[tap] as f64) * weight) / 32768.0;
                    let (left, right) = resampler.history[(resampler.head + tap) % TAPS];
                    reference[0] += left as f64 * c;
                    reference[1] += right as f64 * c;
                }
                for channel in 0..2 {
                    let expected =
                        libm::floor(reference[channel] + 0.5).clamp(-32768.0, 32767.0) as i32;
                    let sample = i16::from_le_bytes(
                        out[n - 4 + channel * 2..n - 2 + channel * 2]
                            .try_into()
                            .unwrap(),
                    ) as i32;
                    assert!(
                        (sample - expected).abs() <= 1,
                        "{rate} Hz channel {channel}: {sample} vs {expected}"
                    );
                }
            }
        }
    }
    #[test]
    fn full_scale_dc_keeps_both_channels_in_range() {
        for rate in [8000, 44100, 64000, 96000] {
            let mut resampler = Resampler::default();
            let mut out = [0; 24];
            for i in 0..200 {
                let n = resampler.frame(32767, -32768, rate, &mut out);
                if i > 100 {
                    for frame in out[..n].chunks_exact(4) {
                        assert_eq!(i16::from_le_bytes(frame[..2].try_into().unwrap()), 32767);
                        assert_eq!(i16::from_le_bytes(frame[2..].try_into().unwrap()), -32768);
                    }
                }
            }
        }
    }
    fn convert(rate: u32, frequency: f64) -> alloc::vec::Vec<i16> {
        let mut resampler = Resampler::default();
        let mut out = [0u8; 24];
        let mut samples = alloc::vec::Vec::new();
        for frame in 0..rate / 5 {
            let left = (16000.0
                * libm::sin(2.0 * core::f64::consts::PI * frequency * frame as f64 / rate as f64))
                as i16;
            let n = resampler.frame(left, -left, rate, &mut out);
            for stereo in out[..n].chunks_exact(4) {
                let l = i16::from_le_bytes(stereo[..2].try_into().unwrap());
                let r = i16::from_le_bytes(stereo[2..].try_into().unwrap());
                assert!((l as i32 + r as i32).abs() <= 1, "channels crossed");
                samples.push(l);
            }
        }
        let mut tail = [0u8; 744];
        let n = resampler.finish(&mut tail);
        samples.extend(
            tail[..n]
                .chunks_exact(4)
                .map(|f| i16::from_le_bytes(f[..2].try_into().unwrap())),
        );
        assert_eq!(resampler.finish(&mut tail), 0);
        assert_eq!(samples.len(), 9600);
        samples
    }
    fn amplitude(samples: &[i16], frequency: f64) -> f64 {
        let (mut real, mut imag) = (0.0, 0.0);
        for (i, &sample) in samples.iter().enumerate() {
            let p = 2.0 * core::f64::consts::PI * frequency * i as f64 / 48000.0;
            real += sample as f64 * libm::cos(p);
            imag += sample as f64 * libm::sin(p);
        }
        2.0 * libm::sqrt(real * real + imag * imag) / samples.len() as f64
    }
    #[test]
    fn filter_preserves_passband_and_suppresses_images_and_aliases() {
        for (rate, frequency, image) in [
            (16000, 5000.0, 11000.0),
            (32000, 10000.0, 22000.0),
            (44100, 18000.0, 21900.0),
        ] {
            let samples = convert(rate, frequency);
            // A coherent middle window excludes the zero-padded edges.
            let samples = &samples[2400..7200];
            let pass = amplitude(samples, frequency);
            assert!(
                (pass / 16000.0 - 1.0).abs() < 0.025,
                "passband {rate}: {pass}"
            );
            let stop = amplitude(samples, image);
            assert!(stop < pass / 1000.0, "image {rate}: {stop}/{pass}");
        }
        let samples = convert(96000, 35000.0);
        assert!(
            amplitude(&samples[2400..7200], 13000.0) < 16.0,
            "downsampling aliases were not filtered"
        );
    }
    #[test]
    fn exact_duration_short_streams_all_rates_and_48khz_bit_exact() {
        for rate in [
            8000, 11025, 12000, 16000, 22050, 24000, 32000, 44100, 48000, 64000, 96000,
        ] {
            for length in [1, 17, 31, 32, 33, 100] {
                let mut r = Resampler::default();
                let mut out = [0u8; 24];
                let mut bytes = 0;
                for _ in 0..length {
                    bytes += r.frame(i16::MIN, i16::MAX, rate, &mut out);
                    if rate == 48000 {
                        assert_eq!(&out[..4], &[0, 128, 255, 127]);
                    }
                }
                let mut tail = [0u8; 744];
                bytes += r.finish(&mut tail);
                assert_eq!(bytes / 4, length * 48000 / rate as usize, "{rate}/{length}");
            }
        }
    }
    #[test]
    fn header_sizes_and_format() {
        let h = wav_header(192000);
        assert_eq!(u32::from_le_bytes(h[4..8].try_into().unwrap()), 192036);
        assert_eq!(
            Format::parse(&h[20..36]).unwrap(),
            Format {
                rate: 48000,
                channels: 2,
                bits: 16
            }
        );
    }
    #[test]
    fn phase_survives_split_blocks() {
        let mut r = Resampler::default();
        let mut out = [0u8; 24];
        let mut bytes = 0;
        for _ in 0..44100 {
            bytes += r.frame(-100, 200, 44100, &mut out);
        }
        let mut tail = [0u8; 744];
        bytes += r.finish(&mut tail);
        assert_eq!(bytes, 48000 * 4);
        assert_eq!(i16::from_le_bytes([out[0], out[1]]), -100);
    }
    #[test]
    fn reject_float_and_bad_alignment() {
        let mut h = wav_header(0);
        h[20] = 3;
        assert!(Format::parse(&h[20..36]).is_err());
        h[20] = 1;
        h[32] = 3;
        assert!(Format::parse(&h[20..36]).is_err());
    }
    #[test]
    fn integer_widths() {
        for (bits, b, want) in [
            (8, [0, 0, 0, 0], -32768),
            (16, [0, 128, 0, 0], -32768),
            (24, [0, 0, 128, 0], -32768),
            (32, [0, 0, 0, 128], -32768),
        ] {
            assert_eq!(
                Format {
                    rate: 48000,
                    channels: 1,
                    bits
                }
                .sample(&b),
                want
            );
        }
    }
}
