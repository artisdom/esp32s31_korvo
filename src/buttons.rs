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
    sample_sum: u32,
    sample_count: u32,
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
            sample_sum: 0,
            sample_count: 0,
            last_mv: 0,
            last_raw: 0,
        }
    }

    /// Poll conversions without spinning: radio calibration can temporarily
    /// own/reconfigure SAR. Waiting here would stall media and all task futures.
    pub fn poll(&mut self) -> Option<Button> {
        // A bounded burst keeps four-sample debounce responsive even when
        // camera capture reduces the loop rate. A pending SAR never blocks.
        for _ in 0..8 {
            let Ok(raw) = self.adc.read_oneshot(&mut self.pin) else {
                continue;
            };
            self.last_raw = raw;
            self.sample_sum += raw_to_mv(raw) as u32;
            self.sample_count += 1;
            if self.sample_count < SAMPLE_COUNT {
                continue;
            }
            self.last_mv = (self.sample_sum / SAMPLE_COUNT) as u16;
            self.sample_sum = 0;
            self.sample_count = 0;
            return self.state.update(
                decode(self.last_mv),
                esp_hal::time::Instant::now()
                    .duration_since_epoch()
                    .as_millis(),
            );
        }
        None
    }

    /// Currently held button, if any.
    pub fn held(&self) -> Option<Button> {
        self.state.held
    }
}
