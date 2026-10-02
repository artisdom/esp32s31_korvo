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
//! The local HAL returns the hardware's weighted code in 0..4393.
//! Convert it directly with `mv = 2000 - 4000 * code / 4393`; applying
//! comparator weights to this code again misclassifies presses as SET.
//! Button state changes are debounced for 20 ms.

use esp_hal::{
    analog::adc::{Adc, AdcConfig, AdcPin, Attenuation},
    peripherals::{ADC1, GPIO42},
};

pub use crate::button_logic::Button;
use crate::button_logic::{Debouncer, decode, raw_to_mv};

const SAMPLE_COUNT: u32 = 4;

pub struct Buttons {
    adc: Adc<'static, ADC1<'static>, esp_hal::Blocking>,
    pin: AdcPin<GPIO42<'static>, ADC1<'static>>,
    state: Debouncer,
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
            state: Debouncer::new(),
            last_mv: 0,
            last_raw: 0,
        }
    }

    fn sample_mv(&mut self) -> u16 {
        let mut sum_mv = 0u32;
        for _ in 0..SAMPLE_COUNT {
            let raw = self.adc.read_blocking(&mut self.pin);
            self.last_raw = raw;
            sum_mv += raw_to_mv(raw) as u32;
        }
        (sum_mv / SAMPLE_COUNT) as u16
    }

    /// Poll the ladder; return a debounced press edge and track releases.
    pub fn poll(&mut self) -> Option<Button> {
        self.last_mv = self.sample_mv();
        self.state.update(
            decode(self.last_mv),
            esp_hal::time::Instant::now()
                .duration_since_epoch()
                .as_millis(),
        )
    }

    /// Currently held button, if any.
    pub fn held(&self) -> Option<Button> {
        self.state.held
    }
}
