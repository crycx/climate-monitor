use alloc::string::String;
use core::fmt::Write;

pub struct SensorValue<'a> {
    pub sensor_type: &'a str,
    pub value: f32,
    pub unit: &'a str,
    pub location: Option<&'a str>,
}

pub fn build_body(values: &[SensorValue<'_>]) -> String {
    let mut out = String::from("[");
    for (idx, value) in values.iter().enumerate() {
        if idx > 0 {
            out.push(',');
        }
        let _ = write!(
            out,
            r#"{{"sensor_type":"{}","value":{:.2},"unit":"{}""#,
            value.sensor_type,
            value.value,
            value.unit
        );
        if let Some(location) = value.location.filter(|loc| !loc.is_empty()) {
            let escaped_location = location.replace('"', "\\\"");
            let _ = write!(out, r#",\"location\":\"{}\""#, escaped_location);
        }
        out.push_str("}");
    }
    out.push(']');
    out
}
