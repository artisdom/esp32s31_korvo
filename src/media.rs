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
    FileNext,
    FilePrevious,
    DeleteTrack,
    DeleteFile,
    ConfirmDelete,
    CancelDelete,
    VideoRecord,
    VideoPlay,
    VideoReplay,
    VideoNext,
    VideoPrevious,
    PlayFile,
}
pub struct Media {
    storage: Option<Storage>,
    pub tracks: heapless::Vec<Name, 64>,
    pub selected: usize,
    pub files: heapless::Vec<Name, 64>,
    pub file_selected: usize,
    pub delete_request: crate::delete_request::DeleteRequest<Name>,
    pub status: heapless::String<64>,
    pub last_recording: Option<Name>,
    player: Option<Player>,
    video_player: Option<crate::video_player::Player>,
    video_decoder: &'static crate::jpeg_decoder::Worker,
    pub videos: heapless::Vec<Name, 64>,
    pub video_selected: usize,
    pub last_video: Option<Name>,
    recorder: Option<Recorder>,
    buffer: Box<[u8]>,
    output: Box<[u8]>,
    samples: Box<[i16]>,
    decoder: Box<nanomp3_core::Decoder>,
    output_len: usize,
    output_pos: usize,
    pub seconds: u32,
    record_failed: bool,
    video: Option<VideoRecorder>,
    pub camera_ready: bool,
    encoder: &'static crate::jpeg_worker::Worker,
    generation: u32,
    pub video_name: Name,
    pub video_frames: u32,
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
struct VideoRecorder {
    file: RawFile,
    name: Name,
    audio_bytes: u32,
    frames: u32,
    movi_bytes: u32,
    pending: usize,
    audio: Box<[u8]>,
    generation: u32,
    submitted: bool,
    jpeg: alloc::vec::Vec<u8>,
    failed: bool,
}
struct Recorder {
    file: RawFile,
    name: Name,
    bytes: u32,
    pending: usize,
    buffer: Box<[u8]>,
}
impl Media {
    pub fn new(
        storage: Option<Storage>,
        encoder: &'static crate::jpeg_worker::Worker,
        video_decoder: &'static crate::jpeg_decoder::Worker,
    ) -> Self {
        let mut m = Self {
            storage,
            tracks: heapless::Vec::new(),
            selected: 0,
            files: heapless::Vec::new(),
            file_selected: 0,
            delete_request: crate::delete_request::DeleteRequest::new(),
            status: heapless::String::new(),
            last_recording: None,
            player: None,
            video_player: None,
            video_decoder,
            videos: heapless::Vec::new(),
            video_selected: 0,
            last_video: None,
            recorder: None,
            buffer: vec![0; 16384].into_boxed_slice(),
            output: vec![0; 32768].into_boxed_slice(),
            samples: vec![0; nanomp3_core::MAX_SAMPLES_PER_FRAME].into_boxed_slice(),
            decoder: Box::new(nanomp3_core::Decoder::new()),
            output_len: 0,
            output_pos: 0,
            seconds: 0,
            record_failed: false,
            video: None,
            camera_ready: false,
            encoder,
            generation: 0,
            video_name: Name::new(),
            video_frames: 0,
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
        self.recorder.is_some() || self.video.is_some()
    }
    pub fn video_recording(&self) -> bool {
        self.video.is_some()
    }
    pub fn playing(&self) -> bool {
        self.player.is_some() || self.video_playing()
    }
    pub fn video_playing(&self) -> bool {
        self.video_player.is_some()
    }
    pub fn playback_frame(&self) -> Option<&[u8]> {
        self.video_player.as_ref()?.image.as_deref()
    }
    pub fn frame_presented(&mut self) {
        if let Some(p) = self.video_player.as_mut() {
            p.changed = false;
        }
    }
    pub fn playback_changed(&self) -> bool {
        self.video_player.as_ref().is_some_and(|p| p.changed)
    }
    pub fn selected_video(&self) -> &str {
        self.videos
            .get(self.video_selected)
            .map(|n| n.as_str())
            .unwrap_or("No AVI recordings")
    }
    pub fn selected_file_video(&self) -> bool {
        self.files
            .get(self.file_selected)
            .is_some_and(|n| n.ends_with(".AVI"))
    }
    fn refresh(&mut self) {
        let selected_video = self.videos.get(self.video_selected).cloned();
        match self.storage.as_ref().map(|s| s.files()) {
            Some(Ok(files)) => {
                match self.storage.as_ref().unwrap().tracks() {
                    Ok(tracks) => self.tracks = tracks,
                    Err(e) => {
                        self.message(e);
                        return;
                    }
                }
                match self.storage.as_ref().unwrap().videos() {
                    Ok(videos) => self.videos = videos,
                    Err(e) => {
                        self.message(e);
                        return;
                    }
                }
                self.video_selected = selected_video
                    .and_then(|name| self.videos.iter().position(|n| *n == name))
                    .unwrap_or_else(|| self.videos.len().saturating_sub(1));
                self.files = files;
                self.file_selected = self.file_selected.min(self.files.len().saturating_sub(1));
                self.selected = self.selected.min(self.tracks.len().saturating_sub(1));
                self.message("Ready (SD root, MP3 / PCM WAV)");
            }
            Some(Err(e)) => self.message(e),
            None => self.message("SD FAT filesystem unavailable"),
        }
    }
    pub fn command(&mut self, cmd: Command, audio: &mut Audio) {
        if !matches!(cmd, Command::ConfirmDelete | Command::CancelDelete) {
            self.delete_request.cancel();
        }
        match cmd {
            Command::VideoNext | Command::VideoPrevious => {
                if !self.videos.is_empty() {
                    self.video_selected = if matches!(cmd, Command::VideoNext) {
                        (self.video_selected + 1) % self.videos.len()
                    } else {
                        (self.video_selected + self.videos.len() - 1) % self.videos.len()
                    };
                    esp_println::println!("Video selected: {}", self.selected_video());
                }
            }
            Command::VideoPlay | Command::VideoReplay | Command::PlayFile => {
                self.stop(audio);
                let name = match cmd {
                    Command::VideoReplay => self
                        .last_video
                        .clone()
                        .or_else(|| self.videos.last().cloned()),
                    Command::PlayFile => self.files.get(self.file_selected).cloned(),
                    _ => self.videos.get(self.video_selected).cloned(),
                };
                let result = match name {
                    Some(name) if name.ends_with(".AVI") => self.start_video_playback(name, audio),
                    Some(name) if name.ends_with(".WAV") || name.ends_with(".MP3") => {
                        self.start_playback(&name, audio)
                    }
                    Some(_) => Err("Select an AVI / WAV / MP3 file"),
                    None => Err("No video recording selected"),
                };
                if let Err(e) = result {
                    self.message(e);
                }
            }
            Command::VideoRecord => {
                if self.video.is_some() {
                    self.stop(audio);
                } else if !self.camera_ready {
                    self.message("Camera not ready - open CAMERA preview");
                } else {
                    self.stop(audio);
                    if let Err(e) = self.start_video() {
                        self.message(e);
                    }
                }
            }
            Command::DeleteTrack | Command::DeleteFile => {
                let busy = self.playing() || self.recording() || audio.source() != Source::Silence;
                if busy {
                    self.message("STOP playback / recording before deleting");
                    return;
                }
                let name = if matches!(cmd, Command::DeleteTrack) {
                    self.tracks.get(self.selected)
                } else {
                    self.files.get(self.file_selected)
                }
                .cloned();
                if let Some(name) = name {
                    esp_println::println!("delete confirmation: {}", name);
                    self.delete_request.arm(name, false);
                } else {
                    self.message("No file selected");
                }
            }
            Command::ConfirmDelete => {
                let busy = self.playing() || self.recording() || audio.source() != Source::Silence;
                if let Some(name) = self.delete_request.confirm(busy) {
                    let result = self
                        .storage
                        .as_ref()
                        .ok_or("No SD")
                        .and_then(|s| s.delete(name.as_str()));
                    match result {
                        Ok(()) => {
                            if self.last_recording.as_ref() == Some(&name) {
                                self.last_recording = None;
                            }
                            if self.last_video.as_ref() == Some(&name) {
                                self.last_video = None;
                            }
                            self.refresh();
                            let mut msg = heapless::String::<64>::new();
                            let _ = write!(msg, "Deleted {}", name);
                            self.message(&msg);
                        }
                        Err(e) => self.message(e),
                    }
                }
            }
            Command::CancelDelete => {
                self.delete_request.cancel();
            }
            Command::FileNext | Command::FilePrevious => {
                if !self.files.is_empty() {
                    self.file_selected = if matches!(cmd, Command::FileNext) {
                        (self.file_selected + 1) % self.files.len()
                    } else {
                        (self.file_selected + self.files.len() - 1) % self.files.len()
                    };
                    esp_println::println!("SD selected: {}", self.files[self.file_selected]);
                }
            }
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
                    esp_println::println!("Audio selected: {}", self.selected_name());
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
                if !self.playing() && !self.recording() {
                    self.refresh();
                }
            }
        }
    }
    fn start_video_playback(&mut self, name: Name, audio: &mut Audio) -> Result<(), &'static str> {
        self.generation = self.generation.wrapping_add(1);
        let player = crate::video_player::Player::new(
            self.storage.as_ref().ok_or("No SD")?,
            name.clone(),
            self.video_decoder,
            self.generation,
        )?;
        self.video_name = name;
        self.video_frames = 0;
        self.seconds = 0;
        self.video_player = Some(player);
        audio.set_source(Source::File);
        self.message("Playing AVI video + microphone audio");
        Ok(())
    }
    fn start_video(&mut self) -> Result<(), &'static str> {
        let s = self.storage.as_ref().ok_or("No writable SD")?;
        let (file, name) = s.create_video()?;
        if s.fs
            .write(
                file,
                &crate::avi::header(crate::avi::HEADER_BYTES as u32, 0, 0, 0),
            )
            .is_err()
        {
            let _ = s.fs.close_file(file);
            return Err("AVI header write failed");
        }
        self.video_name = name.clone();
        self.video_frames = 0;
        self.generation = self.generation.wrapping_add(1);
        self.video = Some(VideoRecorder {
            file,
            name,
            audio_bytes: 0,
            frames: 0,
            movi_bytes: 0,
            pending: 0,
            audio: vec![0; 8192].into_boxed_slice(),
            generation: self.generation,
            submitted: false,
            jpeg: alloc::vec::Vec::new(),
            failed: false,
        });
        self.message("Recording video + microphone - STOP saves AVI");
        esp_println::println!("video record: {}", self.video_name);
        Ok(())
    }
    fn flush_video_audio(&mut self) -> Result<(), &'static str> {
        let v = self.video.as_mut().ok_or("No video recording")?;
        if v.pending == 0 {
            return Ok(());
        }
        let s = self.storage.as_ref().ok_or("No SD")?;
        s.fs.write(v.file, &crate::avi::chunk_header(false, v.pending as u32))
            .map_err(|_| "AVI audio header failed")?;
        s.fs.write(v.file, &v.audio[..v.pending])
            .map_err(|_| "AVI audio write failed")?;
        v.audio_bytes += v.pending as u32;
        v.movi_bytes += v.pending as u32 + 8;
        v.pending = 0;
        Ok(())
    }
    fn capture_video_audio(&mut self, mut bytes: &[u8]) {
        while !bytes.is_empty() {
            let v = self.video.as_mut().unwrap();
            if v.failed {
                return;
            }
            let n = bytes.len().min(v.audio.len() - v.pending);
            v.audio[v.pending..v.pending + n].copy_from_slice(&bytes[..n]);
            v.pending += n;
            bytes = &bytes[n..];
            if v.pending == v.audio.len() {
                if let Err(e) = self.flush_video_audio() {
                    self.video.as_mut().unwrap().failed = true;
                    self.message(e);
                    return;
                }
            }
        }
    }
    pub fn video_frame(&mut self, image: &[u8], new_frame: bool) {
        let completed = self.encoder.take();
        let Some(v) = self.video.as_mut() else {
            return;
        };
        if let Some((generation, result)) = completed {
            if generation == v.generation {
                match result {
                    Ok(jpeg) => v.jpeg = jpeg,
                    Err(e) => {
                        v.failed = true;
                        self.status.clear();
                        let _ = self.status.push_str(e);
                    }
                }
            }
        }
        if new_frame || !v.submitted {
            if self.encoder.submit(image, v.generation) {
                v.submitted = true;
            }
        }
        if v.failed || v.jpeg.is_empty() {
            return;
        }
        // Audio sample counts are the timeline. Duplicate the latest complete
        // image when capture is slower, rather than allowing A/V drift.
        let target =
            (v.audio_bytes + v.pending as u32).div_ceil(crate::avi::AUDIO_RATE / crate::avi::FPS);
        if v.frames < target {
            let result = (|| {
                let s = self.storage.as_ref().ok_or("No SD")?;
                s.fs.write(v.file, &crate::avi::chunk_header(true, v.jpeg.len() as u32))
                    .map_err(|_| "AVI frame header failed")?;
                s.fs.write(v.file, &v.jpeg)
                    .map_err(|_| "AVI frame write failed")?;
                if v.jpeg.len() & 1 != 0 {
                    s.fs.write(v.file, &[0])
                        .map_err(|_| "AVI frame padding failed")?;
                }
                Ok::<_, &'static str>(())
            })();
            match result {
                Ok(()) => {
                    v.frames += 1;
                    v.movi_bytes += v.jpeg.len() as u32 + (v.jpeg.len() as u32 & 1) + 8;
                    self.video_frames = v.frames;
                    self.seconds = (v.audio_bytes + v.pending as u32) / crate::avi::AUDIO_RATE;
                }
                Err(e) => {
                    v.failed = true;
                    self.message(e);
                }
            }
        }
        if self
            .video
            .as_ref()
            .map(|v| v.movi_bytes >= 1024 * 1024 * 1024)
            .unwrap_or(false)
        {
            self.finish_video();
            self.message("AVI saved at 1 GB limit");
        }
    }
    fn finish_video(&mut self) {
        let result = self.flush_video_audio();
        let mut v = self.video.take().unwrap();
        if result.is_err() {
            v.failed = true;
        }
        if v.jpeg.is_empty() {
            v.failed = true;
        }
        let result = (|| {
            let s = self.storage.as_ref().ok_or("No SD")?;
            // Pad the final video interval with the latest complete image.
            let target = v
                .audio_bytes
                .div_ceil(crate::avi::AUDIO_RATE / crate::avi::FPS);
            while !v.failed && !v.jpeg.is_empty() && v.frames < target {
                s.fs.write(v.file, &crate::avi::chunk_header(true, v.jpeg.len() as u32))
                    .map_err(|_| "AVI last frame header failed")?;
                s.fs.write(v.file, &v.jpeg)
                    .map_err(|_| "AVI last frame write failed")?;
                if v.jpeg.len() & 1 != 0 {
                    s.fs.write(v.file, &[0])
                        .map_err(|_| "AVI frame padding failed")?;
                }
                v.frames += 1;
                v.movi_bytes += v.jpeg.len() as u32 + (v.jpeg.len() as u32 & 1) + 8;
            }
            let size = crate::avi::HEADER_BYTES as u32 + v.movi_bytes;
            s.fs.file_seek_from_start(v.file, 0)
                .map_err(|_| "AVI header seek failed")?;
            s.fs.write(
                v.file,
                &crate::avi::header(size, v.frames, v.audio_bytes, v.movi_bytes),
            )
            .map_err(|_| "AVI header finalize failed")?;
            s.fs.flush_file(v.file).map_err(|_| "AVI flush failed")?;
            Ok::<_, &'static str>(())
        })();
        let closed = self
            .storage
            .as_ref()
            .map(|s| s.fs.close_file(v.file).is_ok())
            .unwrap_or(false);
        self.video_frames = v.frames;
        self.refresh();
        if result.is_err() || !closed || v.failed {
            self.message("AVI save incomplete - check SD");
        } else {
            self.last_video = Some(v.name.clone());
            if let Some(index) = self.videos.iter().position(|n| *n == v.name) {
                self.video_selected = index;
            }
            self.message("Video saved - REPLAY LAST to watch");
            esp_println::println!(
                "video saved: {} frames={} audio={} bytes={}",
                v.name,
                v.frames,
                v.audio_bytes,
                crate::avi::HEADER_BYTES as u32 + v.movi_bytes
            );
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
        if let Some(p) = self.video_player.take() {
            esp_println::println!(
                "video playback stopped: {} displayed={} elapsed={}s",
                p.name,
                p.displayed,
                p.seconds
            );
            if let Some(s) = &self.storage {
                let _ = s.fs.close_file(p.file);
            }
        }
        let had_video = self.video.is_some();
        if had_video {
            self.finish_video();
        }
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
        } else if !had_video {
            self.message("Stopped");
        }
        self.output_len = 0;
        self.output_pos = 0;
    }
    pub fn capture(&mut self, mut bytes: &[u8]) {
        if self.video.is_some() {
            self.capture_video_audio(bytes);
            return;
        }
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
        if let Some(player) = self.video_player.as_mut() {
            let result = player.poll(self.storage.as_ref().unwrap(), audio);
            self.seconds = player.seconds;
            self.video_frames = player.displayed;
            match result {
                Ok(true) => {
                    self.stop(audio);
                    self.message("Video playback complete");
                }
                Err(e) => {
                    self.stop(audio);
                    self.message(e);
                }
                Ok(false) => {}
            }
            return;
        }
        // Consume an abandoned decode after STOP so the mailbox can serve a new file.
        let _ = self.video_decoder.take();
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
