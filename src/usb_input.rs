//! Bounded HID input state. Held keys and mouse buttons never repeat actions.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    Key(u8),
    Click(u16, u16),
}
#[derive(Clone)]
pub struct Snapshot {
    pub hubs: u8,
    pub keyboards: u8,
    pub mice: u8,
    pub reports: u32,
    pub modifiers: u8,
    pub last_key: u8,
    pub text: heapless::String<128>,
    pub mouse: Option<(u16, u16)>,
    pub buttons: u8,
    pub wheel: i32,
    pub dropped: u32,
}
pub struct Input {
    pub snapshot: Snapshot,
    keys: [[u8; 32]; 4],
    mods: [u8; 4],
    mouse_buttons: [u8; 4],
    caps: bool,
    events: heapless::Deque<Event, 32>,
}
impl Input {
    pub const fn new() -> Self {
        Self {
            snapshot: Snapshot {
                hubs: 0,
                keyboards: 0,
                mice: 0,
                reports: 0,
                modifiers: 0,
                last_key: 0,
                text: heapless::String::new(),
                mouse: None,
                buttons: 0,
                wheel: 0,
                dropped: 0,
            },
            keys: [[0; 32]; 4],
            mods: [0; 4],
            mouse_buttons: [0; 4],
            caps: false,
            events: heapless::Deque::new(),
        }
    }
    fn emit(&mut self, e: Event) {
        if self.events.push_back(e).is_err() {
            self.snapshot.dropped += 1;
        }
    }
    pub fn pop(&mut self) -> Option<Event> {
        self.events.pop_front()
    }
    pub fn release(&mut self, slot: usize) {
        self.keys[slot] = [0; 32];
        self.mods[slot] = 0;
        self.mouse_buttons[slot] = 0;
        self.snapshot.modifiers = self.mods.iter().fold(0, |a, b| a | b);
        self.snapshot.buttons = self.mouse_buttons.iter().fold(0, |a, b| a | b);
    }
    pub fn keyboard(&mut self, slot: usize, modifiers: u8, keys: [u8; 32]) {
        if keys[0] & 0x0e != 0 {
            return;
        } // preserve held keys on rollover/error reports
        self.mods[slot] = modifiers;
        self.snapshot.modifiers = self.mods.iter().fold(0, |a, b| a | b);
        for key in 4..=255usize {
            let mask = 1 << (key % 8);
            if keys[key / 8] & mask != 0 && self.keys.iter().all(|k| k[key / 8] & mask == 0) {
                let key = key as u8;
                self.snapshot.last_key = key;
                if key == 0x39 {
                    self.caps = !self.caps;
                }
                self.emit(Event::Key(key));
                if key == 0x2a {
                    self.snapshot.text.pop();
                } else if let Some(c) = ascii(key, self.snapshot.modifiers & 0x22 != 0, self.caps) {
                    if self.snapshot.modifiers & 0xdd == 0 {
                        if self.snapshot.text.len() == 128 {
                            self.snapshot.text.remove(0);
                        }
                        let _ = self.snapshot.text.push(c);
                    }
                }
            }
        }
        self.keys[slot] = keys;
    }
    pub fn mouse(&mut self, slot: usize, buttons: u8, dx: i32, dy: i32, wheel: i32) {
        let (x, y) = self.snapshot.mouse.unwrap_or((400, 240));
        let x = i32::from(x).saturating_add(dx).clamp(0, 799) as u16;
        let y = i32::from(y).saturating_add(dy).clamp(0, 479) as u16;
        self.snapshot.mouse = Some((x, y));
        let old = self.snapshot.buttons;
        self.mouse_buttons[slot] = buttons;
        self.snapshot.buttons = self.mouse_buttons.iter().fold(0, |a, b| a | b);
        self.snapshot.wheel = self.snapshot.wheel.saturating_add(wheel);
        if old & 1 == 0 && self.snapshot.buttons & 1 != 0 {
            self.emit(Event::Click(x, y));
        }
    }
}
fn ascii(key: u8, shift: bool, caps: bool) -> Option<char> {
    Some(match key {
        4..=29 => ((if shift ^ caps { b'A' } else { b'a' }) + key - 4) as char,
        30..=39 => (if shift { b"!@#$%^&*()" } else { b"1234567890" })[(key - 30) as usize] as char,
        0x28 => '\n',
        0x2c => ' ',
        0x2d..=0x38 => {
            (if shift {
                b"_+{}|~:\"~<>?"
            } else {
                b"-=[]\\#;'`,./"
            })[(key - 0x2d) as usize] as char
        }
        _ => return None,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    fn keys(k: u8) -> [u8; 32] {
        let mut b = [0; 32];
        b[k as usize / 8] |= 1 << (k % 8);
        b
    }
    #[test]
    fn held_and_multi_keyboard() {
        let mut i = Input::new();
        i.keyboard(0, 0, keys(4));
        i.keyboard(0, 0, keys(4));
        i.keyboard(1, 0, keys(4));
        assert_eq!(i.pop(), Some(Event::Key(4)));
        assert_eq!(i.pop(), None);
        i.release(0);
        i.keyboard(1, 0, keys(4));
        assert_eq!(i.pop(), None);
        i.release(1);
        i.keyboard(0, 0, keys(4));
        assert_eq!(i.snapshot.text, "aa");
    }
    #[test]
    fn rollover_preserves_held_keys() {
        let mut i = Input::new();
        i.keyboard(0, 0, keys(4));
        i.keyboard(0, 0, keys(1));
        i.keyboard(0, 0, keys(4));
        assert_eq!(i.snapshot.text, "a");
    }
    #[test]
    fn shift_caps_backspace() {
        let mut i = Input::new();
        i.keyboard(0, 2, keys(4));
        i.release(0);
        i.keyboard(0, 0, keys(0x39));
        i.release(0);
        i.keyboard(0, 0, keys(5));
        i.release(0);
        i.keyboard(0, 0, keys(0x2a));
        assert_eq!(i.snapshot.text, "A");
    }
    #[test]
    fn clicks_are_edges_and_clamped() {
        let mut i = Input::new();
        i.mouse(0, 1, -1000, 1000, 1);
        i.mouse(0, 1, 0, 0, 0);
        assert_eq!(i.pop(), Some(Event::Click(0, 479)));
        assert_eq!(i.pop(), None);
        i.release(0);
        i.mouse(0, 1, 1000, -1000, -1);
        assert_eq!(i.pop(), Some(Event::Click(799, 0)));
        assert_eq!(i.snapshot.wheel, 0);
    }
}
