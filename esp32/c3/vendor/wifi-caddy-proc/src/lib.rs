#![doc = include_str!("../README.md")]
#![warn(missing_docs)]
#![warn(clippy::all)]

mod config_api;
mod config_form;
mod config_store;
mod utils;

use proc_macro::TokenStream;
use syn::parse_macro_input;

/// Derive macro for WiFi caddy config structs.
///
/// Generates storage (load/store, keys, accessors), form HTML/JS for the config UI,
/// and the group API for the HTTP handler. Use `#[config_server(...)]`, `#[config_notify]`,
/// and `#[config_ui(...)]` on the struct for options. All generated code references only
/// `wifi_caddy::*` (no platform-specific dependencies).
#[proc_macro_derive(
    WifiCaddyConfig,
    attributes(config_store, config_form, config_server, config_notify, config_ui)
)]
pub fn derive_wifi_caddy_config(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as syn::DeriveInput);
    // Three codegen passes, all emitted into the same module:
    // 1. store: ConfigKey enum, load/store, getters/setters
    let store = config_store::derive_config_store_impl(&input);
    // 2. form: HTML/JS segments for config UI
    let form = config_form::derive_config_form_impl(&input);
    // 3. group: ConfigApi, ConfigChange, notify channel, esp-wifi-caddy helpers
    let group = config_api::derive_config_api_impl(&input);
    proc_macro::TokenStream::from(quote::quote! {
        #store
        #form
        #group
    })
}

#[cfg(test)]
mod tests {
    use syn::parse_str;

    /// Asserts that `input_type = "password"` is recognized when other name-value pairs
    /// (page, fieldset, help) appear before it in `#[config_form(...)]`. Without consuming
    /// unrecognized meta values, the parse stream does not advance and input_type stays "text".
    #[test]
    fn config_form_password_recognized_after_fieldset_and_help() {
        let input: syn::DeriveInput = parse_str(
            r#"
            struct S {
                #[config_form(fieldset = "WiFi", input_type = "password", help = "Secret")]
                wifi_pass: String,
            }
            "#,
        )
        .unwrap();
        let syn::Data::Struct(data) = &input.data else {
            panic!("expected struct");
        };
        let field = data.fields.iter().next().unwrap();
        let attr = field
            .attrs
            .iter()
            .find(|a| a.path().is_ident("config_form"))
            .unwrap();
        let mut input_type = String::from("text");
        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("input_type") {
                if let Ok(lit) = meta.value().and_then(|v| v.parse::<syn::LitStr>()) {
                    input_type = lit.value();
                }
            } else {
                // Consume (page, fieldset, help, etc.) so stream advances to input_type
                let _ = meta.value().and_then(|v| v.parse::<syn::Expr>());
            }
            Ok(())
        });
        assert_eq!(
            input_type, "password",
            "input_type must be recognized so GET /config-group/main redacts the field"
        );
    }
}
