//! Shared helpers for config-storage proc macros.

use quote::quote;
use syn::meta::ParseNestedMeta;

/// FNV-1a 64-bit hash constants
pub const FNV_OFFSET: u64 = 0xcbf29ce484222325;
/// FNV-1a 64-bit prime
pub const FNV_PRIME: u64 = 0x100000001b3;

/// Reserved key for format magic value
pub const MAGIC_KEY: &str = "__magic__";
/// Reserved key for format version
pub const FORMAT_VERSION_KEY: &str = "__format_version__";

/// Compute FNV-1a 64-bit hash
pub fn fnv1a_hash(s: &str) -> u64 {
    let mut hash = FNV_OFFSET;
    for b in s.bytes() {
        hash ^= b as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// Capitalize each word and join with the given separator.
///
/// Splits on any non-alphanumeric character (underscore, space, hyphen, etc.)
/// so it works for snake_case, kebab-case, and multi-word strings alike.
pub fn snake_to_cased(s: &str, word_sep: &str) -> String {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut c = part.chars();
            match c.next() {
                None => String::new(),
                Some(f) => f.to_uppercase().chain(c).collect(),
            }
        })
        .collect::<Vec<_>>()
        .join(word_sep)
}

/// Convert snake_case to PascalCase for enum variant names
pub fn to_pascal_case(s: &str) -> String {
    snake_to_cased(s, "")
}

/// Humanize field name for default label: snake_case -> Title Case.
pub fn humanize_label(field_name: &str) -> String {
    snake_to_cased(field_name, " ")
}

/// PascalCase variant ident for a field (for ConfigKey etc.).
pub fn variant_ident_for_field(field_ident: &syn::Ident) -> syn::Ident {
    syn::Ident::new(
        &to_pascal_case(&field_ident.to_string()),
        field_ident.span(),
    )
}

/// Generates the bump statement token stream: `self.<bump_field> = self.<bump_field>.wrapping_add(1)` or empty.
pub fn bump_stmt(
    bump_field: Option<&String>,
    target_ident: &syn::Ident,
) -> proc_macro2::TokenStream {
    bump_field
        .map(|b| {
            let bump_ident = syn::Ident::new(b, target_ident.span());
            quote! {
                self.#bump_ident = self.#bump_ident.wrapping_add(1);
            }
        })
        .unwrap_or_else(|| quote! {})
}

/// Escape for HTML text content (label, help).
pub fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

/// Escape for use inside a JavaScript double-quoted string.
pub fn escape_js_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out
}

/// Convert page name to a valid Rust const suffix (e.g. "basic" -> "BASIC", "Home Assistant" -> "HOME_ASSISTANT").
///
/// Replaces any character that is not alphanumeric or `_` with `_`, then collapses
/// consecutive underscores into one.
pub fn page_name_to_suffix(page: &str) -> String {
    let raw: String = page
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();
    let mut result = String::with_capacity(raw.len());
    let mut prev_underscore = false;
    for c in raw.chars() {
        if c == '_' {
            if !prev_underscore {
                result.push('_');
            }
            prev_underscore = true;
        } else {
            result.push(c);
            prev_underscore = false;
        }
    }
    result
}

/// Convert page name to a valid JS identifier suffix (e.g. "Network" -> "Network", "my-page" -> "my_page").
///
/// Replaces any character that is not alphanumeric or `_` with `_`.
pub fn page_name_to_js_id(page: &str) -> String {
    page.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Parse a `name = "..."` meta value as a `String`. Returns `None` if missing or not a string literal.
pub fn try_parse_lit_str(meta: &ParseNestedMeta) -> Option<String> {
    meta.value()
        .and_then(|v| v.parse::<syn::LitStr>())
        .ok()
        .map(|l| l.value())
}

/// Parse a `name = <integer>` meta value, converting it to `T` via `base10_parse`.
pub fn try_parse_lit_int<T: std::str::FromStr>(meta: &ParseNestedMeta) -> Option<T>
where
    T::Err: std::fmt::Display,
{
    meta.value()
        .and_then(|v| v.parse::<syn::LitInt>())
        .ok()
        .and_then(|l| l.base10_parse().ok())
}

/// Consume an unrecognized name-value meta item so the parse stream advances past it.
pub fn consume_meta_value(meta: &ParseNestedMeta) {
    let _ = meta.value().and_then(|v| v.parse::<syn::Expr>());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_to_pascal_case_and_humanize_label() {
        assert_eq!(to_pascal_case("wifi_pass"), "WifiPass");
        assert_eq!(humanize_label("wifi_pass"), "Wifi Pass");
        assert_eq!(to_pascal_case("foo"), "Foo");
        assert_eq!(humanize_label("foo"), "Foo");
        assert_eq!(to_pascal_case("Home Assistant"), "HomeAssistant");
        assert_eq!(humanize_label("Home Assistant"), "Home Assistant");
        assert_eq!(to_pascal_case("my-page"), "MyPage");
    }

    #[test]
    fn test_escape_html() {
        assert_eq!(escape_html("a & b"), "a &amp; b");
        assert_eq!(escape_html("<tag>"), "&lt;tag&gt;");
        assert_eq!(escape_html(r#" "quoted" "#), " &quot;quoted&quot; ");
    }

    #[test]
    fn test_escape_js_str() {
        // Input: space, backslash, space, double-quote -> backslash and quote are escaped
        assert_eq!(escape_js_str(r#" \ ""#), " \\\\ \\\"");
        assert_eq!(escape_js_str("a\nb"), r#"a\nb"#);
        assert_eq!(escape_js_str("a\rb"), r#"a\rb"#);
    }

    #[test]
    fn test_page_name_to_suffix() {
        assert_eq!(page_name_to_suffix("basic"), "BASIC");
        assert_eq!(page_name_to_suffix("my-page"), "MY_PAGE");
        assert_eq!(page_name_to_suffix("Home Assistant"), "HOME_ASSISTANT");
        assert_eq!(page_name_to_suffix("a--b"), "A_B");
        assert_eq!(page_name_to_suffix("PrusaLink"), "PRUSALINK");
    }

    #[test]
    fn test_page_name_to_js_id() {
        assert_eq!(page_name_to_js_id("Network"), "Network");
        assert_eq!(page_name_to_js_id("my-page"), "my_page");
        assert_eq!(page_name_to_js_id("main"), "main");
        assert_eq!(page_name_to_js_id("Home Assistant"), "Home_Assistant");
    }

    #[test]
    fn test_fnv1a_hash_stability() {
        let h = fnv1a_hash("wifi_ssid");
        assert_eq!(h, fnv1a_hash("wifi_ssid"));
        assert_ne!(h, fnv1a_hash("wifi_pass"));
    }
}
