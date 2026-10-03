#[cfg(test)]
extern crate self as esp_println;
#[cfg(test)]
#[macro_export]
macro_rules! println { ($($arg:tt)*) => { std::println!($($arg)*) }; }
#[cfg(test)]
#[path = "../../../src/video_player.rs"]
mod video_player;
#[cfg(test)]
mod storage {
    pub type Name = crate::fat_files::Name;
    pub struct Storage {
        pub fs: embedded_sdmmc::VolumeManager<crate::integration::Disk, crate::integration::Clock>,
        pub root: embedded_sdmmc::RawDirectory,
    }
    impl Storage {
        pub fn open(
            &self,
            path: &str,
            mode: embedded_sdmmc::Mode,
        ) -> Result<embedded_sdmmc::RawFile, &'static str> {
            crate::fat_files::open(&self.fs, self.root, path, mode)
        }
    }
}
#[cfg(test)]
mod audio {
    pub struct Audio {
        pub queued_bytes: usize,
        pub accepted: Vec<u8>,
        pub played: usize,
        pub gaps: usize,
        pub expected: usize,
    }
    impl Audio {
        pub fn new(expected: usize) -> Self {
            Self {
                queued_bytes: 0,
                accepted: Vec::new(),
                played: 0,
                gaps: 0,
                expected,
            }
        }
        pub fn queued(&mut self) -> usize {
            self.queued_bytes
        }
        pub fn free(&mut self) -> usize {
            65536 - self.queued_bytes
        }
        pub fn queue(&mut self, bytes: &[u8]) -> usize {
            let n = bytes.len().min(self.free());
            self.accepted.extend_from_slice(&bytes[..n]);
            self.queued_bytes += n;
            n
        }
        pub fn advance(&mut self, n: usize) {
            if self.queued_bytes < n && self.accepted.len() < self.expected {
                self.gaps += 1;
            }
            let n = n.min(self.queued_bytes);
            self.queued_bytes -= n;
            self.played += n;
        }
    }
}
#[path = "../../../src/audio_queue.rs"]
mod audio_queue;
#[path = "../../../src/avi.rs"]
mod avi;
#[path = "../../../src/avi_playback.rs"]
mod avi_playback;
#[path = "../../../src/delete_request.rs"]
mod delete_request;
#[path = "../../../src/fat_files.rs"]
mod fat_files;
#[path = "../../../src/fat_layout.rs"]
mod fat_layout;
#[path = "../../../src/jpeg_decoder.rs"]
mod jpeg_decoder;
#[path = "../../../src/jpeg_worker.rs"]
mod jpeg_worker;
#[path = "../../../src/mp3.rs"]
mod mp3;
#[path = "../../../src/pcm.rs"]
mod pcm;
#[path = "../../../src/pcm_worker.rs"]
mod pcm_worker;
#[path = "../../../src/psram_buffer.rs"]
mod psram_buffer;
#[cfg(test)]
mod integration {
    use super::*;
    use embedded_sdmmc::*;
    use std::{
        cell::RefCell,
        fs::{File, OpenOptions},
        io::{Read, Seek, SeekFrom, Write},
        process::Command,
    };
    pub(crate) struct Disk {
        file: RefCell<File>,
        layout: fat_layout::Layout,
    }
    impl BlockDevice for Disk {
        type Error = std::io::Error;
        fn read(&self, b: &mut [Block], start: BlockIdx) -> Result<(), Self::Error> {
            for (i, b) in b.iter_mut().enumerate() {
                let logical = start.0 + i as u32;
                if logical == 0 && self.layout.overlay {
                    b.contents = self.layout.mbr();
                } else {
                    let physical = self
                        .layout
                        .physical(logical)
                        .ok_or(std::io::ErrorKind::InvalidInput)?;
                    let mut f = self.file.borrow_mut();
                    f.seek(SeekFrom::Start(physical as u64 * 512))?;
                    f.read_exact(&mut b.contents)?;
                }
            }
            Ok(())
        }
        fn write(&self, b: &[Block], start: BlockIdx) -> Result<(), Self::Error> {
            for (i, b) in b.iter().enumerate() {
                let physical = self
                    .layout
                    .physical(start.0 + i as u32)
                    .ok_or(std::io::ErrorKind::InvalidInput)?;
                let mut f = self.file.borrow_mut();
                f.seek(SeekFrom::Start(physical as u64 * 512))?;
                f.write_all(&b.contents)?;
            }
            Ok(())
        }
        fn num_blocks(&self) -> Result<BlockCount, Self::Error> {
            Ok(BlockCount(
                self.layout.blocks + u32::from(self.layout.overlay),
            ))
        }
    }
    pub(crate) struct Clock;
    impl TimeSource for Clock {
        fn get_timestamp(&self) -> Timestamp {
            Timestamp {
                year_since_1970: 56,
                zero_indexed_month: 9,
                zero_indexed_day: 1,
                hours: 0,
                minutes: 0,
                seconds: 0,
            }
        }
    }
    #[test]
    fn fat32_superfloppy_record_save_reopen_preserves_existing_file() {
        let path = std::env::temp_dir().join(format!("korvo-media-{}.img", std::process::id()));
        let mut f = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        f.set_len(64 * 1024 * 1024).unwrap();
        assert!(
            Command::new("mkfs.fat")
                .args(["-F", "32"])
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        let mut boot = [0u8; 512];
        f.read_exact(&mut boot).unwrap();
        let layout = fat_layout::Layout::detect(&boot, 64 * 1024 * 1024 / 512);
        assert!(layout.overlay);
        let disk = Disk {
            file: RefCell::new(f),
            layout,
        };
        let fs = VolumeManager::new(disk, Clock);
        let v = fs.open_raw_volume(VolumeIdx(0)).unwrap();
        let root = fs.open_root_dir(v).unwrap();
        let existing = fs
            .open_file_in_dir(root, "KEEP.TXT", Mode::ReadWriteCreate)
            .unwrap();
        fs.write(existing, b"existing file must survive").unwrap();
        fs.close_file(existing).unwrap();
        let recorded = fs
            .open_file_in_dir(root, "REC00001.WAV", Mode::ReadWriteCreate)
            .unwrap();
        fs.write(recorded, &pcm::wav_header(0)).unwrap();
        let block = [0x5au8; 8192];
        for _ in 0..6 {
            fs.write(recorded, &block).unwrap();
        }
        fs.file_seek_from_start(recorded, 0).unwrap();
        fs.write(recorded, &pcm::wav_header(6 * 8192)).unwrap();
        fs.close_file(recorded).unwrap();
        assert!(matches!(
            fs.open_file_in_dir(root, "REC00001.WAV", Mode::ReadWriteCreate),
            Err(Error::FileAlreadyExists)
        ));
        let recorded = fs
            .open_file_in_dir(root, "REC00001.WAV", Mode::ReadOnly)
            .unwrap();
        assert_eq!(fs.file_length(recorded).unwrap(), 44 + 6 * 8192);
        let mut h = [0; 44];
        assert_eq!(fs.read(recorded, &mut h).unwrap(), 44);
        assert_eq!(h, pcm::wav_header(6 * 8192));
        let mut b = [0; 8192];
        assert_eq!(fs.read(recorded, &mut b).unwrap(), 8192);
        assert_eq!(b, block);
        fs.close_file(recorded).unwrap();
        let kept = fs
            .open_file_in_dir(root, "KEEP.TXT", Mode::ReadOnly)
            .unwrap();
        let mut text = [0; 26];
        assert_eq!(fs.read(kept, &mut text).unwrap(), 26);
        assert_eq!(&text, b"existing file must survive");
        fs.close_file(kept).unwrap();
        let mut f = File::open(&path).unwrap();
        let mut after = [0; 512];
        f.read_exact(&mut after).unwrap();
        assert_eq!(boot, after);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn deletion_releases_all_clusters_and_lfn_slots_on_fat16_and_fat32() {
        for bits in [16, 32] {
            let base =
                std::env::temp_dir().join(format!("korvo-delete-{}-{}", std::process::id(), bits));
            let path = base.with_extension("img");
            let payload = base.with_extension("bin");
            let mut f = OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&path)
                .unwrap();
            f.set_len(64 * 1024 * 1024).unwrap();
            assert!(
                Command::new("mkfs.fat")
                    .args(["-F", &bits.to_string()])
                    .arg(&path)
                    .status()
                    .unwrap()
                    .success()
            );
            std::fs::write(&payload, vec![0x5a; 65536]).unwrap();
            // Fourteen short entries place the LFN run across a sector boundary.
            for i in 0..14 {
                assert!(
                    Command::new("mcopy")
                        .arg("-i")
                        .arg(&path)
                        .arg(&payload)
                        .arg(format!("::KEEP{:02}.BIN", i))
                        .status()
                        .unwrap()
                        .success()
                );
            }
            let long = "A long microphone recording filename crossing a directory sector.wav";
            assert!(
                Command::new("mcopy")
                    .arg("-i")
                    .arg(&path)
                    .arg(&payload)
                    .arg(format!("::{}", long))
                    .status()
                    .unwrap()
                    .success()
            );
            let mut boot = [0u8; 512];
            f.read_exact(&mut boot).unwrap();
            let layout = fat_layout::Layout::detect(&boot, 64 * 1024 * 1024 / 512);
            let fs = VolumeManager::new(
                Disk {
                    file: RefCell::new(f),
                    layout,
                },
                Clock,
            );
            let v = fs.open_raw_volume(VolumeIdx(0)).unwrap();
            let root = fs.open_root_dir(v).unwrap();
            let mut target = None;
            fs.iterate_dir(root, |entry| {
                let name = entry.name.to_string();
                if name.ends_with(".WAV") {
                    target = Some(name);
                }
                core::ops::ControlFlow::Continue(())
            })
            .unwrap();
            let target = target.unwrap();
            let opened = fs
                .open_file_in_dir(root, target.as_str(), Mode::ReadOnly)
                .unwrap();
            assert!(matches!(
                fs.delete_entry_in_dir(root, target.as_str()),
                Err(Error::FileAlreadyOpen)
            ));
            fs.close_file(opened).unwrap();
            fs.delete_entry_in_dir(root, target.as_str()).unwrap();
            assert!(matches!(
                fs.find_directory_entry(root, target.as_str()),
                Err(Error::NotFound)
            ));
            for i in 0..14 {
                let kept = fs
                    .open_file_in_dir(root, format!("KEEP{:02}.BIN", i).as_str(), Mode::ReadOnly)
                    .unwrap();
                assert_eq!(fs.file_length(kept).unwrap(), 65536);
                let mut sample = [0; 512];
                fs.read(kept, &mut sample).unwrap();
                assert_eq!(sample, [0x5a; 512]);
                fs.close_file(kept).unwrap();
            }
            for (name, data) in [("EMPTY.TXT", &b""[..]), ("ONE.TXT", &b"one cluster"[..])] {
                let file = fs
                    .open_file_in_dir(root, name, Mode::ReadWriteCreate)
                    .unwrap();
                fs.write(file, data).unwrap();
                fs.close_file(file).unwrap();
                fs.delete_entry_in_dir(root, name).unwrap();
                assert!(matches!(
                    fs.find_directory_entry(root, name),
                    Err(Error::NotFound)
                ));
            }
            fs.close_dir(root).unwrap();
            fs.close_volume(v).unwrap();
            drop(fs);
            let mut after = [0; 512];
            File::open(&path).unwrap().read_exact(&mut after).unwrap();
            assert_eq!(boot, after);
            // fsck catches orphan LFN slots, leaked chains, inconsistent mirrors
            // and stale FAT32 free-space accounting. Read-only inspection.
            let check = Command::new("fsck.fat")
                .arg("-n")
                .arg(&path)
                .output()
                .unwrap();
            assert!(
                check.status.success(),
                "{}",
                String::from_utf8_lossy(&check.stdout)
            );
            std::fs::remove_file(&path).unwrap();
            std::fs::remove_file(&payload).unwrap();
        }
    }
    #[test]
    fn avi_video_and_pcm_audio_decode_with_matching_durations() {
        let path = std::env::temp_dir().join(format!("korvo-avi-{}.avi", std::process::id()));
        let frames = 5;
        let mut image = vec![0; avi::FRAME_BYTES];
        for pair in image.chunks_exact_mut(4) {
            pair.copy_from_slice(&[128, 16, 128, 235]);
        }
        assert_eq!(avi::rgb565(&image, 0, 0), 0);
        assert_eq!(avi::rgb565(&image, 1, 0), 0xffff);
        let mut jpeg = Vec::new();
        avi::encode_jpeg(
            &image,
            &mut vec![0; avi::WIDTH * avi::HEIGHT * 3],
            &mut jpeg,
        )
        .unwrap();
        assert!(jpeg.starts_with(&[0xff, 0xd8]) && jpeg.ends_with(&[0xff, 0xd9]));
        // Exercise odd-sized compressed chunks, even when the encoder happens
        // to generate an even-sized JPEG. Decoders accept trailing zero bytes.
        if jpeg.len() & 1 == 0 {
            jpeg.push(0);
        }
        let movi_bytes = frames * (jpeg.len() as u32 + 1 + 8 + 38400 + 8);
        let mut f = File::create(&path).unwrap();
        f.write_all(&avi::header(
            avi::HEADER_BYTES as u32 + movi_bytes,
            frames,
            192000,
            movi_bytes,
        ))
        .unwrap();
        for _ in 0..frames {
            f.write_all(&avi::chunk_header(true, jpeg.len() as u32))
                .unwrap();
            f.write_all(&jpeg).unwrap();
            f.write_all(&[0]).unwrap();
            f.write_all(&avi::chunk_header(false, 38400)).unwrap();
            f.write_all(&vec![0; 38400]).unwrap();
        }
        drop(f);
        let probe = Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-show_entries",
                "stream=codec_name,codec_tag_string,width,height,sample_rate,channels,duration",
                "-of",
                "compact",
            ])
            .arg(&path)
            .output()
            .unwrap();
        assert!(probe.status.success());
        let info = String::from_utf8(probe.stdout).unwrap();
        assert!(
            info.contains("codec_name=mjpeg") && info.contains("codec_tag_string=MJPG"),
            "{}",
            info
        );
        assert!(info.contains("width=320|height=240"), "{}", info);
        assert!(
            info.contains("codec_name=pcm_s16le") && info.contains("sample_rate=48000|channels=2"),
            "{}",
            info
        );
        assert!(info.contains("duration=1.000000"), "{}", info);
        let audio = Command::new("ffmpeg")
            .args(["-v", "error", "-i"])
            .arg(&path)
            .args(["-map", "0:a:0", "-f", "s16le", "-"])
            .output()
            .unwrap();
        assert!(audio.status.success());
        assert_eq!(audio.stdout.len(), 192000);
        let decode = Command::new("ffmpeg")
            .args(["-v", "error", "-i"])
            .arg(&path)
            .args(["-f", "null", "-"])
            .output()
            .unwrap();
        assert!(
            decode.status.success(),
            "{}",
            String::from_utf8_lossy(&decode.stderr)
        );
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn sd_video_stream_keeps_pcm_continuous_when_decoder_is_slower_than_video() {
        let path =
            std::env::temp_dir().join(format!("korvo-video-stream-{}.img", std::process::id()));
        let mut disk = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        disk.set_len(64 * 1024 * 1024).unwrap();
        assert!(
            Command::new("mkfs.fat")
                .args(["-F", "32"])
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        let mut boot = [0; 512];
        disk.read_exact(&mut boot).unwrap();
        let fs = VolumeManager::new(
            Disk {
                file: RefCell::new(disk),
                layout: fat_layout::Layout::detect(&boot, 64 * 1024 * 1024 / 512),
            },
            Clock,
        );
        let v = fs.open_raw_volume(VolumeIdx(0)).unwrap();
        let root = fs.open_root_dir(v).unwrap();
        let file = fs
            .open_file_in_dir(root, "VIDTEST.AVI", Mode::ReadWriteCreate)
            .unwrap();
        let count = 40;
        let rgb = vec![127; avi::WIDTH * avi::HEIGHT * 3];
        let mut jpeg = Vec::new();
        jpeg_encoder::Encoder::new(&mut jpeg, 45)
            .encode(
                &rgb,
                avi::WIDTH as u16,
                avi::HEIGHT as u16,
                jpeg_encoder::ColorType::Rgb,
            )
            .unwrap();
        let movi = count * (8 + jpeg.len() as u32 + (jpeg.len() as u32 & 1) + 8 + 38400);
        fs.write(file, &avi::header(324 + movi, count, count * 38400, movi))
            .unwrap();
        let mut expected = Vec::new();
        for frame in 0..count {
            fs.write(file, &avi::chunk_header(true, jpeg.len() as u32))
                .unwrap();
            fs.write(file, &jpeg).unwrap();
            if jpeg.len() & 1 != 0 {
                fs.write(file, &[0]).unwrap();
            }
            fs.write(file, &avi::chunk_header(false, 38400)).unwrap();
            let pcm = vec![frame as u8; 38400];
            fs.write(file, &pcm).unwrap();
            expected.extend_from_slice(&pcm);
        }
        fs.close_file(file).unwrap();
        let storage = storage::Storage { fs, root };
        let worker = Box::leak(Box::new(jpeg_decoder::Worker::new()));
        let name = storage::Name::try_from("VIDTEST.AVI").unwrap();
        let mut player = video_player::Player::new(&storage, name, worker, 11).unwrap();
        let mut audio = audio::Audio::new(expected.len());
        // Read-ahead before clocks consume the first PCM. Simulated 2 ms loop
        // and 300 ms decoding: the video worker cannot sustain the 5 fps file.
        for _ in 0..100 {
            assert!(!player.poll(&storage, &mut audio).unwrap());
        }
        let mut complete = false;
        for tick in 0..6000 {
            if tick % 150 == 0 {
                worker.run();
            }
            if player.poll(&storage, &mut audio).unwrap() {
                complete = true;
                break;
            }
            audio.advance(384);
        }
        assert!(complete, "stream stalled");
        assert_eq!(audio.accepted, expected, "PCM was dropped or reordered");
        assert_eq!(audio.gaps, 0, "JPEG decoding held up PCM");
        assert!(
            player.skipped > 0,
            "slow video worker should drop old pending images"
        );
        assert!(player.decoded < count);
        assert_eq!(player.displayed, count);
        storage.fs.close_file(player.file).unwrap();
        drop(player);
        drop(storage);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn recursive_catalog_exceeds_64_tracks_and_pins_nested_delete() {
        let path = std::env::temp_dir().join(format!("korvo-library-{}.img", std::process::id()));
        let mut f = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        f.set_len(64 * 1024 * 1024).unwrap();
        assert!(
            Command::new("mkfs.fat")
                .args(["-F", "32"])
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        let mut boot = [0u8; 512];
        f.read_exact(&mut boot).unwrap();
        let layout = fat_layout::Layout::detect(&boot, 64 * 1024 * 1024 / 512);
        let fs = VolumeManager::new(
            Disk {
                file: RefCell::new(f),
                layout,
            },
            Clock,
        );
        let vol = fs.open_raw_volume(VolumeIdx(0)).unwrap();
        let root = fs.open_root_dir(vol).unwrap();
        for i in 0..70 {
            let name = format!("SONG{i:04}.MP3");
            let file = fs
                .open_file_in_dir(root, name.as_str(), Mode::ReadWriteCreate)
                .unwrap();
            fs.write(file, &[i as u8]).unwrap();
            fs.close_file(file).unwrap();
        }
        let mut current = root;
        let mut deepest = String::new();
        for depth in 0..8 {
            let name = format!("LEVEL{depth}");
            fs.make_dir_in_dir(current, name.as_str()).unwrap();
            let child = fs.open_dir(current, name.as_str()).unwrap();
            if current != root {
                fs.close_dir(current).unwrap();
            }
            current = child;
            deepest.push_str(&name);
            deepest.push('/');
        }
        fs.close_dir(current).unwrap();
        deepest.push_str("SONG0000.MP3");
        let nested = crate::fat_files::open(&fs, root, &deepest, Mode::ReadWriteCreate).unwrap();
        fs.write(nested, b"nested payload").unwrap();
        fs.close_file(nested).unwrap();
        let catalog = crate::fat_files::catalog(&fs, root).unwrap();
        assert_eq!(catalog.len(), 71);
        assert!(catalog.contains(&deepest));
        let open = crate::fat_files::open(&fs, root, &deepest, Mode::ReadOnly).unwrap();
        assert!(crate::fat_files::delete(&fs, root, &deepest).is_err());
        let mut payload = [0; 14];
        assert_eq!(fs.read(open, &mut payload).unwrap(), 14);
        assert_eq!(&payload, b"nested payload");
        fs.close_file(open).unwrap();
        crate::fat_files::delete(&fs, root, &deepest).unwrap();
        assert_eq!(crate::fat_files::catalog(&fs, root).unwrap().len(), 70);
        let sibling = crate::fat_files::open(&fs, root, "SONG0000.MP3", Mode::ReadOnly).unwrap();
        assert_eq!(fs.file_length(sibling).unwrap(), 1);
        fs.close_file(sibling).unwrap();
        assert!(
            crate::fat_files::open(&fs, root, "LEVEL0/../SONG0000.MP3", Mode::ReadOnly).is_err()
        );
        // Failed paths must not leak handles into later traversals.
        for _ in 0..10 {
            assert!(
                crate::fat_files::open(&fs, root, "LEVEL0/MISSING/X.MP3", Mode::ReadOnly).is_err()
            );
        }
        assert_eq!(crate::fat_files::catalog(&fs, root).unwrap().len(), 70);
        fs.close_dir(root).unwrap();
        fs.close_volume(vol).unwrap();
        drop(fs);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn gpt_fat32_mount_record_and_crc_rejection_preserve_partition_tables() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("korvo-gpt-{}.img", std::process::id()));
        let part = path.with_extension("fat");
        let mut disk = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        disk.set_len(66 * 1024 * 1024).unwrap();
        assert!(
            Command::new("sgdisk")
                .args(["-og", "--new=1:2048:133119", "--typecode=1:0700"])
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        let volume = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&part)
            .unwrap();
        volume.set_len(64 * 1024 * 1024).unwrap();
        assert!(
            Command::new("mkfs.fat")
                .args(["-F", "32"])
                .arg(&part)
                .status()
                .unwrap()
                .success()
        );
        disk.seek(SeekFrom::Start(1024 * 1024)).unwrap();
        std::io::copy(&mut File::open(&part).unwrap(), &mut disk).unwrap();
        disk.seek(SeekFrom::Start(0)).unwrap();
        let mut metadata = vec![0; 34 * 512];
        disk.read_exact(&mut metadata).unwrap();
        let layout = fat_layout::Layout::read(66 * 2048, |index, bytes| {
            disk.seek(SeekFrom::Start(index as u64 * 512)).unwrap();
            disk.read_exact(bytes).unwrap();
            Ok(())
        })
        .unwrap();
        assert_eq!(
            (layout.offset, layout.blocks, layout.overlay, layout.fat32),
            (2048, 131072, true, true)
        );
        assert_eq!(layout.physical(0), None);
        assert_eq!(layout.physical(1), Some(2048));
        assert_eq!(layout.physical(131072), Some(133119));
        assert_eq!(layout.physical(131073), None);
        let fs = VolumeManager::new(
            Disk {
                file: RefCell::new(disk),
                layout,
            },
            Clock,
        );
        let vol = fs.open_raw_volume(VolumeIdx(0)).unwrap();
        let root = fs.open_root_dir(vol).unwrap();
        let file = fs
            .open_file_in_dir(root, "REC00001.WAV", Mode::ReadWriteCreate)
            .unwrap();
        fs.write(file, &pcm::wav_header(4096)).unwrap();
        fs.write(file, &[0; 4096]).unwrap();
        fs.close_file(file).unwrap();
        let file = fs
            .open_file_in_dir(root, "REC00001.WAV", Mode::ReadOnly)
            .unwrap();
        assert_eq!(fs.file_length(file).unwrap(), 4140);
        fs.close_file(file).unwrap();
        fs.close_dir(root).unwrap();
        fs.close_volume(vol).unwrap();
        drop(fs);
        let mut disk = File::open(&path).unwrap();
        let mut after = vec![0; metadata.len()];
        disk.read_exact(&mut after).unwrap();
        assert_eq!(metadata, after, "GPT or protective MBR was modified");
        // Independently generated GPT checksums must fail if header/table data changes.
        for corrupt in [512 + 40, 1024] {
            let mut damaged = metadata.clone();
            damaged[corrupt] ^= 1;
            assert!(
                fat_layout::Layout::read(66 * 2048, |index, bytes| {
                    let start = index as usize * 512;
                    bytes.copy_from_slice(&damaged[start..start + 512]);
                    Ok(())
                })
                .is_err()
            );
        }
        std::fs::remove_file(path).unwrap();
        std::fs::remove_file(part).unwrap();
    }
    #[test]
    fn mp3_fixture_decodes_and_resamples_to_48khz_stereo() {
        let mut data = include_bytes!("../../fixtures/demo.mp3").as_slice();
        let mut d = nanomp3_core::Decoder::new();
        let mut pcm = [0i16; nanomp3_core::MAX_SAMPLES_PER_FRAME];
        let mut frames = 0;
        let mut nonzero = 0;
        let mut r = pcm::Resampler::default();
        let mut out = [0u8; 24];
        let mut bytes = 0;
        while !data.is_empty() {
            let (used, info) = d.decode(data, &mut pcm);
            assert!(used > 0);
            data = &data[used..];
            if let Ok(info) = info {
                assert_eq!(info.sample_rate, 44100);
                assert_eq!(info.channels.num(), 2);
                frames += info.samples_produced;
                for frame in pcm[..info.samples_produced * 2].chunks_exact(2) {
                    nonzero += usize::from(frame[0] != 0);
                    bytes += r.frame(frame[0], frame[1], 44100, &mut out);
                }
            }
        }
        assert!(frames >= 44100 * 2);
        assert!(nonzero > 44100);
        let mut tail = [0u8; 744];
        bytes += r.finish(&mut tail);
        assert_eq!(bytes / 4, (frames as u64 * 48000 / 44100) as usize);
    }
}

#[test]
fn jpeg_mailbox_owns_buffers_across_threads_and_preserves_generation() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let worker = Arc::new(jpeg_worker::Worker::new());
    let done = Arc::new(AtomicBool::new(false));
    let encoding = worker.clone();
    let stop = done.clone();
    let encoder = std::thread::spawn(move || {
        while !stop.load(Ordering::Acquire) {
            encoding.encode();
            std::thread::yield_now();
        }
    });
    let frame = [128, 16, 128, 235].repeat(avi::FRAME_BYTES / 4);
    assert!(!worker.submit(&frame[..frame.len() - 1], 999));
    assert!(worker.take().is_none());
    for generation in 1..=8 {
        assert!(worker.submit(&frame, generation));
        assert!(!worker.submit(&frame, 999));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if let Some((id, jpeg)) = worker.take() {
                assert_eq!(id, generation);
                let jpeg = jpeg.unwrap();
                assert_eq!(&jpeg[..2], &[255, 216]);
                assert_eq!(&jpeg[jpeg.len() - 2..], &[255, 217]);
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
    }
    done.store(true, Ordering::Release);
    encoder.join().unwrap();
}

#[test]
fn playback_rejects_unfinished_wrong_format_and_invalid_chunks() {
    use avi_playback::{Chunk, Format, Kind};
    let size = avi::HEADER_BYTES as u32 + 1234;
    let header = avi::header(size, 5, 192000, 1234);
    let f = Format::parse(&header, size).unwrap();
    assert_eq!((f.end, f.frames, f.audio_bytes), (size, 5, 192000));
    assert!(Format::parse(&header, size - 1).is_err());
    assert!(Format::parse(&header[..323], size).is_err());
    for (offset, value) in [
        (112, b'X'),
        (176, 0),
        (298, 1),
        (300, 0),
        (310, 8),
        (140, 4),
    ] {
        let mut bad = header;
        bad[offset] = value;
        assert!(Format::parse(&bad, size).is_err(), "offset {offset}");
    }
    assert!(Format::parse(&avi::header(324, 0, 0, 0), 324).is_err());
    let odd = Chunk::parse(&avi::chunk_header(true, 9), 324, 342).unwrap();
    assert_eq!((odd.kind, odd.bytes, odd.next), (Kind::Video, 9, 342));
    assert!(Chunk::parse(&avi::chunk_header(true, 9), 324, 341).is_err());
    assert!(Chunk::parse(&avi::chunk_header(true, 0), 324, 400).is_err());
    assert!(Chunk::parse(&avi::chunk_header(true, u32::MAX), 324, u32::MAX).is_err());
    assert!(Chunk::parse(&avi::chunk_header(false, 3), 324, 400).is_err());
    assert!(
        Chunk::parse(
            &avi::chunk_header(true, avi::FRAME_BYTES as u32 + 1),
            0,
            u32::MAX
        )
        .is_err()
    );
}
#[test]
fn jpeg_playback_decodes_colours_and_bounds_mailbox_across_threads() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let mut rgb = vec![0u8; avi::WIDTH * avi::HEIGHT * 3];
    for (index, pixel) in rgb.chunks_exact_mut(3).enumerate() {
        pixel.copy_from_slice(if index % avi::WIDTH < avi::WIDTH / 2 {
            &[255, 0, 0]
        } else {
            &[0, 0, 255]
        });
    }
    let mut jpeg = Vec::new();
    jpeg_encoder::Encoder::new(&mut jpeg, 90)
        .encode(
            &rgb,
            avi::WIDTH as u16,
            avi::HEIGHT as u16,
            jpeg_encoder::ColorType::Rgb,
        )
        .unwrap();
    let worker = Arc::new(jpeg_decoder::Worker::new());
    let done = Arc::new(AtomicBool::new(false));
    let decoding = worker.clone();
    let stop = done.clone();
    let decoder = std::thread::spawn(move || {
        while !stop.load(Ordering::Acquire) {
            decoding.run();
            std::thread::yield_now();
        }
    });
    assert!(!worker.submit(&[], 1, 0));
    assert!(!worker.submit(&vec![0; avi::FRAME_BYTES + 1], 1, 0));
    for generation in 1..=5 {
        let input = if generation == 3 {
            b"broken JPEG".as_slice()
        } else {
            &jpeg
        };
        assert!(worker.submit(input, generation, generation + 8));
        assert!(!worker.submit(&jpeg, 999, 0));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if let Some((id, index, output)) = worker.take() {
                assert_eq!((id, index), (generation, generation + 8));
                if generation == 3 {
                    assert!(output.is_err());
                } else {
                    let image = output.unwrap();
                    assert_eq!(image.len(), avi::FRAME_BYTES);
                    let red = u16::from_le_bytes(image[40..42].try_into().unwrap());
                    let blue = u16::from_le_bytes(image[600..602].try_into().unwrap());
                    assert!(red & 0xf800 >= 0xf000 && red & 0x1f < 4, "{red:x}");
                    assert!(blue & 0x1f >= 28 && blue & 0xf800 < 0x1000, "{blue:x}");
                }
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
    }
    done.store(true, Ordering::Release);
    decoder.join().unwrap();
    let mut oversized = Vec::new();
    jpeg_encoder::Encoder::new(&mut oversized, 45)
        .encode(
            &vec![0; 321 * 240 * 3],
            321,
            240,
            jpeg_encoder::ColorType::Rgb,
        )
        .unwrap();
    assert!(jpeg_decoder::decode(&oversized, &mut rgb, &mut vec![0; avi::FRAME_BYTES]).is_err());
}

#[test]
fn lcd_paint_preserves_queued_and_ready_decode_jobs() {
    let worker = jpeg_decoder::Worker::new();
    assert!(worker.paint(|| {
        assert!(!worker.submit(b"invalid", 1, 0));
    }));
    assert!(worker.submit(b"invalid", 7, 42));
    assert!(worker.paint(|| {
        worker.run();
        assert!(worker.take().is_none());
    }));
    worker.run();
    assert!(worker.paint(|| {
        assert!(worker.take().is_none());
    }));
    let (generation, index, result) = worker.take().unwrap();
    assert_eq!((generation, index), (7, 42));
    assert!(result.is_err());
    assert!(worker.idle());
}

#[cfg(test)]
#[path = "../../../src/usb_input.rs"]
mod usb_input;

#[cfg(test)]
#[path = "../../../src/usb_hid.rs"]
mod usb_hid;

#[cfg(test)]
mod mp3_tests;
