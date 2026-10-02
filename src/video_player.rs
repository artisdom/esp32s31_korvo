//! Streaming playback of the recorder's AVI files, with bounded JPEG prefetch.
extern crate alloc;
use crate::{
    audio::Audio,
    avi,
    avi_playback::{Chunk, Format, Kind},
    storage::{Name, Storage},
};
use alloc::{boxed::Box, vec::Vec};
use embedded_sdmmc::{Mode, RawFile};

pub struct Player {
    pub file: RawFile,
    pub name: Name,
    format: Format,
    position: u32,
    chunk: Option<Chunk>,
    pcm: Box<[u8]>,
    pending: usize,
    consumed: usize,
    jpeg: Box<[u8]>,
    decoder: &'static crate::jpeg_decoder::Worker,
    generation: u32,
    decode_pending: bool,
    frames: heapless::Deque<(u32, Vec<u8>), 3>,
    next_frame: u32,
    compressed: heapless::Deque<(u32, Vec<u8>), 3>,
    pub decoded: u32,
    pub skipped: u32,
    submitted: u32,
    pub image: Option<Vec<u8>>,
    pub changed: bool,
    pub displayed: u32,
    pub seconds: u32,
}
impl Player {
    pub fn new(
        s: &Storage,
        name: Name,
        decoder: &'static crate::jpeg_decoder::Worker,
        generation: u32,
    ) -> Result<Self, &'static str> {
        let file =
            s.fs.open_file_in_dir(s.root, name.as_str(), Mode::ReadOnly)
                .map_err(|_| "AVI open failed")?;
        let parsed = (|| {
            let mut header = [0u8; avi::HEADER_BYTES];
            if s.fs
                .read(file, &mut header)
                .map_err(|_| "AVI header read failed")?
                != header.len()
            {
                return Err("AVI header truncated");
            }
            let size = s.fs.file_length(file).map_err(|_| "AVI size unavailable")?;
            Format::parse(&header, size)
        })();
        let format = match parsed {
            Ok(format) => format,
            Err(error) => {
                let _ = s.fs.close_file(file);
                return Err(error);
            }
        };
        esp_println::println!(
            "video play: {} frames={} audio={}",
            name,
            format.frames,
            format.audio_bytes
        );
        Ok(Self {
            file,
            name,
            format,
            position: avi::HEADER_BYTES as u32,
            chunk: None,
            pcm: crate::psram_buffer::zeroed(8192),
            pending: 0,
            consumed: 0,
            jpeg: crate::psram_buffer::zeroed(avi::FRAME_BYTES),
            decoder,
            generation,
            decode_pending: false,
            frames: heapless::Deque::new(),
            next_frame: 0,
            compressed: heapless::Deque::new(),
            decoded: 0,
            skipped: 0,
            submitted: 0,
            image: None,
            changed: false,
            displayed: 0,
            seconds: 0,
        })
    }
    /// True once all media and queued PCM have drained; keep the final LCD image.
    pub fn poll(&mut self, s: &Storage, audio: &mut Audio) -> Result<bool, &'static str> {
        if let Some((generation, index, result)) = self.decoder.take() {
            if generation == self.generation {
                self.decode_pending = false;
                self.decoded += 1;
                self.frames
                    .push_back((index, result?))
                    .map_err(|_| "AVI prefetch overflow")?;
            }
        }
        let played = self.submitted.saturating_sub(audio.queued() as u32);
        self.seconds = played / avi::AUDIO_RATE;
        while self.frames.front().is_some_and(|(index, _)| {
            *index as u64 * avi::AUDIO_RATE as u64 <= played as u64 * avi::FPS as u64
        }) {
            let (index, image) = self.frames.pop_front().unwrap();
            self.image = Some(image);
            self.changed = true;
            self.displayed = index + 1;
        }
        if !self.decode_pending && self.decoder.idle() && self.frames.len() < 3 {
            if let Some((index, jpeg)) = self.compressed.pop_front() {
                if !self.decoder.submit(&jpeg, self.generation, index) {
                    return Err("AVI decoder unavailable");
                }
                self.decode_pending = true;
            }
        }
        if self.consumed < self.pending {
            let n = audio.queue(&self.pcm[self.consumed..self.pending]);
            self.consumed += n;
            self.submitted += n as u32;
            return Ok(false);
        }
        if self.chunk.is_none() {
            if self.position == self.format.end {
                if self.next_frame != self.format.frames
                    || self.submitted != self.format.audio_bytes
                {
                    return Err("AVI media lengths disagree");
                }
                return Ok(self.compressed.is_empty()
                    && !self.decode_pending
                    && self.frames.is_empty()
                    && audio.queued() < 2048);
            }
            if self
                .position
                .checked_add(8)
                .is_none_or(|end| end > self.format.end)
            {
                return Err("AVI chunk header truncated");
            }
            s.fs.file_seek_from_start(self.file, self.position)
                .map_err(|_| "AVI chunk seek failed")?;
            let mut header = [0; 8];
            if s.fs
                .read(self.file, &mut header)
                .map_err(|_| "AVI chunk read failed")?
                != 8
            {
                return Err("AVI chunk header truncated");
            }
            self.chunk = Some(Chunk::parse(&header, self.position, self.format.end)?);
        }
        let chunk = self.chunk.as_mut().unwrap();
        match chunk.kind {
            Kind::Video => {
                let n = chunk.bytes as usize;
                if s.fs
                    .read(self.file, &mut self.jpeg[..n])
                    .map_err(|_| "AVI JPEG read failed")?
                    != n
                {
                    return Err("AVI JPEG truncated");
                }
                // Continue to following PCM while JPEG is busy. Bounded
                // compressed prefetch drops its oldest pending image if the
                // decoder falls behind; audio must never wait for video work.
                if self.compressed.is_full() {
                    self.compressed.pop_front();
                    self.skipped += 1;
                }
                let mut jpeg = crate::psram_buffer::zeroed(n).into_vec();
                jpeg.copy_from_slice(&self.jpeg[..n]);
                let _ = self.compressed.push_back((self.next_frame, jpeg));
                self.next_frame += 1;
                chunk.bytes = 0;
            }
            Kind::Audio => {
                if audio.free() < 8192 {
                    return Ok(false);
                }
                let n = (chunk.bytes as usize).min(self.pcm.len());
                if s.fs
                    .read(self.file, &mut self.pcm[..n])
                    .map_err(|_| "AVI PCM read failed")?
                    != n
                {
                    return Err("AVI PCM truncated");
                }
                self.pending = n;
                self.consumed = 0;
                chunk.bytes -= n as u32;
            }
            Kind::Skip => {
                chunk.bytes = 0;
            }
        }
        if chunk.bytes == 0 {
            self.position = chunk.next;
            s.fs.file_seek_from_start(self.file, self.position)
                .map_err(|_| "AVI padding seek failed")?;
            self.chunk = None;
        }
        Ok(false)
    }
}
