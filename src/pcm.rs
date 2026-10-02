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
#[derive(Default)]
pub struct Resampler {
    phase: u32,
}
impl Resampler {
    /// Nearest-neighbour rate conversion. Carries fractional phase across blocks.
    pub fn frame(&mut self, left: i16, right: i16, rate: u32, out: &mut [u8]) -> usize {
        self.phase += 48000;
        let mut n = 0;
        while self.phase >= rate {
            self.phase -= rate;
            out[n..n + 2].copy_from_slice(&left.to_le_bytes());
            out[n + 2..n + 4].copy_from_slice(&right.to_le_bytes());
            n += 4;
        }
        n
    }
}
#[cfg(test)]
mod tests {
    use super::*;
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
