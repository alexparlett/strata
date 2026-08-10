//! The symbol model the language service resolves against: tables + views (with
//! their columns) projected from `state.project`, plus the registered functions
//! (from the engine, F5). Cheap to build on the UI thread each analysis pass.
//!
//! The store's `TableOrigin` is the **internal-set authority for the offer**
//! ([`TableSym::internal`]); `Engine::is_internal` stays the dispatch gate — the
//! same fact, read from the store because the snapshot is store-built (the store
//! *is* the catalog), never a second engine enumeration.

use std::sync::Arc;

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
    /// `true` for a table whose data Strata owns (`TableOrigin::Internal`) — the
    /// only tables an `INSERT` may target, so the only ones its operand offers.
    pub internal: bool,
    pub columns: Vec<ColumnSym>,
}

impl TableSym {
    fn from_cols(name: &str, is_view: bool, internal: bool, cols: &[ColumnInfo]) -> Self {
        TableSym {
            name: name.to_string(),
            is_view,
            internal,
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

/// One statement `PREPARE` left in the session (ED-08) — what `EXECUTE` and `DEALLOCATE` name.
///
/// Session-scoped and engine-side: it comes off `Engine::prepared`, the mirror of DataFusion's
/// own `prepared_plans` (which is `pub(crate)`), and the parameter types are already rendered in
/// the `short_type` vocabulary a column's dtype uses — so the language service never depends on
/// DataFusion's types, exactly as [`FunctionSym`](super::FunctionSym) does not.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PreparedSym {
    pub name: String,
    /// One label per parameter, in `$1`-first order. Empty for a statement with no placeholders,
    /// or one whose placeholder types DataFusion could not resolve.
    pub params: Vec<String>,
}

impl PreparedSym {
    /// The completion row's detail column: the parameter shape, or the flat noun when there is
    /// none to show.
    pub fn detail(&self) -> String {
        match self.params.is_empty() {
            true => "prepared".into(),
            false => format!("({})", self.params.join(", ")),
        }
    }
}

/// A snapshot of everything the analysis layer resolves against, plus the engine setting it
/// has to *read* the buffer with.
#[derive(Clone, Default)]
pub struct Catalog {
    /// Registered tables and saved views (both address columns).
    pub tables: Vec<TableSym>,
    /// The engine's function catalog, **by handle**: the snapshot is rebuilt on every catalog
    /// epoch and the function set is by far its largest part, so it rides as the `Arc` the engine
    /// already holds rather than as a per-rebuild deep copy of every symbol.
    pub functions: Arc<FunctionCatalog>,
    /// The session's prepared statements — offered at an `EXECUTE` / `DEALLOCATE` operand and
    /// nowhere else. Engine state like [`functions`](Self::functions), and it rides the same
    /// snapshot for the same reason: a completion pass reached from a keystroke has no engine to
    /// ask.
    pub prepared: Vec<PreparedSym>,
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
    /// `(name, columns, internal)` triples for tables and `(name, columns)` pairs for views
    /// (a view is never internal) — the columns are what registration *learned* (they live
    /// on the UI project store's rows, not on the defs), so the caller projects them, and
    /// `internal` is the def's own `TableOrigin`.
    pub fn build<'a>(
        tables: impl IntoIterator<Item = (&'a str, &'a [ColumnInfo], bool)>,
        views: impl IntoIterator<Item = (&'a str, &'a [ColumnInfo])>,
        functions: Arc<FunctionCatalog>,
        prepared: Vec<PreparedSym>,
        dialect: String,
    ) -> Self {
        let mut out = Vec::new();
        for (name, cols, internal) in tables {
            out.push(TableSym::from_cols(name, false, internal, cols));
        }
        for (name, cols) in views {
            out.push(TableSym::from_cols(name, true, false, cols));
        }
        Catalog {
            tables: out,
            functions,
            prepared,
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
