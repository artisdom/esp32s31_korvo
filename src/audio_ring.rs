//! Continuous I2S descriptor rings. DMA owns the descriptors permanently.
//! DMA completion addresses identify completed 1024-byte blocks; the main loop accesses
//! only blocks away from the active DMA position and maintains cache coherency.
use core::sync::atomic::{AtomicU32, Ordering};
use esp_hal::dma::{
    DmaDescriptor, DmaRxBuffer, DmaTxBuffer, Owner, Preparation, aligned::DmaAlignedMut,
};

pub const LEN: usize = 64 * 1024;
pub const CHUNK: usize = 1024;
pub static TX_DONE: AtomicU32 = AtomicU32::new(0);
pub static RX_DONE: AtomicU32 = AtomicU32::new(0);

static TX_BASE: AtomicU32 = AtomicU32::new(0);
static RX_BASE: AtomicU32 = AtomicU32::new(0);
static LAST_POLL: AtomicU32 = AtomicU32::new(0);
fn now_us() -> u32 {
    esp_hal::time::Instant::now()
        .duration_since_epoch()
        .as_micros() as u32
}
pub fn poll_completions() {
    let now = now_us();
    let elapsed = now.wrapping_sub(LAST_POLL.swap(now, Ordering::Relaxed)) as u64;
    let ch = esp_hal::peripherals::DMA::regs().ch(0);
    for (address, base, total) in [
        (ch.out_eof_des_addr().read().bits(), &TX_BASE, &TX_DONE),
        (ch.in_done_des_addr().read().bits(), &RX_BASE, &RX_DONE),
    ] {
        let base = base.load(Ordering::Relaxed);
        if base == 0 || address < base {
            continue;
        }
        let index = (address - base) as usize / core::mem::size_of::<DmaDescriptor>();
        if index >= LEN / CHUNK {
            continue;
        }
        let previous = total.load(Ordering::Relaxed);
        let delta = ((index as u32 + 1).wrapping_sub(previous)) % 64;
        // A complete ring takes 341 ms at 48 kHz. Resolve full wraps from
        // elapsed time, while descriptor addresses determine the exact tail.
        let expected = elapsed * 192000 / 1000000 / CHUNK as u64;
        let wraps = expected.saturating_sub(delta as u64).saturating_add(32) / 64;
        total.store(
            previous
                .wrapping_add(delta)
                .wrapping_add((wraps * 64) as u32),
            Ordering::Release,
        );
    }
}

pub struct Ring<const RX: bool> {
    descriptors: DmaAlignedMut<'static, [DmaDescriptor]>,
    buffer: DmaAlignedMut<'static, [u8]>,
    cursor: u64,
    cleared: u64,
    pub lost: u32,
}
impl<const RX: bool> Ring<RX> {
    pub fn new(buffer: &'static mut [u8], descriptors: &'static mut [DmaDescriptor]) -> Self {
        assert_eq!(buffer.len(), LEN);
        assert_eq!(descriptors.len(), LEN / CHUNK);
        buffer.fill(0);
        Self {
            descriptors: unsafe { DmaAlignedMut::new_unchecked(descriptors) },
            buffer: unsafe { DmaAlignedMut::new_unchecked(buffer) },
            cursor: if RX { 0 } else { (CHUNK * 2) as u64 },
            cleared: 0,
            lost: 0,
        }
    }
    fn setup(&mut self) -> Preparation {
        let base = self.descriptors.as_mut_ptr();
        for (i, d) in self.descriptors.iter_mut().enumerate() {
            d.set_size(CHUNK);
            d.set_length(if RX { 0 } else { CHUNK });
            d.set_suc_eof(!RX);
            d.set_owner(Owner::Dma);
            d.buffer = unsafe { self.buffer.as_mut_ptr().add(i * CHUNK) };
            d.next = unsafe { base.add((i + 1) % (LEN / CHUNK)) };
        }
        self.descriptors.writeback();
        self.buffer.writeback();
        if RX {
            RX_BASE.store(base as u32, Ordering::Relaxed);
        } else {
            TX_BASE.store(base as u32, Ordering::Relaxed);
        }
        LAST_POLL.store(now_us(), Ordering::Relaxed);
        Preparation {
            start: base,
            accesses_psram: false,
            burst_transfer: esp_hal::dma::BurstConfig::default(),
            check_owner: Some(false),
            auto_write_back: false,
        }
    }
    fn done() -> u64 {
        (if RX {
            RX_DONE.load(Ordering::Acquire) as u64
        } else {
            TX_DONE.load(Ordering::Acquire) as u64
        }) * CHUNK as u64
    }
    pub fn available(&mut self) -> usize {
        let done = Self::done();
        if RX {
            if done.saturating_sub(self.cursor) > (LEN - CHUNK) as u64 {
                self.lost += 1;
                self.cursor = done - (LEN - CHUNK) as u64;
            }
            done.saturating_sub(self.cursor) as usize
        } else {
            crate::audio_queue::available(self.cursor, done, LEN, CHUNK)
        }
    }
    pub fn queued(&self) -> usize {
        self.cursor.saturating_sub(Self::done()) as usize
    }
    fn writeback(&mut self, start: usize, count: usize) {
        let begin = start / 64 * 64;
        let end = (start + count + 63) / 64 * 64;
        let mut aligned = unsafe { DmaAlignedMut::new_unchecked(&mut self.buffer[begin..end]) };
        aligned.writeback();
    }
    pub fn clear_played(&mut self) {
        let done = Self::done();
        self.cleared = self.cleared.max(done.saturating_sub((LEN - CHUNK) as u64));
        while self.cleared < done {
            let offset = self.cleared as usize % LEN;
            self.buffer[offset..offset + CHUNK].fill(0);
            self.writeback(offset, CHUNK);
            self.cleared += CHUNK as u64;
        }
    }
    pub fn push(&mut self, bytes: &[u8]) -> usize {
        self.push_pcm(bytes, true)
    }
    pub fn push_silence(&mut self, bytes: &[u8]) -> usize {
        self.push_pcm(bytes, false)
    }
    fn push_pcm(&mut self, bytes: &[u8], report_loss: bool) -> usize {
        if !RX && bytes.len() >= 4 {
            // Recover only when appending PCM, so EOF queries do not keep
            // advancing the tail. Played blocks have already been zeroed.
            let cursor = crate::audio_queue::write_cursor(self.cursor, Self::done(), CHUNK);
            if cursor != self.cursor {
                if report_loss {
                    self.lost += 1;
                }
                self.cursor = cursor;
            }
        }
        let mut n = bytes.len().min(self.available());
        n -= n % 4;
        let mut offset = 0;
        while offset < n {
            let start = self.cursor as usize % LEN;
            let count = (LEN - start).min(n - offset);
            self.buffer[start..start + count].copy_from_slice(&bytes[offset..offset + count]);
            self.writeback(start, count);
            self.cursor += count as u64;
            offset += count;
        }
        n
    }
    pub fn pop(&mut self, out: &mut [u8]) -> usize {
        let n = out.len().min(self.available());
        let start = self.cursor as usize % LEN;
        let count = n.min(LEN - start);
        if count > 0 {
            let aligned =
                unsafe { DmaAlignedMut::new_unchecked(&mut self.buffer[start..start + count]) };
            aligned.invalidate();
            out[..count].copy_from_slice(&self.buffer[start..start + count]);
            self.cursor += count as u64;
        }
        count
    }
}
unsafe impl DmaTxBuffer for Ring<false> {
    type View = Self;
    type Final = Self;
    fn prepare(&mut self) -> Preparation {
        self.setup()
    }
    fn into_view(self) -> Self {
        self
    }
    fn from_view(view: Self) -> Self {
        view
    }
}
unsafe impl DmaRxBuffer for Ring<true> {
    type View = Self;
    type Final = Self;
    fn prepare(&mut self) -> Preparation {
        self.setup()
    }
    fn into_view(self) -> Self {
        self
    }
    fn from_view(view: Self) -> Self {
        view
    }
}
