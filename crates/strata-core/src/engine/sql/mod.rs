//! The **SQL language service** (S26) — one analysis layer over the SQL buffer that
//! backs both autocomplete (S7) and validation (S25). See `docs/SQL_LANGUAGE_SPEC.md`.
//!
//! Layers:
//! - [`lex`] — tokenise via DataFusion's own `sqlparser` (byte spans + kinds).
//! - [`context`] — split statements + classify the caret's clause context.
//! - [`symbols`] — the [`Catalog`] (tables/views/columns from `state.project` +
//!   registered functions) and in-statement alias resolution.
//! - [`validate`] — [`validate::validate`] the full diagnostics pass: lexical lints +
//!   managed-DDL policy + the engine **dry-plan** (parse → resolve → analyze against
//!   the live `SessionContext`, never executing). Byte-spanned for squiggles.
//! - [`complete`] — [`complete`] ranked completions for a caret position.
//!
//! Completion resolves against a [`Catalog`] snapshot (cheap to build on the UI
//! thread); validation runs engine-side via [`Engine::validate`](crate::engine::Engine)
//! so unknown tables/columns/functions and type faults are the *same* errors a Run
//! would hit.

pub mod complete;
pub mod context;
mod fuzzy;
pub mod lex;
pub mod symbols;
pub mod validate;

pub use complete::{complete, Completion, CompletionKind};
pub use lex::is_word_char;
pub use symbols::Catalog;
pub use validate::validate;

/// The engine's registered functions (built-ins + any UDFs), by category — names
/// only. Pushed once from the engine on startup (`engine::Event::Functions`, F5)
/// and held on the per-window [`Engine`](crate::engine::Engine); folded into a
/// [`Catalog`] for completion + validation.
#[derive(Clone, Default, PartialEq)]
pub struct FunctionCatalog {
    pub scalar: Vec<String>,
    pub aggregate: Vec<String>,
    pub window: Vec<String>,
}

impl FunctionCatalog {
    /// Whether `name` (case-insensitive) is a registered function of any category.
    pub fn contains(&self, name: &str) -> bool {
        let n = name.to_ascii_lowercase();
        self.scalar.iter().any(|f| f.eq_ignore_ascii_case(&n))
            || self.aggregate.iter().any(|f| f.eq_ignore_ascii_case(&n))
            || self.window.iter().any(|f| f.eq_ignore_ascii_case(&n))
    }

    /// All names across categories (for a pooled completion fallback).
    pub fn all(&self) -> impl Iterator<Item=&String> {
        self.scalar
            .iter()
            .chain(self.aggregate.iter())
            .chain(self.window.iter())
    }
}
