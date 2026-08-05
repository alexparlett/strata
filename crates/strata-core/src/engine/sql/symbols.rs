//! The symbol model the language service resolves against: tables + views (with
//! their columns) projected from `state.project`, plus the registered functions
//! (from the engine, F5). Cheap to build on the UI thread each analysis pass.

use crate::engine::sql::FunctionCatalog;
use strata_model::ColumnInfo;

#[derive(Clone, Default, PartialEq)]
pub struct ColumnSym {
    pub name: String,
    pub dtype: String,
}

#[derive(Clone, Default, PartialEq)]
pub struct TableSym {
    pub name: String,
    /// `true` for a saved view (vs a registered table) — completion detail only.
    pub is_view: bool,
    pub columns: Vec<ColumnSym>,
}

impl TableSym {
    fn from_cols(name: &str, is_view: bool, cols: &[ColumnInfo]) -> Self {
        TableSym {
            name: name.to_string(),
            is_view,
            columns: cols
                .iter()
                .map(|c| ColumnSym {
                    name: c.name.clone(),
                    dtype: c.dtype.clone(),
                })
                .collect(),
        }
    }

    pub fn column(&self, name: &str) -> Option<&ColumnSym> {
        self.columns
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(name))
    }
}

/// A snapshot of everything the analysis layer resolves against, plus the engine setting it
/// has to *read* the buffer with.
#[derive(Clone, Default)]
pub struct Catalog {
    /// Registered tables and saved views (both address columns).
    pub tables: Vec<TableSym>,
    pub functions: FunctionCatalog,
    /// The engine's `datafusion.sql_parser.dialect`, for [`lex`](super::lex::lex).
    ///
    /// It rides here because this is already the language service's one snapshot of engine
    /// state, rebuilt by one effect: a completion pass reached from a keystroke has no engine
    /// to ask, and the alternative — a second value threaded to the same call — is a second
    /// thing that can go stale on its own. Empty (the `Default`) resolves to `generic`.
    pub dialect: String,
}

impl Catalog {
    /// Build from the project catalog + the engine's function names and parser dialect. Takes
    /// `(name, columns)` pairs — the columns are what registration *learned* (they live
    /// on the UI project store's rows, not on the defs), so the caller projects them.
    pub fn build<'a>(
        tables: impl IntoIterator<Item = (&'a str, &'a [ColumnInfo])>,
        views: impl IntoIterator<Item = (&'a str, &'a [ColumnInfo])>,
        functions: FunctionCatalog,
        dialect: String,
    ) -> Self {
        let mut out = Vec::new();
        for (name, cols) in tables {
            out.push(TableSym::from_cols(name, false, cols));
        }
        for (name, cols) in views {
            out.push(TableSym::from_cols(name, true, cols));
        }
        Catalog {
            tables: out,
            functions,
            dialect,
        }
    }

    pub fn table(&self, name: &str) -> Option<&TableSym> {
        self.tables
            .iter()
            .find(|t| t.name.eq_ignore_ascii_case(name))
    }

    pub fn has_table(&self, name: &str) -> bool {
        self.table(name).is_some()
    }
}
