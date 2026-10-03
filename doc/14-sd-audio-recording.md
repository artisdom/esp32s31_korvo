# SD audio playback and microphone recording

The AUDIO page now controls media on the native SDMMC card. The WS2812 stays
off, audio starts silent, and speaker volume defaults to -30 dB. Use VOL+/VOL-
to adjust playback in 3 dB steps.

## Controls

| Control | Action |
|---|---|
| PREVIOUS / NEXT | Select a file in the recursive file list |
| PLAY | Stop current media and play the selected file |
| STOP | Stop playback, or finalize and save the recording |
| RECORD / SAVE | Start a new recording; a second tap saves it |
| REPLAY LAST | Play the most recent recording saved during this boot |
| RESCAN SD | Refresh the folder/file lists while idle |
| DELETE SELECTED | Ask to permanently delete the selected file |
| CONFIRM / CANCEL | Delete the named file, or dismiss the request |
| SET on AUDIO | Same as RECORD / SAVE |
| MODE | Change pages without interrupting media |

Touch actions require 100 ms of sustained release before another tap can fire.
Empty polls between GT1151 reports no longer retrigger a held button. Status
text names the file being played or recorded; the selected file is separate.

Put MP3/WAV files anywhere on the card before boot. The browser scans folders
and sorts full FAT 8.3 alias paths. The former 64-track cap is removed; available
memory limits catalog size. The UI shows the file alias and its folder.
Recordings use the next unused numbered name, `REC00001.WAV` etc. Existing
files are not overwritten. After reboot, recordings remain in the track list;
select them and use PLAY.

## Supported formats

- MPEG-1, MPEG-2 and MPEG-2.5 Layer III MP3, decoded by the no-std, safe Rust
  `nanomp3-core` crate, with `nanomp3` metadata helpers. All nine rates (8, 11.025,
  12, 16, 22.05, 24, 32, 44.1 and 48 kHz), mono/stereo/joint stereo, CBR, VBR
  and free-format are supported. Mono is duplicated to stereo; independent
  stereo channels are preserved. Standard MPEG-1 bitrates through 320 kbps
  and a 400 kbps free-format stream are covered by tests.
- Leading ID3v2.2/2.3/2.4 tags, including large artwork and v2.4 footers, are
  skipped by seeking, without loading the tag. Trailing ID3v1 and APEv2 tags
  are skipped once. Malformed sizes are rejected. Bounded 16 KiB lookahead
  retains possible frames across junk/read boundaries; corrupt or unavailable
  reservoir frames are skipped rather than ending the whole file.
- Xing/Info tags with LAME/Lavc delay/padding trim encoder and synthesis delay
  and end padding. Without this metadata the full decoded stream is played.
  This is per-file trimming, not automatic gapless playlist playback.
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
- Rate conversion uses a 64-tap Blackman-windowed sinc filter with 64 phases
  and coefficient interpolation. Fractional phase and separate channel history
  persist across blocks. The 31-input-frame lookahead is flushed at EOF,
  preserving `floor(input_frames * 48000 / input_rate)` output frames. Native
  48 kHz passes through bit exactly. WAV rates above 48 kHz use a rate-specific
  anti-alias filter. Playback waits for the final queued PCM to drain.
- Capture is 48 kHz stereo PCM16 WAV, about 11.5 MB/minute. STOP flushes staged
  audio, rewrites RIFF/data lengths, and closes the FAT file.

FAT16/FAT32 cards are supported in MBR, GPT and superfloppy layouts. GPT header
and partition-table CRCs and partition bounds are checked before mounting.
File access maps only the selected FAT partition; the protective MBR and GPT
metadata are preserved. Microsoft Basic Data partitions take precedence over
other FAT partitions. Invalid GPT metadata is rejected, without repair.
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
play MP3VBR.MP3
play MUSIC/ALBUM/SONG.MP3
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

The host suite requires `mkfs.fat`, FFmpeg with libmp3lame, `lame`, and `sgdisk`.
It creates temporary FAT and GPT disk images and generated audio fixtures.
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

## Confirmed audio and file deletion (2026-10-03)

The user confirmed that idle is quiet, KORVOWAV.WAV and KORVOMP3.MP3 play
cleanly, and a new microphone recording replays cleanly with firmware `fe865fc`.
This supersedes the pending sound-quality checks above.

AUDIO offers DELETE SELECTED for its current MP3/WAV selection. SD CARD has a
live root-file browser with PREVIOUS, NEXT, RESCAN SD and DELETE SELECTED; it
includes files of any extension. Each list shows up to 64 files independently,
using FAT 8.3 aliases. Folders and volume labels are excluded.

Deletion requires a separate CONFIRM tap beside the exact filename. CANCEL,
selection changes, other media commands, or changing tabs dismiss the request.
The existing release filter prevents one held touch from both requesting and
confirming deletion. STOP playback or save a recording before deleting; a busy
request is rejected without interrupting audio. Successful deletion refreshes
both lists and clears REPLAY LAST if that saved recording was removed.

Deletion is permanent. The local FAT patch frees file clusters, removes
associated long-name directory slots, and updates free-space metadata. See
[the patch notes](../vendor/embedded-sdmmc/KORVO-PATCH.md). Deletion, like recording,
is not protected against power loss or card removal during a write.

Console equivalents: `delete` (AUDIO selection), `delete-file` (SD selection),
`confirm-delete`, `cancel-delete`, `file-next`, `file-prev`. Confirmation is
always required; sending `confirm-delete` alone does nothing.

The media host suite now has 12 passing tests, including FAT16/FAT32 deletion,
empty/single/multiple cluster files, sector-spanning long names, open-file
rejection, preserved neighboring files and boot sectors, and clean read-only
filesystem checks. Physical UI confirmation is pending.

Firmware `8907271` was flashed to `/dev/ttyUSB0`. On-board console checks
created and removed two disposable `REC00047.WAV` recordings (the unused
highest number was reused after deletion), once through each selection path.
Both paths showed the exact filename, honored CANCEL, ignored a subsequent
confirmation without a pending request, deleted only the newly created test
file, refreshed the browser and invalidated REPLAY LAST. Existing recordings
and demo files were preserved. LCD underruns and microphone overruns stayed
zero throughout. Touch-screen confirmation in both tabs remains pending.

Camera video and microphone audio can also be recorded together in numbered
AVI files. See [camera video recording](15-camera-video-recording.md).

## Expanded MP3 playback (2026-10-03)

The 42-test host suite covers 36 combinations of nine rates, mono/stereo and CBR/VBR,
with exact trimmed duration and 48 kHz output length. A 400 kbps free-format
file, large/repeated ID3 tags, APEv2/ID3v1 tails, a frame crossing a 16 KiB junk
boundary and truncated input also pass. Independent stereo fixtures agree with
FFmpeg within the tested -60 dB error bound on both channels.

Filter tests check duration for very short streams, bit-exact 48 kHz bypass,
channel separation, passband amplitude, and at least 60 dB rejection of selected
upsampling images and a 96-to-48 kHz alias. FAT tests catalog 71 files through
eight folder levels with only the default four directory handles available;
busy deletion is rejected and identical root/nested aliases remain distinct.

`demo` additionally installs `MP3HIGH.MP3` (48 kHz, 320 kbps stereo),
`MP3VBR.MP3` (44.1 kHz VBR stereo), `MP3LOW.MP3` (8 kHz mono MPEG-2.5),
and `MP3STRS.MP3` (eight seconds of 44.1 kHz, 320 kbps independent multitone stereo).
These synthetic fixtures include length/delay metadata. Existing files with
those names are preserved. UART `play <alias/path>` selects a file directly.

The currently inserted 32 GB card uses GPT/FAT32 and now mounts at sector 2048
without repartitioning or formatting. A host test also writes and reads a WAV
on a GPT/FAT image and verifies that its protective MBR and GPT metadata remain
unchanged; corrupted header and table CRCs are rejected.

Decoder resets happen in place to avoid a 22 KiB temporary on the firmware
task stack. DMA free-space queries no longer advance the queued PCM tail:
playback can drain its last sample and turn the amplifier off. DMA write
recovery still keeps new samples away from the active hardware descriptor.
The app selects the supported 320 MHz CPU clock: the default 160 MHz setting
could not sustain stereo sinc conversion alongside the UI and SD reads.
The filter uses the hardware FPU with separate stereo histories; full-range
random input agrees with a double-precision reference within one PCM step.
Exact rational-rate filter phases are prepared once in PSRAM (at most 160 KiB
for the nine MP3 rates). Unusual WAV rates retain bounded on-demand interpolation.
The hot filter executes in internal RAM on core one, using fused multiply-add
instructions and four independent accumulators per channel. Core zero decodes
one frame ahead while the worker converts the preceding frame. A bounded
mailbox reuses PSRAM sample/output buffers; generation IDs discard abandoned
results after STOP or a file switch. Host tests check actual cross-thread buffer
ownership, rate/history reset and deferred framebuffer painting. Framebuffer
writes and conversion serialize to avoid parallel PSRAM bursts. MP3 reads refill 16 KiB only
when lookahead falls below 8 KiB, amortizing SD/FAT transactions. Dynamic widgets repaint every 250 ms during audio playback,
and every 50 ms otherwise. Input is polled each loop; painting waits briefly
when the second-core filter is busy.
Two bounded audio blocks are fed between UI polls, and WAV reads contain up
to 1,024 input frames. The root setup script also applies the S31 RTOS task
affinity and floating-point context fixes described in
[radio support](16-radio-support.md); correct FP preservation is required by
both the decoder and the filter.

These host checks establish decoded samples and filter behavior. Sound quality
on the physical speaker still requires listening to the flashed build.


## Expanded playback board validation (2026-10-03)

The final USB-host + Wi-Fi/BLE release firmware is
`target/lcd-debug/korvo-mp3-worker.elf`. Both debug and release host suites pass
all 42 tests; the legacy `--no-default-features --features usb-device` release
also builds. The default dependency setup reproduces the USB, radio affinity
and complete floating-point context patches, and is idempotent.

A 102-second on-board run is captured in
`target/lcd-debug/mp3-worker-final-validation.log`:

- The 32 GB GPT/FAT32 card mounts without changing its partition layout.
  Wi-Fi obtains DHCP; BLE advertising and the existing two USB hubs with all
  four HID interfaces stay active. Physical HID input confirmation is pending.
- The eight-second 44.1 kHz / 320 kbps independent stereo fixture plays twice
  in about 8.30 seconds, including the final DMA drain. Each run decodes 308
  frames, skips none, and applies encoder-delay/end-padding trimming.
  The first run spends 472 ms reading SD, 2,829 ms decoding on core zero,
  and 2,549 ms filtering on core one; decoding and conversion overlap.
- 48 kHz / 320 kbps stereo, 44.1 kHz VBR, 8 kHz MPEG-2.5 mono, the original
  untagged MP3, and 16 kHz mono WAV all complete at their expected durations.
- A new `REC00009.WAV` saves 565,248 bytes (2.944 seconds) of stereo PCM and
  replays to completion. Existing recordings and media are preserved.
- TX underruns remain zero throughout ordinary playback, recording and replay.
  Deliberately aborting and replacing several files within 120 ms produces
  two queue-recovery events; subsequent sustained playback completes on time.
  LCD underruns stay zero throughout. Two microphone overruns occur during
  boot/radio initialization, with no additional overruns during media testing.
  No crash or stack overflow occurs; the LED stays off and default volume
  remains -30 dB.

An earlier 36-second continuous stress run also records zero TX and LCD
underruns in `target/lcd-debug/mp3-worker-stress-validation.log`. Host sample
comparisons and hardware timing establish decoder/filter correctness and
real-time delivery. Listening to the updated build with the user's own MP3
files remains a separate physical sound-quality check.

A separate 32-second camera regression run records and plays `VID00001.AVI`:
20 MJPEG frames and 738,304 PCM bytes, with all 20 frames decoded, none skipped,
and no camera errors, TX underruns or LCD underruns. Its log is
`target/lcd-debug/mp3-worker-camera-validation.log`. The first `video` command
opens CAMERA; the next starts recording, and `video-replay` saves and replays it.
