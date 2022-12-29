use esp_idf_hal::delay::{Ets, FreeRtos};

/// A `Delay` implementation that uses [`Ets`] for delays <10 ms, and [`FreeRtos`] for delays >=10 ms.
#[derive(Copy, Clone)]
pub struct GeneralPurposeDelay;

impl embedded_hal_0_2::blocking::delay::DelayUs<u16> for GeneralPurposeDelay {
    fn delay_us(&mut self, us: u16) {
        if us < 10_000 {
            Ets::delay_us(us as u32);
        } else {
            FreeRtos::delay_us(us as u32);
        }
    }
}

impl embedded_hal_0_2::blocking::delay::DelayUs<u32> for GeneralPurposeDelay {
    fn delay_us(&mut self, us: u32) {
        if us < 10_000 {
            Ets::delay_us(us);
        } else {
            FreeRtos::delay_us(us);
        }
    }
}

impl embedded_hal_0_2::blocking::delay::DelayMs<u16> for GeneralPurposeDelay {
    fn delay_ms(&mut self, ms: u16) {
        if ms < 10_000 {
            Ets::delay_ms(ms as u32);
        } else {
            FreeRtos::delay_ms(ms as u32);
        }
    }
}
