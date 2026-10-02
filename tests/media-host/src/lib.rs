#[path = "../../../src/fat_layout.rs"]
mod fat_layout;
#[path = "../../../src/pcm.rs"]
mod pcm;
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
