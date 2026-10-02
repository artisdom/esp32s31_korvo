//! Bounded second-core JPEG decoding mailbox, independent of the encoder slot.
extern crate alloc;
use alloc::{boxed::Box, vec::Vec};
use core::{
    cell::UnsafeCell,
    sync::atomic::{AtomicU8, Ordering},
};
use zune_jpeg::{
    JpegDecoder,
    zune_core::{bytestream::ZCursor, colorspace::ColorSpace, options::DecoderOptions},
};

pub fn decode(jpeg: &[u8], rgb: &mut [u8], output: &mut [u8]) -> Result<(), &'static str> {
    let options = DecoderOptions::default()
        .set_max_width(crate::avi::WIDTH)
        .set_max_height(crate::avi::HEIGHT)
        .jpeg_set_out_colorspace(ColorSpace::RGB);
    let mut decoder = JpegDecoder::new_with_options(ZCursor::new(jpeg), options);
    decoder
        .decode_headers()
        .map_err(|_| "JPEG header invalid")?;
    if decoder.dimensions() != Some((crate::avi::WIDTH, crate::avi::HEIGHT))
        || rgb.len() != crate::avi::WIDTH * crate::avi::HEIGHT * 3
        || output.len() != crate::avi::FRAME_BYTES
    {
        return Err("JPEG dimensions unsupported");
    }
    decoder.decode_into(rgb).map_err(|_| "JPEG decode failed")?;
    for (pixel, dest) in rgb.chunks_exact(3).zip(output.chunks_exact_mut(2)) {
        let value =
            ((pixel[0] as u16 >> 3) << 11) | ((pixel[1] as u16 >> 2) << 5) | (pixel[2] as u16 >> 3);
        dest.copy_from_slice(&value.to_le_bytes());
    }
    Ok(())
}
struct Job {
    input: Box<[u8]>,
    len: usize,
    rgb: Box<[u8]>,
    output: Vec<u8>,
    generation: u32,
    index: u32,
    result: Result<(), &'static str>,
}
pub struct Worker {
    state: AtomicU8,
    job: UnsafeCell<Job>,
}
// Exclusive states are the same release/acquire ownership protocol as jpeg_worker:
// idle0, writing1, queued2, decoding3, ready4, consuming5.
unsafe impl Sync for Worker {}
impl Worker {
    pub fn new() -> Self {
        Self {
            state: AtomicU8::new(0),
            job: UnsafeCell::new(Job {
                input: crate::psram_buffer::zeroed(crate::avi::FRAME_BYTES),
                len: 0,
                rgb: crate::psram_buffer::zeroed(crate::avi::WIDTH * crate::avi::HEIGHT * 3),
                output: crate::psram_buffer::zeroed(crate::avi::FRAME_BYTES).into_vec(),
                generation: 0,
                index: 0,
                result: Ok(()),
            }),
        }
    }
    pub fn idle(&self) -> bool {
        self.state.load(Ordering::Acquire) == 0
    }
    pub fn submit(&self, input: &[u8], generation: u32, index: u32) -> bool {
        if input.is_empty()
            || input.len() > crate::avi::FRAME_BYTES
            || self
                .state
                .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
                .is_err()
        {
            return false;
        }
        let job = unsafe { &mut *self.job.get() };
        job.input[..input.len()].copy_from_slice(input);
        job.len = input.len();
        job.generation = generation;
        job.index = index;
        self.state.store(2, Ordering::Release);
        true
    }
    pub fn take(&self) -> Option<(u32, u32, Result<Vec<u8>, &'static str>)> {
        if self
            .state
            .compare_exchange(4, 5, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            return None;
        }
        let job = unsafe { &mut *self.job.get() };
        let result = job.result.map(|_| {
            core::mem::replace(
                &mut job.output,
                crate::psram_buffer::zeroed(crate::avi::FRAME_BYTES).into_vec(),
            )
        });
        let ids = (job.generation, job.index, result);
        self.state.store(0, Ordering::Release);
        Some(ids)
    }
    /// Exclude cache-heavy LCD writes while decoding on the other core. The
    /// saved state keeps a queued/ready job intact; a busy decoder defers paint.
    pub fn paint(&self, paint: impl FnOnce()) -> bool {
        let state = self.state.load(Ordering::Acquire);
        if !matches!(state, 0 | 2 | 4)
            || self
                .state
                .compare_exchange(state, 6, Ordering::Acquire, Ordering::Relaxed)
                .is_err()
        {
            return false;
        }
        struct Restore<'a>(&'a AtomicU8, u8);
        impl Drop for Restore<'_> {
            fn drop(&mut self) {
                self.0.store(self.1, Ordering::Release);
            }
        }
        let _restore = Restore(&self.state, state);
        paint();
        true
    }
    pub fn run(&self) {
        if self
            .state
            .compare_exchange(2, 3, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            return;
        }
        let job = unsafe { &mut *self.job.get() };
        job.result = decode(&job.input[..job.len], &mut job.rgb, &mut job.output);
        self.state.store(4, Ordering::Release);
    }
}
