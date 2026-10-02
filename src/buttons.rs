//! Four buttons (VOL+, VOL-, MODE/PLAY, SET) multiplexed on a resistor
//! ladder read by ADC1/GPIO42.
//!
//! # Why the raw codes look "inverted"
//!
//! The ladder is wired to the **negative** input of the differential SAR
//! channel (`ADC1_CH0_N`, see the vendor BSP comment on `BSP_BUTTONS_ADC`).
//! The ladder idles at 2 V, i.e. at the *bottom* of the code range, and
//! pressing a key *raises* the code as the ladder voltage falls.
//!
//! The raw value is the SAR's 17 comparator bits, and the SAR's comparator
//! weights are non-uniform; the code is the weighted sum of the set bits.
//! Both the weight table and the code→mV mapping are ported from the board
//! vendor's `esp32_s31_adc_calibration.c`
//! (`esp-dev-kits/examples/esp32-s31-korvo/.../esp32_s31_korvo`).
//!
//! Verified with ESP-IDF's own `adc_oneshot` driver on this board: it also
//! reports raw 0 at idle, which the vendor formula maps to 2000 mV — so the
//! hardware is fine, only the interpretation was wrong.

use esp_hal::{
    analog::adc::{Adc, AdcConfig, AdcPin, Attenuation},
    peripherals::{ADC1, GPIO42},
};

use crate::board as b;

/// Weight of each of the 17 SAR bits, MSB first (`s_ideal_weights` in the
/// vendor calibration). Their sum is [`CODE_FULL_SCALE`].
const IDEAL_WEIGHTS: [u16; 17] = [
    2048, 1024, 512, 256, 256, 128, 64, 32, 32, 16, 8, 8, 4, 2, 2, 0, 1,
];

/// Weighted-sum code that corresponds to a full-scale input.
const CODE_FULL_SCALE: u32 = 4393;

/// Ladder full-scale in millivolts at 0 dB attenuation (BSP: ADC_ATTEN_DB_0,
/// `BSP_S31_ADC_MAX_MV = 2000`).
const LADDER_FULL_SCALE_MV: u16 = 2000;

/// Number of samples averaged per poll, like the BSP does.
const SAMPLE_COUNT: u32 = 4;

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

/// Weighted sum of the SAR bit pattern (`bsp_s31_adc_calc_code_q`).
fn code_from_raw(raw: u16) -> u32 {
    let mut code = 0u32;
    for (i, weight) in IDEAL_WEIGHTS.iter().enumerate() {
        if raw & (1 << (16 - i)) != 0 {
            code += *weight as u32;
        }
    }
    code
}

/// Ladder voltage in millivolts (`bsp_s31_adc_calibration_raw_to_mv`).
fn raw_to_mv(raw: u16) -> u16 {
    let code = code_from_raw(raw);
    let mv = LADDER_FULL_SCALE_MV as i64 - (4000 * code as i64) / CODE_FULL_SCALE as i64;
    mv.clamp(0, LADDER_FULL_SCALE_MV as i64) as u16
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
        // The S31 SAR has a single attenuation; the value is ignored.
        let pin = config.enable_pin(pin, Attenuation::_0dB);
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
        let mut sum: u32 = 0;
        for _ in 0..SAMPLE_COUNT {
            sum += self.adc.read_blocking(&mut self.pin) as u32;
        }
        let raw = (sum / SAMPLE_COUNT) as u16;
        self.last_raw = raw;
        raw_to_mv(raw)
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
