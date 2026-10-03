use crate::{mp3, pcm};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
static FILE_ID: AtomicUsize = AtomicUsize::new(0);

fn encode(rate: u32, channels: u8, vbr: bool) -> Vec<u8> {
    let path = std::env::temp_dir().join(format!(
        "korvo-mp3-{}-{}-{rate}-{channels}-{vbr}.mp3",
        std::process::id(),
        FILE_ID.fetch_add(1, Ordering::Relaxed)
    ));
    let mut command = Command::new("ffmpeg");
    command.args([
        "-v",
        "error",
        "-y",
        "-f",
        "lavfi",
        "-i",
        &format!("sine=frequency=997:sample_rate={rate}:duration=0.4"),
        "-ac",
        &channels.to_string(),
        "-c:a",
        "libmp3lame",
    ]);
    if vbr {
        command.args(["-q:a", "2"]);
    } else {
        command.args(["-b:a", if rate >= 32000 { "320k" } else { "64k" }]);
    }
    assert!(
        command
            .args([
                "-id3v2_version",
                "4",
                "-metadata",
                "title=Korvo format test"
            ])
            .arg(&path)
            .status()
            .expect("ffmpeg with libmp3lame is required")
            .success()
    );
    let data = std::fs::read(&path).unwrap();
    std::fs::remove_file(path).unwrap();
    data
}
fn decode(data: &[u8]) -> (Vec<i16>, u32, u8, bool) {
    let mut pos = 0;
    loop {
        let n = mp3::leading_tag(
            &data[pos..data.len().min(pos + 10)],
            (data.len() - pos) as u32,
        )
        .unwrap();
        if n == 0 {
            break;
        }
        pos += n as usize;
    }
    let mut decoder = nanomp3_core::Decoder::new();
    let mut stream = mp3::Stream::default();
    let mut buffer = Vec::new();
    let mut samples = [0i16; nanomp3_core::MAX_SAMPLES_PER_FRAME];
    let mut output = Vec::new();
    let (mut rate, mut channels) = (0, 0);
    let mut iterations = 0;
    loop {
        // Emulate short sector reads while filling the decoder's lookahead.
        if buffer.len() < 8192 {
            while buffer.len() < 16384 && pos < data.len() {
                let n = (16384 - buffer.len()).min(512).min(data.len() - pos);
                buffer.extend_from_slice(&data[pos..pos + n]);
                pos += n;
            }
        }
        let eof = pos == data.len();
        if eof {
            buffer.truncate(stream.strip_tail(&buffer));
        }
        if buffer.is_empty() || stream.complete() {
            break;
        }
        let (used, frame) = stream
            .decode(&mut decoder, &buffer, eof, &mut samples)
            .unwrap();
        assert!(
            used > 0 && used <= buffer.len(),
            "decoder must make bounded progress"
        );
        buffer.drain(..used);
        if let Some(frame) = frame {
            rate = frame.info.sample_rate;
            channels = frame.info.channels.num();
            output.extend_from_slice(&samples[frame.samples]);
        }
        iterations += 1;
        assert!(iterations < data.len() + 10);
    }
    (output, rate, channels, stream.trimmed)
}
#[test]
fn mpeg1_2_25_all_rates_mono_stereo_cbr_vbr_and_gapless() {
    for rate in [8000, 11025, 12000, 16000, 22050, 24000, 32000, 44100, 48000] {
        for channels in [1, 2] {
            for vbr in [false, true] {
                let data = encode(rate, channels, vbr);
                let (samples, actual_rate, actual_channels, trimmed) = decode(&data);
                assert_eq!((actual_rate, actual_channels), (rate, channels));
                assert!(trimmed, "Xing/Info Lavc delay/padding missing at {rate}");
                assert_eq!(
                    samples.len(),
                    rate as usize * 2 / 5 * channels as usize,
                    "wrong trimmed duration for {rate}/{channels}/{vbr}"
                );
                assert!(samples.iter().any(|&s| s.abs() > 1000));
                let mut resampler = pcm::Resampler::default();
                let mut converted = 0;
                let mut out = [0u8; 24];
                for frame in samples.chunks_exact(channels as usize) {
                    converted +=
                        resampler.frame(frame[0], frame[channels as usize - 1], rate, &mut out);
                }
                let mut tail = [0u8; 744];
                converted += resampler.finish(&mut tail);
                assert_eq!(converted / 4, 19200);
            }
        }
    }
}
#[test]
fn large_artwork_footer_tags_junk_boundaries_and_truncation() {
    let encoded = encode(44100, 2, true);
    let baseline = decode(&encoded).0;
    let size = 100_000u32;
    let mut tagged = b"ID3\x04\0\x10".to_vec();
    for shift in [21, 14, 7, 0] {
        tagged.push(((size >> shift) & 127) as u8);
    }
    tagged.extend_from_slice(&vec![0; size as usize]);
    tagged.extend_from_slice(b"3DI\x04\0\x10\0\0\0\0");
    tagged.extend_from_slice(&encoded);
    tagged.extend_from_slice(b"APETAGEX");
    tagged.extend_from_slice(&2000u32.to_le_bytes());
    tagged.extend_from_slice(&32u32.to_le_bytes());
    tagged.extend_from_slice(&[0; 16]);
    tagged.extend_from_slice(b"TAG");
    tagged.extend_from_slice(&[0; 125]);
    assert_eq!(decode(&tagged).0, baseline);
    let mut junk = vec![0x55; 16380];
    junk.extend_from_slice(
        &encoded[mp3::leading_tag(&encoded[..10], encoded.len() as u32).unwrap() as usize..],
    );
    assert_eq!(
        decode(&junk).0,
        baseline,
        "header crossing the lookahead boundary was lost"
    );
    let (short, _, _, _) = decode(&encoded[..encoded.len() - 300]);
    assert!(!short.is_empty() && short.len() <= baseline.len());
    assert!(mp3::leading_tag(b"ID3\x04\0\0\xff\0\0\0", 100).is_err());
    assert!(mp3::leading_tag(b"ID3\x04\0\0\0\0\x01\0", 20).is_err());
}
#[test]
fn free_format_layer3_decodes() {
    let dir = std::env::temp_dir();
    let wav = dir.join(format!("korvo-free-{}.wav", std::process::id()));
    let mp3 = wav.with_extension("mp3");
    assert!(
        Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=880:sample_rate=44100:duration=0.4",
                "-ac",
                "2"
            ])
            .arg(&wav)
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("lame")
            .args(["--silent", "--freeformat", "-b", "400"])
            .arg(&wav)
            .arg(&mp3)
            .status()
            .expect("lame is required for free-format validation")
            .success()
    );
    let data = std::fs::read(&mp3).unwrap();
    let (samples, rate, channels, _) = decode(&data);
    assert_eq!((rate, channels), (44100, 2));
    assert_eq!(samples.len(), 17640 * 2);
    std::fs::remove_file(wav).unwrap();
    std::fs::remove_file(mp3).unwrap();
}

#[test]
fn independent_stereo_channels_match_reference_decoder() {
    for data in [
        include_bytes!("../../fixtures/high320.mp3").as_slice(),
        include_bytes!("../../fixtures/vbr.mp3").as_slice(),
        include_bytes!("../../fixtures/high441.mp3").as_slice(),
    ] {
        let (decoded, rate, channels, trimmed) = decode(data);
        assert_eq!(channels, 2);
        assert!(trimmed);
        let path =
            std::env::temp_dir().join(format!("korvo-reference-{}-{rate}.mp3", std::process::id()));
        std::fs::write(&path, data).unwrap();
        let reference = Command::new("ffmpeg")
            .args(["-v", "error", "-i"])
            .arg(&path)
            .args(["-f", "s16le", "-acodec", "pcm_s16le", "-"])
            .output()
            .unwrap();
        assert!(reference.status.success());
        std::fs::remove_file(path).unwrap();
        let reference: Vec<i16> = reference
            .stdout
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes(b.try_into().unwrap()))
            .collect();
        assert_eq!(decoded.len(), reference.len());
        let mut signal = [0.0; 2];
        let mut error = [0.0; 2];
        for (i, (&sample, &expected)) in decoded.iter().zip(&reference).enumerate() {
            signal[i % 2] += (expected as f64).powi(2);
            error[i % 2] += (sample as f64 - expected as f64).powi(2);
        }
        for c in 0..2 {
            assert!(
                signal[c] / error[c].max(1.0) > 1_000_000.0,
                "channel {c} differs from FFmpeg by more than -60 dB"
            );
        }
        assert!(
            decoded.chunks_exact(2).filter(|s| s[0] != s[1]).count() > decoded.len() / 4,
            "stereo was collapsed to mono"
        );
    }
}

#[test]
fn in_place_decoder_reset_releases_previous_format_and_reservoir() {
    let mut reused = nanomp3_core::Decoder::new();
    let mut scratch = [0i16; nanomp3_core::MAX_SAMPLES_PER_FRAME];
    for data in [
        include_bytes!("../../fixtures/vbr.mp3").as_slice(),
        include_bytes!("../../fixtures/low8k.mp3").as_slice(),
        include_bytes!("../../fixtures/high320.mp3").as_slice(),
    ] {
        nanomp3_core::__private::init(&mut reused);
        let mut fresh = nanomp3_core::Decoder::new();
        let mut expected = [0i16; nanomp3_core::MAX_SAMPLES_PER_FRAME];
        let mut input = data;
        while !input.is_empty() {
            let (used, info) = reused.decode(input, &mut scratch);
            let (reference_used, reference_info) = fresh.decode(input, &mut expected);
            assert_eq!((used, info), (reference_used, reference_info));
            assert!(used > 0);
            if let Ok(info) = info {
                assert_eq!(
                    scratch[..info.samples_produced * info.channels.num() as usize],
                    expected[..info.samples_produced * info.channels.num() as usize]
                );
            }
            input = &input[used..];
        }
    }
}
