//! **One declaration site per command** — the attribute pair behind Strata's command palette,
//! modelled on rmcp's `#[tool_router]` / `#[tool]` (`strata-agent/src/tools.rs`).
//!
//! ```ignore
//! #[command_router]
//! impl PaletteCommands {
//!     /// Execute current SQL
//!     #[command(label = "Run query", icon = IconName::Play, key = Command::RunQuery,
//!               keywords = "execute press")]
//!     fn run_query(ctx: &PaletteCtx) { … }
//! }
//! ```
//!
//! What it takes from rmcp is the **derivation**: identity read off the method, prose off the doc
//! comment, so a command's id and description are never typed twice. What it leaves behind is the
//! dispatch — rmcp resolves `HashMap<name, Arc<dyn Fn>>` because a client names a tool over a wire,
//! while a palette already holds the row the user picked. So this generates an **enum**, which
//! makes dispatch total by construction (no "registered but unrunnable" state) and lets a route be
//! a function pointer in a `const` slice rather than a boxed closure.
//!
//! ## The contract
//!
//! - Every `#[command]` method is an associated fn taking one argument, the context (`fn(&Ctx)`);
//!   no receiver, since the context *is* the state. Methods without `#[command]` are left alone.
//! - The invoking module must define a `CommandRoute` struct with exactly these fields:
//!   `id` · `label` · `sub` · `icon` · `key` · `keywords` · `call`. Their types are the caller's
//!   business — this crate only names the fields, which keeps it free of Strata's vocabulary.
//! - `#[command(…)]` accepts `label` (string literal, required), `icon` (expression, required),
//!   `key` (expression, optional) and `keywords` (string literal, optional). It is not a macro of
//!   its own: [`command_router`] consumes it, so a `#[command]` outside such a block is rustc's own
//!   "cannot find attribute".
//!
//! Generated: `Action` (one variant per command, in declaration order) with `ALL`, `route`, `id`,
//! `label`, `sub`, `keywords` and `run`; and `ROUTES`, the `const` slice `route` indexes. `icon`
//! and `key` are reached through `route()`, because this crate cannot name their types.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::punctuated::Punctuated;
use syn::{
    parse_macro_input, Attribute, Error, Expr, ExprLit, ImplItem, ItemImpl, Lit, Meta,
    MetaNameValue, Token, Type,
};

/// Collect every `#[command]` method in the block into an `Action` enum and a `ROUTES` slice.
/// See the crate docs for the contract and the generated items.
#[proc_macro_attribute]
pub fn command_router(args: TokenStream, input: TokenStream) -> TokenStream {
    let mut block = parse_macro_input!(input as ItemImpl);
    if !args.is_empty() {
        return Error::new_spanned(
            TokenStream2::from(args),
            "command_router takes no arguments; the generated items are `Action` and `ROUTES`",
        )
        .to_compile_error()
        .into();
    }
    match expand(&mut block) {
        Ok(generated) => quote!(#block #generated).into(),
        Err(e) => e.to_compile_error().into(),
    }
}

/// One command, as read off its method.
struct Command {
    /// The variant name — the method ident in PascalCase (`run_query` → `RunQuery`).
    variant: proc_macro2::Ident,
    /// The method ident, which is also the command's stable id.
    method: proc_macro2::Ident,
    /// The doc comment, one line.
    sub: String,
    label: syn::LitStr,
    icon: Expr,
    key: Option<Expr>,
    keywords: Option<syn::LitStr>,
}

fn expand(block: &mut ItemImpl) -> Result<TokenStream2, Error> {
    let mut commands = Vec::new();
    let mut ctx: Option<Type> = None;

    for item in &mut block.items {
        let ImplItem::Fn(method) = item else { continue };
        let Some(index) = method.attrs.iter().position(is_command) else {
            continue;
        };
        let attr = method.attrs.remove(index);
        let sub = doc_line(&method.attrs);

        if method.sig.receiver().is_some() {
            return Err(Error::new_spanned(
                &method.sig,
                "a command takes only its context, not a receiver — the context is the state",
            ));
        }
        let mut inputs = method.sig.inputs.iter();
        let (Some(syn::FnArg::Typed(arg)), None) = (inputs.next(), inputs.next()) else {
            return Err(Error::new_spanned(
                &method.sig,
                "a command takes exactly one argument: the context it acts through",
            ));
        };
        if ctx.is_none() {
            ctx = Some((*arg.ty).clone());
        }

        commands.push(parse_command(&attr, &method.sig.ident, sub)?);
    }

    let Some(ctx) = ctx else {
        return Err(Error::new_spanned(
            &*block,
            "a command router with no #[command] methods registers nothing",
        ));
    };
    let owner = &block.self_ty;

    let variants = commands.iter().map(|c| {
        let (variant, label) = (&c.variant, &c.label);
        quote!(#[doc = #label] #variant)
    });
    let all = commands.iter().map(|c| {
        let variant = &c.variant;
        quote!(Action::#variant)
    });
    let routes = commands.iter().map(|c| {
        let Command {
            method,
            sub,
            label,
            icon,
            key,
            keywords,
            ..
        } = c;
        let id = method.to_string();
        let key = match key {
            Some(key) => quote!(Some(#key)),
            None => quote!(None),
        };
        let keywords = match keywords {
            Some(keywords) => quote!(#keywords),
            None => quote!(""),
        };
        quote! {
            CommandRoute {
                id: #id,
                label: #label,
                sub: #sub,
                icon: #icon,
                key: #key,
                keywords: #keywords,
                call: #owner::#method,
            }
        }
    });

    Ok(quote! {
        /// Every command the palette can run, generated from the bodies that run them
        /// (`strata_command_macro::command_router`). Declaration order is offer order.
        #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
        pub enum Action {
            #(#variants,)*
        }

        /// Each command's metadata beside the function that performs it, in `Action`'s own order —
        /// so `ROUTES[action as usize]` is that action's route, and there is no lookup.
        pub const ROUTES: &[CommandRoute] = &[ #(#routes,)* ];

        impl Action {
            /// Every command, in the order the palette offers them.
            pub const ALL: &'static [Action] = &[ #(#all,)* ];

            /// This command's route — and the way to reach the fields this macro names no
            /// accessors for (`icon`, `key`).
            pub fn route(self) -> &'static CommandRoute {
                &ROUTES[self as usize]
            }

            /// The stable id: the method's own name.
            pub fn id(self) -> &'static str {
                self.route().id
            }

            /// What the row is called.
            pub fn label(self) -> &'static str {
                self.route().label
            }

            /// What it says under its name — the method's doc comment.
            pub fn sub(self) -> &'static str {
                self.route().sub
            }

            /// Words that should find it but appear in neither its label nor its subtext.
            pub fn keywords(self) -> &'static str {
                self.route().keywords
            }

            /// Perform it.
            pub fn run(self, ctx: #ctx) {
                (self.route().call)(ctx)
            }
        }
    })
}

fn is_command(attr: &Attribute) -> bool {
    attr.path().is_ident("command")
}

/// Read `label` / `icon` / `key` / `keywords` off one `#[command(…)]`.
fn parse_command(
    attr: &Attribute,
    method: &proc_macro2::Ident,
    sub: String,
) -> Result<Command, Error> {
    let args = attr.parse_args_with(Punctuated::<MetaNameValue, Token![,]>::parse_terminated)?;
    let (mut label, mut icon, mut key, mut keywords) = (None, None, None, None);
    for arg in &args {
        let Some(name) = arg.path.get_ident() else {
            return Err(Error::new_spanned(
                &arg.path,
                "expected a `name = value` pair",
            ));
        };
        match name.to_string().as_str() {
            "label" => label = Some(string(&arg.value, "label")?),
            "keywords" => keywords = Some(string(&arg.value, "keywords")?),
            "icon" => icon = Some(arg.value.clone()),
            "key" => key = Some(arg.value.clone()),
            _ => {
                return Err(Error::new_spanned(
                    name,
                    "unknown option: a command carries `label`, `icon`, `key` and `keywords`",
                ))
            }
        }
    }
    let (Some(label), Some(icon)) = (label, icon) else {
        return Err(Error::new_spanned(
            attr,
            "a command needs at least `label = \"…\"` and `icon = …`",
        ));
    };
    Ok(Command {
        variant: format_ident!("{}", pascal(&method.to_string()), span = method.span()),
        method: method.clone(),
        sub,
        label,
        icon,
        key,
        keywords,
    })
}

/// The value of a `name = "…"` pair, which has to be a literal string.
fn string(value: &Expr, name: &str) -> Result<syn::LitStr, Error> {
    match value {
        Expr::Lit(ExprLit {
            lit: Lit::Str(s), ..
        }) => Ok(s.clone()),
        other => Err(Error::new_spanned(
            other,
            format!("`{name}` must be a string literal"),
        )),
    }
}

/// The doc comment as one line: each line trimmed, blanks dropped, joined with a space. A row's
/// subtext is a single run of text, so a `\n` in it would be a line break nothing renders.
fn doc_line(attrs: &[Attribute]) -> String {
    let mut lines = Vec::new();
    for attr in attrs.iter().filter(|a| a.path().is_ident("doc")) {
        let Meta::NameValue(nv) = &attr.meta else {
            continue;
        };
        let Expr::Lit(ExprLit {
            lit: Lit::Str(s), ..
        }) = &nv.value
        else {
            continue;
        };
        let line = s.value();
        let line = line.trim();
        if !line.is_empty() {
            lines.push(line.to_string());
        }
    }
    lines.join(" ")
}

/// `run_query` → `RunQuery`.
fn pascal(snake: &str) -> String {
    snake
        .split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::pascal;

    #[test]
    fn a_method_name_becomes_its_variant() {
        assert_eq!(pascal("run_query"), "RunQuery");
        assert_eq!(pascal("settings"), "Settings");
        assert_eq!(pascal("new_table_or_source"), "NewTableOrSource");
    }
}
