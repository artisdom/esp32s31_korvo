//! AVI 1.0 muxing: MJPEG 320x240/5fps plus PCM16 stereo/48k.
//! Fixed headers and word-aligned media chunks; no codec or C dependency.
pub const WIDTH: usize = 320;
pub const HEIGHT: usize = 240;
pub const FPS: u32 = 5;
pub const FRAME_BYTES: usize = WIDTH * HEIGHT * 2;
pub const AUDIO_RATE: u32 = 192000;
pub const HEADER_BYTES: usize = 324;
fn u32le(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}
fn u16le(b: &mut [u8], at: usize, v: u16) {
    b[at..at + 2].copy_from_slice(&v.to_le_bytes());
}
fn tag(b: &mut [u8], at: usize, s: &[u8; 4]) {
    b[at..at + 4].copy_from_slice(s);
}
pub fn header(
    file_bytes: u32,
    frames: u32,
    audio_bytes: u32,
    movi_bytes: u32,
) -> [u8; HEADER_BYTES] {
    let mut b = [0u8; HEADER_BYTES];
    tag(&mut b, 0, b"RIFF");
    u32le(&mut b, 4, file_bytes.saturating_sub(8));
    tag(&mut b, 8, b"AVI ");
    tag(&mut b, 12, b"LIST");
    u32le(&mut b, 16, 292);
    tag(&mut b, 20, b"hdrl");
    tag(&mut b, 24, b"avih");
    u32le(&mut b, 28, 56);
    u32le(&mut b, 32, 1000000 / FPS);
    u32le(&mut b, 36, FRAME_BYTES as u32 * FPS + AUDIO_RATE);
    u32le(&mut b, 44, 0x100);
    u32le(&mut b, 48, frames);
    u32le(&mut b, 56, 2);
    u32le(&mut b, 60, FRAME_BYTES as u32);
    u32le(&mut b, 64, WIDTH as u32);
    u32le(&mut b, 68, HEIGHT as u32);
    tag(&mut b, 88, b"LIST");
    u32le(&mut b, 92, 116);
    tag(&mut b, 96, b"strl");
    tag(&mut b, 100, b"strh");
    u32le(&mut b, 104, 56);
    tag(&mut b, 108, b"vids");
    tag(&mut b, 112, b"MJPG");
    u32le(&mut b, 128, 1);
    u32le(&mut b, 132, FPS);
    u32le(&mut b, 140, frames);
    u32le(&mut b, 144, FRAME_BYTES as u32);
    u32le(&mut b, 148, u32::MAX);
    u16le(&mut b, 160, WIDTH as u16);
    u16le(&mut b, 162, HEIGHT as u16);
    tag(&mut b, 164, b"strf");
    u32le(&mut b, 168, 40);
    u32le(&mut b, 172, 40);
    u32le(&mut b, 176, WIDTH as u32);
    u32le(&mut b, 180, HEIGHT as u32);
    u16le(&mut b, 184, 1);
    u16le(&mut b, 186, 24);
    tag(&mut b, 188, b"MJPG");
    u32le(&mut b, 192, FRAME_BYTES as u32);
    tag(&mut b, 212, b"LIST");
    u32le(&mut b, 216, 92);
    tag(&mut b, 220, b"strl");
    tag(&mut b, 224, b"strh");
    u32le(&mut b, 228, 56);
    tag(&mut b, 232, b"auds");
    u32le(&mut b, 252, 4);
    u32le(&mut b, 256, AUDIO_RATE);
    u32le(&mut b, 264, audio_bytes / 4);
    u32le(&mut b, 268, 8192);
    u32le(&mut b, 272, u32::MAX);
    u32le(&mut b, 276, 4);
    tag(&mut b, 288, b"strf");
    u32le(&mut b, 292, 16);
    u16le(&mut b, 296, 1);
    u16le(&mut b, 298, 2);
    u32le(&mut b, 300, 48000);
    u32le(&mut b, 304, AUDIO_RATE);
    u16le(&mut b, 308, 4);
    u16le(&mut b, 310, 16);
    tag(&mut b, 312, b"LIST");
    u32le(&mut b, 316, 4 + movi_bytes);
    tag(&mut b, 320, b"movi");
    b
}
pub fn chunk_header(video: bool, bytes: u32) -> [u8; 8] {
    let mut h = [0; 8];
    tag(&mut h, 0, if video { b"00dc" } else { b"01wb" });
    u32le(&mut h, 4, bytes);
    h
}
pub fn rgb565(uyvy: &[u8], x: usize, y: usize) -> u16 {
    let at = (y * WIDTH + (x & !1)) * 2;
    let u = uyvy[at] as i32 - 128;
    let v = uyvy[at + 2] as i32 - 128;
    let luma = uyvy[at + if x & 1 == 0 { 1 } else { 3 }] as i32 - 16;
    let r = ((298 * luma + 409 * v + 128) >> 8).clamp(0, 255) as u16;
    let g = ((298 * luma - 100 * u - 208 * v + 128) >> 8).clamp(0, 255) as u16;
    let b = ((298 * luma + 516 * u + 128) >> 8).clamp(0, 255) as u16;
    ((r >> 3) << 11) | ((g >> 2) << 5) | (b >> 3)
}

extern crate alloc;
/// Convert limited-range camera YUV to full-range JPEG YCbCr and encode in Rust.
pub fn encode_jpeg(
    uyvy: &[u8],
    scratch: &mut [u8],
    output: &mut alloc::vec::Vec<u8>,
) -> Result<(), &'static str> {
    if uyvy.len() != FRAME_BYTES || scratch.len() != WIDTH * HEIGHT * 3 {
        return Err("JPEG input size");
    }
    for (src, dst) in uyvy.chunks_exact(4).zip(scratch.chunks_exact_mut(6)) {
        let u = (((src[0] as i32 - 128) * 255 / 224) + 128).clamp(0, 255) as u8;
        let v = (((src[2] as i32 - 128) * 255 / 224) + 128).clamp(0, 255) as u8;
        let y0 = ((src[1] as i32 - 16) * 255 / 219).clamp(0, 255) as u8;
        let y1 = ((src[3] as i32 - 16) * 255 / 219).clamp(0, 255) as u8;
        dst.copy_from_slice(&[y0, u, v, y1, u, v]);
    }
    output.clear();
    jpeg_encoder::Encoder::new(output, 45)
        .encode(
            scratch,
            WIDTH as u16,
            HEIGHT as u16,
            jpeg_encoder::ColorType::Ycbcr,
        )
        .map_err(|_| "JPEG encoding failed")
}
