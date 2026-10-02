//! # ESP32-S31-Korvo-1 all-features Rust demo
//!
//! Drives every peripheral on the board:
//! 4.3" RGB LCD + GT1151 touch, ES8389 codec (speaker + mics) over I2S,
//! WS2812 LED (RMT), ADC button ladder, microSD (SDMMC), DVP camera SCCB
//! probe, USB 2.0 HS CDC-ACM on the Type-A port, PSRAM heap, dual core.
//!
//! Core 0: hardware bring-up + UI/event loop. Core 1: USB device task.

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU32, Ordering};

use crate::{audio::Source, buttons::Button, ui::Page};
use core::fmt::Write as _;
use embassy_time::{Duration, Timer};
use esp_backtrace as _;
use esp_hal::{
    gpio::{Level, Output, OutputConfig},
    i2c::master::{Config as I2cConfig, I2c},
    system::Stack,
    time::Instant,
    timer::timg::TimerGroup,
};
use esp_println::println;
use static_cell::StaticCell;

mod audio;
mod audio_ring;
mod board;
mod button_logic;
mod buttons;
mod camera;
mod console;
mod display;
mod es8389;
mod fat_layout;
mod font;
mod gfx;
mod led;
mod media;
mod pcm;
mod sdcard;
mod storage;
mod touch;
mod ui;
mod usb;

esp_bootloader_esp_idf::esp_app_desc!();

/// Main-loop heartbeat, readable from the USB task on core 1.
static LOOP_COUNT: AtomicU32 = AtomicU32::new(0);

static BOOT_T0: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
fn step(msg: &str) {
    let t0 = BOOT_T0.load(Ordering::Relaxed);
    let now = esp_hal::time::Instant::now().elapsed().as_millis() as u32;
    // t0 is captured on first call
    let base = if t0 == 0 {
        BOOT_T0.store(now, Ordering::Relaxed);
        now
    } else {
        t0
    };
    println!("[{:>5} ms] {}", now - base, msg);
}

#[esp_hal::main]
async fn main(_spawner: embassy_executor::Spawner) {
    let peripherals = esp_hal::init(esp_hal::Config::default());

    step("== ESP32-S31-Korvo-1 Rust demo ==");
    println!("chip: {}", esp_hal::chip!());

    // --- heaps: internal DRAM + PSRAM regions --------------------------------
    esp_alloc::heap_allocator!(size: 48 * 1024);
    let psram =
        esp_hal::psram::Psram::new(peripherals.PSRAM, esp_hal::psram::PsramConfig::default());
    let (_psram_start, psram_size) = psram.raw_parts();
    unsafe {
        esp_alloc::HEAP.add_region(esp_alloc::HeapRegion::new(
            _psram_start,
            psram_size,
            esp_alloc::MemoryCapability::External.into(),
        ));
    }
    step("heap ready");

    // --- timers / RTOS ----------------------------------------------------------
    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0);
    step("rtos ok");
    // --- RGB LCD + PSRAM framebuffer ---------------------------------------------------
    let fb: &'static mut [u8] = alloc_fb().expect("framebuffer alloc");
    let lcd_pins = display::LcdPins {
        vsync: peripherals.GPIO45,
        hsync: peripherals.GPIO44,
        de: peripherals.GPIO43,
        pclk: peripherals.GPIO40,
        d0: peripherals.GPIO8,
        d1: peripherals.GPIO9,
        d2: peripherals.GPIO10,
        d3: peripherals.GPIO11,
        d4: peripherals.GPIO12,
        d5: peripherals.GPIO13,
        d6: peripherals.GPIO14,
        d7: peripherals.GPIO15,
        d8: peripherals.GPIO16,
        d9: peripherals.GPIO17,
        d10: peripherals.GPIO18,
        d11: peripherals.GPIO19,
        d12: peripherals.GPIO33,
        d13: peripherals.GPIO34,
        d14: peripherals.GPIO35,
        d15: peripherals.GPIO36,
    };
    let lcd_cam = esp_hal::lcd_cam::LcdCam::new(peripherals.LCD_CAM);
    // The camera's master clock comes out of the same peripheral. Start it
    // before anything talks to the sensor: without XCLK the SCCB probe is
    // silent (see camera.rs).
    let camera = camera::start_xclk(lcd_cam.cam);
    step(if camera.is_ok() {
        "camera XCLK 20 MHz on GPIO55"
    } else {
        "camera XCLK failed"
    });
    let mut display =
        match display::Display::new(lcd_cam.lcd, peripherals.DMA_AXI_CH0, fb, lcd_pins) {
            Ok(d) => {
                step("LCD scanning");
                d
            }
            Err(e) => panic!("display init failed: {}", e),
        };

    {
        let mut c = display.canvas();
        c.fill(gfx::WHITE);
        c.text(300, 230, "BOOTING...", gfx::BLACK, 2);
    }
    display.flush();
    step("splash shown");

    // --- disabled WS2812 -------------------------------------------------
    step("rmt");
    let rmt = esp_hal::rmt::Rmt::new(peripherals.RMT, esp_hal::time::Rate::from_mhz(10))
        .expect("RMT clock");
    println!(
        "WS2812: configured RMT counter {} Hz",
        rmt.frequency().as_hz()
    );
    let mut led = led::StatusLed::new(rmt, peripherals.GPIO37).expect("LED channel");
    step("led ok");
    // Latch off once, including any colour left from the previous firmware.
    led.send(0, 0, 0);
    step("status LED disabled");

    // --- shared I2C bus (codec + touch + camera SCCB) ------------------------------
    let mut i2c = I2c::new(
        peripherals.I2C0,
        I2cConfig::default().with_frequency(esp_hal::time::Rate::from_hz(400_000)),
    )
    .expect("I2C0")
    .with_sda(peripherals.GPIO0)
    .with_scl(peripherals.GPIO1);
    step("i2c ok");
    {
        // Bus scan for bring-up diagnostics.
        let mut found = heapless::String::<96>::new();
        for addr in 1u8..0x7f {
            let mut scratch = [0u8; 1];
            if i2c.write_read(addr, &[0x00], &mut scratch).is_ok() {
                let _ = core::write!(found, " 0x{:02x}", addr);
            }
        }
        println!("i2c scan:{}", found.as_str());
    }
    static I2C_BUS: StaticCell<board::SharedI2c> = StaticCell::new();
    let i2c: &'static board::SharedI2c =
        I2C_BUS.init(board::SharedI2c::new(core::cell::RefCell::new(i2c)));

    // --- audio codec -----------------------------------------------------------------
    let pa_pin = Output::new(peripherals.GPIO7, Level::Low, OutputConfig::default());
    let mut codec = es8389::Es8389::new(i2c, Some(pa_pin));
    let codec_ok = codec.ping();
    step(if codec_ok {
        "codec detected"
    } else {
        "codec MISSING"
    });

    // --- I2S (builds on I2S0 with DMA_CH0; MCLK on GPIO2) -----------------------------
    let i2s =
        audio::build_i2s(peripherals.I2S0, peripherals.DMA_CH0, peripherals.GPIO2).expect("I2S0");
    let mut audio = {
        let pins = audio::AudioPins {
            bclk: peripherals.GPIO3,
            ws: peripherals.GPIO4,
            dout: peripherals.GPIO5,
            din: peripherals.GPIO6,
        };
        match audio::Audio::new(i2s, pins) {
            Ok(a) => {
                step("i2s streaming");
                a
            }
            Err(e) => panic!("audio init failed: {}", e),
        }
    };
    let codec_id = if codec_ok {
        match codec.init_48k() {
            Ok(()) => {
                let _ = codec.set_volume_db(-30.0);
                let _ = codec.set_mic_gain(9);
                let _ = codec.set_adc_mute(false);
                println!("ES8389 initialized: 48 kHz slave, SCLK-derived clocks");
                codec.chip_id().unwrap_or((0, 0))
            }
            Err(()) => {
                println!("ES8389 register init FAILED");
                (0, 0)
            }
        }
    } else {
        (0, 0)
    };

    // Audio starts silent; playback is always requested by a control.

    // --- touch ---------------------------------------------------------------------------
    let mut touch = touch::Touch::probe(i2c);
    match &touch {
        Some(t) => println!(
            "GT1151 touch ok (id \"{}\")",
            core::str::from_utf8(&t.product_id).unwrap_or("????")
        ),
        None => println!("GT1151 touch NOT detected (LCD subboard absent?)"),
    }

    // --- camera probe ----------------------------------------------------------------------
    // The sensor needs its master clock to settle before it answers SCCB
    // (20 ms for OV3660, 100 ms for SC101IOT - the vendor BSP waits 100 ms).
    Timer::after(Duration::from_millis(120)).await;
    let mut cam_probe = camera::probe(i2c);
    cam_probe.xclk = camera.is_ok();
    match cam_probe.sensor {
        Some((name, pid)) => println!("camera: {} PID=0x{:04x}", name, pid),
        None => println!(
            "camera: no sensor answered on SCCB ({} acked, XCLK {})",
            cam_probe.acked,
            if cam_probe.xclk { "on" } else { "off" }
        ),
    }

    // --- microSD -----------------------------------------------------------------------------
    let sd_power = Output::new(peripherals.GPIO39, Level::High, OutputConfig::default());
    let sd_pins = sdcard::SdPins {
        clk: peripherals.GPIO24,
        cmd: peripherals.GPIO25,
        d0: peripherals.GPIO20,
        d1: peripherals.GPIO21,
        d2: peripherals.GPIO22,
        d3: peripherals.GPIO23,
    };
    step("mounting microSD...");
    let (sd, card_device) = sdcard::mount_and_inspect(peripherals.SDHOST, sd_pins, sd_power).await;
    if sd.card_ok {
        println!(
            "microSD ok: {} MB, {} {}, label \"{}\"",
            sd.capacity_mb, sd.partition, sd.fs_type, sd.volume_label
        );
    } else {
        println!("microSD: {}", sd.error.unwrap_or("unknown failure"));
    }

    let storage = card_device.and_then(|d| match storage::Storage::new(d) {
        Ok(s) => Some(s),
        Err(e) => {
            println!("media mount: {}", e);
            None
        }
    });
    let mut media = media::Media::new(storage);

    // --- buttons -------------------------------------------------------------------------------
    let mut buttons = buttons::Buttons::new(peripherals.ADC1, peripherals.GPIO42);
    step("buttons ok");

    // --- USB CDC task on core 1 -------------------------------------------------------------------
    static CORE1_STACK: StaticCell<Stack<8192>> = StaticCell::new();
    let stack = CORE1_STACK.init(Stack::new());
    let usb_hs = peripherals.USB_HS;
    esp_rtos::start_second_core(peripherals.CPU_CTRL, stack, move || {
        static EXECUTOR: StaticCell<esp_rtos::embassy::Executor> = StaticCell::new();
        let executor = EXECUTOR.init(esp_rtos::embassy::Executor::new());
        executor.run(|spawner| {
            spawner.spawn(usb::usb_task(usb_hs).expect("spawn usb"));
        });
    });
    step("core1 usb task");

    // --- app state ----------------------------------------------------------------------------------
    let mut st = ui::AppStatus {
        page: Page::Home,
        psram_total: psram_size,
        psram_free: 0,
        heap_free: 0,
        codec_ok,
        codec_id,
        touch_id: touch.as_ref().map(|t| t.product_id),
        camera: cam_probe,
        usb_connected: false,
        usb_rx: 0,
        usb_tx: 0,
        sd: Some(sd),
        btn_mv: 0,
        btn_held: None,
        mic_level: 0.0,
        volume_db: -30.0,
        audio_source: audio.source(),
        media_name: heapless::String::new(),
        media_status: heapless::String::new(),
        media_count: 0,
        media_seconds: 0,
        recording: false,
        uptime_s: 0,
        fps: 0,
        touch_point: None,
        led_rgb: (0, 0, 0),
    };

    let start = Instant::now();
    let mut touch_seen = 0u32;
    // Replace the splash with a full page before updating dynamic widgets.
    let mut shown_page = if st.page == Page::Home {
        Page::Audio
    } else {
        Page::Home
    };
    let mut dyn_tick = Instant::now();
    let mut frame = 0u32;
    let mut fps_window = Instant::now();
    let mut fps = 0u32;
    let mut previous_held = None;
    let mut previous_touch = false;
    let mut console = console::Console::new();

    loop {
        LOOP_COUNT.fetch_add(1, Ordering::Relaxed);

        if let Some(cmd) = console.poll() {
            media.command(cmd, &mut audio);
            st.page = Page::Audio;
        }

        // ---- buttons ----
        if let Some(btn) = buttons.poll() {
            match btn {
                Button::Mode => {
                    st.page = st.page.next();
                }
                Button::Set => {
                    if st.page == Page::Audio {
                        media.command(media::Command::Record, &mut audio);
                    } else {
                        media.command(media::Command::Stop, &mut audio);
                        audio.set_source(Source::Chime);
                    }
                }
                Button::VolUp => {
                    st.volume_db = (st.volume_db + 3.0).min(20.0);
                    let _ = codec.set_volume_db(st.volume_db);
                }
                Button::VolDown => {
                    st.volume_db = (st.volume_db - 3.0).max(-60.0);
                    let _ = codec.set_volume_db(st.volume_db);
                }
            }
        }
        st.btn_mv = buttons.last_mv;
        st.btn_held = buttons.held();
        if st.btn_held != previous_held {
            println!(
                "button: {} raw={} mv={}",
                st.btn_held.map(Button::label).unwrap_or("released"),
                buttons.last_raw,
                st.btn_mv
            );
            previous_held = st.btn_held;
        }
        // ---- touch ----
        // Touch, rendering and dirty regions all use screen coordinates.
        if let Some(t) = touch.as_mut() {
            st.touch_point = t.poll().map(|p| (p.x, p.y));
            if st.touch_point.is_some() && touch_seen < 3 {
                touch_seen += 1;
                println!("touch: {:?}", st.touch_point);
            }
            if let Some((x, y)) = st.touch_point {
                if !previous_touch {
                    if let Some(page) = ui::hit_tabs(x, y) {
                        st.page = page;
                    } else if st.page == Page::Audio {
                        if let Some(cmd) = ui::hit_audio(x, y) {
                            media.command(cmd, &mut audio);
                        }
                    }
                }
            }
        }

        previous_touch = st.touch_point.is_some();

        // ---- audio streaming + SD playback / recording ----
        media.capture(audio.poll());
        media.poll(&mut audio);
        st.media_name.clear();
        let _ = st.media_name.push_str("Selected: ");
        let _ = st.media_name.push_str(media.selected_name());
        st.media_status = media.status.clone();
        st.media_count = media.tracks.len();
        st.media_seconds = media.seconds;
        st.recording = media.recording();
        st.mic_level = audio.mic_level;
        st.audio_source = audio.source();

        // ---- status ----
        st.uptime_s = start.elapsed().as_secs();
        st.usb_connected = usb::CONNECTED.load(Ordering::Relaxed);
        st.usb_rx = usb::RX_BYTES.load(Ordering::Relaxed);
        st.usb_tx = usb::TX_BYTES.load(Ordering::Relaxed);
        st.psram_free = esp_alloc::HEAP.free();
        st.heap_free = st.psram_free; // single heap: regions combined

        // ---- redraw ----
        // Full redraw only when the page changes; otherwise repaint just
        // the dynamic widgets and cache-clean those regions (a full-frame
        // clean every tick starves the LCD DMA and the panel rolls).
        if shown_page != st.page {
            shown_page = st.page;
            ui::draw(&mut display.canvas(), &st);
            display.flush();
        } else if dyn_tick.elapsed().as_millis() >= 50 {
            dyn_tick = Instant::now();
            let dirty = ui::draw_dynamic(&mut display.canvas(), &st);
            display.flush_rects(&dirty.as_slice());
        }

        st.led_rgb = (0, 0, 0); // LED intentionally disabled.

        // periodic heartbeat (time-based: `frame` resets every second)
        if st.uptime_s != 0 && st.uptime_s % 10 == 0 && frame == 1 {
            println!(
                "loop ok: {} fps, mic {:3.0}% [{}/{}], btn {} mV (raw {}), heap {} kB",
                fps,
                st.mic_level * 100.0,
                audio.mic_min,
                audio.mic_max,
                st.btn_mv,
                buttons.last_raw,
                esp_alloc::HEAP.free() / 1024
            );
            display::log_stats();
            audio.log_stats();
            println!("WS2812: RMT errors={}", led.error_count());
        }
        // ---- fps ----
        frame += 1;
        if fps_window.elapsed().as_secs() >= 1 {
            fps = frame;
            frame = 0;
            fps_window = Instant::now();
        }
        st.fps = fps;

        Timer::after(Duration::from_millis(2)).await;
    }
}

/// Allocate the 768000-byte, 64-byte-aligned framebuffer (lands in the
/// PSRAM region; too large for internal DRAM).
fn alloc_fb() -> Option<&'static mut [u8]> {
    extern crate alloc;
    const SIZE: usize = board::LCD_H_RES * board::LCD_V_RES * 2;
    let layout = alloc::alloc::Layout::from_size_align(SIZE, 64).ok()?;
    unsafe {
        let ptr = alloc::alloc::alloc(layout);
        if ptr.is_null() {
            return None;
        }
        Some(core::slice::from_raw_parts_mut(ptr, SIZE))
    }
}
