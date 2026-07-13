//! `#[derive(Form)]` for `strata-forms`.
//!
//! Generates `impl strata_forms::Form` from a struct's named fields: string get/set of
//! each via `FormValue`, per-field validation, and `field_ids`. A form is the struct;
//! each field is an input. Field attributes:
//!
//! - `#[field(id = "custom.id")]` — the field's id (default: the field name). Lets the
//!   ids be stable/dotted keys independent of Rust identifiers.
//! - `#[field(validate = path::to::fn)]` — `fn(&FieldType) -> Result<(), String>`.
//! - `#[field(list)]` — a `Vec<_>` field; ids become `id[0]`, `id[1]`, ….
//! - `#[field(skip)]` — excluded from the form.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{parse_macro_input, DeriveInput};

#[proc_macro_derive(Form, attributes(field))]
pub fn derive_form(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    expand(input)
        .unwrap_or_else(|e| e.to_compile_error())
        .into()
}

struct FieldSpec {
    skip: bool,
    list: bool,
    id: Option<String>,
    validate: Option<syn::Path>,
}

fn parse_field_attrs(field: &syn::Field) -> syn::Result<FieldSpec> {
    let mut spec = FieldSpec {
        skip: false,
        list: false,
        id: None,
        validate: None,
    };
    for attr in &field.attrs {
        if !attr.path().is_ident("field") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("skip") {
                spec.skip = true;
                Ok(())
            } else if meta.path.is_ident("list") {
                spec.list = true;
                Ok(())
            } else if meta.path.is_ident("id") {
                spec.id = Some(meta.value()?.parse::<syn::LitStr>()?.value());
                Ok(())
            } else if meta.path.is_ident("validate") {
                spec.validate = Some(meta.value()?.parse::<syn::Path>()?);
                Ok(())
            } else {
                Err(meta.error("unknown #[field] key (expected id, skip, list, or validate)"))
            }
        })?;
    }
    Ok(spec)
}

fn expand(input: DeriveInput) -> syn::Result<TokenStream2> {
    let name = &input.ident;
    let (impl_g, ty_g, where_c) = input.generics.split_for_impl();

    let data = match &input.data {
        syn::Data::Struct(s) => s,
        _ => {
            return Err(syn::Error::new_spanned(
                &input,
                "#[derive(Form)] supports structs only",
            ))
        }
    };
    let fields = match &data.fields {
        syn::Fields::Named(n) => &n.named,
        _ => {
            return Err(syn::Error::new_spanned(
                &input,
                "#[derive(Form)] requires named fields",
            ))
        }
    };

    let mut get_scalar = Vec::new();
    let mut get_list = Vec::new();
    let mut set_scalar = Vec::new();
    let mut set_list = Vec::new();
    let mut val_scalar = Vec::new();
    let mut val_list = Vec::new();
    let mut id_pushes = Vec::new();

    for field in fields {
        let fname = field.ident.as_ref().expect("named field");
        let spec = parse_field_attrs(field)?;
        if spec.skip {
            continue;
        }
        let fid = spec.id.clone().unwrap_or_else(|| fname.to_string());

        if spec.list {
            let prefix = format!("{fid}[");
            get_list.push(quote! {
                if let ::core::option::Option::Some(rest) = id.strip_prefix(#prefix) {
                    if let ::core::option::Option::Some(idx) =
                        rest.strip_suffix(']').and_then(|s| s.parse::<usize>().ok())
                    {
                        return self.#fname.get(idx)
                            .map(|x| ::strata_forms::FormValue::to_field(x));
                    }
                }
            });
            set_list.push(quote! {
                if let ::core::option::Option::Some(rest) = id.strip_prefix(#prefix) {
                    if let ::core::option::Option::Some(idx) =
                        rest.strip_suffix(']').and_then(|s| s.parse::<usize>().ok())
                    {
                        if let ::core::option::Option::Some(slot) = self.#fname.get_mut(idx) {
                            if let ::core::result::Result::Ok(v) =
                                ::strata_forms::FormValue::from_field(raw)
                            {
                                *slot = v;
                            }
                        }
                        return;
                    }
                }
            });
            if let Some(vp) = &spec.validate {
                val_list.push(quote! {
                    if let ::core::option::Option::Some(rest) = id.strip_prefix(#prefix) {
                        if let ::core::option::Option::Some(idx) =
                            rest.strip_suffix(']').and_then(|s| s.parse::<usize>().ok())
                        {
                            return self.#fname.get(idx).and_then(|x| #vp(x).err());
                        }
                    }
                });
            }
            id_pushes.push(quote! {
                for i in 0..self.#fname.len() {
                    ids.push(::std::format!("{}[{}]", #fid, i));
                }
            });
        } else {
            get_scalar.push(quote! {
                #fid => return ::core::option::Option::Some(
                    ::strata_forms::FormValue::to_field(&self.#fname)
                ),
            });
            set_scalar.push(quote! {
                #fid => {
                    if let ::core::result::Result::Ok(v) =
                        ::strata_forms::FormValue::from_field(raw)
                    {
                        self.#fname = v;
                    }
                    return;
                }
            });
            if let Some(vp) = &spec.validate {
                val_scalar.push(quote! {
                    #fid => return #vp(&self.#fname).err(),
                });
            }
            id_pushes.push(quote! {
                ids.push(::std::string::String::from(#fid));
            });
        }
    }

    Ok(quote! {
        impl #impl_g ::strata_forms::Form for #name #ty_g #where_c {
            fn get_field(&self, id: &str) -> ::core::option::Option<::std::string::String> {
                match id {
                    #(#get_scalar)*
                    _ => {}
                }
                #(#get_list)*
                ::core::option::Option::None
            }

            fn set_field(&mut self, id: &str, raw: &str) {
                match id {
                    #(#set_scalar)*
                    _ => {}
                }
                #(#set_list)*
            }

            fn validate_field(&self, id: &str) -> ::core::option::Option<::std::string::String> {
                match id {
                    #(#val_scalar)*
                    _ => {}
                }
                #(#val_list)*
                ::core::option::Option::None
            }

            fn field_ids(&self) -> ::std::vec::Vec<::std::string::String> {
                let mut ids: ::std::vec::Vec<::std::string::String> = ::std::vec::Vec::new();
                #(#id_pushes)*
                ids
            }
        }
    })
}
