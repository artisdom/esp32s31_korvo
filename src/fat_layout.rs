//! In-memory MBR overlay for FAT cards whose boot sector is physical LBA 0.
//! No partition table is ever written to the card.
#[derive(Clone, Copy)]
pub struct Layout {
    pub blocks: u32,
    pub overlay: bool,
    pub fat32: bool,
}
impl Layout {
    pub fn detect(boot: &[u8; 512], blocks: u32) -> Self {
        let overlay = boot[510..] == [0x55, 0xaa]
            && matches!(boot[0], 0xeb | 0xe9)
            && u16::from_le_bytes([boot[11], boot[12]]) == 512
            && matches!(boot[16], 1 | 2);
        Self {
            blocks,
            overlay,
            fat32: boot[17] == 0 && boot[18] == 0,
        }
    }
    pub fn physical(self, logical: u32) -> Option<u32> {
        let physical = logical.checked_sub(u32::from(self.overlay))?;
        if physical < self.blocks {
            Some(physical)
        } else {
            None
        }
    }
    pub fn mbr(self) -> [u8; 512] {
        let mut b = [0u8; 512];
        b[450] = if self.fat32 { 0x0c } else { 0x06 };
        b[454..458].copy_from_slice(&1u32.to_le_bytes());
        b[458..462].copy_from_slice(&self.blocks.to_le_bytes());
        b[510..].copy_from_slice(&[0x55, 0xaa]);
        b
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mapping_rejects_virtual_mbr_and_out_of_range() {
        let l = Layout {
            blocks: 123,
            overlay: true,
            fat32: true,
        };
        assert_eq!(l.physical(0), None);
        assert_eq!(l.physical(1), Some(0));
        assert_eq!(l.physical(123), Some(122));
        assert_eq!(l.physical(124), None);
    }
    #[test]
    fn mbr_partition_spans_exact_physical_card() {
        let l = Layout {
            blocks: 123,
            overlay: true,
            fat32: true,
        };
        let b = l.mbr();
        assert_eq!(b[450], 0x0c);
        assert_eq!(u32::from_le_bytes(b[454..458].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(b[458..462].try_into().unwrap()), 123);
    }
    #[test]
    fn existing_partition_table_is_passthrough() {
        let mut b = [0u8; 512];
        b[510..].copy_from_slice(&[0x55, 0xaa]);
        let l = Layout::detect(&b, 10);
        assert!(!l.overlay);
        assert_eq!(l.physical(0), Some(0));
    }
}
