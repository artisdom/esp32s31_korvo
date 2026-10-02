//! Validate this application's MJPEG/PCM AVI format before streaming SD chunks.
use crate::avi;
fn word(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}
#[derive(Clone, Copy, Debug)]
pub struct Format {
    pub end: u32,
    pub frames: u32,
    pub audio_bytes: u32,
}
impl Format {
    pub fn parse(header: &[u8], file_bytes: u32) -> Result<Self, &'static str> {
        if header.len() != avi::HEADER_BYTES {
            return Err("AVI header truncated");
        }
        if &header[0..4] != b"RIFF"
            || &header[8..12] != b"AVI "
            || &header[108..112] != b"vids"
            || &header[112..116] != b"MJPG"
            || &header[188..192] != b"MJPG"
            || &header[232..236] != b"auds"
            || &header[312..316] != b"LIST"
            || &header[320..324] != b"movi"
        {
            return Err("Only Korvo MJPEG AVI recordings supported");
        }
        if word(header, 176) != avi::WIDTH as u32
            || word(header, 180) != avi::HEIGHT as u32
            || word(header, 128) != 1
            || word(header, 132) != avi::FPS
            || &header[296..300] != [1, 0, 2, 0]
            || word(header, 300) != 48000
            || word(header, 304) != avi::AUDIO_RATE
            || &header[308..312] != [4, 0, 16, 0]
        {
            return Err("Unsupported AVI video/audio format");
        }
        let end = word(header, 316)
            .checked_sub(4)
            .and_then(|n| n.checked_add(avi::HEADER_BYTES as u32))
            .ok_or("AVI size overflow")?;
        if word(header, 4).checked_add(8) != Some(file_bytes) || end != file_bytes {
            return Err("AVI incomplete - save recording first");
        }
        let frames = word(header, 140);
        let audio_bytes = word(header, 264)
            .checked_mul(4)
            .ok_or("AVI audio size overflow")?;
        if frames == 0 || frames != word(header, 48) || audio_bytes == 0 {
            return Err("AVI has no complete media");
        }
        if frames != audio_bytes.div_ceil(avi::AUDIO_RATE / avi::FPS) {
            return Err("AVI stream lengths disagree");
        }
        Ok(Self {
            end,
            frames,
            audio_bytes,
        })
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Video,
    Audio,
    Skip,
}
#[derive(Debug)]
pub struct Chunk {
    pub kind: Kind,
    pub bytes: u32,
    pub next: u32,
}
impl Chunk {
    pub fn parse(header: &[u8; 8], position: u32, end: u32) -> Result<Self, &'static str> {
        let bytes = word(header, 4);
        let next = position
            .checked_add(8)
            .and_then(|n| n.checked_add(bytes))
            .and_then(|n| n.checked_add(bytes & 1))
            .ok_or("AVI chunk overflow")?;
        if next > end {
            return Err("AVI chunk outside media data");
        }
        let kind = match &header[..4] {
            b"00dc" if bytes > 0 && bytes <= avi::FRAME_BYTES as u32 => Kind::Video,
            b"01wb" if bytes > 0 && bytes % 4 == 0 => Kind::Audio,
            b"00dc" | b"01wb" => return Err("Invalid AVI media chunk"),
            _ => Kind::Skip,
        };
        Ok(Self { kind, bytes, next })
    }
}
