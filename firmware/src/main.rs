use anyhow::Context;
use embedded_svc::wifi::{ClientConfiguration, Configuration, Wifi};
use esp_idf_hal::{
    delay::FreeRtos,
    i2c::{config::Config as I2cConfig, I2cDriver},
    peripherals::Peripherals,
    units::FromValueType,
};
use esp_idf_svc::{eventloop::EspSystemEventLoop, nvs::EspDefaultNvsPartition, wifi::EspWifi};
use veml6030::Veml6030;

// VEML sensor integration time
const VEML_INTEGRATION_TIME: veml6030::IntegrationTime = veml6030::IntegrationTime::Ms25;

fn main() -> anyhow::Result<()> {
    esp_idf_sys::link_patches();

    println!("Initializing Sensilo");

    // Core resources
    let peripherals = Peripherals::take().unwrap();
    let sys_loop = EspSystemEventLoop::take().unwrap();
    let nvs = EspDefaultNvsPartition::take().unwrap();

    let mut led_r = esp_idf_hal::gpio::PinDriver::output(peripherals.pins.gpio3)?;

    // I2C bus
    let i2c0 = I2cDriver::new(
        peripherals.i2c0,
        peripherals.pins.gpio6, // SDA
        peripherals.pins.gpio7, // SCL
        &I2cConfig::new()
            .baudrate(380.kHz().into())
            .sda_enable_pullup(true)
            .scl_enable_pullup(true),
    )
    .context("Could not initialize I2C driver")?;
    let i2c: &'static _ = shared_bus::new_std!(I2cDriver = i2c0).unwrap();

    // Initialize VEML7700 lux sensor
    let mut veml = Veml6030::new(i2c.acquire_i2c(), veml6030::SlaveAddr::default());
    FreeRtos::delay_ms(10);
    if let Err(e) = veml.set_gain(veml6030::Gain::OneQuarter) {
        eprintln!("VEML7700: Could not set gain: {:?}", e);
    }
    if let Err(e) = veml.set_integration_time(VEML_INTEGRATION_TIME) {
        eprintln!("VEML7700: Could not set integration time: {:?}", e);
    }

    // Connect WiFi
    let mut wifi = EspWifi::new(peripherals.modem, sys_loop, Some(nvs))
        .context("Could not create EspWifi instance")?;
    wifi.set_configuration(&Configuration::Client(ClientConfiguration {
        ssid: env!("SENSILO_WIFI_SSID").into(),
        password: env!("SENSILO_WIFI_PASSWORD").into(),
        ..Default::default()
    }))
    .unwrap();
    wifi.start().context("Could not start WiFi")?;
    wifi.connect().context("Could not connect WiFi")?;
    print!(
        "Waiting for station with SSID {}",
        wifi.get_configuration()
            .ok()
            .as_ref()
            .and_then(|conf| conf.as_client_conf_ref().map(|client| &client.ssid))
            .unwrap()
    );
    while !wifi.is_connected().unwrap() {
        print!(".");
        FreeRtos::delay_ms(250);
    }
    println!();

    // Wait for IP assignment from DHCP
    print!("WiFi connected! Waiting for IP");
    loop {
        let ip_info = wifi.sta_netif().get_ip_info().unwrap();
        if ip_info.ip.is_unspecified() {
            print!(".");
            FreeRtos::delay_ms(250);
        } else {
            println!("\n  Assigned IP: {}", ip_info.ip);
            if let Some(dns) = ip_info.dns {
                println!("  DNS:         {}", dns);
            } else {
                println!("  Warning: No DNS server assigned!");
            }
            break;
        }
    }

    // Turn on VEML7700
    if let Err(e) = veml.enable() {
        eprintln!("VEML7700: Could not enable sensor: {:?}", e);
    }
    // After enabling the sensor, a startup time of 4 ms plus the integration time must be awaited.
    FreeRtos::delay_us(VEML_INTEGRATION_TIME.as_us() + 4_000);

    println!("Starting main loop");
    loop {
        led_r.set_high()?;
        FreeRtos::delay_ms(1000);
        led_r.set_low()?;
        FreeRtos::delay_ms(1000);

        println!("Lux: {:?}", veml.read_lux());
    }
}
