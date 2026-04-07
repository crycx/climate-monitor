use embedded_dht_rs::{dht22::Dht22, SensorError, SensorReading};
use embedded_hal::{
    delay::DelayNs,
    digital::{InputPin, OutputPin},
};
use esp_hal::gpio::{DriveMode, Flex, OutputConfig, Pin, Pull};

pub fn init_dht22_pin<'d>(pin: impl Pin + 'd) -> Flex<'d> {
    let mut dht22_pin = Flex::new(pin);
    dht22_pin.apply_output_config(
        &OutputConfig::default()
            .with_drive_mode(DriveMode::OpenDrain)
            .with_pull(Pull::None),
    );
    dht22_pin.set_output_enable(true);
    dht22_pin.set_input_enable(true);
    dht22_pin.set_high();
    dht22_pin
}

pub fn read<P, D>(sensor: &mut Dht22<P, D>) -> Result<SensorReading<f32>, SensorError>
where
    P: InputPin + OutputPin,
    D: DelayNs,
{
    sensor.read()
}
