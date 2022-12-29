use anyhow::Context;
use esp_idf_hal::{
    delay::FreeRtos,
    i2c::{config::Config as I2cConfig, I2cDriver},
    peripherals::Peripherals,
    units::FromValueType,
};
use veml6030::Veml6030;

// VEML sensor integration time
const VEML_INTEGRATION_TIME: veml6030::IntegrationTime = veml6030::IntegrationTime::Ms25;

fn main() -> anyhow::Result<()> {
    esp_idf_sys::link_patches();

    println!("Initializing Sensilo");

    let peripherals = Peripherals::take().unwrap();
    let mut led_r = esp_idf_hal::gpio::PinDriver::output(peripherals.pins.gpio3)?;

    // I2C bus
    let mut i2c = I2cDriver::new(
        peripherals.i2c0,
        peripherals.pins.gpio8, // SDA
        peripherals.pins.gpio9, // SCL
        &I2cConfig::new()
            .baudrate(100.kHz().into())
            .sda_enable_pullup(true)
            .scl_enable_pullup(true),
    )
    .context("Could not initialize I2C driver")?;
    println!("Write result: {:?}", i2c.write(0x3c, &[1, 2, 3], 999999));

    // Initialize VEML7700 lux sensor
    let mut veml = Veml6030::new(i2c, veml6030::SlaveAddr::default());
    FreeRtos::delay_ms(10);
    if let Err(e) = veml.set_gain(veml6030::Gain::OneQuarter) {
        eprintln!("VEML7700: Could not set gain: {:?}", e);
    }
    if let Err(e) = veml.set_integration_time(VEML_INTEGRATION_TIME) {
        eprintln!("VEML7700: Could not set integration time: {:?}", e);
    }

    println!("Starting Sensilo");
    loop {
        led_r.set_high()?;
        FreeRtos::delay_ms(1000);
        led_r.set_low()?;
        FreeRtos::delay_ms(1000);
    }
}
