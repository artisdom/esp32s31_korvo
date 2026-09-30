//! LCD pages. All pages share a header + tab bar; MODE/VOL buttons and the
//! touch panel both navigate.

use core::fmt::Write as _;

use crate::gfx::{self, Canvas, Color};
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
}

const HEADER_H: usize = 44;
const TABS_H: usize = 36;

pub fn draw(c: &mut Canvas, st: &AppStatus) {
    c.fill(gfx::BG);
    draw_header(c, st);
    draw_tabs(c, st);
    match st.page {
        Page::Home => page_home(c, st),
        Page::Audio => page_audio(c, st),
        Page::Storage => page_storage(c, st),
        Page::Camera => page_camera(c, st),
        Page::About => page_about(c, st),
    }
    if let Some((x, y)) = st.touch_point {
        c.circle(x as usize, y as usize, 9, gfx::ACCENT);
        c.circle(x as usize, y as usize, 4, gfx::WHITE);
    }
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

fn draw_header(c: &mut Canvas, st: &AppStatus) {
    c.rect(0, 0, 800, HEADER_H, gfx::PANEL);
    c.rect(0, HEADER_H - 2, 800, 2, gfx::ACCENT);
    c.text(12, 12, "ESP32-S31-KORVO", gfx::TEXT, 2);
    c.text(276, 16, "Rust demo", gfx::MUTED, 1);

    // status LEDs in the header
    let mut x = 470;
    for (label, on, color) in [
        ("LCD", true, gfx::OK),
        ("I2S", st.codec_ok, if st.codec_ok { gfx::OK } else { gfx::ERR }),
        ("SD", st.sd.as_ref().is_some_and(|s| s.card_ok), gfx::OK),
        ("USB", st.usb_connected, gfx::SKY),
    ] {
        c.rect(x, 14, 10, 10, if on { color } else { gfx::PANEL_HI });
        c.text(x + 14, 16, label, gfx::MUTED, 1);
        x += 70;
    }
    c.text(760, 16, "fps", gfx::MUTED, 1);
    let mut fps: heapless::String<8> = heapless::String::new();
    let _ = write!(fps, "{}", st.fps);
    c.text_bg(790, 16, &fps, gfx::TEXT, None, 1);
}

fn draw_tabs(c: &mut Canvas, st: &AppStatus) {
    let y = HEADER_H;
    let w = 800 / PAGES.len();
    for (i, (page, label)) in PAGES.iter().enumerate() {
        let x = i * w;
        let active = *page == st.page;
        c.rect(x, y, w - 2, TABS_H, if active { gfx::PANEL_HI } else { gfx::PANEL });
        let tw = Canvas::text_width(label, 2);
        c.text(x + (w - tw) / 2, y + 10, label, if active { gfx::ACCENT } else { gfx::MUTED }, 2);
    }
    c.rect(0, y + TABS_H, 800, 2, gfx::PANEL_HI);
}

fn card(c: &mut Canvas, x: usize, y: usize, w: usize, h: usize, title: &str) {
    c.rect(x, y, w, h, gfx::PANEL);
    c.frame(x, y, w, h, gfx::PANEL_HI);
    c.text(x + 10, y + 8, title, gfx::SKY, 1);
}

fn status_dot(c: &mut Canvas, x: usize, y: usize, ok: bool) -> Color {
    let color = if ok { gfx::OK } else { gfx::ERR };
    c.rect(x, y, 10, 10, color);
    color
}

const CONTENT_Y: usize = HEADER_H + TABS_H + 8;

fn page_home(c: &mut Canvas, st: &AppStatus) {
    use core::fmt::Write;
    let cw = 258;
    let ch = 180;
    let gap = 8;

    // Row 1
    let y0 = CONTENT_Y;
    card(c, 8, y0, cw, ch, "SoC");
    let mut l1: heapless::String<64> = heapless::String::new();
    let _ = write!(l1, "dual-core RISC-V");
    c.text(20, y0 + 34, "ESP32-S31", gfx::TEXT, 1);
    c.text(20, y0 + 50, &l1, gfx::TEXT, 1);
    let mut l2: heapless::String<64> = heapless::String::new();
    let _ = write!(l2, "up {}s", st.uptime_s);
    c.text(20, y0 + 66, &l2, gfx::MUTED, 1);
    let mut l3: heapless::String<64> = heapless::String::new();
    let _ = write!(l3, "loop {} fps", st.fps);
    c.text(20, y0 + 82, &l3, gfx::MUTED, 1);
    c.text(20, y0 + 108, "Wi-Fi 6 / BT 5.4 / 802.15.4", gfx::MUTED, 1);
    c.text(20, y0 + 124, "(radio: upstream esp-radio)", gfx::MUTED, 1);

    card(c, 8 + cw + gap, y0, cw, ch, "Memory");
    let mut s: heapless::String<64> = heapless::String::new();
    let _ = write!(s, "PSRAM {:3} MB free/{} MB", st.psram_free / (1024 * 1024), st.psram_total / (1024 * 1024));
    c.text(20, y0 + 34, &s, gfx::TEXT, 1);
    let mut s2: heapless::String<64> = heapless::String::new();
    let _ = write!(s2, "heap  {} kB free", st.heap_free / 1024);
    c.text(20, y0 + 50, &s2, gfx::TEXT, 1);
    c.text(20, y0 + 72, "16 MB flash (QIO)", gfx::MUTED, 1);
    c.text(20, y0 + 88, "16 MB PSRAM (hex 250 MHz)", gfx::MUTED, 1);
    c.text(20, y0 + 110, "framebuffer: PSRAM DMA", gfx::MUTED, 1);
    c.text(20, y0 + 126, "ring descriptors: DRAM", gfx::MUTED, 1);

    card(c, 8 + 2 * (cw + gap), y0, cw, ch, "Console + touch");
    status_dot(c, 20, y0 + 36, st.touch_id.is_some());
    c.text(36, y0 + 34, "GT1151 touch", gfx::TEXT, 1);
    if let Some(id) = st.touch_id {
        let mut s: heapless::String<32> = heapless::String::new();
        let _ = write!(s, "id \"{}\"", core::str::from_utf8(&id).unwrap_or("????"));
        c.text(20, y0 + 50, &s, gfx::MUTED, 1);
    }
    c.text(20, y0 + 72, "UART0 log: 921600 8N1", gfx::MUTED, 1);
    c.text(20, y0 + 88, "USB-C port (FT232R)", gfx::MUTED, 1);
    c.text(20, y0 + 110, "touch the tabs or cards", gfx::MUTED, 1);
    c.text(20, y0 + 126, "to navigate", gfx::MUTED, 1);

    // Row 2
    let y1 = y0 + ch + gap;
    let ch2 = 480 - y1 - 8;
    card(c, 8, y1, cw, ch2, "Buttons (ADC ladder)");
    let mut s: heapless::String<32> = heapless::String::new();
    let _ = write!(s, "GPIO42: {} mV", st.btn_mv);
    c.text(20, y1 + 34, &s, gfx::TEXT, 1);
    c.bar(20, y1 + 48, cw - 40, 10, st.btn_mv as f32 / 2000.0, gfx::SKY);
    let names = ["VOL+  380mV", "VOL-  820mV", "MODE 1340mV", "SET  1870mV"];
    for (i, n) in names.iter().enumerate() {
        let held = st.btn_held == Some([Button::VolUp, Button::VolDown, Button::Mode, Button::Set][i]);
        c.rect(20, y1 + 70 + i * 18, 10, 10, if held { gfx::ACCENT } else { gfx::PANEL_HI });
        c.text(36, y1 + 68 + i * 18, n, if held { gfx::TEXT } else { gfx::MUTED }, 1);
    }

    card(c, 8 + cw + gap, y1, cw, ch2, "USB HS (Type-A)");
    status_dot(c, 20, y1 + 34, st.usb_connected);
    c.text(
        36,
        y1 + 32,
        if st.usb_connected { "CDC-ACM enumerated" } else { "not connected" },
        gfx::TEXT,
        1,
    );
    let mut s: heapless::String<48> = heapless::String::new();
    let _ = write!(s, "rx {} B  tx {} B", st.usb_rx, st.usb_tx);
    c.text(20, y1 + 54, &s, gfx::MUTED, 1);
    let last = crate::usb::USB_LAST_RX.read();
    let mut lbl: heapless::String<64> = heapless::String::new();
    let _ = write!(lbl, "last: {}", last.as_str());
    c.text(20, y1 + 72, &lbl, gfx::MUTED, 1);
    c.text(20, y1 + 96, "screen - /dev/ttyACM0", gfx::MUTED, 1);
    c.text(20, y1 + 112, "115200 baud, send ? for info", gfx::MUTED, 1);

    card(c, 8 + 2 * (cw + gap), y1, cw, ch2, "Status LED (WS2812)");
    c.text(20, y1 + 34, "GPIO37, RMT-driven", gfx::MUTED, 1);
    let led_color = if st.flash_led { gfx::ACCENT } else { gfx::PANEL_HI };
    c.circle(40, y1 + 90, 22, led_color);
    c.circle(40, y1 + 90, 14, if st.flash_led { gfx::WARN } else { gfx::PANEL });
    c.text(76, y1 + 66, "breathing cyan:", gfx::TEXT, 1);
    c.text(76, y1 + 80, "system alive;", gfx::MUTED, 1);
    c.text(76, y1 + 94, "orange blink:", gfx::TEXT, 1);
    c.text(76, y1 + 108, "button event", gfx::MUTED, 1);
}

fn page_audio(c: &mut Canvas, st: &AppStatus) {
    let y = CONTENT_Y;
    card(c, 8, y, 500, 150, "Speaker (ES8389 + 2x NS4150B, 3 W)");
    status_dot(c, 20, y + 36, st.codec_ok);
    let mut s: heapless::String<48> = heapless::String::new();
    if st.codec_ok {
        let _ = write!(s, "codec id 0x{:02X}:0x{:02X}", st.codec_id.0, st.codec_id.1);
    } else {
        s.push_str("not detected").ok();
    }
    c.text(36, y + 34, &s, gfx::TEXT, 1);
    c.text(20, y + 58, "I2S0 48 kHz / 16-bit / stereo, DMA stream", gfx::MUTED, 1);

    let mut tone_buf: heapless::String<24> = heapless::String::new();
    let src: &str = match st.audio_source {
        audio::Source::Silence => "silence",
        audio::Source::Tone(hz) => {
            let _ = write!(tone_buf, "tone {:.0} Hz", hz);
            tone_buf.as_str()
        }
        audio::Source::Chime => "chime",
    };
    let mut srcl: heapless::String<32> = heapless::String::new();
    let _ = write!(srcl, "source: {}", src);
    c.text(20, y + 78, &srcl, gfx::ACCENT, 1);

    let mut v: heapless::String<32> = heapless::String::new();
    let _ = write!(v, "volume {:+.0} dB", st.volume_db);
    c.text(20, y + 98, &v, gfx::TEXT, 1);
    c.bar(20, y + 114, 460, 10, (st.volume_db + 60.0) / 72.0, gfx::OK);
    c.text(20, y + 130, "SET = chime   MODE = 440 Hz tone on/off   VOL+- = volume", gfx::MUTED, 1);

    card(c, 8, y + 158, 500, 480 - y - 158 - 8, "Microphones (2x analog, ADC loop)");
    c.text(20, y + 192, "speak into the left mic ->", gfx::MUTED, 1);
    // VU meter, vertical
    let bx = 40;
    let bw = 380;
    let by = y + 210;
    let bh = 480 - by - 20;
    c.rect(bx, by, bw, bh, gfx::PANEL_HI);
    let filled = (st.mic_level.clamp(0.0, 1.0) * bw as f32) as usize;
    let grad = st.mic_level.clamp(0.0, 1.0);
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
    let _ = write!(ml, "{:3}%", (grad * 100.0) as u32);
    c.text(bx + bw + 12, by + bh / 2, &ml, gfx::TEXT, 1);

    // right column
    card(c, 516, y, 276, 480 - y - 8, "Signal chain");
    c.text(530, y + 34, "mic -> ES8389 ADC", gfx::MUTED, 1);
    c.text(530, y + 48, "    -> I2S0 RX DMA", gfx::MUTED, 1);
    c.text(530, y + 62, "    -> RMS meter", gfx::MUTED, 1);
    c.text(530, y + 86, "Rust synth -> I2S0 TX", gfx::MUTED, 1);
    c.text(530, y + 100, "    -> ES8389 DAC", gfx::MUTED, 1);
    c.text(530, y + 114, "    -> PA (GPIO7)", gfx::MUTED, 1);
    c.text(530, y + 138, "no external DSP:", gfx::TEXT, 1);
    c.text(530, y + 152, "waveforms are", gfx::TEXT, 1);
    c.text(530, y + 166, "generated on-chip", gfx::TEXT, 1);
}

fn page_storage(c: &mut Canvas, st: &AppStatus) {
    let y = CONTENT_Y;
    let rep = st.sd.as_ref();
    card(c, 8, y, 784, 140, "microSD (SDMMC 4-bit @ 20 MHz, power GPIO39)");
    match rep {
        None => {
            status_dot(c, 20, y + 36, false);
            c.text(36, y + 34, "not probed", gfx::TEXT, 1);
        }
        Some(r) => {
            status_dot(c, 20, y + 36, r.card_ok);
            let mut s: heapless::String<64> = heapless::String::new();
            if r.card_ok {
                if r.capacity_mb >= 1000 {
                    let _ = write!(s, "{:.1} GB  bus {} MHz  mid 0x{:02X}", r.capacity_mb as f32 / 1024.0, r.freq_khz / 1000, r.manuf_id);
                } else {
                    let _ = write!(s, "{} MB  bus {} MHz  mid 0x{:02X}", r.capacity_mb, r.freq_khz / 1000, r.manuf_id);
                }
            } else {
                let _ = write!(s, "card error: {}", r.error.unwrap_or("?"));
            }
            c.text(36, y + 34, &s, gfx::TEXT, 1);
            let mut s2: heapless::String<64> = heapless::String::new();
            let _ = write!(s2, "{} {} \"{}\"", r.partition, r.fs_type, r.volume_label);
            c.text(20, y + 56, &s2, gfx::MUTED, 1);
        }
    }
    c.text(20, y + 116, "read-only demo: the card is never written", gfx::MUTED, 1);

    let y2 = y + 148;
    card(c, 8, y2, 380, 480 - y2 - 8, "Root directory");
    if let Some(r) = rep {
        if r.entries.is_empty() {
            let mut s: heapless::String<64> = heapless::String::new();
            let _ = write!(s, "{}", r.error.unwrap_or("(empty or unsupported layout)"));
            c.text(20, y2 + 34, &s, gfx::MUTED, 1);
        }
        for (i, e) in r.entries.iter().enumerate() {
            let ey = y2 + 34 + i * 20;
            if ey > 460 {
                break;
            }
            c.rect(20, ey + 2, 8, 8, if e.is_dir { gfx::SKY } else { gfx::ACCENT });
            let mut s: heapless::String<64> = heapless::String::new();
            if e.is_dir {
                let _ = write!(s, "{} <DIR>", e.name);
            } else {
                let _ = write!(s, "{} {:9} B", e.name, e.size);
            }
            c.text(34, ey, &s, gfx::TEXT, 1);
        }
    }

    card(c, 396, y2, 396, 480 - y2 - 8, "First text file");
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
            let ly = y2 + 34 + i * 18;
            if ly > 450 {
                break;
            }
            c.text(410, ly, line, gfx::TEXT, 1);
        }
        if r.preview.is_empty() {
            c.text(410, y2 + 34, "(no .TXT file at root)", gfx::MUTED, 1);
        }
    }
}

fn page_camera(c: &mut Canvas, st: &AppStatus) {
    let y = CONTENT_Y;
    card(c, 8, y, 784, 130, "DVP camera");
    match st.camera.sensor {
        Some((name, pid)) => {
            status_dot(c, 20, y + 36, true);
            let mut s: heapless::String<48> = heapless::String::new();
            let _ = write!(s, "{} detected (PID 0x{:04X})", name, pid);
            c.text(36, y + 34, &s, gfx::TEXT, 1);
        }
        None => {
            status_dot(c, 20, y + 36, false);
            c.text(36, y + 34, "no sensor answered on SCCB", gfx::TEXT, 1);
        }
    }
    c.text(20, y + 58, "8-bit DVP: D0..D7 GPIO46..53, PCLK 54, XCLK 55, VSYNC 56, HREF 57", gfx::MUTED, 1);
    c.text(20, y + 78, "SCCB shares the I2C bus with codec/touch (GPIO0/1)", gfx::MUTED, 1);

    card(c, 8, y + 138, 784, 480 - y - 138 - 8, "Why not streaming video?");
    c.text(24, y + 172, "esp-hal merged the ESP32-S31 DVP (LCD_CAM) driver after v1.2.2, but a", gfx::TEXT, 1);
    c.text(24, y + 190, "Rust register driver for the OV3660/SC101IOT sensors does not exist yet", gfx::TEXT, 1);
    c.text(24, y + 208, "(the stock firmware uses ~2k lines of vendor init tables).", gfx::TEXT, 1);
    c.text(24, y + 232, "Next step: port esp_cam_sensor init tables, then DVP->LCD passthrough", gfx::MUTED, 1);
    c.text(24, y + 250, "becomes possible with esp_hal::lcd_cam::cam.", gfx::MUTED, 1);
}

fn page_about(c: &mut Canvas, st: &AppStatus) {
    let y = CONTENT_Y;
    card(c, 8, y, 784, 480 - y - 8, "About");
    c.text(24, y + 34, "ESP32-S31-Korvo-1 + ESP32-S31-WROOM-3 (16M flash / 16M PSRAM)", gfx::TEXT, 1);
    c.text(24, y + 50, "4.3\" 800x480 RGB LCD + GT1151 touch, ES8389 audio, DVP camera,", gfx::TEXT, 1);
    c.text(24, y + 66, "WS2812 LED, ADC button ladder, microSD, USB 2.0 HS.", gfx::TEXT, 1);

    c.text(24, y + 94, "100% Rust firmware:", gfx::ACCENT, 1);
    c.text(24, y + 110, "esp-hal (git main, pre-1.3: S31 I2S/LCD_CAM/USB-HS), esp-rtos,", gfx::MUTED, 1);
    c.text(24, y + 126, "embassy, esp-bootloader-esp-idf second stage.", gfx::MUTED, 1);

    c.text(24, y + 154, "Feature status on this build:", gfx::ACCENT, 1);
    let rows: [(&str, &str); 9] = [
        ("RGB LCD + framebuffer", "works - PSRAM ring DMA @ 60 Hz"),
        ("Touch (GT1151)", if st.touch_id.is_some() { "works" } else { "no chip" }),
        ("Audio playback (I2S+ES8389)", if st.codec_ok { "works" } else { "no codec" }),
        ("Mic capture", "works (RMS meter)"),
        ("microSD + FAT read", if st.sd.as_ref().is_some_and(|s| s.card_ok) { "works" } else { "no card" }),
        ("WS2812 LED (RMT)", "works"),
        ("ADC button ladder", "works"),
        ("USB HS CDC (Type-A)", "works"),
        ("Wi-Fi/BLE/Thread", "upstream (esp-radio) - not yet"),
    ];
    for (i, (k, v)) in rows.iter().enumerate() {
        let ry = y + 172 + i * 18;
        c.text(24, ry, k, gfx::TEXT, 1);
        c.text(300, ry, v, gfx::MUTED, 1);
    }
    let mut up: heapless::String<40> = heapless::String::new();
    let _ = write!(up, "uptime {} s", st.uptime_s);
    c.text(640, y + 34, &up, gfx::MUTED, 1);
    c.text(560, y + 66, "built with love & cargo", gfx::MUTED, 1);
}
