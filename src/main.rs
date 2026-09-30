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
use static_cell::StaticCell;
use esp_println::println;
use crate::{audio::Source, buttons::Button, ui::Page};

mod audio;
mod board;
mod buttons;
mod camera;
mod display;
mod es8389;
mod font;
mod gfx;
mod led;
mod sdcard;
mod touch;
mod ui;
mod usb;

esp_bootloader_esp_idf::esp_app_desc!();

/// Main-loop heartbeat, readable from the USB task on core 1.
static LOOP_COUNT: AtomicU32 = AtomicU32::new(0);

#[esp_hal::main]
async fn main(_spawner: embassy_executor::Spawner) {
    let peripherals = esp_hal::init(esp_hal::Config::default());

    println!("== ESP32-S31-Korvo-1 Rust demo ==");
    println!("chip: {}", esp_hal::chip!());

    // --- heaps: internal DRAM + PSRAM regions --------------------------------
    esp_alloc::heap_allocator!(size: 64 * 1024);
    let psram = esp_hal::psram::Psram::new(peripherals.PSRAM, esp_hal::psram::PsramConfig::default());
    let (_psram_start, psram_size) = psram.raw_parts();
    unsafe {
        esp_alloc::HEAP.add_region(esp_alloc::HeapRegion::new(
            _psram_start,
            psram_size,
            esp_alloc::MemoryCapability::External.into(),
        ));
    }
    println!("heap: +64k DRAM +{} MiB PSRAM", psram_size / (1024 * 1024));

    // --- timers / RTOS ----------------------------------------------------------
    println!("init: timer/rtos...");
    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0);
    println!("init: rtos ok");

    // --- boot blip on the WS2812 -------------------------------------------------
    println!("init: rmt...");
    let rmt = esp_hal::rmt::Rmt::new(peripherals.RMT, esp_hal::time::Rate::from_mhz(10))
        .expect("RMT clock");
    let mut led = led::StatusLed::new(
        rmt,
        peripherals.GPIO37,
    )
    .expect("LED channel");
    println!("init: led ok");
    led.send(0, 0, 32);

    // --- shared I2C bus (codec + touch + camera SCCB) ------------------------------
    let mut i2c = I2c::new(
        peripherals.I2C0,
        I2cConfig::default().with_frequency(esp_hal::time::Rate::from_hz(400_000)),
    )
    .expect("I2C0")
    .with_sda(peripherals.GPIO0)
    .with_scl(peripherals.GPIO1);
    println!("init: i2c ok");
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
    println!("ES8389 codec: {}", if codec_ok { "detected" } else { "MISSING" });
    let codec_id = if codec_ok {
        match codec.init_48k() {
            Ok(()) => {
                let _ = codec.set_volume_db(-6.0);
                let _ = codec.set_mic_gain(21);
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

    // --- I2S (builds on I2S0 with DMA_CH0; MCLK on GPIO2) -----------------------------
    let i2s = audio::build_i2s(
        peripherals.I2S0,
        peripherals.DMA_CH0,
        peripherals.GPIO2,
    )
    .expect("I2S0");
    let mut audio = {
        let pins = audio::AudioPins {
            bclk: peripherals.GPIO3,
            ws: peripherals.GPIO4,
            dout: peripherals.GPIO5,
            din: peripherals.GPIO6,
        };
        match audio::Audio::new(i2s, pins) {
            Ok(a) => {
                println!("I2S0 DMA streaming started (TX+RX)");
                a
            }
            Err(e) => panic!("audio init failed: {}", e),
        }
    };
    audio.set_source(Source::Chime); // startup arpeggio

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
    let (mut display, _keep_alive) =
        match display::Display::new(lcd_cam, peripherals.DMA_AXI_CH0, fb, lcd_pins) {
            Ok(d) => {
                println!("RGB LCD scanning from PSRAM ring buffer");
                d
            }
            Err(e) => panic!("display init failed: {}", e),
        };

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
    let cam_probe = camera::probe(i2c);
    match cam_probe.sensor {
        Some((name, pid)) => println!("camera: {} PID=0x{:04x}", name, pid),
        None => println!("camera: no sensor answered on SCCB"),
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
    println!("mounting microSD...");
    let sd = sdcard::mount_and_inspect(peripherals.SDHOST, sd_pins, sd_power).await;
    if sd.card_ok {
        println!(
            "microSD ok: {} MB, {} {}, label \"{}\"",
            sd.capacity_mb, sd.partition, sd.fs_type, sd.volume_label
        );
    } else {
        println!("microSD: {}", sd.error.unwrap_or("unknown failure"));
    }

    // --- buttons -------------------------------------------------------------------------------
    let mut buttons = buttons::Buttons::new(peripherals.ADC1, peripherals.GPIO42);
    println!("init: buttons ok");

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
    println!("core 1: USB CDC task running");

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
        volume_db: -6.0,
        audio_source: audio.source(),
        uptime_s: 0,
        fps: 0,
        touch_point: None,
        flash_led: false,
    };

    let start = Instant::now();
    let mut frame = 0u32;
    let mut fps_window = Instant::now();
    let mut fps = 0u32;
    let mut led_phase: u8 = 0;
    let mut tone_on = false;

    loop {
        LOOP_COUNT.fetch_add(1, Ordering::Relaxed);

        // ---- buttons ----
        st.flash_led = false;
        if let Some(btn) = buttons.poll() {
            st.flash_led = true;
            match btn {
                Button::Mode => {
                    st.page = st.page.next();
                    tone_on = !tone_on;
                    audio.set_source(if tone_on {
                        Source::Tone(440.0)
                    } else {
                        Source::Silence
                    });
                }
                Button::Set => {
                    audio.set_source(Source::Chime);
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

        // ---- touch ----
        if let Some(t) = touch.as_mut() {
            st.touch_point = t.poll().map(|p| (p.x, p.y));
            if let Some((x, y)) = st.touch_point {
                if let Some(page) = ui::hit_tabs(x, y) {
                    st.page = page;
                }
            }
        }

        // ---- audio streaming + mic meter ----
        audio.poll();
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
        ui::draw(&mut display.canvas(), &st);
        display.flush();

        // ---- status LED: slow colour cycle, orange flash on key press ----
        led_phase = led_phase.wrapping_add(2);
        let (r, g, b) = if st.flash_led {
            (0xff, 0x66, 0x00)
        } else {
            let w = led::wheel(led_phase);
            ((w.0 as u16 * 4 / 10) as u8, (w.1 as u16 * 4 / 10) as u8, (w.2 as u16 * 4 / 10) as u8)
        };
        led.set(r, g, b);
        led.update();

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
