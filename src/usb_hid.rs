//! Interpret bounded HID report descriptors without assuming packet layouts.
use embassy_usb_host::class::hid::ReportDescriptor;
pub fn decode(d: &ReportDescriptor<64>, buf: &[u8], i: &mut crate::usb_input::Input, slot: usize) {
    let (id, payload) = if d.has_report_ids {
        let Some((&id, p)) = buf.split_first() else {
            return;
        };
        (id, p)
    } else {
        (0, buf)
    };
    let mut keys = [0; 32];
    let mut mods = 0;
    let mut keyboard = false;
    let mut mouse = false;
    let mut buttons = 0;
    let mut dx = 0;
    let mut dy = 0;
    let mut wheel = 0;
    for f in d.fields().filter(|f| f.report_id == id && f.flags & 1 == 0) {
        for n in 0..f.count as usize {
            let Some(v) = f.extract_u32(payload, n) else {
                return;
            };
            let usage = f.usage_min.saturating_add(n as u16);
            if f.usage_page == 7 {
                keyboard = true;
                let key = if f.flags & 2 != 0 {
                    if v == 0 {
                        continue;
                    }
                    usage as u32
                } else {
                    v
                };
                if key <= 255 {
                    keys[key as usize / 8] |= 1 << (key % 8);
                    if (0xe0..=0xe7).contains(&key) {
                        mods |= 1 << (key - 0xe0);
                        keys[key as usize / 8] &= !(1 << (key % 8));
                    }
                }
            } else if f.usage_page == 9 && (1..=8).contains(&usage) {
                mouse = true;
                if v != 0 {
                    buttons |= 1 << (usage - 1);
                }
            } else if f.usage_page == 1 && f.flags & 4 != 0 {
                let Some(v) = f.extract_i32(payload, n) else {
                    return;
                };
                match usage {
                    0x30 => {
                        dx = v;
                        mouse = true;
                    }
                    0x31 => {
                        dy = v;
                        mouse = true;
                    }
                    0x38 => {
                        wheel = v;
                        mouse = true;
                    }
                    _ => {}
                }
            }
        }
    }
    if keyboard {
        i.keyboard(slot, mods, keys);
    }
    if mouse {
        i.mouse(slot, buttons, dx, dy, wheel);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::usb_input::{Event, Input};
    #[test]
    fn keyboard_report_id_rollover_and_truncation() {
        let d = ReportDescriptor::<64>::parse(&[
            0x05, 7, 0x85, 3, 0x19, 0xe0, 0x29, 0xe7, 0x15, 0, 0x25, 1, 0x75, 1, 0x95, 8, 0x81, 2,
            0x75, 8, 0x95, 1, 0x81, 1, 0x19, 0, 0x29, 0xff, 0x15, 0, 0x26, 0xff, 0, 0x95, 6, 0x81,
            0,
        ]);
        let mut i = Input::new();
        decode(&d, &[3, 2, 0, 4, 0, 0, 0, 0, 0], &mut i, 0);
        assert_eq!(i.snapshot.text, "A");
        assert_eq!(i.pop(), Some(Event::Key(4)));
        decode(&d, &[3, 2, 0, 4], &mut i, 0); // short reports do not change state
        decode(&d, &[2, 2, 0, 5, 0, 0, 0, 0, 0], &mut i, 0); // unrelated report ID
        decode(&d, &[3, 0, 0, 1, 1, 1, 1, 1, 1], &mut i, 0); // rollover
        decode(&d, &[3, 2, 0, 4, 0, 0, 0, 0, 0], &mut i, 0);
        assert_eq!(i.snapshot.text, "A");
        assert_eq!(i.pop(), None);
    }
    #[test]
    fn signed_wide_mouse_axes_and_wheel() {
        let d = ReportDescriptor::<64>::parse(&[
            0x05, 9, 0x19, 1, 0x29, 3, 0x15, 0, 0x25, 1, 0x75, 1, 0x95, 3, 0x81, 2, 0x75, 5, 0x95,
            1, 0x81, 1, 0x05, 1, 0x09, 0x30, 0x09, 0x31, 0x16, 0, 0x80, 0x26, 0xff, 0x7f, 0x75, 16,
            0x95, 2, 0x81, 6, 0x09, 0x38, 0x15, 0x81, 0x25, 0x7f, 0x75, 8, 0x95, 1, 0x81, 6,
        ]);
        let mut i = Input::new();
        decode(&d, &[1, 0x38, 0xff, 0x2c, 1, 0xff], &mut i, 0);
        assert_eq!(i.snapshot.mouse, Some((200, 479)));
        assert_eq!(i.snapshot.wheel, -1);
        assert_eq!(i.pop(), Some(Event::Click(200, 479)));
        decode(&d, &[1, 0, 0, 0, 0, 0], &mut i, 0);
        assert_eq!(i.pop(), None);
    }
    #[test]
    fn nkro_bitmap() {
        let d = ReportDescriptor::<64>::parse(&[
            0x05, 7, 0x19, 4, 0x29, 11, 0x15, 0, 0x25, 1, 0x75, 1, 0x95, 8, 0x81, 2,
        ]);
        let mut i = Input::new();
        decode(&d, &[0x81], &mut i, 0);
        assert_eq!(i.snapshot.text, "ah");
        decode(&d, &[0x81], &mut i, 0);
        assert_eq!(i.snapshot.text, "ah");
    }
}
