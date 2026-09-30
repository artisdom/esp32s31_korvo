//! Four buttons (VOL+, VOL-, MODE/PLAY, SET) multiplexed on a resistor
//! ladder read by ADC1/GPIO42.
//!
//! The ESP32-S31 SAR ADC is differential with only one pad wired, so raw
//! codes run roughly `ZERO_DIFF_CODE..=FULL_SCALE` (ground ~2198, full
//! scale ~4393). We convert back to the ladder's millivolts using those
//! constants and the 2 V full-scale of the 0 dB attenuation setting used
//! by the stock BSP.

use esp_hal::{
    analog::adc::{Adc, AdcConfig, AdcPin, Attenuation, FULL_SCALE, ZERO_DIFF_CODE},
    peripherals::{ADC1, GPIO42},
};

use crate::board as b;

/// Ladder full-scale in millivolts at 0 dB attenuation (BSP: ADC_ATTEN_DB_0,
/// `BSP_S31_ADC_MAX_MV = 2000`).
const LADDER_FULL_SCALE_MV: u16 = 2000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Button {
    VolUp,
    VolDown,
    Mode,
    Set,
}

pub const ALL: [Button; 4] = [Button::VolUp, Button::VolDown, Button::Mode, Button::Set];

impl Button {
    pub fn label(self) -> &'static str {
        match self {
            Button::VolUp => "VOL+",
            Button::VolDown => "VOL-",
            Button::Mode => "MODE",
            Button::Set => "SET",
        }
    }

    fn center_mv(self) -> u16 {
        b::BUTTON_CENTER_MV[match self {
            Button::VolUp => b::BTN_VOLUP,
            Button::VolDown => b::BTN_VOLDOWN,
            Button::Mode => b::BTN_MODE,
            Button::Set => b::BTN_SET,
        }]
    }
}

pub struct Buttons {
    adc: Adc<'static, ADC1<'static>, esp_hal::Blocking>,
    pin: AdcPin<GPIO42<'static>, ADC1<'static>>,
    held: Option<Button>,
    /// Millivolts of the last sample (for the UI).
    pub last_mv: u16,
    /// Last raw code (diagnostics).
    pub last_raw: u16,
}

impl Buttons {
    pub fn new(adc_peri: ADC1<'static>, pin: GPIO42<'static>) -> Self {
        let mut config = AdcConfig::<ADC1<'static>>::default();
        let pin = config.enable_pin(pin, Attenuation::_11dB);
        let adc = Adc::new(adc_peri, config);
        Self {
            adc,
            pin,
            held: None,
            last_mv: 0,
            last_raw: 0,
        }
    }

    fn sample_mv(&mut self) -> u16 {
        // Average a few samples like the BSP does to reject ladder noise.
        let mut sum: u32 = 0;
        for _ in 0..4 {
            sum += self.adc.read_blocking(&mut self.pin) as u32;
        }
        let raw = (sum / 4) as u16;
        self.last_raw = raw;
        let mv = ((raw.saturating_sub(ZERO_DIFF_CODE) as u32) * LADDER_FULL_SCALE_MV as u32)
            / (FULL_SCALE.saturating_sub(ZERO_DIFF_CODE) as u32);
        mv.min(LADDER_FULL_SCALE_MV as u32) as u16
    }

    fn decode(mv: u16) -> Option<Button> {
        let mut best: Option<Button> = None;
        let mut best_dist = u32::MAX;
        for btn in ALL {
            let center = btn.center_mv() as i32;
            let dist = (mv as i32 - center).unsigned_abs();
            // A key is only valid if it is closer to its centre than to the
            // idle level, with a little margin.
            let idle_dist = (b::BUTTON_IDLE_MV as i32 - mv as i32).unsigned_abs();
            if dist < best_dist && dist < idle_dist {
                best = Some(btn);
                best_dist = dist;
            }
        }
        best
    }

    /// Poll the ladder; returns a button on the press edge only.
    pub fn poll(&mut self) -> Option<Button> {
        let mv = self.sample_mv();
        self.last_mv = mv;
        let pressed = Self::decode(mv);
        match (pressed, self.held) {
            (Some(btn), None) => {
                self.held = Some(btn);
                Some(btn)
            }
            (None, Some(_)) => {
                self.held = None;
                None
            }
            _ => None,
        }
    }

    /// Currently held button, if any.
    pub fn held(&self) -> Option<Button> {
        self.held
    }
}
