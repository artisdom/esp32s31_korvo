//! Single-slot JPEG mailbox: the second core compresses while core zero services I2S.
extern crate alloc;
use alloc::{boxed::Box, vec::Vec};
use core::{
    cell::UnsafeCell,
    sync::atomic::{AtomicU8, Ordering},
};

struct Job {
    image: Box<[u8]>,
    scratch: Box<[u8]>,
    output: Vec<u8>,
    generation: u32,
    result: Result<(), &'static str>,
}
pub struct Worker {
    state: AtomicU8,
    job: UnsafeCell<Job>,
}
// Atomic states provide exclusive ownership: 0 idle, 1 copying, 2 queued,
// 3 encoding, 4 ready, 5 consuming. Release/acquire publishes buffer changes.
unsafe impl Sync for Worker {}
impl Worker {
    pub fn new() -> Self {
        Self {
            state: AtomicU8::new(0),
            job: UnsafeCell::new(Job {
                image: crate::psram_buffer::zeroed(crate::avi::FRAME_BYTES),
                scratch: crate::psram_buffer::zeroed(crate::avi::WIDTH * crate::avi::HEIGHT * 3),
                output: output_buffer(),
                generation: 0,
                result: Ok(()),
            }),
        }
    }
    pub fn submit(&self, image: &[u8], generation: u32) -> bool {
        if image.len() != crate::avi::FRAME_BYTES
            || self
                .state
                .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
                .is_err()
        {
            return false;
        }
        let job = unsafe { &mut *self.job.get() };
        job.image.copy_from_slice(image);
        job.generation = generation;
        self.state.store(2, Ordering::Release);
        true
    }
    pub fn take(&self) -> Option<(u32, Result<Vec<u8>, &'static str>)> {
        if self
            .state
            .compare_exchange(4, 5, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            return None;
        }
        let job = unsafe { &mut *self.job.get() };
        let result = job
            .result
            .map(|_| core::mem::replace(&mut job.output, output_buffer()));
        let generation = job.generation;
        self.state.store(0, Ordering::Release);
        Some((generation, result))
    }
    pub fn encode(&self) {
        if self
            .state
            .compare_exchange(2, 3, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            return;
        }
        let job = unsafe { &mut *self.job.get() };
        job.result = crate::avi::encode_jpeg(&job.image, &mut job.scratch, &mut job.output);
        self.state.store(4, Ordering::Release);
    }
}

fn output_buffer() -> Vec<u8> {
    let mut output = crate::psram_buffer::zeroed(crate::avi::FRAME_BYTES).into_vec();
    output.clear();
    output
}
