use std::sync::Mutex;

use anyhow::Context;
use embedded_hal_0_2::blocking::delay::{DelayMs, DelayUs};
use embedded_svc::wifi::{ClientConfiguration, Configuration, Wifi};
use esp_idf_hal::{
    i2c::{config::Config as I2cConfig, I2cDriver},
    peripherals::Peripherals,
    units::FromValueType,
};
use esp_idf_svc::{eventloop::EspSystemEventLoop, nvs::EspDefaultNvsPartition, wifi::EspWifi};
use sgp30::Sgp30;
use shared_bus::I2cProxy;
use veml6030::Veml6030;

mod delay;

use crate::delay::GeneralPurposeDelay;

/// VEML sensor integration time
const VEML_INTEGRATION_TIME: veml6030::IntegrationTime = veml6030::IntegrationTime::Ms25;

type SharedBuxProxyI2c<'a> = I2cProxy<'a, Mutex<I2cDriver<'a>>>;

#[derive(Default)]
struct Sensors<'a> {
    lux: Option<Veml6030<SharedBuxProxyI2c<'a>>>,
    gas: Option<Sgp30<SharedBuxProxyI2c<'a>, GeneralPurposeDelay>>,
}

fn main() -> anyhow::Result<()> {
    esp_idf_sys::link_patches();

    println!("Initializing Sensilo");

    // Core resources
    let peripherals = Peripherals::take().unwrap();
    let sys_loop = EspSystemEventLoop::take().unwrap();
    let nvs = EspDefaultNvsPartition::take().unwrap();

    // Delay provider
    let mut delay = GeneralPurposeDelay;

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

    // Sensors wrapper
    let mut sensors = Sensors::default();

    // Initialize VEML7700 lux sensor
    if cfg!(feature = "lux") {
        println!("VEML7700: Enabled");
        let mut veml = Veml6030::new(i2c.acquire_i2c(), veml6030::SlaveAddr::default());
        delay.delay_ms(10);
        let mut success = true;
        if let Err(e) = veml.set_gain(veml6030::Gain::OneQuarter) {
            eprintln!("  Error: Could not set gain: {:?}", e);
            success = false;
        }
        if let Err(e) = veml.set_integration_time(VEML_INTEGRATION_TIME) {
            eprintln!("  Error: Could not set integration time: {:?}", e);
            success = false;
        }
        if let Err(e) = veml.enable() {
            eprintln!("  Error: Could not enable sensor: {:?}", e);
            success = false;
        }

        // After enabling the sensor, a startup time of 4 ms plus the integration time must be awaited.
        delay.delay_us(VEML_INTEGRATION_TIME.as_us() + 4_000);

        if success {
            sensors.lux = Some(veml);
        }
    }

    // Initialize SGP30 gas sensor
    if cfg!(feature = "gas") {
        println!("SGP30: Enabled");
        let mut sgp30 = Sgp30::new(i2c.acquire_i2c(), 0x58, delay);
        let mut success = true;
        match sgp30.serial() {
            Ok(serial) => println!("  Serial: {:?}", serial),
            Err(e) => {
                eprintln!("  Error: Could not get serial: {:?}", e);
                success = false;
            }
        }
        if let Err(e) = sgp30.init() {
            eprintln!("  Error: Could not initialize: {:?}", e);
            success = false;
        }
        if success {
            sensors.gas = Some(sgp30);
        }
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
        delay.delay_ms(250);
    }
    println!();

    // Wait for IP assignment from DHCP
    print!("WiFi connected! Waiting for IP");
    loop {
        let ip_info = wifi.sta_netif().get_ip_info().unwrap();
        if ip_info.ip.is_unspecified() {
            print!(".");
            delay.delay_ms(250);
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
    println!();

    println!("Usable sensors:");
    println!("  Lux (VEML7700): {}", sensors.lux.is_some());
    println!();

    println!("Starting main loop");
    loop {
        println!("------");
        // Read lux sensor, if present
        if let Some(ref mut veml) = sensors.lux {
            match veml.read_lux() {
                Ok(lux) => println!("Lux:       {}", lux),
                Err(e) => eprintln!("Lux: ERROR: {:?}", e),
            }
        }

        // Read gas sensor, if present
        if let Some(ref mut sgp30) = sensors.gas {
            match sgp30.measure() {
                Ok(measurement) => {
                    println!("CO₂eq PPM: {}", measurement.co2eq_ppm);
                    println!("TVOC PPB:  {}", measurement.tvoc_ppb);
                }
                Err(e) => eprintln!("Gas: ERROR: {:?}", e),
            }
        }

        delay.delay_ms(1000 - 12);
    }
}
