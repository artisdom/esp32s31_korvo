//! WS2812 addressable status LED on GPIO37, driven by an RMT TX channel.

use esp_hal::{
    gpio::Level,
    peripherals::GPIO37,
    rmt::{PulseCode, Rmt, Tx as RmtTx, TxChannelConfig, TxChannelCreator},
};

const RMT_FREQ_HZ: u64 = 10_000_000; // 100 ns ticks, like the stock driver

/// T1H/T0H/T1L/T0L in 100 ns units (WS2812 timing).
const T1H: u16 = 9 - 1;
const T0H: u16 = 3 - 1;
const T1L: u16 = 3 - 1;
const T0L: u16 = 9 - 1;
const RESET_TICKS: u16 = 300 - 1; // > 50 us low

pub struct StatusLed {
    channel: Option<Channel>,
    pending: Option<(u8, u8, u8)>,
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
            pending: None,
        })
    }

    /// Queue a colour; takes effect on the next [`Self::update`] call.
    pub fn set(&mut self, r: u8, g: u8, b: u8) {
        self.pending = Some((r, g, b));
    }

    /// Push the pending colour out on the wire (non-blocking fire-and-forget;
    /// the caller is expected to call this at a few kHz at most).
    pub fn update(&mut self) {
        if let Some((r, g, b)) = self.pending.take() {
            self.send(r, g, b);
        }
    }

    pub fn send(&mut self, r: u8, g: u8, b: u8) {
        // GRB order, MSB first.
        let mut codes = [PulseCode::new(Level::High, 0, Level::Low, 0); 25];
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
                    Err((_e, ch)) => self.channel = Some(ch),
                },
                Err((_e, ch)) => self.channel = Some(ch),
            }
        }
    }
}

/// Colour-cycle helper for the boot animation.
pub fn wheel(pos: u8) -> (u8, u8, u8) {
    let pos = pos % 255;
    if pos < 85 {
        (255 - pos * 3, pos * 3, 0)
    } else if pos < 170 {
        let p = pos - 85;
        (0, 255 - p * 3, p * 3)
    } else {
        let p = pos - 170;
        (p * 3, 0, 255 - p * 3)
    }
}

