//! Single-flight PCM conversion on core one, with reusable PSRAM buffers.
extern crate alloc;
use alloc::boxed::Box;
use core::{
    cell::UnsafeCell,
    sync::atomic::{AtomicU8, Ordering},
};

struct Job {
    samples: Box<[i16]>,
    output: Box<[u8]>,
    len: usize,
    rate: u32,
    channels: usize,
    generation: u32,
    previous: Option<u32>,
    finish: bool,
    output_len: usize,
    elapsed_us: u64,
    resampler: crate::pcm::Resampler,
}

pub struct Completed {
    pub generation: u32,
    pub len: usize,
    pub finish: bool,
    pub elapsed_us: u64,
}
pub struct Worker {
    state: AtomicU8,
    job: UnsafeCell<Job>,
}
// Exclusive states: idle 0, copying 1, queued 2, processing 3, ready 4,
// consuming 5, painting 6. Release/acquire transfers all buffer ownership.
unsafe impl Sync for Worker {}
impl Worker {
    pub fn new() -> Self {
        Self {
            state: AtomicU8::new(0),
            job: UnsafeCell::new(Job {
                samples: crate::psram_buffer::samples(nanomp3_core::MAX_SAMPLES_PER_FRAME),
                output: crate::psram_buffer::zeroed(32768),
                len: 0,
                rate: 0,
                channels: 2,
                generation: 0,
                previous: None,
                finish: false,
                output_len: 0,
                elapsed_us: 0,
                resampler: crate::pcm::Resampler::default(),
            }),
        }
    }
    pub fn submit(
        &self,
        samples: &[i16],
        rate: u32,
        channels: usize,
        generation: u32,
        finish: bool,
    ) -> bool {
        if samples.len() > nanomp3_core::MAX_SAMPLES_PER_FRAME
            || !matches!(channels, 1 | 2)
            || samples.len() % channels != 0
            || (!finish && (samples.is_empty() || !(8000..=48000).contains(&rate)))
            || self
                .state
                .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
                .is_err()
        {
            return false;
        }
        let job = unsafe { &mut *self.job.get() };
        job.samples[..samples.len()].copy_from_slice(samples);
        job.len = samples.len();
        job.rate = rate;
        job.channels = channels;
        job.generation = generation;
        job.finish = finish;
        self.state.store(2, Ordering::Release);
        true
    }
    pub fn take(&self, output: &mut Box<[u8]>) -> Option<Completed> {
        if self
            .state
            .compare_exchange(4, 5, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            return None;
        }
        let job = unsafe { &mut *self.job.get() };
        assert_eq!(output.len(), job.output.len());
        core::mem::swap(output, &mut job.output);
        let completed = Completed {
            generation: job.generation,
            len: job.output_len,
            finish: job.finish,
            elapsed_us: job.elapsed_us,
        };
        self.state.store(0, Ordering::Release);
        Some(completed)
    }
    pub fn run(&self) {
        if self
            .state
            .compare_exchange(2, 3, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            return;
        }
        #[cfg(target_arch = "riscv32")]
        let start = esp_hal::time::Instant::now();
        let job = unsafe { &mut *self.job.get() };
        if job.previous != Some(job.generation) {
            job.resampler = crate::pcm::Resampler::default();
            job.previous = Some(job.generation);
        }
        job.output_len = 0;
        if job.finish {
            job.output_len = job.resampler.finish(&mut job.output);
        } else {
            for frame in job.samples[..job.len].chunks_exact(job.channels) {
                job.output_len += job.resampler.frame(
                    frame[0],
                    frame[job.channels - 1],
                    job.rate,
                    &mut job.output[job.output_len..],
                );
            }
        }
        #[cfg(target_arch = "riscv32")]
        {
            job.elapsed_us = start.elapsed().as_micros();
        }
        self.state.store(4, Ordering::Release);
    }
    /// Serialize framebuffer writes with the filter's PSRAM reads, while
    /// allowing the main core's MP3 decoder to overlap conversion.
    pub fn paint<R>(&self, paint: impl FnOnce() -> R) -> Option<R> {
        let state = self.state.load(Ordering::Acquire);
        if !matches!(state, 0 | 2 | 4)
            || self
                .state
                .compare_exchange(state, 6, Ordering::Acquire, Ordering::Relaxed)
                .is_err()
        {
            return None;
        }
        struct Restore<'a>(&'a AtomicU8, u8);
        impl Drop for Restore<'_> {
            fn drop(&mut self) {
                self.0.store(self.1, Ordering::Release);
            }
        }
        let _restore = Restore(&self.state, state);
        Some(paint())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn input() -> Vec<i16> {
        (0..1152)
            .flat_map(|i| [(i * 23) as i16, -(i * 13) as i16])
            .collect()
    }
    fn reference(samples: &[i16], rate: u32) -> Vec<u8> {
        let mut r = crate::pcm::Resampler::default();
        let mut out = vec![0; 32768];
        let mut n = 0;
        for f in samples.chunks_exact(2) {
            n += r.frame(f[0], f[1], rate, &mut out[n..]);
        }
        n += r.finish(&mut out[n..]);
        out.truncate(n);
        out
    }
    #[test]
    fn mailbox_preserves_pcm_and_resets_history_after_abandoned_generation() {
        let worker = Worker::new();
        let samples = input();
        let mut output = crate::psram_buffer::zeroed(32768);
        assert!(worker.submit(&samples, 44100, 2, 1, false));
        assert!(!worker.submit(&samples, 44100, 2, 2, false));
        worker.run();
        let abandoned = worker.take(&mut output).unwrap();
        assert_eq!(abandoned.generation, 1);
        // New generation must have neither old samples nor fractional phase.
        for (generation, rate) in [(2, 44100), (3, 8000), (4, 48000)] {
            assert!(worker.submit(&samples, rate, 2, generation, false));
            worker.run();
            let done = worker.take(&mut output).unwrap();
            let mut actual = output[..done.len].to_vec();
            assert!(!done.finish);
            assert!(worker.submit(&[], 0, 2, generation, true));
            worker.run();
            let tail = worker.take(&mut output).unwrap();
            assert!(tail.finish);
            actual.extend_from_slice(&output[..tail.len]);
            assert_eq!(actual, reference(&samples, rate));
        }
    }
    #[test]
    fn framebuffer_guard_keeps_a_queued_job_intact() {
        let worker = Worker::new();
        assert!(worker.submit(&input(), 48000, 2, 7, false));
        worker
            .paint(|| {
                worker.run();
                assert!(
                    worker
                        .take(&mut crate::psram_buffer::zeroed(32768))
                        .is_none()
                );
            })
            .unwrap();
        worker.run();
        assert_eq!(
            worker
                .take(&mut crate::psram_buffer::zeroed(32768))
                .unwrap()
                .generation,
            7
        );
    }
    #[test]
    fn cross_thread_exchange_preserves_sample_order_and_single_ownership() {
        let worker = std::sync::Arc::new(Worker::new());
        let reader = worker.clone();
        let stop = std::sync::Arc::new(core::sync::atomic::AtomicBool::new(false));
        let worker_stop = stop.clone();
        let task = std::thread::spawn(move || {
            while !worker_stop.load(Ordering::Acquire) {
                reader.run();
                std::thread::yield_now();
            }
        });
        let mut output = crate::psram_buffer::zeroed(32768);
        for generation in 0..100 {
            let samples = [generation as i16, -(generation as i16)];
            assert!(worker.submit(&samples, 48000, 2, generation, false));
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            let done = loop {
                if let Some(done) = worker.take(&mut output) {
                    break done;
                }
                assert!(std::time::Instant::now() < deadline);
                std::thread::yield_now();
            };
            assert_eq!(done.generation, generation);
            assert_eq!(done.len, 4);
            assert_eq!(&output[..2], &samples[0].to_le_bytes());
            assert_eq!(&output[2..4], &samples[1].to_le_bytes());
        }
        stop.store(true, Ordering::Release);
        task.join().unwrap();
    }
}
