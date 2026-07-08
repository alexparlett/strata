//! The **SQL language service** (S26) — one analysis layer over the SQL buffer that
//! backs both autocomplete (S7) and validation (S25). See `docs/SQL_LANGUAGE_SPEC.md`.
//!
//! Layers:
//! - [`lex`] — tokenise via DataFusion's own `sqlparser` (byte spans + kinds).
//! - [`context`] — split statements + classify the caret's clause context.
//! - [`symbols`] — the [`Catalog`] (tables/views/columns from `state.project` +
//!   registered functions) and in-statement alias resolution.
//! - [`validate`] — [`analyze`] structural + lint diagnostics (feeds `crate::diagnostics`).
//! - [`complete`] — [`complete`] ranked completions for a caret position.
//!
//! Nothing here touches the engine or the UI directly: callers pass a [`Catalog`]
//! snapshot (cheap to build on the UI thread) and get plain data back.

pub mod complete;
pub mod context;
pub mod lex;
pub mod symbols;
pub mod validate;

pub use complete::{complete, Completion, CompletionKind};
pub use symbols::{Catalog, ColumnSym, TableSym};
pub use validate::analyze;

/// The engine's registered functions (built-ins + any UDFs), by category — names
/// only. Pushed once from the engine on startup (`engine::Event::Functions`, F5)
/// and held on `AppState`; folded into a [`Catalog`] for completion + validation.
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
    pub fn all(&self) -> impl Iterator<Item = &String> {
        self.scalar
            .iter()
            .chain(self.aggregate.iter())
            .chain(self.window.iter())
    }
}
