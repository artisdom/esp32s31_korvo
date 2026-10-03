# Generated audio test tones

These synthetic tones contain no third-party recorded audio. Generate them with:

```
ffmpeg -f lavfi -i 'sine=frequency=660:sample_rate=16000:duration=1' -ac 1 -c:a pcm_s16le -bitexact demo.wav
ffmpeg -f lavfi -i 'sine=frequency=440:sample_rate=44100:duration=2' -ac 2 -c:a libmp3lame -b:a 64k -map_metadata -1 -id3v2_version 0 -write_xing 0 demo.mp3
```

WAV exercises mono and 16-to-48 kHz conversion. MP3 exercises stereo decoding
and 44.1-to-48 kHz conversion. Firmware `demo` installs them as new FAT files
without overwriting existing files; the host suite decodes the MP3 fixture.

Additional synthetic fixtures (quiet, independent stereo channels):

```sh
ffmpeg -f lavfi -i 'aevalsrc=0.08*sin(2*PI*440*t)|0.08*sin(2*PI*880*t):s=48000:d=1.25' -c:a libmp3lame -b:a 320k -id3v2_version 4 -metadata title='Korvo 320 kbps stereo test' high320.mp3
ffmpeg -f lavfi -i 'aevalsrc=0.08*sin(2*PI*660*t)|0.08*sin(2*PI*1320*t):s=44100:d=1.25' -c:a libmp3lame -q:a 2 -id3v2_version 3 -metadata title='Korvo VBR stereo test' vbr.mp3
ffmpeg -f lavfi -i 'sine=frequency=550:sample_rate=8000:duration=1.25' -ac 1 -c:a libmp3lame -b:a 24k -id3v2_version 4 -metadata title='Korvo MPEG 2.5 mono test' low8k.mp3
```

Firmware `demo` installs them as `MP3HIGH.MP3`, `MP3VBR.MP3` and `MP3LOW.MP3`
without replacing existing files. The host suite also generates a 36-format
matrix with FFmpeg and a free-format file with `lame`; temporary files are
removed after successful tests. `scripts/generate-resampler.py` reproduces the
constant filter coefficient table using only Python's standard library.

`high441.mp3` adds eight seconds of 320 kbps stereo at 44.1 kHz to exercise
decoding, SD reads and filtered conversion continuously. Its two channels use
different sets of ten simultaneous quiet tones; it contains no recorded music.
Firmware installs it as `MP3STRS.MP3`. Regenerate from the project root with:

```python
import subprocess
left = [223, 337, 509, 761, 1151, 1727, 2593, 3889, 5839, 8753]
right = [277, 419, 631, 947, 1423, 2137, 3203, 4801, 7207, 10811]
channels = ['0.006*(' + '+'.join(f'sin(2*PI*{f}*t)' for f in frequencies) + ')'
            for frequencies in (left, right)]
source = 'aevalsrc=' + '|'.join(channels) + ':s=44100:d=8'
subprocess.run(['ffmpeg', '-v', 'error', '-f', 'lavfi', '-i', source,
                '-c:a', 'libmp3lame', '-b:a', '320k', '-id3v2_version', '4',
                '-metadata', 'title=Korvo 44.1 kHz 320 kbps stereo multitone stress test',
                'tests/fixtures/high441.mp3'], check=True)
```
