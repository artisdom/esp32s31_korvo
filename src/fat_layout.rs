//! FAT volume selection and in-memory MBR overlay for superfloppy/MBR/GPT cards.
//! No partition table is ever written to the card.
#[derive(Clone, Copy)]
pub struct Layout {
    pub blocks: u32,
    pub overlay: bool,
    pub fat32: bool,
    /// Physical start of a FAT partition exposed through the virtual MBR.
    pub offset: u32,
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
            offset: 0,
        }
    }
    pub fn physical(self, logical: u32) -> Option<u32> {
        let relative = logical.checked_sub(u32::from(self.overlay))?;
        if relative < self.blocks {
            relative.checked_add(self.offset)
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
            offset: 0,
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
            offset: 0,
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

/// Reflected IEEE CRC32, as used for GPT header/entry integrity.
fn crc(mut value: u32, bytes: &[u8]) -> u32 {
    for &b in bytes {
        value ^= b as u32;
        for _ in 0..8 {
            value = (value >> 1) ^ (0xedb88320u32 & 0u32.wrapping_sub(value & 1));
        }
    }
    value
}
fn u32_at(b: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(b[offset..offset + 4].try_into().unwrap())
}
fn u64_at(b: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(b[offset..offset + 8].try_into().unwrap())
}
pub fn fat_kind(b: &[u8; 512]) -> Option<bool> {
    if b[510..] != [0x55, 0xaa]
        || !matches!(b[0], 0xeb | 0xe9)
        || u16::from_le_bytes([b[11], b[12]]) != 512
        || !matches!(b[16], 1 | 2)
        || !b[13].is_power_of_two()
        || b[13] > 128
        || u16::from_le_bytes([b[14], b[15]]) == 0
    {
        return None;
    }
    let fat16 = u16::from_le_bytes([b[22], b[23]]) as u32;
    let fat32 = u32_at(b, 36);
    if fat16 != 0 {
        Some(false)
    } else if fat32 != 0 && b[17..19] == [0, 0] {
        Some(true)
    } else {
        None
    }
}
impl Layout {
    /// Select FAT16/32 on superfloppy, MBR, or a CRC-validated primary GPT.
    /// Only a synthetic MBR exists in memory. GPT, partition tables and boot
    /// sectors remain at their original physical locations and are never rewritten.
    pub fn read(
        blocks: u32,
        mut read: impl FnMut(u32, &mut [u8; 512]) -> Result<(), &'static str>,
    ) -> Result<Self, &'static str> {
        let mut boot = [0; 512];
        read(0, &mut boot)?;
        let detected = Self::detect(&boot, blocks);
        if detected.overlay {
            return Ok(detected);
        }
        if boot[510..] != [0x55, 0xaa] {
            return Err("Invalid SD partition signature");
        }
        let gpt = boot[446..510].chunks_exact(16).any(|p| p[4] == 0xee);
        if !gpt {
            for p in boot[446..510].chunks_exact(16) {
                if !matches!(p[4], 0x04 | 0x06 | 0x0b | 0x0c | 0x0e) {
                    continue;
                }
                let start = u32_at(p, 8);
                let len = u32_at(p, 12);
                if start == 0 || len == 0 || start.checked_add(len).is_none_or(|end| end > blocks) {
                    continue;
                }
                let mut vbr = [0; 512];
                read(start, &mut vbr)?;
                if let Some(fat32) = fat_kind(&vbr) {
                    return Ok(Self {
                        blocks: len,
                        overlay: true,
                        fat32,
                        offset: start,
                    });
                }
            }
            return Err("No FAT16/32 partition (exFAT unsupported)");
        }
        read(1, &mut boot)?;
        if &boot[..8] != b"EFI PART" || u32_at(&boot, 8) != 0x00010000 || u64_at(&boot, 24) != 1 {
            return Err("Invalid primary GPT header");
        }
        let size = u32_at(&boot, 12) as usize;
        if !(92..=512).contains(&size) {
            return Err("Invalid GPT header size");
        }
        let expected = u32_at(&boot, 16);
        boot[16..20].fill(0);
        if !crc(!0, &boot[..size]) != expected {
            return Err("GPT header CRC mismatch");
        }
        let table = u64_at(&boot, 72);
        let entries = u32_at(&boot, 80);
        let stride = u32_at(&boot, 84);
        let table_crc = u32_at(&boot, 88);
        let first = u64_at(&boot, 40);
        let last = u64_at(&boot, 48);
        if entries == 0
            || entries > 4096
            || stride < 128
            || stride > 4096
            || !stride.is_power_of_two()
            || table < 2
            || first > last
            || last >= blocks as u64
        {
            return Err("Unsupported GPT geometry");
        }
        let bytes = entries as u64 * stride as u64;
        if table
            .checked_add(bytes.div_ceil(512))
            .is_none_or(|end| end > first)
        {
            return Err("GPT table outside reserved region");
        }
        let mut sum = !0;
        let mut cached = [0; 512];
        for i in 0..bytes.div_ceil(512) {
            read((table + i) as u32, &mut cached)?;
            sum = crc(sum, &cached[..(bytes - i * 512).min(512) as usize]);
        }
        if !sum != table_crc {
            return Err("GPT entries CRC mismatch");
        }
        let mut selected = None;
        let mut selected_basic = false;
        let mut cached_lba = u64::MAX;
        for i in 0..entries as u64 {
            let pos = i * stride as u64;
            let lba = table + pos / 512;
            if lba != cached_lba {
                read(lba as u32, &mut cached)?;
                cached_lba = lba;
            }
            let entry = &cached[(pos % 512) as usize..(pos % 512) as usize + 128];
            if entry[..16].iter().all(|b| *b == 0) {
                continue;
            }
            let start = u64_at(entry, 32);
            let end = u64_at(entry, 40);
            if start < first || end > last || end < start {
                return Err("GPT partition outside usable region");
            }
            // Microsoft Basic Data, in GPT's mixed-endian byte representation.
            let basic = entry[..16]
                == [
                    0xa2, 0xa0, 0xd0, 0xeb, 0xe5, 0xb9, 0x33, 0x44, 0x87, 0xc0, 0x68, 0xb6, 0xb7,
                    0x26, 0x99, 0xc7,
                ];
            let mut vbr = [0; 512];
            read(start as u32, &mut vbr)?;
            if let Some(fat32) = fat_kind(&vbr) {
                if selected.is_none() || (basic && !selected_basic) {
                    selected = Some(Self {
                        blocks: (end - start + 1) as u32,
                        overlay: true,
                        fat32,
                        offset: start as u32,
                    });
                    selected_basic = basic;
                }
            }
        }
        selected.ok_or("GPT has no FAT16/32 volume (exFAT unsupported)")
    }
}
