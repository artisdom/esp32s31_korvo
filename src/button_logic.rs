//! Pure ADC-ladder conversion, decoding and time-based debouncing.

pub const CENTERS_MV: [u16; 4] = [380, 820, 1340, 1870];
pub const IDLE_MV: u16 = 2000;
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Button {
    VolUp,
    VolDown,
    Mode,
    Set,
}
pub const ALL: [Button; 4] = [Button::VolUp, Button::VolDown, Button::Mode, Button::Set];

impl Button {
    pub fn label(self) -> &'static str {
        match self {
            Self::VolUp => "VOL+",
            Self::VolDown => "VOL-",
            Self::Mode => "MODE",
            Self::Set => "SET",
        }
    }
}

/// The HAL returns the SAR's weighted code (0..4393), not the comparator
/// bit pattern. GPIO42 is the negative input, hence the inverted voltage.
pub fn raw_to_mv(raw: u16) -> u16 {
    (2000 - 4000 * raw as i64 / 4393).clamp(0, 2000) as u16
}

/// Non-overlapping voltage windows matching the vendor BSP midpoints.
pub fn decode(mv: u16) -> Option<Button> {
    if mv < 160 || mv >= (CENTERS_MV[3] + IDLE_MV) / 2 {
        return None;
    }
    for i in 0..3 {
        if mv < (CENTERS_MV[i] + CENTERS_MV[i + 1]) / 2 {
            return Some(ALL[i]);
        }
    }
    Some(Button::Set)
}

pub struct Debouncer {
    pub held: Option<Button>,
    candidate: Option<Button>,
    since_ms: u64,
}
impl Debouncer {
    pub const fn new() -> Self {
        Self {
            held: None,
            candidate: None,
            since_ms: 0,
        }
    }
    /// Commit both press and release after 20 ms stable; a direct change to
    /// another key updates the held state and generates that key's press.
    pub fn update(&mut self, reading: Option<Button>, now_ms: u64) -> Option<Button> {
        if reading != self.candidate {
            self.candidate = reading;
            self.since_ms = now_ms;
        }
        if reading != self.held && now_ms.saturating_sub(self.since_ms) >= 20 {
            self.held = reading;
            return reading;
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn centers_idle_and_invalid_readings() {
        for (mv, btn) in CENTERS_MV.into_iter().zip(ALL) {
            assert_eq!(decode(mv), Some(btn));
        }
        assert_eq!(decode(2000), None);
        assert_eq!(decode(1935), None);
        assert_eq!(decode(0), None);
        assert_eq!(raw_to_mv(0), 2000);
        assert_eq!(raw_to_mv(4393), 0);
    }
    #[test]
    fn bounce_hold_direct_change_and_release() {
        let mut d = Debouncer::new();
        assert_eq!(d.update(Some(Button::VolUp), 1), None);
        assert_eq!(d.update(None, 5), None);
        assert_eq!(d.update(Some(Button::VolUp), 9), None);
        assert_eq!(d.update(Some(Button::VolUp), 28), None);
        assert_eq!(d.update(Some(Button::VolUp), 29), Some(Button::VolUp));
        assert_eq!(d.update(Some(Button::VolUp), 100), None);
        assert_eq!(d.update(Some(Button::Set), 101), None);
        assert_eq!(d.update(Some(Button::Set), 121), Some(Button::Set));
        assert_eq!(d.held, Some(Button::Set));
        assert_eq!(d.update(None, 122), None);
        assert_eq!(d.held, Some(Button::Set));
        assert_eq!(d.update(None, 142), None);
        assert_eq!(d.held, None);
    }
    #[test]
    fn measured_weighted_codes_identify_each_key() {
        for (raw, expected) in [
            (1809, Button::VolUp),
            (1361, Button::VolDown),
            (835, Button::Mode),
            (292, Button::Set),
        ] {
            assert_eq!(decode(raw_to_mv(raw)), Some(expected));
        }
    }
}
