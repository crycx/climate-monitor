use crate::time_utils::{parse_unix_seconds_from_header_value, parse_unix_seconds_from_http_date};

fn is_server_time_header(name: &str) -> bool {
    name.eq_ignore_ascii_case("x-server-time")
        || name.eq_ignore_ascii_case("x-server-unix")
        || name.eq_ignore_ascii_case("x-server-unix-time")
        || name.eq_ignore_ascii_case("x-unix-time")
        || name.eq_ignore_ascii_case("x-timestamp")
        || name.eq_ignore_ascii_case("server-time")
        || name.eq_ignore_ascii_case("server-unix")
}

pub fn extract_server_unix_from_headers<'a, I>(headers: I) -> Option<u64>
where
    I: IntoIterator<Item = (&'a str, &'a [u8])>,
{
    let mut server_unix_sec: Option<u64> = None;
    let mut date_header_raw: Option<&[u8]> = None;

    for (name, value) in headers {
        if is_server_time_header(name) {
            server_unix_sec = parse_unix_seconds_from_header_value(value);
        }

        if name.eq_ignore_ascii_case("date") {
            date_header_raw = Some(value);
        }
    }

    if server_unix_sec.is_none() {
        if let Some(date_raw) = date_header_raw {
            server_unix_sec = parse_unix_seconds_from_http_date(date_raw);
        }
    }

    server_unix_sec
}
