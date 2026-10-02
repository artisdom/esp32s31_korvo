# SD audio playback and microphone recording

The AUDIO page now controls media on the native SDMMC card. The WS2812 stays
off, audio starts silent, and speaker volume defaults to -30 dB. Use VOL+/VOL-
to adjust playback in 3 dB steps.

## Controls

| Control | Action |
|---|---|
| PREVIOUS / NEXT | Select a file in the root-directory list |
| PLAY | Stop current media and play the selected file |
| STOP | Stop playback, or finalize and save the recording |
| RECORD / SAVE | Start a new recording; a second tap saves it |
| REPLAY LAST | Play the most recent recording saved during this boot |
| RESCAN SD | Refresh the root list while idle |
| SET on AUDIO | Same as RECORD / SAVE |
| MODE | Change pages without interrupting media |

Touch actions require 100 ms of sustained release before another tap can fire.
Empty polls between GT1151 reports no longer retrigger a held button. Status
text names the file being played or recorded; the selected file is separate.

Put MP3/WAV files in the root before boot. The browser lists up to 64 tracks
with their FAT 8.3 names (long-name files appear under their short aliases).
Recordings use the next unused numbered name, `REC00001.WAV` etc. Existing
files are not overwritten. After reboot, recordings remain in the track list;
select them and use PLAY.

## Supported formats

- MPEG Layer III MP3, decoded by the no-std, safe Rust `nanomp3-core` crate.
  ID3v2 tags are skipped; mono is duplicated to stereo.
- RIFF WAV with integer PCM: 8/16/24/32-bit, mono/stereo, 8–96 kHz. Additional
  chunks are skipped. Float, extensible WAV, AAC, FLAC and Ogg are unsupported.
- Microphone gain defaults to 9.5 dB on both channels, without a DAC-reference
  channel. The codec is initialised after I2S clocks start.
- Output is 48 kHz stereo PCM16 in 32-bit I2S wire slots (64 clocks/frame).
  SCLK is 3.072 MHz; the codec uses its matching ratio-64 coefficients.
  DMA and WAV files still store two packed 16-bit samples per frame. The app
  explicitly sets slot widths and BCLK because the current S31 HAL derives
  them from data width and reverses mixed data/slot register widths.
- The speaker amplifier is disabled at startup, while idle, and while
  recording; it is enabled for requested playback and disabled after file
  playback drains. I2S clocks and microphone capture continue throughout.
- Output rate conversion currently uses nearest-neighbour sampling; it maintains
  pitch/duration but is not a high-quality resampler.
  MP3 encoder delay/padding is retained (no gapless playback).
- Capture is 48 kHz stereo PCM16 WAV, about 11.5 MB/minute. STOP flushes staged
  audio, rewrites RIFF/data lengths, and closes the FAT file.

FAT16/FAT32 cards are supported, both MBR-partitioned and superfloppy layouts.
The existing 128 GB superfloppy FAT32 card mounts without modification to its
boot sector. There is no formatting, exFAT media support, hot removal or
power-loss recovery. Stop recording before removing the card or powering off.
FAT creation timestamps use the fixed date 2026-10-02.

## Console

The USB-C UART console is 115200 8N1. Send a command followed by newline:

```
next
prev
play
stop
record
replay
rescan
tone
mic
demo
```

`record` toggles recording/saving; `tone` requests 440 Hz until STOP; `mic`
prints raw stereo samples for diagnosis. `demo` creates `KORVOWAV.WAV`
(16 kHz mono, 660 Hz) and `KORVOMP3.MP3` (44.1 kHz stereo, 440 Hz), skipping
existing files with those names. These generated test tones were installed
on the board during validation. Reproduction commands are in
[fixtures/README.md](../tests/fixtures/README.md).

## Validation

Host tests:

```
cargo test --manifest-path tests/media-host/Cargo.toml --target x86_64-unknown-linux-gnu
cargo build --release
```

The host suite requires `mkfs.fat` and creates a temporary 64 MiB test image.
It checks FAT recording across cluster boundaries, final header/readback,
collision rejection, existing-file preservation, an unchanged superfloppy
boot sector, MP3 decoding/resampling, WAV widths/alignment, and rate-conversion
phase across blocks.

Board checks on 2026-10-03 (Pacific/Auckland):

- Native SDMMC mounted the 128 GB FAT32 card for media file access.
- Created/reopened numbered WAV recordings; a ten-second test saved
  1,940,480 PCM bytes (10.107 seconds). Replay reached completion.
- Both generated WAV and MP3 files were read from SD and played to completion.
  MP3 decoding reported 44,100 Hz, two channels.
- Capture reported zero overruns during the ten-second recording/replay test;
  LCD underruns remained zero. Four TX underrun events occurred around startup
  and file boundaries; clocks continued and played blocks cleared to silence.
- The user confirmed the earlier chime was audible but too loud, prompting
  silent startup and the -30 dB default. Speech quality validation is pending.

Final committed firmware `3c5eafd` was flashed after USB reconnection. Startup
confirmed the codec, touch, and 128 GB FAT32 card; RX and LCD overrun counts
remained zero. Microphone levels varied between runs and sometimes reached
clipping, so intelligible speech playback still needs user confirmation.

## Static and repeated-touch correction (2026-10-03)

The user subsequently reported static during recording/replay and while idle,
although the generated test tones remained audible. The earlier file-completion
checks did not establish sound quality. PREVIOUS/NEXT also repeatedly selected
files because empty touch polls were treated as releases.

With the former 16-bit wire slots and ratio-32 codec configuration, captured
blocks showed clipped bursts reaching both PCM limits. A temporary internal
GPIO-matrix TX-to-RX loopback reproduced the 440 Hz tone correctly on both
channels (peak 16000, RMS about 11313), isolating the digital streaming path.
Using 32-bit wire slots and ratio-64 codec settings, quiet microphone captures
with the amplifier disabled measured roughly 2–3 PCM counts RMS per channel,
with extrema within -9..7. These are bench observations, not a speech-quality
confirmation. The unsuccessful external-MCLK experiment and blocking snapshot
diagnostics were removed from the final firmware.

The release build and three touch-action tests pass. Physical confirmation of
quiet idle, clean WAV/MP3 playback, intelligible new recordings, and one file
selection per held PREVIOUS/NEXT touch is still pending. Existing recordings
from the noisy configuration are preserved and may still contain static; make
a new recording to evaluate the corrected capture path.
