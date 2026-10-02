//! One action per contact, despite empty polls between GT1151 reports.
#[derive(Default)]
pub struct TouchAction {
    held: bool,
    released_since: Option<u64>,
}
impl TouchAction {
    pub fn update(&mut self, point: Option<(u16, u16)>, now_ms: u64) -> Option<(u16, u16)> {
        if let Some(point) = point {
            self.released_since = None;
            if !self.held {
                self.held = true;
                return Some(point);
            }
        } else if self.held {
            let since = *self.released_since.get_or_insert(now_ms);
            if now_ms.saturating_sub(since) >= 100 {
                self.held = false;
                self.released_since = None;
            }
        }
        None
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn held_contact_and_empty_report_gaps_fire_once() {
        let mut touch = TouchAction::default();
        assert_eq!(touch.update(Some((200, 300)), 0), Some((200, 300)));
        for t in 1..500 {
            assert_eq!(
                touch.update(if t % 10 == 0 { Some((200, 300)) } else { None }, t),
                None
            );
        }
    }
    #[test]
    fn sustained_release_rearms_for_next_tap() {
        let mut touch = TouchAction::default();
        assert!(touch.update(Some((200, 300)), 0).is_some());
        assert!(touch.update(None, 10).is_none());
        assert!(touch.update(None, 109).is_none());
        assert!(touch.update(None, 110).is_none());
        assert_eq!(touch.update(Some((220, 300)), 120), Some((220, 300)));
    }
    #[test]
    fn dragging_and_a_long_block_without_a_release_do_not_repeat() {
        let mut touch = TouchAction::default();
        assert!(touch.update(Some((200, 300)), 0).is_some());
        assert_eq!(touch.update(Some((400, 300)), 2000), None);
    }
}
