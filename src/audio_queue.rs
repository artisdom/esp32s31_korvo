//! DMA write planning. Inspecting free space must not enqueue silent padding:
//! otherwise the last PCM samples can never drain to zero.
pub fn write_cursor(cursor: u64, done: u64, chunk: usize) -> u64 {
    if cursor < done + chunk as u64 {
        done + (chunk * 2) as u64
    } else {
        cursor
    }
}

pub fn available(cursor: u64, done: u64, len: usize, chunk: usize) -> usize {
    (done + (len - chunk) as u64).saturating_sub(write_cursor(cursor, done, chunk)) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inspecting_space_does_not_prevent_the_last_pcm_from_draining() {
        let cursor = 2308; // A final sample four bytes into a DMA block.
        for done in [1024, 2048, 3072, 4096, 8192] {
            let free = available(cursor, done, 65536, 1024);
            assert!(free <= 65536 - 2048);
        }
        assert_eq!(cursor.saturating_sub(3072), 0);
    }
    #[test]
    fn resuming_after_a_gap_stays_clear_of_active_dma() {
        assert_eq!(write_cursor(2308, 8192, 1024), 10240);
        assert_eq!(available(2308, 8192, 65536, 1024), 62464);
        assert_eq!(write_cursor(13000, 8192, 1024), 13000);
    }
}
