//! Zigbee beacon payload (Zigbee specification Annex D), after the standard
//! 802.15.4 beacon content. Discovery only; no network keys or joining.
#[derive(Debug, PartialEq, Eq)]
pub struct ZigbeeBeacon {
    pub extended_pan: u64,
    pub version: u8,
    pub depth: u8,
    pub router_capacity: bool,
    pub end_device_capacity: bool,
}
impl ZigbeeBeacon {
    pub fn parse(payload: &[u8]) -> Option<Self> {
        // Protocol ID 0, Zigbee PRO stack profile 2, and complete beacon data.
        if payload.len() < 15 || payload[0] != 0 || payload[1] & 0xf != 2 {
            return None;
        }
        Some(Self {
            extended_pan: u64::from_le_bytes(payload[3..11].try_into().ok()?),
            version: payload[1] >> 4,
            depth: (payload[2] >> 3) & 0xf,
            router_capacity: payload[2] & 4 != 0,
            end_device_capacity: payload[2] & 0x80 != 0,
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    const BEACON: [u8; 15] = [
        0, 0x22, 0x9c, 0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11, 0xff, 0xff, 0xff, 1,
    ];
    #[test]
    fn decodes_pan_and_capacity_bits() {
        assert_eq!(
            ZigbeeBeacon::parse(&BEACON),
            Some(ZigbeeBeacon {
                extended_pan: 0x1122334455667788,
                version: 2,
                depth: 3,
                router_capacity: true,
                end_device_capacity: true,
            })
        );
        let mut full = BEACON;
        full[2] = 0;
        let b = ZigbeeBeacon::parse(&full).unwrap();
        assert!(!b.router_capacity && !b.end_device_capacity);
    }
    #[test]
    fn rejects_truncated_and_other_protocol_beacons() {
        for size in 0..15 {
            assert!(ZigbeeBeacon::parse(&BEACON[..size]).is_none());
        }
        let mut other = BEACON;
        other[0] = 3;
        assert!(ZigbeeBeacon::parse(&other).is_none());
        other[0] = 0;
        other[1] = 0x21;
        assert!(ZigbeeBeacon::parse(&other).is_none());
    }
}
