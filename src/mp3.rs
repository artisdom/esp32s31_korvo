//! Bounded streaming MP3 framing, metadata and encoder-delay trimming.
use core::ops::Range;
use nanomp3_core::{DecodeError, Decoder, FrameInfo};

/// A leading ID3v2 tag can contain megabytes of artwork: seek over it rather
/// than feeding it to the decoder or allocating its declared size.
pub fn leading_tag(header: &[u8], remaining: u32) -> Result<u32, &'static str> {
    if !header.starts_with(b"ID3") {
        return Ok(0);
    }
    if header.len() < 10
        || !matches!(header[3], 2..=4)
        || header[4] == 255
        || header[6..10].iter().any(|b| b & 128 != 0)
    {
        return Err("Invalid ID3v2 header");
    }
    let body = header[6..10].iter().fold(0u32, |n, &b| (n << 7) | b as u32);
    let size = 10
        + body
        + if header[3] == 4 && header[5] & 16 != 0 {
            10
        } else {
            0
        };
    if size >= remaining {
        return Err("Truncated ID3v2 / empty MP3");
    }
    Ok(size)
}

#[derive(Default)]
pub struct Stream {
    started: bool,
    skip: u32,
    remaining: Option<u64>,
    pub frames: u32,
    pub skipped: u32,
    pub trimmed: bool,
    tail_checked: bool,
}
pub struct AudioFrame {
    pub info: FrameInfo,
    /// Interleaved samples, after removing encoder delay and end padding.
    pub samples: Range<usize>,
}
impl Stream {
    pub fn strip_tail(&mut self, input: &[u8]) -> usize {
        if self.tail_checked {
            return input.len();
        }
        self.tail_checked = true;
        nanomp3::strip_trailing_tags(input).len()
    }
    pub fn complete(&self) -> bool {
        self.remaining == Some(0)
    }
    pub fn decode(
        &mut self,
        decoder: &mut Decoder,
        input: &[u8],
        eof: bool,
        samples: &mut [i16],
    ) -> Result<(usize, Option<AudioFrame>), &'static str> {
        if !self.started {
            if let Some(frame) = nanomp3::frames(input).next() {
                self.started = true;
                if frame.layer != 3 {
                    return Err("File is not MPEG Layer III");
                }
                if let Some(tag) = nanomp3::VbrTag::parse(frame.data) {
                    if let Some(frames) = tag.frames {
                        let (delay, padding) = tag.encoder_delay_padding.unwrap_or((0, 0));
                        // The Layer III synthesis filter adds 529 samples.
                        self.skip = if tag.encoder_delay_padding.is_some() {
                            delay as u32 + 529
                        } else {
                            0
                        };
                        let end = (padding as u32).saturating_sub(529);
                        self.remaining = Some(
                            (frames as u64 * frame.samples as u64)
                                .saturating_sub(self.skip as u64 + end as u64),
                        );
                        self.trimmed = tag.encoder_delay_padding.is_some();
                    }
                    // Xing/Info is a metadata frame, not part of the audio.
                    return Ok((frame.offset + frame.data.len(), None));
                }
            }
        }
        let (used, result) = decoder.decode(input, samples);
        match result {
            Ok(info) => {
                if info.layer != 3 || !(8000..=48000).contains(&info.sample_rate) {
                    return Err("Unsupported MPEG audio format");
                }
                self.frames = self.frames.saturating_add(1);
                let start = (self.skip as usize).min(info.samples_produced);
                self.skip -= start as u32;
                let available = info.samples_produced - start;
                let count = self
                    .remaining
                    .map_or(available, |n| n.min(available as u64) as usize);
                if let Some(n) = &mut self.remaining {
                    *n -= count as u64;
                }
                let channels = info.channels.num() as usize;
                Ok((
                    used,
                    Some(AudioFrame {
                        info,
                        samples: start * channels..(start + count) * channels,
                    }),
                ))
            }
            Err(DecodeError::NoFrame) => {
                // The core consumes incomplete frames as junk. Keep enough
                // lookahead for three maximum-size free-format frames when a
                // damaged/junk region ends near a filesystem buffer boundary.
                let used = if eof {
                    used
                } else {
                    used.saturating_sub(8192).max(1).min(input.len())
                };
                Ok((used, None))
            }
            Err(DecodeError::UnsupportedLayer(_)) => Err("File is not MPEG Layer III"),
            Err(_) => {
                self.skipped = self.skipped.saturating_add(1);
                Ok((used, None))
            }
        }
    }
}
