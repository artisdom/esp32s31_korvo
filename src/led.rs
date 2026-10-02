//! WS2812 addressable status LED on GPIO37, driven by an RMT TX channel.

use esp_hal::{
    gpio::Level,
    peripherals::GPIO37,
    rmt::{PulseCode, Rmt, Tx as RmtTx, TxChannelConfig, TxChannelCreator},
};

/// T1H/T0H/T1L/T0L in 100 ns units (WS2812 timing).
const T1H: u16 = 9;
const T0H: u16 = 3;
const T1L: u16 = 3;
const T0L: u16 = 9;
const RESET_TICKS: u16 = 1500; // 150 us per half: 300 us low before end marker

pub struct StatusLed {
    channel: Option<Channel>,
    errors: u32,
}

type Channel = esp_hal::rmt::Channel<'static, esp_hal::Blocking, RmtTx>;

impl StatusLed {
    pub fn new(rmt: Rmt<'static, esp_hal::Blocking>, pin: GPIO37<'static>) -> Result<Self, ()> {
        let config = TxChannelConfig::default()
            .with_clk_divider(1)
            .with_idle_output_level(Level::Low)
            .with_idle_output(true);
        let channel = rmt
            .channel0
            .configure_tx(&config)
            .map_err(|_| ())?
            .with_pin(pin);
        Ok(Self {
            channel: Some(channel),
            errors: 0,
        })
    }

    pub fn send(&mut self, r: u8, g: u8, b: u8) {
        // GRB order, MSB first.
        let mut codes = [PulseCode::end_marker(); 26];
        let mut idx = 0;
        for byte in [g, r, b] {
            for bit in (0..8).rev() {
                codes[idx] = if byte & (1 << bit) != 0 {
                    PulseCode::new(Level::High, T1H, Level::Low, T1L)
                } else {
                    PulseCode::new(Level::High, T0H, Level::Low, T0L)
                };
                idx += 1;
            }
        }
        codes[24] = PulseCode::new(Level::Low, RESET_TICKS, Level::Low, RESET_TICKS);
        if let Some(channel) = self.channel.take() {
            match channel.transmit(&codes) {
                Ok(tx) => match tx.wait() {
                    Ok(ch) => self.channel = Some(ch),
                    Err((e, ch)) => {
                        self.channel = Some(ch);
                        self.record_error(e);
                    }
                },
                Err((e, ch)) => {
                    self.channel = Some(ch);
                    self.record_error(e);
                }
            }
        }
    }
    pub fn error_count(&self) -> u32 {
        self.errors
    }

    fn record_error(&mut self, error: esp_hal::rmt::Error) {
        self.errors += 1;
        if self.errors == 1 {
            esp_println::println!("WS2812 RMT error: {}", error);
        }
    }
}
