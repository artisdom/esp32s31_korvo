//! Host regression checks: rustc --edition=2024 --test tests/canvas.rs -o target/canvas-tests
#![allow(dead_code)]

#[path = "../src/font.rs"]
mod font;
#[path = "../src/gfx.rs"]
mod gfx;

#[test]
fn title_stays_in_the_header() {
    let mut pixels = vec![gfx::BLACK; 800 * 480];
    let mut canvas = gfx::Canvas::new(&mut pixels, 800, 480);
    canvas.text(12, 12, "ESP32-S31-KORVO", gfx::WHITE, 2);
    assert!(pixels[12 * 800..28 * 800].iter().any(|&p| p == gfx::WHITE));
    assert!(pixels[..12 * 800].iter().all(|&p| p == gfx::BLACK));
    assert!(pixels[28 * 800..].iter().all(|&p| p == gfx::BLACK));
}

#[test]
fn drawing_at_an_edge_clips_without_wrapping() {
    let mut pixels = vec![gfx::BLACK; 800 * 480];
    let mut canvas = gfx::Canvas::new(&mut pixels, 800, 480);
    canvas.rect(798, 478, 8, 8, gfx::WHITE);
    canvas.set(800, 0, gfx::WHITE);
    canvas.set(0, 480, gfx::WHITE);
    assert_eq!(pixels.iter().filter(|&&p| p == gfx::WHITE).count(), 4);
    assert!(pixels[..478 * 800].iter().all(|&p| p == gfx::BLACK));
    assert_eq!(pixels[478 * 800 + 798], gfx::WHITE);
    assert_eq!(pixels[479 * 800 + 799], gfx::WHITE);
}
