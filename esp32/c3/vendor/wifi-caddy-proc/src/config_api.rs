//! Group API and notify codegen for `WifiCaddyConfig`: JSON get/set, optional channel, config statics.

use crate::utils::{consume_meta_value, to_pascal_case, try_parse_lit_int, try_parse_lit_str};
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::DeriveInput;

// ---------------------------------------------------------------------------
// Intermediate representation
// ---------------------------------------------------------------------------

/// Logical field for ConfigApi: name, type, page, notify variant, and whether to redact/skip-if-empty.
#[derive(Clone)]
struct ApiField {
    ident: syn::Ident,
    ty: syn::Type,
    page: String,
    is_password: bool,
    /// From config_store(notify = "Wifi") or notify_group = "wifi" (PascalCase); variant for ConfigChange.
    notify: Option<String>,
}

/// All values parsed from struct-level attributes on the annotated config struct.
struct StructAttrs {
    /// Whether `#[config_server(...)]` was present at all (needed to emit the esp block).
    config_server_present: bool,
    /// `storage_magic = 0x...` from `#[config_server]`.
    storage_magic: Option<u32>,
    /// `storage_version = N` from `#[config_server]`.
    storage_version: Option<u32>,
    /// Whether `#[config_notify]` was present.
    notify_channel: bool,
    /// `cap = N` from `#[config_notify]`.
    notify_cap: Option<usize>,
    /// UI string overrides from `#[config_ui(...)]`.
    ui_page_heading: Option<String>,
    ui_title: Option<String>,
    ui_subtitle: Option<String>,
    ui_nav_left: Option<String>,
    ui_nav_right: Option<String>,
    /// `extra_css = "..."` from `#[config_ui]`; custom CSS appended after built-in stylesheet.
    ui_extra_css: Option<String>,
    /// `extra_js = "..."` from `#[config_ui]`; custom JavaScript appended after built-in scripts.
    ui_extra_js: Option<String>,
    /// `default_group = "..."` from `#[config_ui]`; which tab to show first.
    ui_default_group: Option<String>,
}

// ---------------------------------------------------------------------------
// Phase 1 – parse struct-level attributes
// ---------------------------------------------------------------------------

fn parse_struct_attrs(attrs: &[syn::Attribute]) -> StructAttrs {
    let mut config_server_present = false;
    let mut storage_magic: Option<u32> = None;
    let mut storage_version: Option<u32> = None;
    let mut notify_channel = false;
    let mut notify_cap: Option<usize> = None;
    let mut ui_page_heading: Option<String> = None;
    let mut ui_title: Option<String> = None;
    let mut ui_subtitle: Option<String> = None;
    let mut ui_nav_left: Option<String> = None;
    let mut ui_nav_right: Option<String> = None;
    let mut ui_extra_css: Option<String> = None;
    let mut ui_extra_js: Option<String> = None;
    let mut ui_default_group: Option<String> = None;

    for attr in attrs {
        if attr.path().is_ident("config_server") {
            config_server_present = true;
            let _ = attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("storage_magic") {
                    storage_magic = try_parse_lit_int(&meta);
                } else if meta.path.is_ident("storage_version") {
                    storage_version = try_parse_lit_int(&meta);
                } else {
                    consume_meta_value(&meta);
                }
                Ok(())
            });
        } else if attr.path().is_ident("config_notify") {
            notify_channel = true;
            if let syn::Meta::List(_) = &attr.meta {
                let _ = attr.parse_nested_meta(|meta| {
                    if meta.path.is_ident("cap") {
                        notify_cap = try_parse_lit_int(&meta);
                    } else {
                        consume_meta_value(&meta);
                    }
                    Ok(())
                });
            }
        } else if attr.path().is_ident("config_ui") {
            let _ = attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("page_heading") {
                    ui_page_heading = try_parse_lit_str(&meta);
                } else if meta.path.is_ident("title") {
                    ui_title = try_parse_lit_str(&meta);
                } else if meta.path.is_ident("subtitle") {
                    ui_subtitle = try_parse_lit_str(&meta);
                } else if meta.path.is_ident("nav_left") {
                    ui_nav_left = try_parse_lit_str(&meta);
                } else if meta.path.is_ident("nav_right") {
                    ui_nav_right = try_parse_lit_str(&meta);
                } else if meta.path.is_ident("extra_css") {
                    ui_extra_css = try_parse_lit_str(&meta);
                } else if meta.path.is_ident("extra_js") {
                    ui_extra_js = try_parse_lit_str(&meta);
                } else if meta.path.is_ident("default_group") {
                    ui_default_group = try_parse_lit_str(&meta);
                } else {
                    consume_meta_value(&meta);
                }
                Ok(())
            });
        }
    }

    StructAttrs {
        config_server_present,
        storage_magic,
        storage_version,
        notify_channel,
        notify_cap,
        ui_page_heading,
        ui_title,
        ui_subtitle,
        ui_nav_left,
        ui_nav_right,
        ui_extra_css,
        ui_extra_js,
        ui_default_group,
    }
}

// ---------------------------------------------------------------------------
// Phase 2 – collect ApiFields from struct fields
// ---------------------------------------------------------------------------

fn collect_api_fields(fields: &syn::Fields) -> Vec<ApiField> {
    let mut api_fields = Vec::new();

    for field in fields {
        let field_ident = field.ident.as_ref().expect("unnamed fields not supported");

        let mut has_config_form = false;
        let mut skip = false;
        let mut page = String::from("main");
        let mut input_type = String::from("text");
        let mut notify: Option<String> = None;

        // from config_form: input_type (password → redact in GET), skip
        for attr in &field.attrs {
            if attr.path().is_ident("config_form") {
                has_config_form = true;
                let _ = attr.parse_nested_meta(|meta| {
                    if meta.path.is_ident("skip") {
                        skip = true;
                    } else if meta.path.is_ident("page") {
                        if let Some(v) = try_parse_lit_str(&meta) {
                            page = v;
                        }
                    } else if meta.path.is_ident("input_type") {
                        if let Some(v) = try_parse_lit_str(&meta) {
                            input_type = v;
                        }
                    } else {
                        // Unrecognized (e.g. fieldset, help): consume so stream advances
                        consume_meta_value(&meta);
                    }
                    Ok(())
                });
            }
        }

        // from config_store: notify / notify_group → ConfigChange variant name
        for attr in &field.attrs {
            if attr.path().is_ident("config_store") {
                let _ = attr.parse_nested_meta(|meta| {
                    if meta.path.is_ident("notify") {
                        notify = try_parse_lit_str(&meta);
                    } else if meta.path.is_ident("notify_group") {
                        // Backward compat: convert to PascalCase variant name
                        if let Ok(expr) = meta.value().and_then(|v| v.parse::<syn::Expr>()) {
                            if let syn::Expr::Lit(expr_lit) = expr {
                                if let syn::Lit::Str(s) = expr_lit.lit {
                                    notify = Some(to_pascal_case(&s.value()));
                                }
                            }
                        }
                    } else {
                        consume_meta_value(&meta);
                    }
                    Ok(())
                });
            }
        }

        if !has_config_form || skip {
            continue;
        }

        api_fields.push(ApiField {
            ident: field_ident.clone(),
            ty: field.ty.clone(),
            page,
            is_password: input_type == "password",
            notify,
        });
    }

    api_fields
}

// ---------------------------------------------------------------------------
// Phase 3 – ConfigChange enum
// ---------------------------------------------------------------------------

/// Collect distinct notify variant names and emit the ConfigChange enum.
/// Uses __None placeholder when no fields have a notify variant (EnumSet needs at least one variant).
fn gen_config_change_enum(
    pages: &std::collections::BTreeMap<String, Vec<ApiField>>,
) -> TokenStream {
    let mut variant_names: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for fields in pages.values() {
        for f in fields {
            if let Some(ref v) = f.notify {
                variant_names.insert(v.clone());
            }
        }
    }
    let variant_idents: Vec<syn::Ident> = variant_names
        .iter()
        .map(|s| syn::Ident::new(s, proc_macro2::Span::call_site()))
        .collect();

    // EnumSetType provides Clone, Copy, PartialEq, Eq; repr = "u64" for bitflags-style sets
    if variant_idents.is_empty() {
        quote! {
            #[derive(enumset::EnumSetType)]
            #[enumset(repr = "u64")]
            pub enum ConfigChange {
                #[doc(hidden)]
                __None,
            }
        }
    } else {
        let variants = variant_idents.iter().map(|v| quote! { #v });
        quote! {
            #[derive(enumset::EnumSetType)]
            #[enumset(repr = "u64")]
            pub enum ConfigChange {
                #(#variants),*
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Phase 4 – per-page DTOs and ConfigApi match arms
// ---------------------------------------------------------------------------

/// Returns (dto_structs, get_group_json arms, set_group_json arms).
fn gen_dto_and_group_arms(
    name: &syn::Ident,
    pages: &std::collections::BTreeMap<String, Vec<ApiField>>,
) -> (Vec<TokenStream>, Vec<TokenStream>, Vec<TokenStream>) {
    let mut dto_structs = Vec::new();
    let mut get_arms = Vec::new();
    let mut set_arms = Vec::new();

    for (page_name, fields) in pages {
        let dto_name = format_ident!("{}Config", to_pascal_case(page_name));
        let page_lit = syn::LitStr::new(page_name, proc_macro2::Span::call_site());

        // DTO struct: one public field per config field, serialisable to/from JSON
        let dto_fields = fields.iter().map(|f| {
            let i = &f.ident;
            let t = &f.ty;
            quote! { pub #i: #t }
        });
        let dto_doc = format!("DTO for config page \"{}\".", page_name);
        dto_structs.push(quote! {
            #[doc = #dto_doc]
            #[derive(serde::Serialize, serde::Deserialize)]
            pub struct #dto_name {
                #(#dto_fields),*
            }
        });

        // GET: password fields return Default::default() (redacted); others clone the value
        let get_dto_fields = fields.iter().map(|f| {
            let i = &f.ident;
            if f.is_password {
                quote! { #i: Default::default() }
            } else {
                quote! { #i: self.#i.clone() }
            }
        });
        get_arms.push(quote! {
            #page_lit => {
                let dto = #dto_name { #(#get_dto_fields),* };
                let len = serde_json_core::to_slice(&dto, buf)
                    .map_err(|_| wifi_caddy::config_storage::ConfigError::InvalidData)?;
                Ok(len)
            }
        });

        // SET: compare before applying each field; accumulate changed ConfigChange variants
        let set_compare_apply: Vec<TokenStream> = fields
            .iter()
            .map(|f| {
                let i = &f.ident;
                let setter = format_ident!("set_{}", i);
                let field_ty = &f.ty;
                let insert_line: TokenStream = f
                    .notify
                    .as_ref()
                    .map(|v| {
                        let variant_ident = syn::Ident::new(v, proc_macro2::Span::call_site());
                        quote! { changed.insert(ConfigChange::#variant_ident); }
                    })
                    .unwrap_or_else(|| quote! {});

                if f.is_password {
                    // Skip applying if the form sent an empty password (keep existing value)
                    quote! {
                        if dto.#i != <#field_ty as Default>::default() {
                            if self.#i != dto.#i {
                                #insert_line
                                self.#setter(dto.#i.clone());
                            }
                        }
                    }
                } else {
                    quote! {
                        if self.#i != dto.#i {
                            #insert_line
                            self.#setter(dto.#i.clone());
                        }
                    }
                }
            })
            .collect();

        let _ = name; // suppress unused warning; name is used in the ConfigApi impl below
        set_arms.push(quote! {
            #page_lit => {
                let (dto, _) = serde_json_core::from_str::<#dto_name>(json)
                    .map_err(|_| wifi_caddy::config_storage::ConfigError::InvalidData)?;
                let mut changed = enumset::EnumSet::<ConfigChange>::new();
                #(#set_compare_apply)*
                Ok(changed)
            }
        });
    }

    (dto_structs, get_arms, set_arms)
}

// ---------------------------------------------------------------------------
// Phase 5 – set_field arms (single key=value from HTTP query string)
// ---------------------------------------------------------------------------

fn gen_set_field_arms(
    pages: &std::collections::BTreeMap<String, Vec<ApiField>>,
) -> Vec<TokenStream> {
    pages
        .values()
        .flat_map(|fields| fields.iter())
        .map(|f| {
            let key_str = f.ident.to_string();
            let key_lit = syn::LitStr::new(&key_str, proc_macro2::Span::call_site());
            let i = &f.ident;
            let setter = format_ident!("set_{}", i);
            let field_ty = &f.ty;

            // Build the compare-and-apply body, optionally inserting a ConfigChange variant
            let variant_ts = f.notify.as_ref().map(|v| {
                let vid = syn::Ident::new(v, proc_macro2::Span::call_site());
                quote! { ConfigChange::#vid }
            });
            let parse_and_apply = match &variant_ts {
                Some(v) => quote! {
                    if let Ok(parsed) = value.parse::<#field_ty>() {
                        if self.#i != parsed {
                            self.#setter(parsed);
                            let mut changed = enumset::EnumSet::<ConfigChange>::new();
                            changed.insert(#v);
                            Ok(Some(changed))
                        } else {
                            Ok(Some(enumset::EnumSet::new()))
                        }
                    } else {
                        Err(wifi_caddy::config_storage::ConfigError::InvalidData)
                    }
                },
                None => quote! {
                    if let Ok(parsed) = value.parse::<#field_ty>() {
                        if self.#i != parsed {
                            self.#setter(parsed);
                        }
                        Ok(Some(enumset::EnumSet::new()))
                    } else {
                        Err(wifi_caddy::config_storage::ConfigError::InvalidData)
                    }
                },
            };

            if f.is_password {
                // Empty value → keep existing; non-empty → parse and apply
                quote! {
                    #key_lit => {
                        if value.is_empty() {
                            Ok(Some(enumset::EnumSet::new()))
                        } else {
                            #parse_and_apply
                        }
                    }
                }
            } else {
                quote! {
                    #key_lit => { #parse_and_apply }
                }
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Phase 6 – notify channel (Channel + helper fns)
// ---------------------------------------------------------------------------

fn gen_notify_channel(attrs: &StructAttrs, num_pages: usize) -> TokenStream {
    if !attrs.notify_channel {
        return quote! {};
    }

    let cap_val = attrs.notify_cap.unwrap_or(num_pages);
    let cap_lit = proc_macro2::Literal::usize_unsuffixed(cap_val);

    quote! {
        type ConfigUpdateChannel = embassy_sync::channel::Channel<
            embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
            enumset::EnumSet<ConfigChange>,
            #cap_lit,
        >;
        pub type ConfigUpdateReceiver = &'static ConfigUpdateChannel;
        static CONFIG_UPDATE_CHANNEL: static_cell::StaticCell<ConfigUpdateChannel> =
            static_cell::StaticCell::new();
        static mut CONFIG_UPDATE_CHANNEL_REF: Option<&'static ConfigUpdateChannel> = None;

        fn config_update_notify(changed: enumset::EnumSet<ConfigChange>) {
            if let Some(ch) = unsafe { CONFIG_UPDATE_CHANNEL_REF } {
                let _ = ch.try_send(changed);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Phase 7 – storage_params / ui_options as associated fns (always emitted)
// ---------------------------------------------------------------------------

fn gen_config_statics(attrs: &StructAttrs) -> TokenStream {
    if !attrs.config_server_present {
        return quote! {};
    }

    let storage_magic_val = attrs.storage_magic.unwrap_or(0x4255_aa42);
    let storage_version_val = attrs.storage_version.unwrap_or(1);
    let storage_magic_lit = proc_macro2::Literal::u32_unsuffixed(storage_magic_val);
    let storage_version_lit = proc_macro2::Literal::u32_unsuffixed(storage_version_val);

    let ui_default_group_val = attrs.ui_default_group.as_deref().unwrap_or("main");
    let ui_page_heading_val = attrs.ui_page_heading.as_deref().unwrap_or("Configuration");
    let ui_title_val = attrs.ui_title.as_deref().unwrap_or("Configuration");
    let ui_subtitle_val = attrs.ui_subtitle.as_deref().unwrap_or("");
    let ui_nav_left_val = attrs
        .ui_nav_left
        .as_deref()
        .unwrap_or("<span>Configuration</span>");
    let ui_nav_right_val = attrs.ui_nav_right.as_deref().unwrap_or("<span></span>");
    let ui_extra_css_val = attrs.ui_extra_css.as_deref().unwrap_or("");
    let ui_extra_js_val = attrs.ui_extra_js.as_deref().unwrap_or("");

    let ui_default_group_lit =
        syn::LitStr::new(ui_default_group_val, proc_macro2::Span::call_site());
    let ui_page_heading_lit = syn::LitStr::new(ui_page_heading_val, proc_macro2::Span::call_site());
    let ui_title_lit = syn::LitStr::new(ui_title_val, proc_macro2::Span::call_site());
    let ui_subtitle_lit = syn::LitStr::new(ui_subtitle_val, proc_macro2::Span::call_site());
    let ui_nav_left_lit = syn::LitStr::new(ui_nav_left_val, proc_macro2::Span::call_site());
    let ui_nav_right_lit = syn::LitStr::new(ui_nav_right_val, proc_macro2::Span::call_site());
    let ui_extra_css_lit = syn::LitStr::new(ui_extra_css_val, proc_macro2::Span::call_site());
    let ui_extra_js_lit = syn::LitStr::new(ui_extra_js_val, proc_macro2::Span::call_site());

    quote! {
        #[doc(hidden)]
        pub fn __storage_params() -> wifi_caddy::ConfigStorageParams {
            wifi_caddy::ConfigStorageParams {
                magic: #storage_magic_lit,
                format_version: #storage_version_lit,
            }
        }

        #[doc(hidden)]
        pub fn __ui_options() -> wifi_caddy::ConfigUiOptions {
            wifi_caddy::ConfigUiOptions {
                default_group: #ui_default_group_lit,
                page_heading: #ui_page_heading_lit,
                title: #ui_title_lit,
                subtitle: #ui_subtitle_lit,
                nav_left: #ui_nav_left_lit,
                nav_right: #ui_nav_right_lit,
                extra_css: #ui_extra_css_lit,
                extra_js: #ui_extra_js_lit,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Builds the group API, optional notify channel, and config statics for `WifiCaddyConfig`.
///
/// Struct-level attributes: `#[config_server(storage_magic, storage_version)]`,
/// `#[config_notify(cap)]`, `#[config_ui(page_heading, title, subtitle, nav_left, nav_right)]`.
/// Field-level: from `#[config_form]` we use `skip` and `input_type` (password → redacted in GET);
/// from `#[config_store]`, `notify = "Wifi"` or `notify_group = "wifi"` add a `ConfigChange` variant.
///
/// Emits: per-page DTOs (e.g. `MainConfig`) for JSON, `ConfigChange` enum, `ConfigApi` impl;
/// if `#[config_notify]`, the channel types, `config_update_notify` (private), and
/// `MyConfig::__init_config_update_channel()` (returns `ConfigUpdateReceiver`);
/// if `#[config_server]`, `MyConfig::__storage_params()` and `MyConfig::__ui_options()`
/// (referencing `wifi_caddy::*`).
///
/// Always emits `MyConfig::__init_config_update_channel()` (returns `()` when notify is off).
///
/// All generated code references only `wifi_caddy::*` — no platform-specific types.
/// Platform crates (e.g. `esp-wifi-caddy`) provide `wifi_init!` macros that call the
/// `#[doc(hidden)]` associated functions.
pub fn derive_config_api_impl(input: &DeriveInput) -> TokenStream {
    let name = &input.ident;

    let syn::Data::Struct(data) = &input.data else {
        return syn::Error::new_spanned(input, "ConfigApi only supports structs")
            .to_compile_error();
    };

    let attrs = parse_struct_attrs(&input.attrs);
    let api_fields = collect_api_fields(&data.fields);

    // Group fields by page name (single-page: all "main")
    let mut pages: std::collections::BTreeMap<String, Vec<ApiField>> =
        std::collections::BTreeMap::new();
    for f in api_fields {
        pages.entry(f.page.clone()).or_default().push(f);
    }

    let config_change_enum = gen_config_change_enum(&pages);
    let (dto_structs, get_arms, set_arms) = gen_dto_and_group_arms(name, &pages);
    let set_field_arms = gen_set_field_arms(&pages);
    let notify_channel_block = gen_notify_channel(&attrs, pages.len());
    let config_statics_block = gen_config_statics(&attrs);

    let on_updated_body = if attrs.notify_channel {
        quote! { Some(&config_update_notify) }
    } else {
        quote! { None }
    };

    let (init_channel_ret_type, init_channel_body) = if attrs.notify_channel {
        (
            quote! { ConfigUpdateReceiver },
            quote! {
                let channel_ref = CONFIG_UPDATE_CHANNEL.init(ConfigUpdateChannel::new());
                unsafe { CONFIG_UPDATE_CHANNEL_REF = Some(channel_ref) };
                channel_ref
            },
        )
    } else {
        (quote! { () }, quote! { () })
    };

    let default_err = quote! { _ => Err(wifi_caddy::config_storage::ConfigError::InvalidData) };

    quote! {
        #(#dto_structs)*

        #config_change_enum

        impl wifi_caddy::config_storage::ConfigApi for #name {
            type Error = wifi_caddy::config_storage::ConfigError;
            type ChangedSet = enumset::EnumSet<ConfigChange>;

            fn get_group_json(&self, group: &str, buf: &mut [u8]) -> Result<usize, Self::Error> {
                match group {
                    #(#get_arms)*
                    #default_err
                }
            }

            fn set_group_json(&mut self, group: &str, json: &str) -> Result<Self::ChangedSet, Self::Error> {
                match group {
                    #(#set_arms)*
                    #default_err
                }
            }

            fn set_field(&mut self, key: &str, value: &str) -> Result<Option<Self::ChangedSet>, Self::Error> {
                match key {
                    #(#set_field_arms),*,
                    _ => Ok(None),
                }
            }
        }

        #notify_channel_block

        impl #name {
            #[doc(hidden)]
            pub fn __config_on_updated() -> Option<&'static (dyn Fn(
                <Self as wifi_caddy::config_storage::ConfigApi>::ChangedSet,
            ) + Send)> {
                #on_updated_body
            }

            #[doc(hidden)]
            pub fn __init_config_update_channel() -> #init_channel_ret_type {
                #init_channel_body
            }

            #config_statics_block
        }
    }
}
