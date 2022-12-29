//! Blink the color LED on the devboard

use esp_idf_hal::delay::FreeRtos;
use esp_idf_hal::gpio::*;
use esp_idf_hal::peripherals::Peripherals;

fn main() -> anyhow::Result<()> {
    esp_idf_sys::link_patches();

    let peripherals = Peripherals::take().unwrap();
    let mut led_r = PinDriver::output(peripherals.pins.gpio3)?;

    loop {
        led_r.set_high()?;
        FreeRtos::delay_ms(1000);

        led_r.set_low()?;
        FreeRtos::delay_ms(1000);
    }
}
