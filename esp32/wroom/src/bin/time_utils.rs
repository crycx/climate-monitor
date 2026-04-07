use alloc::string::String;

fn normalize_unix_seconds(value: u64) -> u64 {
    if value > 9_999_999_999 {
        value / 1000
    } else {
        value
    }
}

pub fn parse_unix_seconds_from_header_value(value: &[u8]) -> Option<u64> {
    let raw = core::str::from_utf8(value).ok()?.trim();
    let trimmed = raw.trim_matches(|c| c == '"' || c == '\'');

    if let Ok(parsed) = trimmed.parse::<u64>() {
        return Some(normalize_unix_seconds(parsed));
    }

    let mut acc: u64 = 0;
    let mut found_any_digit = false;
    for b in trimmed.as_bytes() {
        if b.is_ascii_digit() {
            found_any_digit = true;
            acc = acc.saturating_mul(10).saturating_add((b - b'0') as u64);
        } else if found_any_digit {
            break;
        }
    }

    if found_any_digit {
        Some(normalize_unix_seconds(acc))
    } else {
        None
    }
}

fn month_from_abbr(month: &str) -> Option<u32> {
    match month {
        "Jan" => Some(1),
        "Feb" => Some(2),
        "Mar" => Some(3),
        "Apr" => Some(4),
        "May" => Some(5),
        "Jun" => Some(6),
        "Jul" => Some(7),
        "Aug" => Some(8),
        "Sep" => Some(9),
        "Oct" => Some(10),
        "Nov" => Some(11),
        "Dec" => Some(12),
        _ => None,
    }
}

fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let y = year - if month <= 2 { 1 } else { 0 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let month_i = month as i32;
    let doy = (153 * (month_i + if month_i > 2 { -3 } else { 9 }) + 2) / 5 + day as i32 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era as i64 * 146_097 + doe as i64 - 719_468
}

fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if month <= 2 { 1 } else { 0 };
    (year as i32, month as u32, day as u32)
}

pub fn unix_to_utc_string(unix_sec: u64) -> String {
    let days = (unix_sec / 86_400) as i64;
    let sec_of_day = unix_sec % 86_400;
    let hour = sec_of_day / 3_600;
    let minute = (sec_of_day % 3_600) / 60;
    let second = sec_of_day % 60;
    let (year, month, day) = civil_from_days(days);

    alloc::format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
        year,
        month,
        day,
        hour,
        minute,
        second
    )
}

pub fn parse_unix_seconds_from_http_date(value: &[u8]) -> Option<u64> {
    let s = core::str::from_utf8(value).ok()?.trim();
    let mut parts = s.split_whitespace();

    let _weekday = parts.next()?;
    let day = parts.next()?.parse::<u32>().ok()?;
    let month = month_from_abbr(parts.next()?)?;
    let year = parts.next()?.parse::<i32>().ok()?;

    let time_part = parts.next()?;
    let mut hms = time_part.split(':');
    let hour = hms.next()?.parse::<u32>().ok()?;
    let minute = hms.next()?.parse::<u32>().ok()?;
    let second = hms.next()?.parse::<u32>().ok()?;

    let _tz = parts.next()?;

    let days = days_from_civil(year, month, day);
    if days < 0 {
        return None;
    }

    let day_seconds = (hour as u64)
        .saturating_mul(3600)
        .saturating_add((minute as u64).saturating_mul(60))
        .saturating_add(second as u64);

    Some((days as u64).saturating_mul(86_400).saturating_add(day_seconds))
}
