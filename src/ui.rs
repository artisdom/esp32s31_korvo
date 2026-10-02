//! LCD pages, split into static and dynamic rendering.
//!
//! The LCD DMA streams the PSRAM framebuffer continuously; rewriting or
//! cache-cleaning the whole 768 KB every frame starves it and the panel
//! loses sync (the image rolls). Pages are therefore drawn once when
//! entered, and only small dynamic widgets are repainted per tick, with
//! the cache cleaned over just those regions (see `Display::flush_rects`).

use core::fmt::Write as _;

use crate::gfx::{self, Canvas, Rect};
use crate::{audio, buttons::Button, camera, sdcard};

#[derive(Clone, Copy, PartialEq)]
pub enum Page {
    Home,
    Audio,
    Storage,
    Camera,
    Usb,
    About,
}

pub const PAGES: [(Page, &str); 6] = [
    (Page::Home, "HOME"),
    (Page::Audio, "AUDIO"),
    (Page::Storage, "SD CARD"),
    (Page::Camera, "CAMERA"),
    (Page::Usb, "USB"),
    (Page::About, "ABOUT"),
];

impl Page {
    pub fn next(self) -> Page {
        let idx = PAGES.iter().position(|(p, _)| *p == self).unwrap_or(0);
        PAGES[(idx + 1) % PAGES.len()].0
    }
}

/// Snapshot of everything the UI shows. Main updates it each tick.
pub struct AppStatus {
    pub page: Page,
    pub psram_total: usize,
    pub psram_free: usize,
    pub heap_free: usize,
    pub codec_ok: bool,
    pub codec_id: (u8, u8),
    pub touch_id: Option<[u8; 4]>,
    pub camera: camera::CameraProbe,
    pub usb_connected: bool,
    pub usb_rx: u32,
    pub usb_tx: u32,
    pub usb_input: crate::usb_input::Snapshot,
    pub sd: Option<sdcard::SdReport>,
    pub btn_mv: u16,
    pub btn_held: Option<Button>,
    pub mic_level: f32,
    pub volume_db: f32,
    pub audio_source: audio::Source,
    pub media_name: heapless::String<64>,
    pub media_status: heapless::String<64>,
    pub media_seconds: u32,
    pub media_count: usize,
    pub sd_file_name: crate::storage::Name,
    pub sd_file_count: usize,
    pub sd_file_index: usize,
    pub delete_name: Option<crate::storage::Name>,
    pub recording: bool,
    pub video_recording: bool,
    pub video_playing: bool,
    pub video_selected: heapless::String<32>,
    pub video_count: usize,
    pub video_index: usize,
    pub video_name: crate::storage::Name,
    pub video_frames: u32,
    pub camera_frames: u32,
    pub camera_errors: u32,
    pub uptime_s: u64,
    pub fps: u32,
    pub touch_point: Option<(u16, u16)>,
    pub led_rgb: (u8, u8, u8),
}

const HEADER_H: usize = 44;
const TABS_H: usize = 36;
const CONTENT_Y: usize = HEADER_H + TABS_H + 8;

const MAX_DIRTY: usize = 10;
type Dirty = heapless::Vec<Rect, MAX_DIRTY>;

/// Previous touch-cursor position, packed into an atomic (0 = none).
/// The cursor is drawn with XOR so redrawing the same pixels erases it
/// and restores the content underneath.
static LAST_CURSOR: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

fn pack_cursor(p: Option<(u16, u16)>) -> u32 {
    match p {
        None => 0,
        Some((x, y)) => (1 << 24) | ((x as u32) << 12) | (y as u32 & 0xfff),
    }
}

fn unpack_cursor(v: u32) -> Option<(u16, u16)> {
    if v & (1 << 24) == 0 {
        return None;
    }
    Some((((v >> 12) & 0xfff) as u16, (v & 0xfff) as u16))
}

// ---------------------------------------------------------------- layout

struct Card {
    x: usize,
    y: usize,
}

impl Card {
    fn new(x: usize, y: usize, w: usize, h: usize, c: &mut Canvas, title: &str) -> Self {
        c.rect(x, y, w, h, gfx::PANEL);
        c.frame(x, y, w, h, gfx::PANEL_HI);
        c.text(x + 12, y + 8, title, gfx::SKY, 1);
        Self { x, y }
    }

    fn at(x: usize, y: usize, _w: usize, _h: usize) -> Self {
        Self { x, y }
    }

    /// Erase a dynamic area of this card and return the dirty rect.
    fn begin_dyn(&self, c: &mut Canvas, rx: usize, ry: usize, rw: usize, rh: usize) -> Rect {
        let r = Rect::new(self.x + rx, self.y + ry, rw, rh);
        c.rect(r.x, r.y, r.w, r.h, gfx::PANEL);
        r
    }
}

// ---------------------------------------------------------------- full draw

/// Full page redraw: static chrome + current dynamic values.
pub fn draw(c: &mut Canvas, st: &AppStatus) {
    LAST_CURSOR.store(0, core::sync::atomic::Ordering::Relaxed);
    c.fill(gfx::BG);
    draw_header_static(c);
    draw_header_dyn(c, st);
    draw_tabs(c, st);
    match st.page {
        Page::Home => page_home_static(c, st),
        Page::Audio => page_audio_static(c, st),
        Page::Storage => page_storage_static(c, st),
        Page::Camera => page_camera_static(c, st),
        Page::Usb => page_usb_static(c),
        Page::About => page_about_static(c, st),
    }
    let mut d = Dirty::new();
    match st.page {
        Page::Home => page_home_dyn(c, st, &mut d),
        Page::Audio => page_audio_dyn(c, st, &mut d),
        Page::Storage => page_storage_dyn(c, st, &mut d),
        Page::Camera => page_camera_dyn(c, st, &mut d),
        Page::Usb => page_usb_dyn(c, st, &mut d),
        Page::About => page_about_dyn(c, st, &mut d),
    }
}

/// Repaint only the dynamic widgets; returns the regions touched.
pub fn draw_dynamic(c: &mut Canvas, st: &AppStatus) -> Dirty {
    let mut d = Dirty::new();

    d.push(draw_header_dyn(c, st)).ok();
    match st.page {
        Page::Home => page_home_dyn(c, st, &mut d),
        Page::Audio => page_audio_dyn(c, st, &mut d),
        Page::Storage => page_storage_dyn(c, st, &mut d),
        Page::Camera => page_camera_dyn(c, st, &mut d),
        Page::Usb => page_usb_dyn(c, st, &mut d),
        Page::About => page_about_dyn(c, st, &mut d),
    }

    d
}

/// Hit-test the tab bar; returns the page for a touch at (x, y).
pub fn hit_tabs(x: u16, y: u16) -> Option<Page> {
    if (y as usize) < HEADER_H || (y as usize) >= HEADER_H + TABS_H {
        return None;
    }
    let w = 800 / PAGES.len();
    let idx = (x as usize / w).min(PAGES.len() - 1);
    Some(PAGES[idx].0)
}

// ---------------------------------------------------------------- header/tabs

fn draw_header_static(c: &mut Canvas) {
    c.rect(0, 0, 800, HEADER_H, gfx::PANEL);
    c.rect(0, HEADER_H - 2, 800, 2, gfx::ACCENT);
    c.text(12, 12, "ESP32-S31-KORVO", gfx::TEXT, 2);
    c.text(280, 16, "Rust demo", gfx::MUTED, 1);
}

fn draw_header_dyn(c: &mut Canvas, st: &AppStatus) -> Rect {
    let r = Rect::new(420, 8, 372, 28);
    c.rect(r.x, r.y, r.w, r.h, gfx::PANEL);
    let mut x = 430;
    for (label, on, color) in [
        ("LCD", true, gfx::OK),
        ("I2S", st.codec_ok, gfx::OK),
        ("SD", st.sd.as_ref().is_some_and(|s| s.card_ok), gfx::OK),
        ("USB", st.usb_connected, gfx::SKY),
    ] {
        c.rect(x, 14, 10, 10, if on { color } else { gfx::PANEL_HI });
        c.text(x + 14, 16, label, gfx::MUTED, 1);
        x += 62;
    }
    let mut s: heapless::String<24> = heapless::String::new();
    let _ = write!(s, "{}fps {}s", st.fps, st.uptime_s);
    c.text(694, 16, &s, gfx::TEXT, 1);
    r
}

fn draw_tabs(c: &mut Canvas, st: &AppStatus) {
    let y = HEADER_H;
    let w = 800 / PAGES.len();
    for (i, (page, label)) in PAGES.iter().enumerate() {
        let x = i * w;
        let active = *page == st.page;
        c.rect(
            x,
            y,
            w - 2,
            TABS_H,
            if active { gfx::PANEL_HI } else { gfx::PANEL },
        );
        let tw = Canvas::text_width(label, 2);
        c.text(
            x + (w - tw) / 2,
            y + 10,
            label,
            if active { gfx::ACCENT } else { gfx::MUTED },
            2,
        );
    }
    c.rect(0, y + TABS_H, 800, 2, gfx::PANEL_HI);
}

// ---------------------------------------------------------------- cursor

/// Cursor updates are outside widget/video painting so its XOR backing remains intact.
pub fn cursor(c: &mut Canvas, point: Option<(u16, u16)>) -> Dirty {
    let mut dirty = Dirty::new();
    draw_cursor(c, point, Some(&mut dirty));
    dirty
}

fn draw_cursor(c: &mut Canvas, cur: Option<(u16, u16)>, mut dirty: Option<&mut Dirty>) {
    use core::sync::atomic::Ordering;
    let last = unpack_cursor(LAST_CURSOR.load(Ordering::Relaxed));
    if cur == last {
        return;
    }
    if let Some((x, y)) = last {
        c.xor_ring(x as usize, y as usize, 9);
        c.xor_ring(x as usize, y as usize, 4);
        if let Some(d) = dirty.as_deref_mut() {
            d.push(Rect::new(
                x.saturating_sub(10) as usize,
                y.saturating_sub(10) as usize,
                20,
                20,
            ))
            .ok();
        }
    }
    if let Some((x, y)) = cur {
        c.xor_ring(x as usize, y as usize, 9);
        c.xor_ring(x as usize, y as usize, 4);
        if let Some(d) = dirty.as_deref_mut() {
            d.push(Rect::new(
                x.saturating_sub(10) as usize,
                y.saturating_sub(10) as usize,
                20,
                20,
            ))
            .ok();
        }
    }
    LAST_CURSOR.store(pack_cursor(cur), Ordering::Relaxed);
}

// ---------------------------------------------------------------- HOME

const CW: usize = 258;
const CH1: usize = 180;
const GAP: usize = 8;
const ROW2_Y: usize = CONTENT_Y + CH1 + GAP;
const CH2: usize = 480 - ROW2_Y - 8;

fn page_home_static(c: &mut Canvas, st: &AppStatus) {
    let soc = Card::new(8, CONTENT_Y, CW, CH1, c, "SoC");
    c.text(soc.x + 12, soc.y + 34, "ESP32-S31", gfx::TEXT, 1);
    c.text(soc.x + 12, soc.y + 50, "dual-core RISC-V", gfx::MUTED, 1);
    c.text(
        soc.x + 12,
        soc.y + 104,
        "Wi-Fi 6 / BT 5.4 / 802.15.4",
        gfx::MUTED,
        1,
    );

    let mem = Card::new(8 + CW + GAP, CONTENT_Y, CW, CH1, c, "Memory");
    c.text(mem.x + 12, mem.y + 74, "16 MB flash (QIO)", gfx::MUTED, 1);
    c.text(
        mem.x + 12,
        mem.y + 90,
        "16 MB PSRAM (hex 250 MHz)",
        gfx::MUTED,
        1,
    );
    c.text(
        mem.x + 12,
        mem.y + 112,
        "framebuffer: PSRAM DMA",
        gfx::MUTED,
        1,
    );
    c.text(
        mem.x + 12,
        mem.y + 128,
        "ring descriptors: DRAM",
        gfx::MUTED,
        1,
    );

    let con = Card::new(8 + 2 * (CW + GAP), CONTENT_Y, CW, CH1, c, "Console + touch");
    c.rect(
        con.x + 12,
        con.y + 36,
        10,
        10,
        if st.touch_id.is_some() {
            gfx::OK
        } else {
            gfx::ERR
        },
    );
    c.text(con.x + 28, con.y + 34, "GT1151 touch", gfx::TEXT, 1);
    if let Some(id) = st.touch_id {
        let mut s: heapless::String<32> = heapless::String::new();
        let _ = write!(s, "id \"{}\"", core::str::from_utf8(&id).unwrap_or("????"));
        c.text(con.x + 12, con.y + 52, &s, gfx::MUTED, 1);
    }
    c.text(
        con.x + 12,
        con.y + 76,
        "UART log: 115200 8N1",
        gfx::MUTED,
        1,
    );
    c.text(con.x + 12, con.y + 92, "USB-C port (FT232R)", gfx::MUTED, 1);
    c.text(
        con.x + 12,
        con.y + 114,
        "touch the tabs to navigate",
        gfx::MUTED,
        1,
    );

    let btn = Card::new(8, ROW2_Y, CW, CH2, c, "Buttons (ADC ladder)");
    let names = ["VOL+  380mV", "VOL-  820mV", "MODE 1340mV", "SET  1870mV"];
    for (i, n) in names.iter().enumerate() {
        c.rect(btn.x + 14, btn.y + 106 + i * 18, 10, 10, gfx::PANEL_HI);
        c.text(btn.x + 30, btn.y + 104 + i * 18, n, gfx::MUTED, 1);
    }

    let usb = Card::new(8 + CW + GAP, ROW2_Y, CW, CH2, c, "USB (Type-A)");
    c.text(
        usb.x + 12,
        usb.y + 100,
        if cfg!(feature = "usb-host") {
            "HUB + KEYBOARD + MOUSE"
        } else {
            "screen /dev/ttyACM0"
        },
        gfx::MUTED,
        1,
    );
    c.text(
        usb.x + 12,
        usb.y + 116,
        if cfg!(feature = "usb-host") {
            "open USB tab to test input"
        } else {
            "send ? for info"
        },
        gfx::MUTED,
        1,
    );

    let led = Card::new(
        8 + 2 * (CW + GAP),
        ROW2_Y,
        CW,
        CH2,
        c,
        "Status LED (WS2812)",
    );
    c.text(led.x + 60, led.y + 66, "disabled", gfx::TEXT, 1);
    c.text(led.x + 60, led.y + 82, "LED off", gfx::MUTED, 1);
    c.text(led.x + 60, led.y + 102, "", gfx::TEXT, 1);
    c.text(led.x + 60, led.y + 118, "", gfx::MUTED, 1);
    c.text(led.x + 12, led.y + 150, "GPIO37, RMT-driven", gfx::MUTED, 1);
}

fn page_home_dyn(c: &mut Canvas, st: &AppStatus, d: &mut Dirty) {
    // SoC: uptime + loop fps
    {
        let card = Card::at(8, CONTENT_Y, CW, CH1);
        let r = card.begin_dyn(c, 6, 62, 246, 24);
        let mut s: heapless::String<40> = heapless::String::new();
        let _ = write!(s, "up {}s   loop {} fps", st.uptime_s, st.fps);
        c.text(card.x + 12, card.y + 66, &s, gfx::MUTED, 1);
        d.push(r).ok();
    }
    {
        let card = Card::at(8, CONTENT_Y, CW, CH1);
        let r = card.begin_dyn(c, 6, 118, 246, 58);
        #[cfg(feature = "radio-wifi-ble")]
        {
            use crate::radio::status;
            use core::sync::atomic::Ordering;
            let wifi = match status::WIFI.load(Ordering::Relaxed) {
                1 => "Wi-Fi scanning",
                2 => "Wi-Fi scan ready",
                3 => "Wi-Fi connecting",
                4 => "Wi-Fi associated",
                5 => "Wi-Fi connected / IP ready",
                6 => "Wi-Fi error / retry",
                _ => "Wi-Fi starting",
            };
            c.text(card.x + 12, card.y + 120, wifi, gfx::SKY, 1);
            let ble = match status::BLE.load(Ordering::Relaxed) {
                1 => "BLE advertising: Korvo-S31",
                2 => "BLE connected / GATT",
                _ => "BLE starting",
            };
            c.text(card.x + 12, card.y + 138, ble, gfx::TEXT, 1);
            let mut counts = heapless::String::<40>::new();
            let _ = write!(
                counts,
                "{} APs / TCP echo :2323",
                status::APS.load(Ordering::Relaxed)
            );
            c.text(card.x + 12, card.y + 156, &counts, gfx::MUTED, 1);
        }
        #[cfg(feature = "radio-802154")]
        {
            use crate::radio::status;
            use core::sync::atomic::Ordering;
            let mut counts = heapless::String::<40>::new();
            let _ = write!(
                counts,
                "802.15.4 scanning ch {}",
                status::CHANNEL.load(Ordering::Relaxed)
            );
            c.text(card.x + 12, card.y + 120, &counts, gfx::SKY, 1);
            counts.clear();
            let _ = write!(
                counts,
                "RX {} / Zigbee beacons {}",
                status::RECEIVED.load(Ordering::Relaxed),
                status::ZIGBEE_BEACONS.load(Ordering::Relaxed)
            );
            c.text(card.x + 12, card.y + 138, &counts, gfx::TEXT, 1);
            c.text(
                card.x + 12,
                card.y + 156,
                "Discovery / no network joined",
                gfx::MUTED,
                1,
            );
        }
        #[cfg(feature = "radio-classic")]
        {
            use crate::radio::status;
            use core::sync::atomic::Ordering;
            let phase = match status::CLASSIC.load(Ordering::Relaxed) {
                1 => "Classic: inquiry scanning",
                2 => "Classic: waiting for next scan",
                3 => "Classic error / check UART",
                _ => "Classic starting",
            };
            c.text(card.x + 12, card.y + 120, phase, gfx::SKY, 1);
            let mut counts = heapless::String::<40>::new();
            let _ = write!(
                counts,
                "Inquiry reports: {}",
                status::CLASSIC_REPORTS.load(Ordering::Relaxed)
            );
            c.text(card.x + 12, card.y + 138, &counts, gfx::TEXT, 1);
            c.text(
                card.x + 12,
                card.y + 156,
                "Discovery / no pairing",
                gfx::MUTED,
                1,
            );
        }
        #[cfg(feature = "radio-zigbee")]
        {
            use core::sync::atomic::Ordering;
            let phase = match crate::radio::status::ZIGBEE.load(Ordering::Relaxed) {
                1 => "Zigbee: configure PAN/channel",
                2 => "Zigbee: partition required",
                3 => "Zigbee commissioning",
                4 => "Zigbee: initial join complete",
                5 => "Zigbee stopped / check UART",
                _ => "Zigbee starting",
            };
            c.text(card.x + 12, card.y + 120, phase, gfx::SKY, 1);
            c.text(
                card.x + 12,
                card.y + 138,
                "Rust Basic / Identify demo",
                gfx::TEXT,
                1,
            );
        }
        #[cfg(not(any(
            feature = "radio-wifi-ble",
            feature = "radio-802154",
            feature = "radio-zigbee",
            feature = "radio-classic"
        )))]
        c.text(
            card.x + 12,
            card.y + 120,
            "Radios disabled in this build",
            gfx::MUTED,
            1,
        );
        d.push(r).ok();
    }
    // Memory: free numbers
    {
        let card = Card::at(8 + CW + GAP, CONTENT_Y, CW, CH1);
        let r = card.begin_dyn(c, 6, 28, 246, 40);
        let mut s: heapless::String<40> = heapless::String::new();
        let _ = write!(
            s,
            "PSRAM {:2} MB free/{} MB",
            st.psram_free / (1024 * 1024),
            st.psram_total / (1024 * 1024)
        );
        c.text(card.x + 12, card.y + 34, &s, gfx::TEXT, 1);
        let mut s2: heapless::String<40> = heapless::String::new();
        let _ = write!(s2, "heap  {} kB free", st.heap_free / 1024);
        c.text(card.x + 12, card.y + 52, &s2, gfx::TEXT, 1);
        d.push(r).ok();
    }
    // Buttons: mv + bar + held states
    {
        let card = Card::at(8, ROW2_Y, CW, CH2);
        let r = card.begin_dyn(c, 6, 28, 246, 146);
        let mut s: heapless::String<32> = heapless::String::new();
        let _ = write!(s, "GPIO42: {} mV", st.btn_mv);
        c.text(card.x + 12, card.y + 32, &s, gfx::TEXT, 1);
        c.bar(
            card.x + 12,
            card.y + 48,
            CW - 24,
            10,
            st.btn_mv as f32 / 2000.0,
            gfx::SKY,
        );
        let held = [Button::VolUp, Button::VolDown, Button::Mode, Button::Set];
        for (i, b) in held.iter().enumerate() {
            let on = st.btn_held == Some(*b);
            c.rect(
                card.x + 14,
                card.y + 106 + i * 18,
                10,
                10,
                if on { gfx::ACCENT } else { gfx::PANEL_HI },
            );
            // begin_dyn cleared this whole area, including the static labels.
            let mut label: heapless::String<24> = heapless::String::new();
            let _ = write!(
                label,
                "{:<4} {:4}mV",
                b.label(),
                crate::button_logic::CENTERS_MV[i]
            );
            c.text(
                card.x + 30,
                card.y + 104 + i * 18,
                &label,
                if on { gfx::TEXT } else { gfx::MUTED },
                1,
            );
        }
        d.push(r).ok();
    }
    // USB: state + counters + last rx
    {
        let card = Card::at(8 + CW + GAP, ROW2_Y, CW, CH2);
        let r = card.begin_dyn(c, 6, 24, 246, 72);
        c.rect(
            card.x + 12,
            card.y + 26,
            10,
            10,
            if st.usb_connected {
                gfx::OK
            } else {
                gfx::PANEL_HI
            },
        );
        c.text(
            card.x + 28,
            card.y + 24,
            if st.usb_connected {
                if cfg!(feature = "usb-host") {
                    "USB host attached"
                } else {
                    "CDC-ACM up"
                }
            } else {
                "not connected"
            },
            gfx::TEXT,
            1,
        );
        let mut s: heapless::String<48> = heapless::String::new();
        if cfg!(feature = "usb-host") {
            let _ = write!(
                s,
                "{} hubs {} keyboards {} mice",
                st.usb_input.hubs, st.usb_input.keyboards, st.usb_input.mice
            );
        } else {
            let _ = write!(s, "rx {} B  tx {} B", st.usb_rx, st.usb_tx);
        }
        c.text(card.x + 12, card.y + 44, &s, gfx::MUTED, 1);
        let last = if cfg!(feature = "usb-host") {
            crate::usb::USB_REPORT.read()
        } else {
            crate::usb::USB_LAST_RX.read()
        };
        let mut s2: heapless::String<64> = heapless::String::new();
        let _ = write!(s2, "last: {}", last.as_str());
        c.text(card.x + 12, card.y + 62, &s2, gfx::MUTED, 1);
        d.push(r).ok();
    }
    // LED indicator
    {
        let card = Card::at(8 + 2 * (CW + GAP), ROW2_Y, CW, CH2);
        let r = card.begin_dyn(c, 6, 60, 48, 72);
        let (red, green, blue) = st.led_rgb;
        let color = gfx::rgb565(red, green, blue);
        c.circle(card.x + 28, card.y + 96, 20, color);
        c.circle(card.x + 28, card.y + 96, 12, color);
        d.push(r).ok();
    }
}

// ---------------------------------------------------------------- AUDIO

pub fn hit_media(page: Page, x: u16, y: u16, confirming: bool) -> Option<crate::media::Command> {
    use crate::media::Command;
    if page == Page::Camera {
        if (400..=447).contains(&y) {
            return match x {
                20..=139 => Some(Command::VideoPrevious),
                148..=267 => Some(Command::VideoNext),
                276..=395 => Some(Command::VideoPlay),
                404..=523 => Some(Command::Stop),
                532..=651 => Some(Command::VideoRecord),
                660..=779 => Some(Command::VideoReplay),
                _ => None,
            };
        }
        return None;
    }
    let top = if page == Page::Audio { 280 } else { 348 };
    if (548..=779).contains(&x) {
        if confirming && (top + 60..=top + 107).contains(&y) {
            return Some(if x < 664 {
                Command::ConfirmDelete
            } else {
                Command::CancelDelete
            });
        }
        if !confirming && (top..=top + 47).contains(&y) {
            return Some(if page == Page::Audio {
                Command::DeleteTrack
            } else {
                Command::DeleteFile
            });
        }
    }
    if page == Page::Audio {
        return hit_audio(x, y);
    }
    if (408..=455).contains(&y) && (364..=527).contains(&x) {
        return Some(Command::PlayFile);
    }
    if (348..=395).contains(&y) {
        return match x {
            20..=183 => Some(Command::FilePrevious),
            192..=355 => Some(Command::FileNext),
            364..=527 => Some(Command::Refresh),
            _ => None,
        };
    }
    None
}
fn delete_controls(c: &mut Canvas, st: &AppStatus, y: usize, d: &mut Dirty) {
    c.rect(548, y, 232, 108, gfx::PANEL);
    if let Some(name) = &st.delete_name {
        c.text(558, y + 6, "Permanently delete?", gfx::WARN, 1);
        c.text(558, y + 28, name, gfx::TEXT, 1);
        for (x, label) in [(548, "CONFIRM"), (664, "CANCEL")] {
            c.rect(x, y + 60, 112, 48, gfx::PANEL_HI);
            c.text(x + 10, y + 78, label, gfx::WARN, 1);
        }
    } else {
        c.rect(548, y, 232, 48, gfx::PANEL_HI);
        c.text(558, y + 18, "DELETE SELECTED", gfx::WARN, 1);
        c.text(558, y + 70, "Confirmation required", gfx::MUTED, 1);
    }
    d.push(Rect::new(548, y, 232, 108)).ok();
}
pub fn hit_audio(x: u16, y: u16) -> Option<crate::media::Command> {
    use crate::media::Command;
    if (280..=327).contains(&y) {
        return match x {
            20..=139 => Some(Command::Previous),
            148..=267 => Some(Command::Next),
            276..=395 => Some(Command::Play),
            404..=523 => Some(Command::Stop),
            _ => None,
        };
    }
    if (340..=387).contains(&y) {
        return match x {
            20..=183 => Some(Command::Record),
            192..=355 => Some(Command::Replay),
            364..=527 => Some(Command::Refresh),
            _ => None,
        };
    }
    None
}
fn page_audio_static(c: &mut Canvas, st: &AppStatus) {
    Card::new(
        8,
        CONTENT_Y,
        784,
        384,
        c,
        "SD player / microphone recorder (ES8389)",
    );
    c.text(
        20,
        124,
        "SD root: MP3 + integer PCM WAV (mono / stereo)",
        gfx::MUTED,
        1,
    );
    c.text(
        20,
        252,
        "Touch a control; SET starts / stops recording",
        gfx::MUTED,
        1,
    );
    for (x, y, w, label) in [
        (20, 280, 120, "PREVIOUS"),
        (148, 280, 120, "NEXT"),
        (276, 280, 120, "PLAY"),
        (404, 280, 120, "STOP"),
        (20, 340, 164, "RECORD / SAVE"),
        (192, 340, 164, "REPLAY LAST"),
        (364, 340, 164, "RESCAN SD"),
    ] {
        c.rect(x, y, w, 48, gfx::PANEL_HI);
        c.text(x + 10, y + 18, label, gfx::ACCENT, 1);
    }
    c.text(
        20,
        414,
        "Recording: 48 kHz stereo WAV; STOP saves. VOL+- adjusts speaker.",
        gfx::MUTED,
        1,
    );
    c.text(
        20,
        438,
        "Stop recording before removing SD or turning off power.",
        gfx::MUTED,
        1,
    );
    let mut id: heapless::String<32> = heapless::String::new();
    let _ = write!(id, "codec {:02X}:{:02X}", st.codec_id.0, st.codec_id.1);
    c.text(604, 124, &id, gfx::MUTED, 1);
}
fn page_audio_dyn(c: &mut Canvas, st: &AppStatus, d: &mut Dirty) {
    let card = Card::at(8, CONTENT_Y, 784, 384);
    let r = card.begin_dyn(c, 6, 56, 768, 100);
    c.text(20, 150, &st.media_name, gfx::ACCENT, 2);
    c.text(
        20,
        180,
        &st.media_status,
        if st.recording { gfx::WARN } else { gfx::TEXT },
        1,
    );
    let mut line: heapless::String<96> = heapless::String::new();
    let source = match st.audio_source {
        audio::Source::File => "SD",
        audio::Source::Silence => "idle",
        audio::Source::Chime => "chime",
        audio::Source::Tone(_) => "tone",
    };
    let _ = write!(
        line,
        "{} tracks   {} s   {}   volume {:+.0} dB",
        st.media_count, st.media_seconds, source, st.volume_db
    );
    c.text(20, 202, &line, gfx::TEXT, 1);
    c.text(20, 222, "MIC", gfx::MUTED, 1);
    c.bar(62, 220, 460, 14, st.mic_level.min(1.0), gfx::OK);
    let mut level: heapless::String<24> = heapless::String::new();
    let _ = write!(level, "RMS {:.1}%", st.mic_level * 100.0);
    c.text(544, 222, &level, gfx::MUTED, 1);
    d.push(r).ok();
    delete_controls(c, st, 280, d);
}

// ---------------------------------------------------------------- STORAGE

fn page_storage_static(c: &mut Canvas, st: &AppStatus) {
    let y = CONTENT_Y;
    let rep = st.sd.as_ref();
    let info = Card::new(
        8,
        y,
        784,
        140,
        c,
        "microSD (SDMMC 4-bit @ 20 MHz, power GPIO39)",
    );
    match rep {
        None => {
            c.rect(info.x + 12, info.y + 36, 10, 10, gfx::ERR);
            c.text(info.x + 28, info.y + 34, "not probed", gfx::TEXT, 1);
        }
        Some(r) => {
            let ok = r.card_ok;
            c.rect(
                info.x + 12,
                info.y + 36,
                10,
                10,
                if ok { gfx::OK } else { gfx::ERR },
            );
            let mut s: heapless::String<64> = heapless::String::new();
            if ok {
                if r.capacity_mb >= 1000 {
                    let _ = write!(
                        s,
                        "{:.1} GB  bus {} MHz  mid 0x{:02X}",
                        r.capacity_mb as f32 / 1024.0,
                        r.freq_khz / 1000,
                        r.manuf_id
                    );
                } else {
                    let _ = write!(
                        s,
                        "{} MB  bus {} MHz  mid 0x{:02X}",
                        r.capacity_mb,
                        r.freq_khz / 1000,
                        r.manuf_id
                    );
                }
            } else {
                let _ = write!(s, "card error: {}", r.error.unwrap_or("?"));
            }
            c.text(info.x + 28, info.y + 34, &s, gfx::TEXT, 1);
            let mut s2: heapless::String<64> = heapless::String::new();
            let _ = write!(
                s2,
                "{} {}  label \"{}\"",
                r.partition, r.fs_type, r.volume_label
            );
            c.text(info.x + 12, info.y + 58, &s2, gfx::MUTED, 1);
        }
    }
    c.text(
        info.x + 12,
        info.y + 116,
        "AUDIO: play MP3/WAV and save new microphone WAV files",
        gfx::MUTED,
        1,
    );

    Card::new(
        8,
        y + 148,
        784,
        480 - y - 156,
        c,
        "Live SD root files (8.3 aliases; up to 64)",
    );
    for (x, label) in [(20, "PREVIOUS"), (192, "NEXT"), (364, "RESCAN SD")] {
        c.rect(x, 348, 164, 48, gfx::PANEL_HI);
        c.text(x + 10, 366, label, gfx::ACCENT, 1);
    }
    c.rect(364, 408, 164, 48, gfx::PANEL_HI);
    c.text(374, 426, "PLAY FILE", gfx::ACCENT, 1);
    c.text(20, 424, "STOP before deleting.", gfx::MUTED, 1);
    c.text(20, 446, "Delete cannot be undone.", gfx::MUTED, 1);
}
fn page_storage_dyn(c: &mut Canvas, st: &AppStatus, d: &mut Dirty) {
    c.rect(20, 264, 760, 68, gfx::PANEL);
    c.text(
        20,
        264,
        if st.sd_file_name.is_empty() {
            "No files in SD root"
        } else {
            &st.sd_file_name
        },
        gfx::ACCENT,
        2,
    );
    let mut count = heapless::String::<64>::new();
    let _ = write!(
        count,
        "File {} / {}",
        if st.sd_file_count == 0 {
            0
        } else {
            st.sd_file_index + 1
        },
        st.sd_file_count
    );
    c.text(20, 292, &count, gfx::MUTED, 1);
    c.text(20, 314, &st.media_status, gfx::TEXT, 1);
    d.push(Rect::new(20, 264, 760, 68)).ok();
    delete_controls(c, st, 348, d);
}

// ---------------------------------------------------------------- CAMERA

fn page_camera_static(c: &mut Canvas, st: &AppStatus) {
    Card::new(
        8,
        CONTENT_Y,
        784,
        384,
        c,
        "SC101IOT camera + microphone video recorder",
    );
    c.rect(20, 120, 320, 240, gfx::BLACK);
    let mut sensor = heapless::String::<64>::new();
    let _ = write!(
        sensor,
        "{} / 320 x 240",
        st.camera.sensor.map(|s| s.0).unwrap_or("No sensor")
    );
    c.text(364, 124, &sensor, gfx::TEXT, 1);
    c.text(364, 146, "AVI: MJPEG / 5 fps + stereo PCM", gfx::MUTED, 1);
    c.text(364, 168, "Microphone: 48 kHz / 16 bit", gfx::MUTED, 1);
    c.text(364, 190, "Speaker stays off while recording", gfx::MUTED, 1);
    c.text(
        364,
        212,
        "SET starts / saves video on this tab",
        gfx::MUTED,
        1,
    );
    for (x, label) in [
        (20, "PREVIOUS"),
        (148, "NEXT"),
        (276, "PLAY"),
        (404, "STOP / SAVE"),
        (532, "REC / SAVE"),
        (660, "REPLAY LAST"),
    ] {
        c.rect(x, 400, 120, 48, gfx::PANEL_HI);
        c.text(x + 8, 418, label, gfx::ACCENT, 1);
    }
    c.text(
        20,
        460,
        "Select an AVI to watch; REC / SAVE records camera + microphone. Stop before power off.",
        gfx::MUTED,
        1,
    );
}
fn page_camera_dyn(c: &mut Canvas, st: &AppStatus, d: &mut Dirty) {
    c.rect(364, 242, 416, 122, gfx::PANEL);
    c.text(
        364,
        242,
        if st.video_playing {
            "PLAYING VIDEO + MICROPHONE AUDIO"
        } else if st.camera_frames == 0 {
            "Waiting for camera frames..."
        } else if st.video_recording {
            "RECORDING VIDEO + MICROPHONE"
        } else {
            "Camera ready"
        },
        if st.video_recording {
            gfx::WARN
        } else {
            gfx::OK
        },
        1,
    );
    c.text(
        364,
        264,
        if st.video_playing || st.video_recording {
            &st.video_name
        } else {
            &st.video_selected
        },
        gfx::ACCENT,
        2,
    );
    let mut counts = heapless::String::<96>::new();
    let _ = write!(
        counts,
        "AVI {}/{}  frame {}  {}s",
        if st.video_count == 0 {
            0
        } else {
            st.video_index + 1
        },
        st.video_count,
        st.video_frames,
        st.media_seconds
    );
    c.text(364, 294, &counts, gfx::TEXT, 1);
    c.text(364, 316, &st.media_status, gfx::TEXT, 1);
    c.bar(364, 344, 240, 12, st.mic_level.min(1.0), gfx::OK);
    d.push(Rect::new(364, 242, 416, 122)).ok();
}
pub fn draw_camera_frame(c: &mut Canvas, frame: &[u8]) {
    for y in 0..crate::avi::HEIGHT {
        for x in 0..crate::avi::WIDTH {
            c.set(20 + x, 120 + y, crate::avi::rgb565(frame, x, y));
        }
    }
}

pub fn draw_playback_frame(c: &mut Canvas, frame: &[u8]) {
    for (index, pixel) in frame.chunks_exact(2).enumerate() {
        c.set(
            20 + index % crate::avi::WIDTH,
            120 + index / crate::avi::WIDTH,
            u16::from_le_bytes([pixel[0], pixel[1]]),
        );
    }
}

// ---------------------------------------------------------------- ABOUT

fn page_about_static(c: &mut Canvas, st: &AppStatus) {
    let y = CONTENT_Y;
    let a = Card::new(8, y, 784, 480 - y - 8, c, "About");
    c.text(
        a.x + 16,
        a.y + 34,
        "ESP32-S31-Korvo-1 + ESP32-S31-WROOM-3 (16M flash / 16M PSRAM)",
        gfx::TEXT,
        1,
    );
    c.text(
        a.x + 16,
        a.y + 50,
        "4.3\" 800x480 RGB LCD (ST7262E43) + GT1151 touch, ES8389 audio,",
        gfx::TEXT,
        1,
    );
    c.text(
        a.x + 16,
        a.y + 66,
        "DVP camera, WS2812 LED, ADC button ladder, microSD, USB 2.0 HS.",
        gfx::TEXT,
        1,
    );
    c.text(a.x + 16, a.y + 94, "100% Rust firmware:", gfx::ACCENT, 1);
    c.text(
        a.x + 16,
        a.y + 110,
        "esp-hal (git main, pre-1.3: S31 I2S/LCD_CAM/USB-HS), esp-rtos,",
        gfx::MUTED,
        1,
    );
    c.text(
        a.x + 16,
        a.y + 126,
        "embassy, esp-bootloader-esp-idf second stage.",
        gfx::MUTED,
        1,
    );
    c.text(
        a.x + 16,
        a.y + 154,
        "Feature status on this build:",
        gfx::ACCENT,
        1,
    );
    let rows: [(&str, &str); 9] = [
        ("RGB LCD + framebuffer", "works - PSRAM ring DMA @ 35 Hz"),
        (
            "Touch (GT1151)",
            if st.touch_id.is_some() {
                "works"
            } else {
                "no chip"
            },
        ),
        (
            "Audio playback (I2S+ES8389)",
            if st.codec_ok { "works" } else { "no codec" },
        ),
        ("Mic capture", "works (RMS meter)"),
        (
            "microSD + FAT read",
            if st.sd.as_ref().is_some_and(|s| s.card_ok) {
                "works"
            } else {
                "no card"
            },
        ),
        ("WS2812 LED (RMT)", "disabled at user request"),
        ("ADC button ladder", "debounced; 0 raw = idle"),
        ("USB HS CDC (Type-A)", "works"),
        ("Radios", "optional Wi-Fi/BLE or 802.15.4"),
    ];
    for (i, (k, v)) in rows.iter().enumerate() {
        let ry = a.y + 172 + i * 18;
        c.text(a.x + 16, ry, k, gfx::TEXT, 1);
        c.text(a.x + 290, ry, v, gfx::MUTED, 1);
    }
    c.text(
        a.x + 560,
        a.y + 94,
        "built with love & cargo",
        gfx::MUTED,
        1,
    );
}

fn page_about_dyn(c: &mut Canvas, st: &AppStatus, d: &mut Dirty) {
    let card = Card::at(8, CONTENT_Y, 784, 480 - CONTENT_Y - 8);
    let r = card.begin_dyn(c, 548, 60, 230, 24);
    let mut up: heapless::String<40> = heapless::String::new();
    let _ = write!(up, "uptime {} s", st.uptime_s);
    c.text(card.x + 552, card.y + 66, &up, gfx::MUTED, 1);
    d.push(r).ok();
}

// ---------------------------------------------------------------- USB input
fn page_usb_static(c: &mut Canvas) {
    Card::new(8, CONTENT_Y, 480, 240, c, "USB KEYBOARD - US layout");
    Card::new(496, CONTENT_Y, 296, 240, c, "USB MOUSE");
    Card::new(8, 336, 784, 132, c, "USB HOST - Type-A port");
    c.text(
        20,
        380,
        "F1..F6: pages   TAB: next page   LEFT/RIGHT: previous/next file",
        gfx::TEXT,
        1,
    );
    c.text(
        20,
        402,
        "ENTER: play / confirm deletion   ESC: cancel deletion and stop",
        gfx::TEXT,
        1,
    );
    c.text(
        20,
        424,
        "Mouse: move cursor and left-click tabs or media controls",
        gfx::TEXT,
        1,
    );
    c.text(
        20,
        446,
        "Full/low speed hubs; 2 hubs, 8 ports each, 4 HID interfaces",
        gfx::MUTED,
        1,
    );
}
fn page_usb_dyn(c: &mut Canvas, st: &AppStatus, d: &mut Dirty) {
    let s = &st.usb_input;
    let r = Rect::new(16, CONTENT_Y + 26, 464, 204);
    c.rect(r.x, r.y, r.w, r.h, gfx::PANEL);
    let mut line: heapless::String<72> = heapless::String::new();
    let _ = write!(
        line,
        "{} keyboard(s)  modifiers {:02X}  key {:02X}",
        s.keyboards, s.modifiers, s.last_key
    );
    c.text(20, 120, &line, gfx::TEXT, 1);
    c.text(
        20,
        150,
        "Type here (SHIFT, CAPS LOCK, BACKSPACE):",
        gfx::MUTED,
        1,
    );
    let mut row: heapless::String<72> = heapless::String::new();
    let mut y = 176;
    for ch in s.text.chars() {
        if ch == '\n' || row.len() >= 70 {
            c.text(20, y, &row, gfx::ACCENT, 1);
            row.clear();
            y += 22;
            if y > 286 {
                break;
            }
        }
        if ch != '\n' {
            let _ = row.push(ch);
        }
    }
    c.text(20, y, &row, gfx::ACCENT, 1);
    d.push(r).ok();
    let r = Rect::new(504, CONTENT_Y + 26, 280, 204);
    c.rect(r.x, r.y, r.w, r.h, gfx::PANEL);
    line.clear();
    let _ = write!(line, "{} mouse(s)", s.mice);
    c.text(508, 120, &line, gfx::TEXT, 1);
    line.clear();
    let _ = write!(line, "position {:?}", s.mouse);
    c.text(508, 146, &line, gfx::TEXT, 1);
    line.clear();
    let _ = write!(line, "buttons {:02X}  wheel {}", s.buttons, s.wheel);
    c.text(508, 172, &line, gfx::TEXT, 1);
    c.text(508, 204, "1=LEFT  2=RIGHT  4=MIDDLE", gfx::MUTED, 1);
    line.clear();
    let _ = write!(line, "reports {}  dropped {}", s.reports, s.dropped);
    c.text(508, 232, &line, gfx::MUTED, 1);
    line.clear();
    let _ = write!(
        line,
        "hubs {}  USB {}",
        s.hubs,
        if st.usb_connected {
            "attached"
        } else {
            "waiting"
        }
    );
    c.text(508, 260, &line, gfx::SKY, 1);
    d.push(r).ok();
    let r = Rect::new(16, 358, 768, 16);
    c.rect(r.x, r.y, r.w, r.h, gfx::PANEL);
    c.text(20, 360, &crate::usb::USB_REPORT.read(), gfx::MUTED, 1);
    d.push(r).ok();
}
