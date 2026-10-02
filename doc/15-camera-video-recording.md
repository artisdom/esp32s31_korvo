# Camera video with microphone audio

The CAMERA page captures the fitted **SC101IOT** and records video and both
microphones together into numbered `VID00001.AVI`, `VID00002.AVI`, ... files
in the SD root. Existing files are preserved. This is Rust sensor control,
DVP DMA, JPEG compression and AVI muxing; no ESP-IDF camera or media component
is linked into the application.

## Controls and formats

Open CAMERA and wait for the preview and “Camera ready”. Tap RECORD / SAVE,
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
confirmation flow after recording stops. AVI playback is on a computer, such
as VLC or ffplay; the on-board AUDIO browser still plays MP3 and PCM WAV.
Recordings automatically save at a 1 GiB media-data limit. Stop and wait for
“Video + microphone saved” before disconnecting power or removing the card;
unfinished files are not recovered automatically after interruption.

UART0 command `video` toggles video recording and opens CAMERA. At boot, send
it once to open the preview, wait for frames, then send it again to record.
`stop` saves. A missing card, unsupported sensor, failed encoder or SD write
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

Core 0 services I2S, SD, inputs and LCD. Core 1 compresses JPEG through a bounded
atomic mailbox and also services USB CDC. Record generations prevent a JPEG
from a stopped session being used in a later recording. Core 1 requires a
64 KiB stack; the original 8 KiB USB-only stack was insufficient. Its ROM PMA
layout allowed PSRAM reads but rejected writes, so the bootloader's working
core 0 PMA layout is mirrored before starting the JPEG executor. Large camera
and JPEG buffers explicitly use the PSRAM heap. JPEG quality is 45.

## Validation, 2026-10-03

The host media suite has 14 passing tests, including an AVI decoded by
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
video playback on the LCD, AVI indexing and interrupted-file recovery remain
future work.
