//! Validate the reserved Zigbee flash partition before any persistent writes.
const START: u32 = 0xff0000;
const LENGTH: u32 = 0x10000;
pub fn zigbee_partition_reserved(table: &[u8]) -> bool {
    let mut found = false;
    for entry in table.chunks_exact(32) {
        let magic = u16::from_le_bytes([entry[0], entry[1]]);
        if magic == 0xffff || magic == 0xebeb {
            break;
        } // end or MD5 trailer
        if magic != 0x50aa {
            return false;
        }
        let offset = u32::from_le_bytes(entry[4..8].try_into().unwrap());
        let size = u32::from_le_bytes(entry[8..12].try_into().unwrap());
        let Some(end) = offset.checked_add(size) else {
            return false;
        };
        if offset < START + LENGTH && end > START {
            let label = &entry[12..28];
            if found
                || offset != START
                || size != LENGTH
                || entry[2] != 1
                || entry[3] != 0x40
                || label[..7] != *b"zigbee\0"
                || label[7..].iter().any(|b| *b != 0)
            {
                return false;
            }
            found = true;
        }
    }
    found
}
#[cfg(test)]
mod tests {
    use super::*;
    fn entry(offset: u32, size: u32, kind: u8, subtype: u8, label: &[u8]) -> [u8; 32] {
        let mut entry = [0; 32];
        entry[..2].copy_from_slice(&0x50aau16.to_le_bytes());
        entry[2] = kind;
        entry[3] = subtype;
        entry[4..8].copy_from_slice(&offset.to_le_bytes());
        entry[8..12].copy_from_slice(&size.to_le_bytes());
        entry[12..12 + label.len()].copy_from_slice(label);
        entry
    }
    #[test]
    fn accepts_explicit_partition_and_separate_factory() {
        let mut table = Vec::new();
        table.extend(entry(0x10000, 0xe00000, 0, 0, b"factory"));
        table.extend(entry(START, LENGTH, 1, 0x40, b"zigbee"));
        table.extend([0xff; 32]);
        assert!(zigbee_partition_reserved(&table));
    }
    #[test]
    fn rejects_overlap_missing_duplicates_wrong_type_and_overflow() {
        let reserved = entry(START, LENGTH, 1, 0x40, b"zigbee");
        assert!(!zigbee_partition_reserved(&[]));
        let mut duplicate = reserved.to_vec();
        duplicate.extend(reserved);
        assert!(!zigbee_partition_reserved(&duplicate));
        let mut overlap = reserved.to_vec();
        overlap.extend(entry(0x10000, 0xff0000, 0, 0, b"factory"));
        assert!(!zigbee_partition_reserved(&overlap));
        assert!(!zigbee_partition_reserved(&entry(
            START, LENGTH, 0, 0x40, b"zigbee"
        )));
        let mut overflow = reserved.to_vec();
        overflow.extend(entry(0xfffffff0, 128, 1, 1, b"oops"));
        assert!(!zigbee_partition_reserved(&overflow));
    }
}
