
use esp_idf_sys::{self as _};
use ssd1306::Ssd1306; // If using the `binstart` feature of `esp-idf-sys`, always keep this module imported

use std::{cell::RefCell, env, sync::atomic::*, sync::Arc, thread, time::*};

use bme280::BME280;
use bme280::Measurements;
use esp_idf_hal::prelude::*;
use esp_idf_hal::{i2c, delay};
use esp_idf_hal::i2c::I2cError;
use embedded_svc::wifi::*;
use embedded_hal::blocking::delay::DelayMs;
use ssd1306;
use ssd1306::mode::DisplayConfig;

use embedded_graphics::mono_font::{ascii::FONT_10X20, MonoTextStyle};
use embedded_graphics::pixelcolor::*;
use embedded_graphics::prelude::*;
use embedded_graphics::text::*;
use embedded_graphics::mono_font::ascii::*;

use embedded_svc::eth;
use embedded_svc::eth::{Eth, TransitionalState};
use embedded_svc::httpd::registry::*;
use embedded_svc::httpd::*;
use embedded_svc::io;
use embedded_svc::ipv4;
use embedded_svc::mqtt::client::{Client, Connection, MessageImpl, Publish, QoS};
use embedded_svc::ping::Ping;
use embedded_svc::sys_time::SystemTime;
use embedded_svc::timer::TimerService;
use embedded_svc::timer::*;
use embedded_svc::wifi::*;

use esp_idf_svc::eth::*;
use esp_idf_svc::eventloop::*;
use esp_idf_svc::eventloop::*;
use esp_idf_svc::httpd as idf;
use esp_idf_svc::httpd::ServerRegistry;
use esp_idf_svc::mqtt::client::*;
use esp_idf_svc::netif::*;
use esp_idf_svc::nvs::*;
use esp_idf_svc::ping;
use esp_idf_svc::sntp;
use esp_idf_svc::sysloop::*;
use esp_idf_svc::systime::EspSystemTime;
use esp_idf_svc::timer::*;
use esp_idf_svc::wifi::*;
use esp_idf_sys::*;

use log::*;
use anyhow::bail;

const SSID: &str = "TelekomSlovenije_4e46";
const PASS: &str = "drcuk421";

fn wifi(
    netif_stack: Arc<EspNetifStack>,
    sys_loop_stack: Arc<EspSysLoopStack>,
    default_nvs: Arc<EspDefaultNvs>,
) -> Result<Box<EspWifi>> {
    let mut wifi = Box::new(EspWifi::new(netif_stack, sys_loop_stack, default_nvs)?);

    info!("Wifi created, about to scan");

    let ap_infos = wifi.scan()?;

    let ours = ap_infos.into_iter().find(|a| a.ssid == SSID);

    let channel = if let Some(ours) = ours {
        info!(
            "Found configured access point {} on channel {}",
            SSID, ours.channel
        );
        Some(ours.channel)
    } else {
        info!(
            "Configured access point {} not found during scanning, will go with unknown channel",
            SSID
        );
        None
    };
    
    wifi.set_configuration(&Configuration::Mixed(
        ClientConfiguration {
            ssid: SSID.into(),
            password: PASS.into(),
            channel,
            ..Default::default()
        },
        AccessPointConfiguration {
            ssid: "aptest".into(),
            channel: channel.unwrap_or(1),
            ..Default::default()
        },
    ))?;

    info!("Wifi configuration set, about to get status");

    wifi.wait_status_with_timeout(Duration::from_secs(20), |status| !status.is_transitional())
        .map_err(|e| anyhow::anyhow!("Unexpected Wifi status: {:?}", e))?;

    let status = wifi.get_status();

    if let Status(
        ClientStatus::Started(ClientConnectionStatus::Connected(ClientIpStatus::Done(ip_settings))),
        ApStatus::Started(ApIpStatus::Done),
    ) = status
    {
        info!("Wifi connected");

        ping(&ip_settings)?;
    } else {
        bail!("Unexpected Wifi status: {:?}", status);
    }

    Ok(wifi)
}

fn ping(ip_settings: &ipv4::ClientSettings) -> Result<()> {
    info!("About to do some pings for {:?}", ip_settings);

    let ping_summary =
        ping::EspPing::default().ping(ip_settings.subnet.gateway, &Default::default())?;
    if ping_summary.transmitted != ping_summary.received {
        bail!(
            "Pinging gateway {} resulted in timeouts",
            ip_settings.subnet.gateway
        );
    }

    info!("Pinging done");

    Ok(())
}


fn main() {
    // Temporary. Will disappear once ESP-IDF 4.4 is released, but for now it is necessary to call this function once,
    // or else some patches to the runtime implemented by esp-idf-sys might not link properly.
    esp_idf_sys::link_patches();

    let netif_stack = Arc::new(EspNetifStack::new().unwrap());
    let sys_loop_stack = Arc::new(EspSysLoopStack::new().unwrap());
    let default_nvs = Arc::new(EspDefaultNvs::new().unwrap());

    let mut wifi = wifi(
        netif_stack.clone(),
        sys_loop_stack.clone(),
        default_nvs.clone(),
    ).unwrap();

    let peripherals = Peripherals::take().unwrap();
    let pins = peripherals.pins;

    let i2c_pins = i2c::MasterPins { 
        sda: pins.gpio33,
        scl: pins.gpio32,
    };
    let i2c_config = i2c::config::MasterConfig::default();
    
    let i2c = i2c::Master::new(
        peripherals.i2c0, 
        i2c_pins,
        i2c_config,
    ).unwrap();

    
    let mut bme280 = BME280::new_primary(i2c, delay::Ets);
    // initialize the sensor
    bme280.init().unwrap();


    let i2c_pins = i2c::MasterPins { 
        sda: pins.gpio25,
        scl: pins.gpio26,
    };
    let i2c_config = i2c::config::MasterConfig::default();
    let i2c = i2c::Master::new(
        peripherals.i2c1, 
        i2c_pins,
        i2c_config,
    ).unwrap();

    let di = ssd1306::I2CDisplayInterface::new(i2c);
    let mut display = ssd1306::Ssd1306::new(
        di,
        ssd1306::size::DisplaySize128x64,
        ssd1306::rotation::DisplayRotation::Rotate0,
    )
    .into_buffered_graphics_mode();
    display
        .init()
        .map_err(|e| anyhow::anyhow!("Display error: {:?}", e)).unwrap();


    // measure temperature, pressure, and humidity
    let measurements = bme280.measure().unwrap();

    println!("Relative Humidity = {}%", measurements.humidity);
    println!("Temperature = {} deg C", measurements.temperature);
    println!("Pressure = {} pascals", measurements.pressure);

    display.clear();

    Text::new(
        format!("Temperature: {} C", measurements.temperature).as_str(),
        Point::new(1, 10),
        MonoTextStyle::new(&FONT_5X7, Rgb565::WHITE.into()),
    )
    .draw(&mut display).unwrap();

    Text::new(
        format!("Humidity: {} %", measurements.humidity).as_str(),
        Point::new(1, 20),
        MonoTextStyle::new(&FONT_5X7, Rgb565::WHITE.into()),
    )
    .draw(&mut display).unwrap();

    Text::new(
        format!("Pressure: {} pascals", measurements.pressure).as_str(),
        Point::new(1, 30),
        MonoTextStyle::new(&FONT_5X7, Rgb565::WHITE.into()),
    )
    .draw(&mut display).unwrap();

    display
    .flush()
    .map_err(|e| anyhow::anyhow!("Display error: {:?}", e)).unwrap();

    println!("Displayed on the display");

    let conf = MqttClientConfiguration {
        client_id: Some("esp32"),

        ..Default::default()
    };

    let (mut client, mut connection) =
        EspMqttClient::new_with_conn("mqtt://192.168.1.18:1883", &conf).unwrap();

    println!("Connected to MQTT broker");

    thread::spawn(move || {
        info!("MQTT Listening for messages");

        while let Some(msg) = connection.next() {
            match msg {
                Err(e) => info!("MQTT Message ERROR: {}", e),
                Ok(msg) => info!("MQTT Message: {:?}", msg),
            }
        }

        info!("MQTT connection loop exit");
    });

    client.publish(
        "home/temperature",
        QoS::AtMostOnce,
        false,
        format!("{}", measurements.temperature).as_bytes(),
    ).unwrap();

    client.publish(
        "home/humidity",
        QoS::AtMostOnce,
        false,
        format!("{}", measurements.humidity).as_bytes(),
    ).unwrap();


    client.publish(
        "home/pressure",
        QoS::AtMostOnce,
        false,
        format!("{}", measurements.pressure).as_bytes(),
    ).unwrap();

    println!("Finished, going to sleep!");

    drop(client);
    drop(wifi);

    unsafe {
        esp_sleep_enable_timer_wakeup((60*1000000) as u64);
        esp_deep_sleep_start();
    }
    
}
