//! Pure helpers for the portal's HTML form (host-testable).

use alloc::{string::String, vec::Vec};

pub fn url_decode(input: &[u8]) -> String {
    let hex = |b: u8| match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    };
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        match input[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < input.len() => {
                match (hex(input[i + 1]), hex(input[i + 2])) {
                    (Some(h), Some(l)) => {
                        out.push(h << 4 | l);
                        i += 2;
                    }
                    _ => out.push(b'%'),
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn parse_form(body: &[u8]) -> Vec<(String, String)> {
    body.split(|b| *b == b'&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| match pair.iter().position(|b| *b == b'=') {
            Some(eq) => (url_decode(&pair[..eq]), url_decode(&pair[eq + 1..])),
            None => (url_decode(pair), String::new()),
        })
        .collect()
}

pub fn html_escape(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}
