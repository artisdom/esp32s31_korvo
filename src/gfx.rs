//! Tiny RGB565 drawing layer for the raw framebuffer.

use crate::font::FONT;

pub type Color = u16;

/// A rectangular region of the framebuffer (used for partial cache cleans).
#[derive(Clone, Copy)]
pub struct Rect {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
}

impl Rect {
    pub fn new(x: usize, y: usize, w: usize, h: usize) -> Self {
        Self { x, y, w, h }
    }
}

pub const fn rgb565(r: u8, g: u8, b: u8) -> Color {
    (((r as u16) & 0xf8) << 8) | (((g as u16) & 0xfc) << 3) | ((b as u16) >> 3)
}

// Palette
pub const BLACK: Color = rgb565(0x00, 0x00, 0x00);
pub const BG: Color = rgb565(0x07, 0x12, 0x22);
pub const PANEL: Color = rgb565(0x10, 0x1f, 0x32);
pub const PANEL_HI: Color = rgb565(0x18, 0x30, 0x4c);
pub const TEXT: Color = rgb565(0xf4, 0xf5, 0xf7);
pub const MUTED: Color = rgb565(0x9e, 0xae, 0xbf);
pub const ACCENT: Color = rgb565(0xff, 0x8a, 0x5b);
pub const SKY: Color = rgb565(0x61, 0xa8, 0xff);
pub const OK: Color = rgb565(0x30, 0xd5, 0x88);
pub const WARN: Color = rgb565(0xff, 0xc0, 0x3b);
pub const ERR: Color = rgb565(0xff, 0x5a, 0x66);
pub const WHITE: Color = rgb565(0xff, 0xff, 0xff);

/// A framebuffer viewport: horizontal slice of the 800x480 panel.
///
/// `offset_x/offset_y` shift everything drawn by a fixed amount with
/// wrap-around: the panel locks to the DPI stream with a random phase on
/// each boot, so rendering is rotated to match (auto-calibrated at boot
/// from a touch on the crosshair - see `main.rs`).
pub struct Canvas<'a> {
    pub pixels: &'a mut [Color],
    pub width: usize,
    pub height: usize,
    pub offset_x: usize,
    pub offset_y: usize,
}

impl<'a> Canvas<'a> {
    pub fn new(pixels: &'a mut [Color], width: usize, height: usize) -> Self {
        assert_eq!(pixels.len(), width * height);
        Self {
            pixels,
            width,
            height,
            offset_x: 0,
            offset_y: 0,
        }
    }

    #[inline]
    pub fn set(&mut self, x: usize, y: usize, c: Color) {
        if x < self.width && y < self.height {
            let xx = (x + self.offset_x) % self.width;
            let yy = (y + self.offset_y) % self.height;
            self.pixels[yy * self.width + xx] = c;
        }
    }

    pub fn fill(&mut self, c: Color) {
        self.pixels.fill(c);
    }

    pub fn rect(&mut self, x: usize, y: usize, w: usize, h: usize, c: Color) {
        if self.offset_x == 0 && self.offset_y == 0 {
            let x0 = x.min(self.width);
            let y0 = y.min(self.height);
            let x1 = (x + w).min(self.width);
            let y1 = (y + h).min(self.height);
            for yy in y0..y1 {
                let row = &mut self.pixels[yy * self.width + x0..yy * self.width + x1];
                row.fill(c);
            }
        } else {
            for yy in y..(y + h).min(self.height) {
                for xx in x..(x + w).min(self.width) {
                    self.set(xx, yy, c);
                }
            }
        }
    }

    pub fn frame(&mut self, x: usize, y: usize, w: usize, h: usize, c: Color) {
        let x1 = x.saturating_sub(1) + w;
        let y1 = y.saturating_sub(1) + h;
        for xx in x..x1 {
            self.set(xx, y, c);
            self.set(xx, y1.saturating_sub(1), c);
        }
        for yy in y..y1 {
            self.set(x, yy, c);
            self.set(x1.saturating_sub(1), yy, c);
        }
    }

    /// 8x8 font glyph scaled by an integer factor.
    pub fn text(&mut self, x: usize, y: usize, s: &str, c: Color, scale: usize) {
        self.text_bg(x, y, s, c, None, scale);
    }

    pub fn text_bg(&mut self, x: usize, y: usize, s: &str, c: Color, bg: Option<Color>, scale: usize) {
        let scale = scale.max(1);
        let mut cx = x;
        for ch in s.chars() {
            if cx + 8 * scale > self.width {
                break;
            }
            let glyph = &FONT[(ch as usize) & 0x7f];
            for (row, bits) in glyph.iter().enumerate() {
                for col in 0..8 {
                    let on = (bits >> col) & 1 == 1;
                    let color = if on { c } else { bg.unwrap_or(c) };
                    if on || bg.is_some() {
                        self.rect(cx + col * scale, y + row * scale, scale, scale, color);
                    }
                }
            }
            cx += 8 * scale;
        }
    }

    pub fn text_width(s: &str, scale: usize) -> usize {
        s.chars().count() * 8 * scale
    }

    /// Filled circle (used for the touch cursor).
    pub fn circle(&mut self, cx: usize, cy: usize, r: usize, c: Color) {
        let r2 = (r * r) as i32;
        for dy in -(r as i32)..=(r as i32) {
            for dx in -(r as i32)..=(r as i32) {
                if dx * dx + dy * dy <= r2 {
                    self.set(
                        (cx as i32 + dx).max(0) as usize,
                        (cy as i32 + dy).max(0) as usize,
                        c,
                    );
                }
            }
        }
    }

    /// XOR-draw a ring (self-inverse: drawing again erases it).
    pub fn xor_ring(&mut self, cx: usize, cy: usize, r: usize) {
        let r2 = (r * r) as i32;
        let inner = ((r - 1) * (r - 1)) as i32;
        for dy in -(r as i32)..=(r as i32) {
            for dx in -(r as i32)..=(r as i32) {
                let d2 = dx * dx + dy * dy;
                if d2 <= r2 && d2 >= inner {
                    let x = (cx as i32 + dx).max(0) as usize;
                    let y = (cy as i32 + dy).max(0) as usize;
                    if x < self.width && y < self.height {
                        let xx = (x + self.offset_x) % self.width;
                        let yy = (y + self.offset_y) % self.height;
                        self.pixels[yy * self.width + xx] ^= 0xFFFF;
                    }
                }
            }
        }
    }

    /// Horizontal level bar.
    pub fn bar(&mut self, x: usize, y: usize, w: usize, h: usize, frac: f32, c: Color) {
        self.rect(x, y, w, h, PANEL_HI);
        let filled = ((frac.clamp(0.0, 1.0)) * w as f32) as usize;
        if filled > 0 {
            self.rect(x, y, filled.min(w), h, c);
        }
    }
}
