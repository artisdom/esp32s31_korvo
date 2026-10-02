# Generated audio test tones

These synthetic tones contain no third-party recorded audio. Generate them with:

```
ffmpeg -f lavfi -i 'sine=frequency=660:sample_rate=16000:duration=1' -ac 1 -c:a pcm_s16le -bitexact demo.wav
ffmpeg -f lavfi -i 'sine=frequency=440:sample_rate=44100:duration=2' -ac 2 -c:a libmp3lame -b:a 64k -map_metadata -1 -id3v2_version 0 -write_xing 0 demo.mp3
```

WAV exercises mono and 16-to-48 kHz conversion. MP3 exercises stereo decoding
and 44.1-to-48 kHz conversion. Firmware `demo` installs them as new FAT files
without overwriting existing files; the host suite decodes the MP3 fixture.
