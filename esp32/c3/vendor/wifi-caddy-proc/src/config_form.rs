//! Config form codegen for `WifiCaddyConfig`: HTML/JS for the config UI.

use crate::utils::{
    consume_meta_value, escape_html, escape_js_str, humanize_label, page_name_to_js_id,
    page_name_to_suffix, try_parse_lit_str,
};
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::DeriveInput;

// ---------------------------------------------------------------------------
// Intermediate representation
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct FormField {
    name: String,
    page: String,
    fieldset: Option<String>,
    hidden: bool,
    label: String,
    help: String,
    class: Option<String>,
    field_type: syn::Type,
    min: Option<String>,
    max: Option<String>,
    input_type: Option<String>,
}

// ---------------------------------------------------------------------------
// Attribute helpers
// ---------------------------------------------------------------------------

/// Converts a parsed expression (min/max attr value) to a string for HTML attributes.
/// Handles LitInt (including negative via Unary minus), LitFloat, and LitStr.
fn expr_to_min_max_string(expr: &syn::Expr) -> Option<String> {
    match expr {
        syn::Expr::Unary(unary) if matches!(unary.op, syn::UnOp::Neg(_)) => {
            expr_to_min_max_string(&unary.expr).map(|s| format!("-{s}"))
        }
        syn::Expr::Lit(expr_lit) => match &expr_lit.lit {
            syn::Lit::Int(i) => i.base10_parse::<i64>().ok().map(|n| n.to_string()),
            syn::Lit::Float(f) => f.base10_parse::<f64>().ok().map(|n| n.to_string()),
            syn::Lit::Str(s) => Some(s.value()),
            _ => None,
        },
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Phase 1 – parse `#[config_form(...)]` on each struct field
// ---------------------------------------------------------------------------

fn parse_form_fields(data: &syn::DataStruct) -> Vec<FormField> {
    let mut form_fields = Vec::new();

    for field in &data.fields {
        let ident = field.ident.as_ref().expect("unnamed fields not supported");
        let field_name = ident.to_string();

        let mut has_config_form = false;
        let mut skip = false;
        let mut page = String::from("main");
        let mut fieldset: Option<String> = None;
        let mut hidden = false;
        let mut label = humanize_label(&field_name);
        let mut help = String::new();
        let mut class: Option<String> = None;
        let mut input_type: Option<String> = None;
        let mut min: Option<String> = None;
        let mut max: Option<String> = None;

        for attr in &field.attrs {
            if attr.path().is_ident("config_form") {
                has_config_form = true;
                let _ = attr.parse_nested_meta(|meta| {
                    if meta.path.is_ident("skip") {
                        skip = true;
                    } else if meta.path.is_ident("hidden") {
                        hidden = true;
                    } else if meta.path.is_ident("page") {
                        if let Some(v) = try_parse_lit_str(&meta) {
                            page = v;
                        }
                    } else if meta.path.is_ident("fieldset") {
                        fieldset = try_parse_lit_str(&meta);
                    } else if meta.path.is_ident("label") {
                        if let Some(v) = try_parse_lit_str(&meta) {
                            label = v;
                        }
                    } else if meta.path.is_ident("help") {
                        if let Some(v) = try_parse_lit_str(&meta) {
                            help = v;
                        }
                    } else if meta.path.is_ident("class") {
                        class = try_parse_lit_str(&meta);
                    } else if meta.path.is_ident("input_type") {
                        input_type = try_parse_lit_str(&meta);
                    } else if meta.path.is_ident("min") {
                        if let Ok(expr) = meta.value().and_then(|v| v.parse::<syn::Expr>()) {
                            min = expr_to_min_max_string(&expr);
                        }
                    } else if meta.path.is_ident("max") {
                        if let Ok(expr) = meta.value().and_then(|v| v.parse::<syn::Expr>()) {
                            max = expr_to_min_max_string(&expr);
                        }
                    } else {
                        // Consume unrecognized meta so the parse stream advances to next attr
                        consume_meta_value(&meta);
                    }
                    Ok(())
                });
            }
        }

        if !has_config_form || skip {
            continue;
        }

        form_fields.push(FormField {
            name: field_name,
            page,
            fieldset,
            hidden,
            label,
            help,
            class,
            field_type: field.ty.clone(),
            min,
            max,
            input_type,
        });
    }

    form_fields
}

// ---------------------------------------------------------------------------
// Phase 2 – HTML segment generation
// ---------------------------------------------------------------------------

/// Generate the HTML segments for a visible (non-hidden) input field.
/// Returns several &str fragments that are concatenated at runtime in the browser.
fn gen_visible_input_html(f: &FormField) -> Vec<TokenStream> {
    let mut segs = Vec::new();

    let fname = &f.name;
    let ftype = &f.field_type;
    let label_esc = escape_html(&f.label);
    let field_class = f
        .class
        .as_ref()
        .map(|c| format!(" {}", escape_html(c)))
        .unwrap_or_default();
    let wrapper_class = format!("config-form-group config-form-field-{}", f.name);
    let min_attr = f
        .min
        .as_ref()
        .map(|s| format!(r#" min="{}""#, escape_html(s)))
        .unwrap_or_default();
    let max_attr = f
        .max
        .as_ref()
        .map(|s| format!(r#" max="{}""#, escape_html(s)))
        .unwrap_or_default();
    let help_esc = escape_html(&f.help);

    // Opening div + label + `<input type="`
    let html_prefix = format!(
        r#"<div class="{}"><label for="{}" class="config-form-label" style="color:var(--config-form-label-color,#555)">{}</label><input type=""#,
        wrapper_class, fname, label_esc
    );
    let prefix_lit = syn::LitStr::new(&html_prefix, proc_macro2::Span::call_site());
    segs.push(quote! { #prefix_lit });

    // input type: either a literal (e.g. "password") or a const from ConfigValue
    if let Some(ref it) = f.input_type {
        let input_lit = syn::LitStr::new(it, proc_macro2::Span::call_site());
        segs.push(quote! { #input_lit });
    } else {
        segs.push(quote! {
            <#ftype as wifi_caddy::config_storage::ConfigValue>::DEFAULT_INPUT_TYPE
        });
    }

    // step="any" for floats
    segs.push(quote! {
        if <#ftype as wifi_caddy::config_storage::ConfigValue>::IS_FLOAT {
            " step=\"any\""
        } else {
            ""
        }
    });

    // required attribute (omitted for password inputs)
    let req_str = match f.input_type.as_deref() {
        Some("password") => "",
        _ => " required",
    };
    let req_lit = syn::LitStr::new(req_str, proc_macro2::Span::call_site());
    segs.push(quote! { #req_lit });

    // Closing attributes, help div, closing div
    let html_suffix = format!(
        r#"" id="{}" name="{}" class="config-form-input config-form-input-{}{}" style="border:var(--config-form-input-border,2px solid #ddd)"{}{}><div class="config-form-help" style="color:var(--config-form-help-color,#888)">{}</div></div>"#,
        fname, fname, fname, field_class, min_attr, max_attr, help_esc
    );
    let suffix_lit = syn::LitStr::new(&html_suffix, proc_macro2::Span::call_site());
    segs.push(quote! { #suffix_lit });

    segs
}

/// Build the HTML segment expression array for one page.
fn gen_html_segments(fields: &[FormField]) -> Vec<TokenStream> {
    let mut segs: Vec<TokenStream> = Vec::new();
    segs.push(quote! { "<div class=\"config-form\">" });

    let mut current_fieldset: Option<&str> = None;
    for f in fields {
        // Emit <fieldset><legend> when fieldset changes; close previous fieldset first
        let fieldset_changed = current_fieldset.as_deref() != f.fieldset.as_deref();
        if fieldset_changed {
            if current_fieldset.is_some() {
                segs.push(quote! { "</fieldset>" });
            }
            current_fieldset = f.fieldset.as_deref();
            if let Some(legend) = current_fieldset {
                let legend_html = format!(
                    "<fieldset class=\"config-form-fieldset\" style=\"border:var(--config-form-fieldset-border,2px solid #e0e0e0)\"><legend class=\"config-form-legend\" style=\"color:var(--config-form-legend-color,#667eea)\">{}</legend>",
                    escape_html(legend)
                );
                let lit = syn::LitStr::new(&legend_html, proc_macro2::Span::call_site());
                segs.push(quote! { #lit });
            }
        }

        if f.hidden {
            // Hidden fields: single <input type="hidden">
            let name_esc = escape_html(&f.name);
            let hidden_html = format!(r#"<input type="hidden" id="{0}" name="{0}">"#, name_esc);
            let lit = syn::LitStr::new(&hidden_html, proc_macro2::Span::call_site());
            segs.push(quote! { #lit });
        } else {
            segs.extend(gen_visible_input_html(f));
        }
    }

    if current_fieldset.is_some() {
        segs.push(quote! { "</fieldset>" });
    }
    segs.push(quote! { "</div>" });

    segs
}

// ---------------------------------------------------------------------------
// Phase 3 – JS segment generation
// ---------------------------------------------------------------------------

/// Build the JS segment expression array for one page: loadConfig_<page> + saveConfig_<page>.
fn gen_js_segments(page_name: &str, fields: &[FormField]) -> Vec<TokenStream> {
    let mut segs: Vec<TokenStream> = Vec::new();
    let page_js_id = page_name_to_js_id(page_name);
    let page_esc = escape_js_str(page_name);
    let form_id = format!("configForm-{}", page_js_id);
    let form_id_esc = escape_js_str(&form_id);
    let load_fn = format!("loadConfig_{}", page_js_id);
    let save_fn = format!("saveConfig_{}", page_js_id);

    // loadConfig_<page> prologue: fetch JSON and populate form fields
    let js_prologue = format!(
        "const CONFIG_PAGE_{0}=\"{1}\";const CONFIG_URL_{0}=\"/config-group/\"+CONFIG_PAGE_{0};async function {2}(){{const response=await fetch(CONFIG_URL_{0});if(!response.ok)throw new Error(\"HTTP \"+response.status);const data=await response.json();",
        page_js_id, page_esc, load_fn
    );
    let prologue_lit = syn::LitStr::new(&js_prologue, proc_macro2::Span::call_site());
    segs.push(quote! { #prologue_lit });

    // Per-field load: `el.value = data[name] ?? ""`
    for f in fields {
        let name_js = escape_js_str(&f.name);
        let fname = &f.name;
        let load_stmt = format!(
            "var el=document.getElementById(\"{0}\");if(el)el.value=data[\"{1}\"]!==undefined?String(data[\"{1}\"]):\"\";",
            fname, name_js
        );
        let load_lit = syn::LitStr::new(&load_stmt, proc_macro2::Span::call_site());
        segs.push(quote! { #load_lit });
    }

    // saveConfig_<page> prologue: read FormData into `data` object
    let save_start = format!(
        "}} async function {0}(){{const form=document.getElementById(\"{1}\");if(!form)return;const formData=new FormData(form);const data={{}};",
        save_fn, form_id_esc
    );
    let save_start_lit = syn::LitStr::new(&save_start, proc_macro2::Span::call_site());
    segs.push(quote! { #save_start_lit });

    // Per-field save: JS_SAVE_KIND picks String/Int/Float for formData→data conversion
    for f in fields {
        let name_js = escape_js_str(&f.name);
        let fname = &f.name;
        let ftype = &f.field_type;
        let str_line = format!("data[\"{}\"]=formData.get(\"{}\")??\"\";", name_js, fname);
        let int_line = format!(
            "data[\"{}\"]=parseInt(formData.get(\"{}\"),10);",
            name_js, fname
        );
        let float_line = format!(
            "data[\"{}\"]=parseFloat(formData.get(\"{}\"));",
            name_js, fname
        );
        let str_lit = syn::LitStr::new(&str_line, proc_macro2::Span::call_site());
        let int_lit = syn::LitStr::new(&int_line, proc_macro2::Span::call_site());
        let float_lit = syn::LitStr::new(&float_line, proc_macro2::Span::call_site());
        segs.push(quote! {
            match <#ftype as wifi_caddy::config_storage::ConfigValue>::JS_SAVE_KIND {
                wifi_caddy::config_storage::JsSaveKind::String => #str_lit,
                wifi_caddy::config_storage::JsSaveKind::Int => #int_lit,
                wifi_caddy::config_storage::JsSaveKind::Float => #float_lit,
            }
        });
    }

    // saveConfig_<page> epilogue: POST data to config endpoint; register both fns on window
    let fetch_line = format!(
        "const response=await fetch(CONFIG_URL_{0}+\"?set=\"+encodeURIComponent(JSON.stringify(data)),{{method:\"GET\"}});if(!response.ok)throw new Error(await response.text()||\"HTTP \"+response.status);}};window.{1}=window.{1}||{1};window.{2}=window.{2}||{2};",
        page_js_id, load_fn, save_fn
    );
    let fetch_lit = syn::LitStr::new(&fetch_line, proc_macro2::Span::call_site());
    segs.push(quote! { #fetch_lit });

    segs
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Builds the form half of `WifiCaddyConfig`: HTML and JS for the config UI.
///
/// Reads field-level `#[config_form(...)]`: `skip`, `hidden`, `fieldset = "Legend"`, `label`,
/// `help`, `class`, `input_type` (e.g. `"password"` for string fields), `min`/`max` for numerics.
/// A field is only in the form if it has `#[config_form]`.
///
/// Emits: const `FORM_HTML_<PAGE>_SEGMENTS` and `FORM_JS_<PAGE>_SEGMENTS` (arrays of `&str`),
/// and `ConfigFormGen` with `html_segments_for_group` / `js_segments_for_group` for zero-allocation streaming.
pub fn derive_config_form_impl(input: &DeriveInput) -> TokenStream {
    let name = &input.ident;

    let syn::Data::Struct(data) = &input.data else {
        return syn::Error::new_spanned(input, "ConfigForm only supports structs")
            .to_compile_error();
    };

    let form_fields = parse_form_fields(data);

    // Group fields by page name
    let mut pages: std::collections::BTreeMap<String, Vec<FormField>> =
        std::collections::BTreeMap::new();
    for f in form_fields {
        pages.entry(f.page.clone()).or_default().push(f);
    }

    // Validate: no fieldset spans multiple pages
    let mut fieldset_pages: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
    for (page, fields) in &pages {
        for f in fields {
            if let Some(ref fs) = f.fieldset {
                if let Some(existing_page) = fieldset_pages.get(fs) {
                    if existing_page != page {
                        return syn::Error::new_spanned(
                            input,
                            format!(
                                "fieldset \"{}\" appears on pages \"{}\" and \"{}\"; groups cannot be split across pages",
                                fs, existing_page, page
                            ),
                        )
                        .to_compile_error();
                    }
                } else {
                    fieldset_pages.insert(fs.clone(), page.clone());
                }
            }
        }
    }

    let page_names: Vec<&str> = pages.keys().map(String::as_str).collect();
    let page_names_lits: Vec<syn::LitStr> = page_names
        .iter()
        .map(|s| syn::LitStr::new(s, proc_macro2::Span::call_site()))
        .collect();

    let mut const_html_defs = Vec::new();
    let mut const_js_defs = Vec::new();
    let mut html_match_arms = Vec::new();
    let mut js_match_arms = Vec::new();

    for (page_name, fields) in &pages {
        let suffix = page_name_to_suffix(page_name);
        let html_const_name = format_ident!("FORM_HTML_{}_SEGMENTS", suffix);
        let js_const_name = format_ident!("FORM_JS_{}_SEGMENTS", suffix);

        let html_segment_exprs = gen_html_segments(fields);
        let js_segment_exprs = gen_js_segments(page_name, fields);

        const_html_defs.push(quote! {
            const #html_const_name: &[&str] = &[#(#html_segment_exprs),*];
        });
        const_js_defs.push(quote! {
            const #js_const_name: &[&str] = &[#(#js_segment_exprs),*];
        });

        let page_lit = syn::LitStr::new(page_name, proc_macro2::Span::call_site());
        html_match_arms.push(quote! { #page_lit => Some(Self::#html_const_name), });
        js_match_arms.push(quote! { #page_lit => Some(Self::#js_const_name), });
    }

    let default_arm = quote! { _ => None };
    quote! {
        impl #name {
            const PAGE_NAMES: &[&str] = &[#(#page_names_lits),*];
            #(#const_html_defs)*
            #(#const_js_defs)*
        }

        impl wifi_caddy::config_storage::ConfigFormGen for #name {
            fn page_names() -> &'static [&'static str] {
                Self::PAGE_NAMES
            }

            fn html_segments_for_group(group: &str) -> Option<&'static [&'static str]> {
                match group {
                    #(#html_match_arms)*
                    #default_arm
                }
            }

            fn js_segments_for_group(group: &str) -> Option<&'static [&'static str]> {
                match group {
                    #(#js_match_arms)*
                    #default_arm
                }
            }
        }
    }
}
