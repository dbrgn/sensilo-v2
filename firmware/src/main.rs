use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::Context;
use embedded_hal_0_2::blocking::delay::{DelayMs, DelayUs};
use embedded_svc::{
    http::{client::Client as HttpClient, Status},
    io::Write,
    utils::io,
    wifi::{ClientConfiguration, Configuration as WifiConfiguration, Wifi},
};
use esp_idf_hal::{
    delay::FreeRtos,
    i2c::{config::Config as I2cConfig, I2cDriver},
    modem::Modem,
    peripherals::Peripherals,
    units::FromValueType,
};
use esp_idf_svc::{
    eventloop::{EspEventLoop, EspSystemEventLoop, System},
    http::client::{Configuration as HttpConfiguration, EspHttpConnection},
    nvs::{EspDefaultNvsPartition, EspNvsPartition, NvsDefault},
    timer::EspTaskTimerService,
    wifi::EspWifi,
};
use sgp30::Sgp30;
use shared_bus::I2cProxy;
use shtcx::ShtC3;
use veml6030::Veml6030;

mod delay;

use crate::delay::GeneralPurposeDelay;

// VEML sensor integration time
const VEML_INTEGRATION_TIME: veml6030::IntegrationTime = veml6030::IntegrationTime::Ms25;

// Sensor information
const SENSILO_NAME: &str = env!("SENSILO_NAME");

// WiFi credentials
const SENSILO_WIFI_SSID: &str = env!("SENSILO_WIFI_SSID");
const SENSILO_WIFI_PASSWORD: &str = env!("SENSILO_WIFI_PASSWORD");

// InfluxDB
const SENSILO_INFLUXDB_HOST: &str = env!("SENSILO_INFLUXDB_HOST");
const SENSILO_INFLUXDB_ORG: &str = env!("SENSILO_INFLUXDB_ORG");
const SENSILO_INFLUXDB_BUCKET: &str = env!("SENSILO_INFLUXDB_BUCKET");
const SENSILO_INFLUXDB_API_TOKEN: &str = env!("SENSILO_INFLUXDB_API_TOKEN");

// Firmware version
const VERSION: &str = env!("CARGO_PKG_VERSION");

type SharedBuxProxyI2c<'a> = I2cProxy<'a, Mutex<I2cDriver<'a>>>;

#[derive(Default)]
struct Sensors<'a> {
    temp_humi: Option<ShtC3<SharedBuxProxyI2c<'a>>>,
    lux: Option<Veml6030<SharedBuxProxyI2c<'a>>>,
    gas: Option<Sgp30<SharedBuxProxyI2c<'a>, GeneralPurposeDelay>>,
}

#[derive(Default)]
struct Measurements {
    /// Temperature
    temperature: Option<shtcx::Temperature>,
    /// Humidity
    humidity: Option<shtcx::Humidity>,
    /// Illuminance in Lux
    illuminance: Option<f32>,
    /// CO2 equivalent in PPM
    co2eq_ppm: Option<u16>,
    /// TVOC equivalent in PPB
    tvoc_ppb: Option<u16>,
}

impl Measurements {
    /// Reset measurement values to their defaults
    fn reset(&mut self) {
        *self = Self::default();
    }
}

fn main() -> anyhow::Result<()> {
    esp_idf_sys::link_patches();

    println!("Initializing Sensilo\n");

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

    // Initialize SHTC3 temperature/humidity sensor
    if cfg!(feature = "temp_humi") {
        println!("SHTC3: Enabled");
        init_shtc3(&mut sensors, i2c.acquire_i2c());
    }

    // Initialize VEML7700 lux sensor
    if cfg!(feature = "lux") {
        println!("VEML7700: Enabled");
        init_veml7700(&mut sensors, i2c.acquire_i2c());
    }

    // Initialize SGP30 gas sensor
    if cfg!(feature = "gas") {
        println!("SGP30: Enabled");
        init_sgp30(&mut sensors, i2c.acquire_i2c());
    }

    println!();

    // Connect WiFi
    let wifi = connect_wifi(peripherals.modem, sys_loop, nvs)?;

    // Wait for IP assignment from DHCP
    println!("WiFi connected! Waiting for IP...");
    loop {
        let ip_info = wifi.sta_netif().get_ip_info().unwrap();
        if ip_info.ip.is_unspecified() {
            delay.delay_ms(100);
        } else {
            println!("  Assigned IP: {}", ip_info.ip);
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
    println!(
        "  Temperature/Humidity (SHTC3): {}",
        sensors.temp_humi.is_some()
    );
    println!("  Lux (VEML7700): {}", sensors.lux.is_some());
    println!("  Gas (SGP30): {}", sensors.gas.is_some());
    println!();

    println!("Starting main loop");

    let schedule_gas_sensor_timer = sensors.gas.is_some();

    let sensors = Arc::new(Mutex::new(sensors));
    let measurements = Arc::new(Mutex::new(Measurements::default()));

    // The SGP30 requires to be called at 1s intervals for the internal algorithm to work. Thus,
    // schedule a periodic timer task.
    let mut gas_sensor_timer = None;
    if schedule_gas_sensor_timer {
        // Create timer task
        let timer_sensors = sensors.clone();
        let timer_measurements = measurements.clone();
        let mut seconds_since_start = 0usize;
        let timer = EspTaskTimerService::new()?.timer(move || {
            seconds_since_start = seconds_since_start.saturating_add(1);
            let mut s = timer_sensors.lock().expect("Failed to lock sensors mutex");
            if let Some(ref mut sgp30) = s.gas {
                match sgp30.measure() {
                    Ok(measurement) => {
                        println!(":: CO₂eq: {} PPM", measurement.co2eq_ppm);
                        println!(":: TVOC:  {} PPB", measurement.tvoc_ppb);
                        if seconds_since_start > 32 {
                            // Note: Give sensor some time for initial calibration (>15s)
                            let mut m = timer_measurements
                                .lock()
                                .expect("Failed to lock measurements mutex");
                            m.co2eq_ppm = Some(measurement.co2eq_ppm);
                            m.tvoc_ppb = Some(measurement.tvoc_ppb);
                        }
                    }
                    Err(e) => eprintln!("SGP30: ERROR: {:?}", e),
                }
            }
        })?;

        // Schedule timer
        timer.every(Duration::from_secs(1))?;

        // Prevent timer from being dropped, and thus, being cancelled
        gas_sensor_timer = Some(timer);
    }
    if gas_sensor_timer.is_some() {
        println!("Scheduled periodic gas sensor task at 1s intervals");
    }

    loop {
        {
            // Get access to shared data
            let mut s = sensors.lock().expect("Failed to lock sensors mutex");
            let mut m = measurements
                .lock()
                .expect("Failed to lock measurements mutex");

            // Read sensors
            read_sensors(&mut s, &mut m, &mut delay);

            // Submit measurements
            if let Err(e) = submit_measurements(&m) {
                eprintln!("Error: Could not submit measurement: {}", e);
            }

            // Reset measurements
            m.reset();
        }

        // Wait for a few seconds until the next submission interval.
        //
        // Note: It's important that the mutexes are not locked while sleeping!
        delay.delay_ms(30 * 1000);
    }
}

/// Initialize the SHTC3 sensor. If successful, add it to the [`Sensors`] instance.
fn init_shtc3<'a>(sensors: &mut Sensors<'a>, i2c: SharedBuxProxyI2c<'a>) {
    let mut shtc3 = shtcx::shtc3(i2c);
    let mut success = true;
    match shtc3.device_identifier() {
        Ok(id) => println!("  Device ID: {}", id),
        Err(e) => {
            eprintln!("  Error: Could not get device ID: {:?}", e);
            success = false;
        }
    }
    if success {
        sensors.temp_humi = Some(shtc3);
    }
}

/// Initialize the VEML7700 sensor. If successful, add it to the [`Sensors`] instance.
fn init_veml7700<'a>(sensors: &mut Sensors<'a>, i2c: SharedBuxProxyI2c<'a>) {
    let mut delay = GeneralPurposeDelay;
    let mut veml = Veml6030::new(i2c, veml6030::SlaveAddr::default());
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

/// Initialize the SGP30 sensor. If successful, add it to the [`Sensors`] instance.
fn init_sgp30<'a>(sensors: &mut Sensors<'a>, i2c: SharedBuxProxyI2c<'a>) {
    let mut sgp30 = Sgp30::new(i2c, 0x58, GeneralPurposeDelay);
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

fn connect_wifi(
    modem: Modem,
    event_loop: EspEventLoop<System>,
    nvs: EspNvsPartition<NvsDefault>,
) -> anyhow::Result<EspWifi<'static>> {
    let mut wifi =
        EspWifi::new(modem, event_loop, Some(nvs)).context("Could not create EspWifi instance")?;

    wifi.set_configuration(&WifiConfiguration::Client(ClientConfiguration {
        ssid: SENSILO_WIFI_SSID.into(),
        password: SENSILO_WIFI_PASSWORD.into(),
        ..Default::default()
    }))
    .unwrap();
    wifi.start().context("Could not start WiFi")?;
    wifi.connect().context("Could not connect WiFi")?;
    println!(
        "Waiting for station with SSID {}...",
        wifi.get_configuration()
            .ok()
            .as_ref()
            .and_then(|conf| conf.as_client_conf_ref().map(|client| &client.ssid))
            .unwrap()
    );
    while !wifi.is_connected().unwrap() {
        FreeRtos::delay_ms(100);
    }
    println!();

    Ok(wifi)
}

/// Read sensors, print data and update measurements.
///
/// Note: The gas sensor is not being read here, since it needs to be processed at a 1s intervals
/// inside the periodic timer task!
fn read_sensors(
    sensors: &mut Sensors,
    measurements: &mut Measurements,
    delay: &mut GeneralPurposeDelay,
) {
    // Read temp/humi sensor, if present
    if let Some(ref mut shtc3) = sensors.temp_humi {
        match shtc3.measure(shtcx::PowerMode::NormalMode, delay) {
            Ok(measurement) => {
                println!(
                    ":: Temp:  {} °C",
                    measurement.temperature.as_degrees_celsius()
                );
                println!(":: Humi:  {} %RH", measurement.humidity.as_percent());
                measurements.temperature = Some(measurement.temperature);
                measurements.humidity = Some(measurement.humidity);
            }
            Err(e) => eprintln!("Temp/Humi: ERROR: {:?}", e),
        }
    }

    // Read lux sensor, if present
    if let Some(ref mut veml) = sensors.lux {
        match veml.read_lux() {
            Ok(lux) => {
                println!(":: Lux:   {}", lux);
                measurements.illuminance = Some(lux);
            }
            Err(e) => eprintln!("Lux: ERROR: {:?}", e),
        }
    }
}

fn submit_measurements(measurements: &Measurements) -> anyhow::Result<()> {
    println!("-> Submitting measurements");

    // Create HTTP(S) client
    let mut client = HttpClient::wrap(EspHttpConnection::new(&HttpConfiguration {
        timeout: Some(Duration::from_secs(10)),
        crt_bundle_attach: Some(esp_idf_sys::esp_crt_bundle_attach), // Needed for HTTPS support
        ..Default::default()
    })?);

    // Prepare payload
    let mut lines = Vec::new();
    let tags = format!("name={},fw_version={}", SENSILO_NAME, VERSION);
    if let Some(temp) = measurements.temperature {
        let val = temp.as_degrees_celsius();
        lines.push(format!("temperature,{} celsius={:.2}", tags, val));
    }
    if let Some(humi) = measurements.humidity {
        let val = humi.as_percent();
        lines.push(format!("humidity,{} percent={:.2}", tags, val));
    }
    if let Some(lux) = measurements.illuminance {
        lines.push(format!("illumination,{} lux={:.2}", tags, lux));
    }
    if let Some(co2eq) = measurements.co2eq_ppm {
        lines.push(format!("co2,sensor_type=mox,{} ppm={}u", tags, co2eq));
    }
    if let Some(tvoc) = measurements.tvoc_ppb {
        lines.push(format!("tvoc,{} ppb={}u", tags, tvoc));
    }
    let payload: String = lines.join("\n").chars().collect();
    println!("Sending payload:\n{}", &payload);

    // Prepare headers and URL
    let authorization_header = format!("Token {}", SENSILO_INFLUXDB_API_TOKEN);
    let content_length_header = format!("{}", payload.len());
    let headers = [
        ("authorization", &*authorization_header),
        ("content-type", "text/plain; charset=utf-8"),
        ("content-length", &*content_length_header),
        ("accept", "application/json"),
        ("connection", "close"),
    ];
    let url = format!(
        "{}/api/v2/write?org={}&bucket={}",
        SENSILO_INFLUXDB_HOST.trim_end_matches('/'),
        SENSILO_INFLUXDB_ORG,
        SENSILO_INFLUXDB_BUCKET,
    );

    // Send request
    let mut request = client.post(&url, &headers)?;
    request.write_all(payload.as_bytes())?;
    request.flush()?;

    // Read response
    let mut response = request.submit()?;
    let status = response.status();
    let (_headers, mut body) = response.split();
    let success = status == 204;
    if success {
        println!("-> Data sent successfully to InfluxDB!");
    } else {
        eprintln!("-> Error: Server returned HTTP {}", status);
    }

    // Drain body, print it if not successful
    let mut buf = [0u8; 1024];
    if !success {
        let bytes_read = io::try_read_full(&mut body, &mut buf).map_err(|e| e.0)?;
        println!("  Read {} bytes", bytes_read);
        match std::str::from_utf8(&buf[0..bytes_read]) {
            Ok(body_string) => println!(
                "   Response body (truncated to {} bytes): {}",
                buf.len(),
                body_string
            ),
            Err(e) => eprintln!("  Error decoding response body: {}", e),
        };
    }
    while body.read(&mut buf)? > 0 {} // Drain the remaining response bytes
    println!();

    Ok(())
}
