# Camera video with microphone audio

The CAMERA page captures the fitted **SC101IOT** and records video and both
microphones together into numbered `VID00001.AVI`, `VID00002.AVI`, ... files
in the SD root. Existing files are preserved. This is Rust sensor control,
DVP DMA, JPEG compression and AVI muxing; no ESP-IDF camera or media component
is linked into the application.

## Controls and formats

Open CAMERA and wait for the preview and “Camera ready”. Tap REC / SAVE,
or press SET on CAMERA, to start. Tap it again or STOP / SAVE to finalize the
AVI header and close the file. Recording continues when changing tabs. STOP
on AUDIO also saves an active video recording. The speaker amplifier stays
off while recording; the LED remains disabled.

The file contains 320x240 MJPEG at a 5 fps container rate, plus 48 kHz,
16-bit stereo PCM microphone audio. Camera capture and JPEG compression can
be slower than 5 fps: the latest completed image is repeated to keep video
aligned with the audio sample timeline. Video duration rounds up to the next
200 ms interval. The audio stream retains its actual sample count.

Files appear in SD CARD and can be removed through its existing filename
confirmation flow after recording or playback stops. CAMERA has PREVIOUS/NEXT
for its independent AVI list, PLAY for the selected recording and REPLAY LAST
for the recording saved this session (or the last AVI in the directory after
boot). STOP ends playback. SD CARD's PLAY FILE opens CAMERA for AVI or AUDIO
for WAV/MP3. The AUDIO browser continues to list MP3 and PCM WAV.

On-board playback supports **this application's finalized 320x240/5 fps
MJPEG + PCM16 stereo/48 kHz AVI format**. External AVI layouts, other video
codecs/resolutions, MP4 and unfinished recordings are rejected. Recordings can
also be played on a computer with VLC or ffplay.
Recordings automatically save at a 1 GiB media-data limit. Stop and wait for
“Video saved - REPLAY LAST to watch” before disconnecting power or removing the card;
unfinished files are not recovered automatically after interruption.

UART0 command `video` toggles video recording and opens CAMERA. At boot, send
it once to open the preview, wait for frames, then send it again to record.
`stop` saves. `video-play` plays the selected AVI, `video-replay` replays the
last recording, `video-next` / `video-prev` select files, and `file-play` opens
the selected SD CARD file. A missing card, unsupported sensor, failed encoder or SD write
is reported rather than treated as a successful save.

## Sensor and DMA implementation

The Apache-2.0 SC101IOT initialization table is transcribed from the local
Espressif Korvo factory example. GPIO55 supplies 20 MHz XCLK; GPIO46..53 carry
the eight data bits, GPIO54 pixel clock, GPIO56 inverted VSYNC, GPIO57 HENABLE.
The vendor VGA mode is configured, then its documented window registers on
page 0x01 are adjusted to a centered 320x240 crop of the native 1280x720 array
(H start 480/end 800; V start 240/end 480). This favors low DMA traffic over field
of view; it is a crop, not a full-sensor scaled image.

LCD TX and camera RX share DMA_AXI_CH0's separate halves. Camera descriptors
and PSRAM storage are aligned to 64-byte boundaries and use 64-byte external
bursts. A finite capture spans multiple sensor frames. The first frame may
start mid-image and is discarded. Only subsequent VSYNC-delimited frames
with exactly 153600 bytes are displayed or encoded; incomplete frames are
retried, with a visible retry counter. A later complete frame in the same
capture can recover an earlier partial frame. Camera RX is paused during a
full LCD page repaint and resumed afterward. The camera STOP_EN setting
follows the vendor's continuous-capture policy, rather than aborting whenever
the RX FIFO briefly fills.

Core 0 services I2S, SD, inputs and LCD. Core 1 compresses and decodes JPEG through separate bounded
atomic mailboxes and also services USB CDC. Record generations prevent a JPEG
from a stopped session being used in a later recording. Core 1 requires a
64 KiB stack; the original 8 KiB USB-only stack was insufficient. Its ROM PMA
layout allowed PSRAM reads but rejected writes, so the bootloader's working
core 0 PMA layout is mirrored before starting the JPEG executor. Large camera
and JPEG buffers explicitly use the PSRAM heap. JPEG quality is 45.

## Validation, 2026-10-03

The host media suite has 18 passing tests, including an AVI decoded by
FFmpeg/ffprobe, odd-length JPEG chunk padding, matching one-second audio/video
streams, and cross-thread JPEG mailbox ownership and generation checks.

An actual on-board SD recording, `VID00007.AVI`, was downloaded using a
**temporary diagnostic removed from the final firmware**. FFprobe found
16 MJPEG frames at 320x240 and 145152 stereo PCM samples at 48000 Hz, in a
614096-byte AVI. FFmpeg decoded both streams without errors; the extracted
image showed the camera's view of the ceiling. Its video duration was 3.2 s,
with 3.024 s of microphone samples, as expected from 200 ms frame rounding.
Microphone overruns stayed 0 during recording. Earlier raw-video prototypes
lost audio samples, and were superseded by second-core JPEG encoding.

The cleaned-up build saved `VID00008.AVI` during a 10-second recording with
51 frames, 1925120 audio bytes (10.027 s) and 2029094 total bytes. LCD
underruns, microphone overruns and camera retries all stayed 0; camera capture
advanced through 41 complete frames during the test. Two idle TX underruns
were reported before recording; RX audio remained continuous.

Physical verification of preview appearance, motion, intelligible speech
and touch/button controls remains pending. Prototype recordings already on
the SD card may be partial or faulty; use a new recording to evaluate this
implementation. Higher resolutions, full-field scaling, hardware JPEG,
AVI indexing and interrupted-file recovery remain
future work.

## On-board playback implementation

AVI parsing bounds every chunk against the declared file/movi lengths, checks
word padding and PCM frame alignment, and rejects oversized JPEGs. SD chunks
are streamed through an 8 KiB PCM staging buffer and bounded JPEG input;
the whole recording is never loaded into RAM. Pure Rust `zune-jpeg` decodes
on core 1 into RGB565 in PSRAM. A generation and frame index accompany every
job, so STOP/restart cannot reuse an earlier session's image.

Three decoded frames and three compressed frames can be prefetched. JPEG
jobs do not hold the SD reader at a video chunk: it continues to the following
PCM. If the decoder falls behind, the oldest compressed pending image is
replaced. Playback preserves microphone audio and advances video by its source
frame timestamp. Completed/skipped decode counts are printed at STOP. The number of PCM bytes queued minus
bytes remaining in I2S DMA determines the playback time and which frame is due.
Playback pauses live DVP capture; changing tabs keeps the file playing and
returning to CAMERA draws the current decoded frame. LCD painting defers while
the decoder or encoder owns the output guard, excluding simultaneous
cache-heavy writes on both cores. Private allocations made by Rust JPEG
libraries on core 1 use PSRAM through the application's allocator; media
staging/sample/MP3-decoder buffers also use PSRAM. Radio controller allocations
retain internal RAM through the dependency patch described in radio docs. Pending frame changes survive that deferral.

Microphone draining was increased from 8 KiB to 32 KiB per loop, with the
scratch buffer in PSRAM, and silent/tone TX fills all available 8 KiB blocks.
This prevents a camera/radio-loaded loop from falling behind the 192000-byte/s
capture stream. Combined Wi-Fi/BLE camera tests saved `VID00011.AVI` with
166 frames and 6336512 audio bytes over about 33 seconds, and `VID00012.AVI`
with 165 frames and 6335488 audio bytes. No additional microphone overruns
occurred during those recordings; the combined build had one startup overrun.

The first playback hardware build completed `VID00012.AVI` (all 165 frames),
then passed STOP/restart and a second complete replay. Audio RX overruns stayed
zero. That build accumulated LCD underruns during decoding, motivating the
paint/decode exclusion added above; the revised build replayed all 165 frames with LCD underruns and RX overruns
both staying zero. Its one TX underrun occurred before playback.
Physical confirmation of motion, intelligible speech and A/V alignment remains
pending. Automated console tests do not establish those perceptual checks.

The final combined Wi-Fi/BLE build replayed `VID00012.AVI` with **165 decodes,
zero skipped images and zero LCD underruns**, while a BLE client read uptime
and received five notifications and TCP echo handled ten payloads. TX underruns
stayed at the two-startup-event baseline throughout playback; one additional
TX event was counted at completion. RX overruns stayed at the one-startup-event
baseline. Internal heap remained about 21–24 KiB free during this test.

A fresh record/save/replay cycle produced `VID00013.AVI`: 55 frames, 2105344
PCM bytes (10.965 s) and 2221050 total bytes. All 55 frames decoded without
skips. LCD underruns stayed zero; no additional microphone RX overruns or TX
underruns occurred while recording and replaying. These hardware checks use
the default CPU clock, not the experimental higher-clock build.

The host suite now streams an actual FAT-backed AVI through the production
player and a simulated 64 KiB DMA queue with 300 ms JPEG completion latency,
slower than the 5 fps file. It checks that PCM is delivered unchanged and
without gaps, older video jobs are bounded/dropped and the final frame arrives.
