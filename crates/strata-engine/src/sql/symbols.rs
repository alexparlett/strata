//! The symbol model the language service resolves against: tables + views (with
//! their columns) projected from `state.project`, plus the registered functions
//! (from the engine, F5). Cheap to build on the UI thread each analysis pass.
//!
//! The store's `TableOrigin` is the **internal-set authority for the offer**
//! ([`TableSym::internal`]); `Engine::is_internal` stays the dispatch gate — the
//! same fact, read from the store because the snapshot is store-built (the store
//! *is* the catalog), never a second engine enumeration.

use std::sync::Arc;

use crate::sql::FunctionCatalog;
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

/// One **database connection's catalog**, as a qualified name's first segment (DB-06).
///
/// The two halves come from two places on purpose. The [`name`](Self::name) is the connection's
/// def — so it is offered whether or not the connection is live, exactly as the tree draws a
/// collapsed database node it has never reached. The [`schemas`](Self::schemas) are the
/// connect-time enumeration, scoped by the def's enabled set
/// ([`Engine::source_listing`](crate::Engine::source_listing), the one visibility source), so a
/// connection that has not answered offers its name and nothing under it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DatabaseSym {
    pub name: String,
    pub schemas: Vec<SchemaSym>,
}

impl DatabaseSym {
    /// The schema this catalog spells `name`, case-insensitively as SQL resolves it.
    pub fn schema(&self, name: &str) -> Option<&SchemaSym> {
        self.schemas
            .iter()
            .find(|s| s.name.eq_ignore_ascii_case(name))
    }
}

/// One remote schema and the relations in it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SchemaSym {
    pub name: String,
    pub relations: Vec<RelationSym>,
}

/// One remote relation. **No columns**: reading them is an introspection round trip, and the
/// completion path does no I/O (§7) — so a three-part qualifier completes no further.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RelationSym {
    pub name: String,
    /// Whether the server calls it a view — the completion row's glyph and detail.
    pub view: bool,
}

/// A snapshot of everything the analysis layer resolves against, plus the engine setting it
/// has to *read* the buffer with.
#[derive(Clone, Default)]
pub struct Catalog {
    /// Registered tables and saved views (both address columns).
    pub tables: Vec<TableSym>,
    /// The project's database connections, for the qualified offer (DB-06). Set through
    /// [`with_databases`](Self::with_databases) rather than taken by [`build`](Self::build),
    /// because a project with no database says nothing about them.
    pub databases: Vec<DatabaseSym>,
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
            databases: Vec::new(),
            functions,
            prepared,
            dialect,
        }
    }

    /// The project's database connections — see [`DatabaseSym`].
    pub fn with_databases(mut self, databases: Vec<DatabaseSym>) -> Self {
        self.databases = databases;
        self
    }

    /// The database connection addressed as `name`, case-insensitively as SQL resolves it.
    pub fn database(&self, name: &str) -> Option<&DatabaseSym> {
        self.databases
            .iter()
            .find(|d| d.name.eq_ignore_ascii_case(name))
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
