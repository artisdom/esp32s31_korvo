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
    About,
}

pub const PAGES: [(Page, &str); 5] = [
    (Page::Home, "HOME"),
    (Page::Audio, "AUDIO"),
    (Page::Storage, "SD CARD"),
    (Page::Camera, "CAMERA"),
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
    pub sd: Option<sdcard::SdReport>,
    pub btn_mv: u16,
    pub btn_held: Option<Button>,
    pub mic_level: f32,
    pub volume_db: f32,
    pub audio_source: audio::Source,
    pub uptime_s: u64,
    pub fps: u32,
    pub touch_point: Option<(u16, u16)>,
    pub flash_led: bool,
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
    w: usize,
    h: usize,
}

impl Card {
    fn new(x: usize, y: usize, w: usize, h: usize, c: &mut Canvas, title: &str) -> Self {
        c.rect(x, y, w, h, gfx::PANEL);
        c.frame(x, y, w, h, gfx::PANEL_HI);
        c.text(x + 12, y + 8, title, gfx::SKY, 1);
        Self { x, y, w, h }
    }

    fn at(x: usize, y: usize, w: usize, h: usize) -> Self {
        Self { x, y, w, h }
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
    c.fill(gfx::BG);
    draw_header_static(c);
    draw_header_dyn(c, st);
    draw_tabs(c, st);
    match st.page {
        Page::Home => page_home_static(c, st),
        Page::Audio => page_audio_static(c, st),
        Page::Storage => page_storage_static(c, st),
        Page::Camera => page_camera_static(c, st),
        Page::About => page_about_static(c, st),
    }
    let mut d = Dirty::new();
    match st.page {
        Page::Home => page_home_dyn(c, st, &mut d),
        Page::Audio => page_audio_dyn(c, st, &mut d),
        Page::Storage => {}
        Page::Camera => {}
        Page::About => page_about_dyn(c, st, &mut d),
    }
    draw_cursor(c, st.touch_point, None);
    LAST_CURSOR.store(pack_cursor(st.touch_point), core::sync::atomic::Ordering::Relaxed);
}

/// Repaint only the dynamic widgets; returns the regions touched.
pub fn draw_dynamic(c: &mut Canvas, st: &AppStatus) -> Dirty {
    let mut d = Dirty::new();
    d.push(draw_header_dyn(c, st)).ok();
    match st.page {
        Page::Home => page_home_dyn(c, st, &mut d),
        Page::Audio => page_audio_dyn(c, st, &mut d),
        Page::Storage => {}
        Page::Camera => {}
        Page::About => page_about_dyn(c, st, &mut d),
    }
    draw_cursor(c, st.touch_point, Some(&mut d));
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
        c.rect(x, y, w - 2, TABS_H, if active { gfx::PANEL_HI } else { gfx::PANEL });
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

fn draw_cursor(c: &mut Canvas, cur: Option<(u16, u16)>, mut dirty: Option<&mut Dirty>) {
    use core::sync::atomic::Ordering;
    let last = unpack_cursor(LAST_CURSOR.load(Ordering::Relaxed));
    if cur == last {
        return;
    }
    if let Some((x, y)) = last {
        c.xor_ring(x as usize, y as usize, 9);
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
    c.text(soc.x + 12, soc.y + 104, "Wi-Fi 6 / BT 5.4 / 802.15.4", gfx::MUTED, 1);
    c.text(soc.x + 12, soc.y + 120, "(radio: upstream esp-radio)", gfx::MUTED, 1);

    let mem = Card::new(8 + CW + GAP, CONTENT_Y, CW, CH1, c, "Memory");
    c.text(mem.x + 12, mem.y + 74, "16 MB flash (QIO)", gfx::MUTED, 1);
    c.text(mem.x + 12, mem.y + 90, "16 MB PSRAM (hex 250 MHz)", gfx::MUTED, 1);
    c.text(mem.x + 12, mem.y + 112, "framebuffer: PSRAM DMA", gfx::MUTED, 1);
    c.text(mem.x + 12, mem.y + 128, "ring descriptors: DRAM", gfx::MUTED, 1);

    let con = Card::new(8 + 2 * (CW + GAP), CONTENT_Y, CW, CH1, c, "Console + touch");
    c.rect(
        con.x + 12,
        con.y + 36,
        10,
        10,
        if st.touch_id.is_some() { gfx::OK } else { gfx::ERR },
    );
    c.text(con.x + 28, con.y + 34, "GT1151 touch", gfx::TEXT, 1);
    if let Some(id) = st.touch_id {
        let mut s: heapless::String<32> = heapless::String::new();
        let _ = write!(s, "id \"{}\"", core::str::from_utf8(&id).unwrap_or("????"));
        c.text(con.x + 12, con.y + 52, &s, gfx::MUTED, 1);
    }
    c.text(con.x + 12, con.y + 76, "UART log: 115200 8N1", gfx::MUTED, 1);
    c.text(con.x + 12, con.y + 92, "USB-C port (FT232R)", gfx::MUTED, 1);
    c.text(con.x + 12, con.y + 114, "touch the tabs to navigate", gfx::MUTED, 1);

    let btn = Card::new(8, ROW2_Y, CW, CH2, c, "Buttons (ADC ladder)");
    let names = ["VOL+  380mV", "VOL-  820mV", "MODE 1340mV", "SET  1870mV"];
    for (i, n) in names.iter().enumerate() {
        c.rect(btn.x + 14, btn.y + 106 + i * 18, 10, 10, gfx::PANEL_HI);
        c.text(btn.x + 30, btn.y + 104 + i * 18, n, gfx::MUTED, 1);
    }

    let usb = Card::new(8 + CW + GAP, ROW2_Y, CW, CH2, c, "USB HS (Type-A)");
    c.text(usb.x + 12, usb.y + 100, "screen /dev/ttyACM0", gfx::MUTED, 1);
    c.text(usb.x + 12, usb.y + 116, "send ? for info", gfx::MUTED, 1);

    let led = Card::new(8 + 2 * (CW + GAP), ROW2_Y, CW, CH2, c, "Status LED (WS2812)");
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
        c.bar(card.x + 12, card.y + 48, CW - 24, 10, st.btn_mv as f32 / 2000.0, gfx::SKY);
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
            let _ = write!(label, "{:<4} {:4}mV", b.label(), crate::button_logic::CENTERS_MV[i]);
            c.text(card.x + 30, card.y + 104 + i * 18, &label,
                if on { gfx::TEXT } else { gfx::MUTED }, 1);
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
            if st.usb_connected { gfx::OK } else { gfx::PANEL_HI },
        );
        c.text(
            card.x + 28,
            card.y + 24,
            if st.usb_connected { "CDC-ACM up" } else { "not connected" },
            gfx::TEXT,
            1,
        );
        let mut s: heapless::String<48> = heapless::String::new();
        let _ = write!(s, "rx {} B  tx {} B", st.usb_rx, st.usb_tx);
        c.text(card.x + 12, card.y + 44, &s, gfx::MUTED, 1);
        let last = crate::usb::USB_LAST_RX.read();
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
        c.circle(
            card.x + 28,
            card.y + 96,
            12,
            if st.flash_led { gfx::WARN } else { color },
        );
        d.push(r).ok();
    }
}

// ---------------------------------------------------------------- AUDIO

fn page_audio_static(c: &mut Canvas, st: &AppStatus) {
    let y = CONTENT_Y;
    let spk = Card::new(8, y, 500, 150, c, "Speaker (ES8389 + 2x NS4150B, 3 W)");
    let mut s: heapless::String<48> = heapless::String::new();
    if st.codec_ok {
        let _ = write!(s, "codec id 0x{:02X}:0x{:02X}", st.codec_id.0, st.codec_id.1);
    } else {
        s.push_str("not detected").ok();
    }
    c.rect(spk.x + 12, spk.y + 34, 10, 10, if st.codec_ok { gfx::OK } else { gfx::ERR });
    c.text(spk.x + 28, spk.y + 32, &s, gfx::TEXT, 1);
    c.text(spk.x + 12, spk.y + 54, "I2S0 48 kHz / 16-bit / stereo, DMA stream", gfx::MUTED, 1);
    c.text(spk.x + 12, spk.y + 130, "SET = chime   MODE = 440 Hz   VOL+- = volume", gfx::MUTED, 1);

    let y2 = y + 158;
    let mic = Card::new(8, y2, 500, 480 - y2 - 8, c, "Microphones (2x analog, ADC loop)");
    c.text(mic.x + 12, mic.y + 24, "speak into the left mic ->", gfx::MUTED, 1);

    let chain = Card::new(516, y, 276, 480 - y - 8, c, "Signal chain");
    c.text(chain.x + 12, chain.y + 34, "mic -> ES8389 ADC", gfx::MUTED, 1);
    c.text(chain.x + 12, chain.y + 48, "    -> I2S0 RX DMA", gfx::MUTED, 1);
    c.text(chain.x + 12, chain.y + 62, "    -> RMS meter", gfx::MUTED, 1);
    c.text(chain.x + 12, chain.y + 86, "Rust synth -> I2S0 TX", gfx::MUTED, 1);
    c.text(chain.x + 12, chain.y + 100, "    -> ES8389 DAC", gfx::MUTED, 1);
    c.text(chain.x + 12, chain.y + 114, "    -> PA (GPIO7)", gfx::MUTED, 1);
    c.text(chain.x + 12, chain.y + 138, "no external DSP:", gfx::TEXT, 1);
    c.text(chain.x + 12, chain.y + 152, "waveforms are", gfx::TEXT, 1);
    c.text(chain.x + 12, chain.y + 166, "generated on-chip", gfx::TEXT, 1);
}

fn page_audio_dyn(c: &mut Canvas, st: &AppStatus, d: &mut Dirty) {
    let y = CONTENT_Y;
    // source + volume
    {
        let card = Card::at(8, y, 500, 150);
        let r = card.begin_dyn(c, 6, 70, 488, 54);
        let mut tone_buf: heapless::String<24> = heapless::String::new();
        let src: &str = match st.audio_source {
            audio::Source::Silence => "silence",
            audio::Source::Tone(hz) => {
                let _ = write!(tone_buf, "tone {:.0} Hz", hz);
                tone_buf.as_str()
            }
            audio::Source::Chime => "chime",
        };
        let mut s: heapless::String<32> = heapless::String::new();
        let _ = write!(s, "source: {}", src);
        c.text(card.x + 12, card.y + 76, &s, gfx::ACCENT, 1);
        let mut v: heapless::String<32> = heapless::String::new();
        let _ = write!(v, "volume {:+.0} dB", st.volume_db);
        c.text(card.x + 12, card.y + 94, &v, gfx::TEXT, 1);
        c.bar(card.x + 12, card.y + 112, 476, 10, (st.volume_db + 60.0) / 80.0, gfx::OK);
        d.push(r).ok();
    }
    // mic meter
    {
        let y2 = y + 158;
        let card = Card::at(8, y2, 500, 480 - y2 - 8);
        let bx = card.x + 12;
        let bw = 420;
        let by = card.y + 48;
        let bh = card.h - 60;
        let r = card.begin_dyn(c, 6, 40, 488, card.h - 46);
        c.rect(bx, by, bw, bh, gfx::PANEL_HI);
        let filled = (st.mic_level.clamp(0.0, 1.0) * bw as f32) as usize;
        for i in 0..filled {
            let frac = i as f32 / bw as f32;
            let color = if frac < 0.6 {
                gfx::OK
            } else if frac < 0.85 {
                gfx::WARN
            } else {
                gfx::ERR
            };
            c.rect(bx + i, by + 4, 1, bh - 8, color);
        }
        let mut ml: heapless::String<24> = heapless::String::new();
        let _ = write!(ml, "{:3}%", (st.mic_level * 100.0) as u32);
        c.text(bx + bw + 10, by + bh / 2, &ml, gfx::TEXT, 1);
        d.push(r).ok();
    }
}

// ---------------------------------------------------------------- STORAGE

fn page_storage_static(c: &mut Canvas, st: &AppStatus) {
    let y = CONTENT_Y;
    let rep = st.sd.as_ref();
    let info = Card::new(8, y, 784, 140, c, "microSD (SDMMC 4-bit @ 20 MHz, power GPIO39)");
    match rep {
        None => {
            c.rect(info.x + 12, info.y + 36, 10, 10, gfx::ERR);
            c.text(info.x + 28, info.y + 34, "not probed", gfx::TEXT, 1);
        }
        Some(r) => {
            let ok = r.card_ok;
            c.rect(info.x + 12, info.y + 36, 10, 10, if ok { gfx::OK } else { gfx::ERR });
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
            let _ = write!(s2, "{} {}  label \"{}\"", r.partition, r.fs_type, r.volume_label);
            c.text(info.x + 12, info.y + 58, &s2, gfx::MUTED, 1);
        }
    }
    c.text(info.x + 12, info.y + 116, "read-only demo: the card is never written", gfx::MUTED, 1);

    let y2 = y + 148;
    let list = Card::new(8, y2, 380, 480 - y2 - 8, c, "Root directory");
    if let Some(r) = rep {
        if r.entries.is_empty() {
            let mut s: heapless::String<64> = heapless::String::new();
            let _ = write!(s, "{}", r.error.unwrap_or("(empty or unsupported layout)"));
            c.text(list.x + 12, list.y + 34, &s, gfx::MUTED, 1);
        }
        for (i, e) in r.entries.iter().enumerate() {
            let ey = list.y + 34 + i * 20;
            if ey > 460 {
                break;
            }
            c.rect(list.x + 12, ey + 2, 8, 8, if e.is_dir { gfx::SKY } else { gfx::ACCENT });
            let mut s: heapless::String<64> = heapless::String::new();
            if e.is_dir {
                let _ = write!(s, "{} <DIR>", e.name);
            } else {
                let _ = write!(s, "{} {:9} B", e.name, e.size);
            }
            c.text(list.x + 26, ey, &s, gfx::TEXT, 1);
        }
    }

    let txt = Card::new(396, y2, 396, 480 - y2 - 8, c, "First text file");
    if let Some(r) = rep {
        let lines: heapless::Vec<heapless::String<48>, 12> = {
            let mut v = heapless::Vec::new();
            let mut cur = heapless::String::new();
            for ch in r.preview.chars() {
                if ch == '\n' {
                    if !cur.is_empty() {
                        v.push(cur.clone()).ok();
                        cur.clear();
                    }
                } else if cur.push(ch).is_err() {
                    v.push(cur.clone()).ok();
                    cur.clear();
                }
            }
            if !cur.is_empty() {
                v.push(cur).ok();
            }
            v
        };
        for (i, line) in lines.iter().enumerate() {
            let ly = txt.y + 34 + i * 18;
            if ly > 450 {
                break;
            }
            c.text(txt.x + 12, ly, line, gfx::TEXT, 1);
        }
        if r.preview.is_empty() {
            c.text(txt.x + 12, txt.y + 34, "(no .TXT file at root)", gfx::MUTED, 1);
        }
    }
}

// ---------------------------------------------------------------- CAMERA

fn page_camera_static(c: &mut Canvas, st: &AppStatus) {
    let y = CONTENT_Y;
    let cam = Card::new(8, y, 784, 130, c, "DVP camera");
    match st.camera.sensor {
        Some((name, pid)) => {
            c.rect(cam.x + 12, cam.y + 36, 10, 10, gfx::OK);
            let mut s: heapless::String<48> = heapless::String::new();
            let _ = write!(s, "{} detected (PID 0x{:04X})", name, pid);
            c.text(cam.x + 28, cam.y + 34, &s, gfx::TEXT, 1);
        }
        None => {
            c.rect(cam.x + 12, cam.y + 36, 10, 10, gfx::ERR);
            c.text(cam.x + 28, cam.y + 34, "no sensor answered on SCCB", gfx::TEXT, 1);
        }
    }
    c.text(
        cam.x + 12,
        cam.y + 58,
        "8-bit DVP: D0..D7 GPIO46..53, PCLK 54, XCLK 55, VSYNC 56, HREF 57",
        gfx::MUTED,
        1,
    );
    c.text(
        cam.x + 12,
        cam.y + 78,
        "SCCB shares the I2C bus with codec/touch (GPIO0/1)",
        gfx::MUTED,
        1,
    );

    let why = Card::new(8, y + 138, 784, 480 - y - 138 - 8, c, "Why not streaming video?");
    c.text(why.x + 16, why.y + 34, "esp-hal merged the ESP32-S31 DVP (LCD_CAM) driver after v1.2.2, but a", gfx::TEXT, 1);
    c.text(why.x + 16, why.y + 52, "Rust register driver for the OV3660/SC101IOT sensors does not exist yet", gfx::TEXT, 1);
    c.text(why.x + 16, why.y + 70, "(the stock firmware uses ~2k lines of vendor init tables).", gfx::TEXT, 1);
    c.text(why.x + 16, why.y + 94, "Next step: port esp_cam_sensor init tables, then DVP->LCD passthrough", gfx::MUTED, 1);
    c.text(why.x + 16, why.y + 112, "becomes possible with esp_hal::lcd_cam::cam.", gfx::MUTED, 1);
}

// ---------------------------------------------------------------- ABOUT

fn page_about_static(c: &mut Canvas, st: &AppStatus) {
    let y = CONTENT_Y;
    let a = Card::new(8, y, 784, 480 - y - 8, c, "About");
    c.text(a.x + 16, a.y + 34, "ESP32-S31-Korvo-1 + ESP32-S31-WROOM-3 (16M flash / 16M PSRAM)", gfx::TEXT, 1);
    c.text(a.x + 16, a.y + 50, "4.3\" 800x480 RGB LCD (ST7262E43) + GT1151 touch, ES8389 audio,", gfx::TEXT, 1);
    c.text(a.x + 16, a.y + 66, "DVP camera, WS2812 LED, ADC button ladder, microSD, USB 2.0 HS.", gfx::TEXT, 1);
    c.text(a.x + 16, a.y + 94, "100% Rust firmware:", gfx::ACCENT, 1);
    c.text(a.x + 16, a.y + 110, "esp-hal (git main, pre-1.3: S31 I2S/LCD_CAM/USB-HS), esp-rtos,", gfx::MUTED, 1);
    c.text(a.x + 16, a.y + 126, "embassy, esp-bootloader-esp-idf second stage.", gfx::MUTED, 1);
    c.text(a.x + 16, a.y + 154, "Feature status on this build:", gfx::ACCENT, 1);
    let rows: [(&str, &str); 9] = [
        ("RGB LCD + framebuffer", "works - PSRAM ring DMA @ 35 Hz"),
        ("Touch (GT1151)", if st.touch_id.is_some() { "works" } else { "no chip" }),
        ("Audio playback (I2S+ES8389)", if st.codec_ok { "works" } else { "no codec" }),
        ("Mic capture", "works (RMS meter)"),
        (
            "microSD + FAT read",
            if st.sd.as_ref().is_some_and(|s| s.card_ok) {
                "works"
            } else {
                "no card"
            },
        ),
        ("WS2812 LED (RMT)", "works"),
        ("ADC button ladder", "debounced; 0 raw = idle"),
        ("USB HS CDC (Type-A)", "works"),
        ("Wi-Fi/BLE/Thread", "upstream (esp-radio) - not yet"),
    ];
    for (i, (k, v)) in rows.iter().enumerate() {
        let ry = a.y + 172 + i * 18;
        c.text(a.x + 16, ry, k, gfx::TEXT, 1);
        c.text(a.x + 290, ry, v, gfx::MUTED, 1);
    }
    c.text(a.x + 560, a.y + 94, "built with love & cargo", gfx::MUTED, 1);
}

fn page_about_dyn(c: &mut Canvas, st: &AppStatus, d: &mut Dirty) {
    let card = Card::at(8, CONTENT_Y, 784, 480 - CONTENT_Y - 8);
    let r = card.begin_dyn(c, 548, 60, 230, 24);
    let mut up: heapless::String<40> = heapless::String::new();
    let _ = write!(up, "uptime {} s", st.uptime_s);
    c.text(card.x + 552, card.y + 66, &up, gfx::MUTED, 1);
    d.push(r).ok();
}
