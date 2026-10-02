//! SD playback and microphone recording, with pure Rust decoding.
extern crate alloc;
use crate::{
    audio::{Audio, Source},
    pcm::{Format, Resampler, wav_header},
    storage::{Name, Storage},
};
use alloc::{boxed::Box, vec};
use core::fmt::Write;
use embedded_sdmmc::{Mode, RawFile};

#[derive(Clone, Copy)]
pub enum Command {
    Next,
    Previous,
    Play,
    Stop,
    Record,
    Replay,
    Refresh,
    Demo,
    Tone,
    Mic,
}
pub struct Media {
    storage: Option<Storage>,
    pub tracks: heapless::Vec<Name, 64>,
    pub selected: usize,
    pub status: heapless::String<64>,
    pub last_recording: Option<Name>,
    player: Option<Player>,
    recorder: Option<Recorder>,
    buffer: Box<[u8]>,
    output: Box<[u8]>,
    samples: Box<[i16]>,
    decoder: Box<nanomp3_core::Decoder>,
    output_len: usize,
    output_pos: usize,
    pub seconds: u32,
    record_failed: bool,
}
struct Player {
    file: RawFile,
    format: Option<Format>,
    remaining: u32,
    input_len: usize,
    resampler: Resampler,
    draining: bool,
    played_bytes: u64,
    decoded_any: bool,
}
struct Recorder {
    file: RawFile,
    name: Name,
    bytes: u32,
    pending: usize,
    buffer: Box<[u8]>,
}
impl Media {
    pub fn new(storage: Option<Storage>) -> Self {
        let mut m = Self {
            storage,
            tracks: heapless::Vec::new(),
            selected: 0,
            status: heapless::String::new(),
            last_recording: None,
            player: None,
            recorder: None,
            buffer: vec![0; 16384].into_boxed_slice(),
            output: vec![0; 32768].into_boxed_slice(),
            samples: vec![0; nanomp3_core::MAX_SAMPLES_PER_FRAME].into_boxed_slice(),
            decoder: Box::new(nanomp3_core::Decoder::new()),
            output_len: 0,
            output_pos: 0,
            seconds: 0,
            record_failed: false,
        };
        m.refresh();
        m
    }
    fn message(&mut self, text: &str) {
        self.status.clear();
        let _ = self.status.push_str(text);
        esp_println::println!("media: {}", text);
    }
    pub fn selected_name(&self) -> &str {
        self.tracks
            .get(self.selected)
            .map(|s| s.as_str())
            .unwrap_or("No MP3/WAV files in SD root")
    }
    pub fn recording(&self) -> bool {
        self.recorder.is_some()
    }
    fn refresh(&mut self) {
        match self.storage.as_ref().map(|s| s.tracks()) {
            Some(Ok(tracks)) => {
                self.tracks = tracks;
                self.selected = self.selected.min(self.tracks.len().saturating_sub(1));
                self.message("Ready (SD root, MP3 / PCM WAV)");
            }
            Some(Err(e)) => self.message(e),
            None => self.message("SD FAT filesystem unavailable"),
        }
    }
    pub fn command(&mut self, cmd: Command, audio: &mut Audio) {
        match cmd {
            Command::Mic => audio.debug_samples(),
            Command::Tone => {
                self.stop(audio);
                audio.set_source(Source::Tone(440.0));
                self.message("440 Hz test tone (STOP ends)");
            }
            Command::Next | Command::Previous => {
                if !self.tracks.is_empty() {
                    self.selected = if matches!(cmd, Command::Next) {
                        (self.selected + 1) % self.tracks.len()
                    } else {
                        (self.selected + self.tracks.len() - 1) % self.tracks.len()
                    };
                }
            }
            Command::Stop => self.stop(audio),
            Command::Record => {
                if self.recorder.is_some() {
                    self.stop(audio);
                } else {
                    self.stop(audio);
                    if let Err(e) = self.start_recording() {
                        self.message(e);
                    }
                }
            }
            Command::Play | Command::Replay => {
                self.stop(audio);
                let name = if matches!(cmd, Command::Replay) {
                    self.last_recording.clone()
                } else {
                    self.tracks.get(self.selected).cloned()
                };
                match name {
                    Some(name) => {
                        if let Err(e) = self.start_playback(&name, audio) {
                            self.message(e);
                        }
                    }
                    None => self.message("No file selected / recording yet"),
                }
            }
            Command::Demo => {
                self.stop(audio);
                if let Err(e) = self.install_demo() {
                    self.message(e);
                } else {
                    self.refresh();
                    self.message("Demo WAV / MP3 installed (NEXT selects)");
                }
            }
            Command::Refresh => {
                if self.player.is_none() && self.recorder.is_none() {
                    self.refresh();
                }
            }
        }
    }
    fn install_demo(&self) -> Result<(), &'static str> {
        let s = self.storage.as_ref().ok_or("No SD")?;
        for (name, data) in [
            (
                "KORVOWAV.WAV",
                include_bytes!("../tests/fixtures/demo.wav").as_slice(),
            ),
            (
                "KORVOMP3.MP3",
                include_bytes!("../tests/fixtures/demo.mp3").as_slice(),
            ),
        ] {
            let file = match s.fs.open_file_in_dir(s.root, name, Mode::ReadWriteCreate) {
                Ok(file) => file,
                Err(embedded_sdmmc::Error::FileAlreadyExists) => continue,
                Err(_) => return Err("Demo file create failed"),
            };
            let result = s.fs.write(file, data);
            let closed = s.fs.close_file(file);
            if result.is_err() || closed.is_err() {
                return Err("Demo file save failed");
            }
            esp_println::println!("demo: saved {} {} bytes", name, data.len());
        }
        Ok(())
    }
    fn start_recording(&mut self) -> Result<(), &'static str> {
        let s = self.storage.as_ref().ok_or("No writable SD")?;
        let (file, name) = s.create_recording()?;
        if s.fs.write(file, &wav_header(0)).is_err() {
            let _ = s.fs.close_file(file);
            return Err("WAV header write failed");
        }
        esp_println::println!("record: {} 48000 Hz stereo PCM16", name);
        let mut status = heapless::String::<64>::new();
        let _ = write!(status, "Recording {} - STOP saves WAV", name);
        self.recorder = Some(Recorder {
            file,
            name,
            bytes: 0,
            pending: 0,
            buffer: vec![0; 8192].into_boxed_slice(),
        });
        self.seconds = 0;
        self.record_failed = false;
        self.message(&status);
        Ok(())
    }
    fn start_playback(&mut self, name: &Name, audio: &mut Audio) -> Result<(), &'static str> {
        let s = self.storage.as_ref().ok_or("No SD")?;
        let file =
            s.fs.open_file_in_dir(s.root, name.as_str(), Mode::ReadOnly)
                .map_err(|_| "Cannot open track")?;
        let parsed = Self::parse_track(s, file, name);
        match parsed {
            Err(e) => {
                let _ = s.fs.close_file(file);
                Err(e)
            }
            Ok((format, remaining)) => {
                esp_println::println!("play: {} format {:?} bytes {}", name, format, remaining);
                self.player = Some(Player {
                    file,
                    format,
                    remaining,
                    input_len: 0,
                    resampler: Resampler::default(),
                    draining: false,
                    played_bytes: 0,
                    decoded_any: false,
                });
                self.output_len = 0;
                self.output_pos = 0;
                self.seconds = 0;
                *self.decoder = nanomp3_core::Decoder::new();
                audio.set_source(Source::File);
                let mut status = heapless::String::<64>::new();
                let _ = write!(status, "Playing {}", name);
                self.message(&status);
                Ok(())
            }
        }
    }
    fn parse_track(
        s: &Storage,
        file: RawFile,
        name: &Name,
    ) -> Result<(Option<Format>, u32), &'static str> {
        let len = s.fs.file_length(file).map_err(|_| "File length")?;
        if name.ends_with(".MP3") {
            let mut tag = [0u8; 10];
            let n = s.fs.read(file, &mut tag).map_err(|_| "MP3 read")?;
            let skip = if n == 10 && &tag[..3] == b"ID3" {
                if tag[6..10].iter().any(|b| b & 128 != 0) {
                    return Err("Invalid ID3 tag");
                }
                10 + ((tag[6] as u32) << 21)
                    + ((tag[7] as u32) << 14)
                    + ((tag[8] as u32) << 7)
                    + tag[9] as u32
                    + if tag[3] == 4 && tag[5] & 16 != 0 {
                        10
                    } else {
                        0
                    }
            } else {
                0
            };
            if skip >= len {
                return Err("Empty MP3");
            }
            s.fs.file_seek_from_start(file, skip)
                .map_err(|_| "MP3 seek")?;
            return Ok((None, len - skip));
        }
        let mut header = [0u8; 12];
        if s.fs.read(file, &mut header).map_err(|_| "WAV read")? != 12
            || &header[..4] != b"RIFF"
            || &header[8..] != b"WAVE"
        {
            return Err("Invalid RIFF WAV");
        }
        let riff_end = u32::from_le_bytes(header[4..8].try_into().unwrap())
            .checked_add(8)
            .ok_or("WAV length overflow")?
            .min(len);
        let mut offset = 12u32;
        let mut format = None;
        while offset.checked_add(8).ok_or("WAV overflow")? <= riff_end {
            let mut h = [0u8; 8];
            if s.fs.read(file, &mut h).map_err(|_| "WAV chunk")? != 8 {
                return Err("Truncated WAV");
            }
            let size = u32::from_le_bytes(h[4..8].try_into().unwrap());
            let data = offset + 8;
            let end = data.checked_add(size).ok_or("WAV overflow")?;
            if end > riff_end {
                return Err("Truncated WAV data");
            }
            if &h[..4] == b"fmt " {
                if size < 16 {
                    return Err("WAV fmt too short");
                }
                let mut fmt = [0u8; 16];
                if s.fs.read(file, &mut fmt).map_err(|_| "WAV format")? != 16 {
                    return Err("Truncated fmt");
                }
                format = Some(Format::parse(&fmt)?);
            }
            if &h[..4] == b"data" {
                let f = format.ok_or("WAV fmt must precede data")?;
                if size % f.frame_bytes() as u32 != 0 {
                    return Err("WAV partial sample");
                }
                return Ok((Some(f), size));
            }
            offset = end.checked_add(size & 1).ok_or("WAV overflow")?;
            s.fs.file_seek_from_start(file, offset)
                .map_err(|_| "WAV chunk seek")?;
        }
        Err("WAV has no data chunk")
    }
    pub fn stop(&mut self, audio: &mut Audio) {
        if let Some(p) = self.player.take() {
            if let Some(s) = &self.storage {
                if s.fs.close_file(p.file).is_err() {
                    self.message("Track close failed");
                }
            }
        }
        audio.set_source(Source::Silence);
        if let Some(r) = self.recorder.take() {
            let result = (|| {
                let s = self.storage.as_ref().ok_or("No SD")?;
                if !self.record_failed {
                    s.fs.write(r.file, &r.buffer[..r.pending])
                        .map_err(|_| "Final recording write failed")?;
                }
                let saved =
                    s.fs.file_length(r.file)
                        .map_err(|_| "Recording length failed")?
                        .saturating_sub(44)
                        / 4
                        * 4;
                s.fs.file_seek_from_start(r.file, 0)
                    .map_err(|_| "WAV header seek failed")?;
                s.fs.write(r.file, &wav_header(saved))
                    .map_err(|_| "WAV header update failed")?;
                s.fs.flush_file(r.file)
                    .map_err(|_| "Recording flush failed")?;
                Ok::<_, &'static str>(())
            })();
            let closed = self
                .storage
                .as_ref()
                .map(|s| s.fs.close_file(r.file).is_ok())
                .unwrap_or(false);
            if let Err(e) = result {
                self.message(e);
            } else if !closed {
                self.message("Recording close failed");
            } else {
                esp_println::println!(
                    "saved: {} {} bytes ({} s)",
                    r.name,
                    r.bytes,
                    r.bytes / 192000
                );
                self.last_recording = Some(r.name);
                self.refresh();
                if self.record_failed {
                    self.message("Partial WAV saved after SD error");
                } else {
                    self.message("Recording saved - REPLAY to listen");
                }
            }
        } else {
            self.message("Stopped");
        }
        self.output_len = 0;
        self.output_pos = 0;
    }
    pub fn capture(&mut self, mut bytes: &[u8]) {
        if self.record_failed {
            return;
        }
        while !bytes.is_empty() {
            let Some(r) = self.recorder.as_mut() else {
                return;
            };
            if r.bytes > u32::MAX - 65536 {
                self.record_failed = true;
                self.message("WAV size limit reached; press STOP");
                return;
            }
            let n = (r.buffer.len() - r.pending).min(bytes.len());
            r.buffer[r.pending..r.pending + n].copy_from_slice(&bytes[..n]);
            r.pending += n;
            r.bytes += n as u32;
            bytes = &bytes[n..];
            self.seconds = r.bytes / 192000;
            if r.pending == r.buffer.len() {
                let result = self.storage.as_ref().unwrap().fs.write(r.file, &r.buffer);
                if result.is_err() {
                    self.record_failed = true;
                    self.message("Recording SD write failed; press STOP");
                    return;
                }
                self.recorder.as_mut().unwrap().pending = 0;
            }
        }
    }
    pub fn poll(&mut self, audio: &mut Audio) {
        if self.player.is_none() {
            return;
        }
        if self.output_pos < self.output_len {
            self.output_pos += audio.queue(&self.output[self.output_pos..self.output_len]);
            return;
        }
        let p = self.player.as_mut().unwrap();
        if p.draining {
            if audio.queued() < 2048 {
                self.stop(audio);
                self.message("Playback complete");
            }
            return;
        }
        // Refill only after queued DMA audio has made space. This bounds
        // filesystem work and lets microphone capture run between blocks.
        if audio.free() < 8192 {
            return;
        }
        let s = self.storage.as_ref().unwrap();
        let result = (|| {
            self.output_len = 0;
            self.output_pos = 0;
            if let Some(f) = p.format {
                let count = (1024 * f.frame_bytes())
                    .min(2048 / f.frame_bytes() * f.frame_bytes())
                    .min(p.remaining as usize);
                if count == 0 {
                    p.draining = true;
                    return Ok(());
                }
                let n =
                    s.fs.read(p.file, &mut self.buffer[..count])
                        .map_err(|_| "WAV read failed")?;
                if n != count {
                    return Err("WAV unexpectedly ended");
                }
                p.remaining -= n as u32;
                for frame in self.buffer[..n].chunks_exact(f.frame_bytes()) {
                    let left = f.sample(frame);
                    let right = if f.channels == 2 {
                        f.sample(&frame[f.bits / 8..])
                    } else {
                        left
                    };
                    self.output_len +=
                        p.resampler
                            .frame(left, right, f.rate, &mut self.output[self.output_len..]);
                }
            } else {
                let need = (self.buffer.len() - p.input_len).min(p.remaining as usize);
                if need > 0 {
                    let n =
                        s.fs.read(p.file, &mut self.buffer[p.input_len..p.input_len + need])
                            .map_err(|_| "MP3 read failed")?;
                    if n == 0 {
                        return Err("MP3 unexpectedly ended");
                    }
                    p.input_len += n;
                    p.remaining -= n as u32;
                }
                if p.input_len == 0 {
                    if !p.decoded_any {
                        return Err("MP3 has no decodable audio");
                    }
                    p.draining = true;
                    return Ok(());
                }
                let (consumed, info) = self
                    .decoder
                    .decode(&self.buffer[..p.input_len], &mut self.samples);
                if consumed == 0 {
                    if p.remaining == 0 {
                        if !p.decoded_any {
                            return Err("MP3 has no decodable audio");
                        }
                        p.draining = true;
                        return Ok(());
                    }
                    return Err("Invalid MP3 frame");
                }
                self.buffer.copy_within(consumed..p.input_len, 0);
                p.input_len -= consumed;
                if let Ok(info) = info {
                    if p.played_bytes == 0 {
                        esp_println::println!(
                            "MP3 decoded: {} Hz {} channels",
                            info.sample_rate,
                            info.channels.num()
                        );
                    }
                    let channels = info.channels.num() as usize;
                    for frame in
                        self.samples[..info.samples_produced * channels].chunks_exact(channels)
                    {
                        self.output_len += p.resampler.frame(
                            frame[0],
                            frame[if channels == 2 { 1 } else { 0 }],
                            info.sample_rate,
                            &mut self.output[self.output_len..],
                        );
                    }
                }
            }
            p.decoded_any |= self.output_len > 0;
            p.played_bytes += self.output_len as u64;
            self.seconds = (p.played_bytes / 192000) as u32;
            Ok::<_, &'static str>(())
        })();
        if let Err(e) = result {
            self.stop(audio);
            self.message(e);
        } else if self.output_len > 0 {
            self.output_pos = audio.queue(&self.output[..self.output_len]);
        }
    }
}
