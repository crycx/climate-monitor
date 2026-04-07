use alloc::string::String;
use core::fmt::Write;

pub struct SensorValue<'a> {
    pub sensor_type: &'a str,
    pub value: f32,
    pub unit: &'a str,
}

pub fn build_body(values: &[SensorValue<'_>]) -> String {
    let mut out = String::from("[");
    for (idx, value) in values.iter().enumerate() {
        if idx > 0 {
            out.push(',');
        }
        let _ = write!(
            out,
            r#"{{"sensor_type":"{}","value":{:.2},"unit":"{}"}}"#,
            value.sensor_type,
            value.value,
            value.unit
        );
    }
    out.push(']');
    out
}
