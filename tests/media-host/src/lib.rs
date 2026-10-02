#[path = "../../../src/avi.rs"]
mod avi;
#[path = "../../../src/avi_playback.rs"]
mod avi_playback;
#[path = "../../../src/delete_request.rs"]
mod delete_request;
#[path = "../../../src/fat_layout.rs"]
mod fat_layout;
#[path = "../../../src/jpeg_decoder.rs"]
mod jpeg_decoder;
#[path = "../../../src/jpeg_worker.rs"]
mod jpeg_worker;
#[path = "../../../src/pcm.rs"]
mod pcm;
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
    struct Disk {
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
    struct Clock;
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
