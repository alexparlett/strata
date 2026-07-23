//! The symbol model the language service resolves against: tables + views (with
//! their columns) projected from `state.project`, plus the registered functions
//! (from the engine, F5). Cheap to build on the UI thread each analysis pass.

use crate::engine::sql::FunctionCatalog;

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
    fn from_cols(name: &str, is_view: bool, cols: &[strata_model::ColumnInfo]) -> Self {
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

/// A snapshot of everything the analysis layer resolves against.
#[derive(Clone, Default)]
pub struct Catalog {
    /// Registered tables and saved views (both address columns).
    pub tables: Vec<TableSym>,
    pub functions: FunctionCatalog,
}

impl Catalog {
    /// Build from the project catalog + the engine's function names. Takes
    /// `(name, columns)` pairs — the columns are what registration *learned* (they live
    /// on the UI project store's rows, not on the defs), so the caller projects them.
    pub fn build<'a>(
        tables: impl IntoIterator<Item = (&'a str, &'a [strata_model::ColumnInfo])>,
        views: impl IntoIterator<Item = (&'a str, &'a [strata_model::ColumnInfo])>,
        functions: FunctionCatalog,
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
