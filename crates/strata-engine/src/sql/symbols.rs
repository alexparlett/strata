//! The symbol model the language service resolves against: tables + views (with
//! their columns) projected from `state.project`, plus the registered functions
//! (from the engine, F5). Cheap to build on the UI thread each analysis pass.
//!
//! The store's `TableOrigin` is the **internal-set authority for the offer**
//! ([`TableSym::internal`]); `Catalog::is_internal` stays the dispatch gate — the
//! same fact, read from the store because the snapshot is store-built (the store
//! *is* the catalog), never a second engine enumeration.

use std::sync::Arc;

use crate::formats::FormatInfo;
use crate::generation::CatalogGen;
use crate::sql::FunctionCatalog;
use strata_model::ColumnInfo;

/// One column of a workspace relation, as completion offers it.
#[derive(Clone, Default, PartialEq)]
pub struct ColumnSym {
    /// The column's name.
    pub name: String,
    /// Its type, in the `short_type` spelling.
    pub dtype: String,
}

/// One workspace table or view, and its columns.
#[derive(Clone, Default, PartialEq)]
pub struct TableSym {
    /// The relation's name.
    pub name: String,
    /// `true` for a saved view (vs a registered table) — completion detail only.
    pub is_view: bool,
    /// `true` for a table whose data Strata owns (`TableOrigin::Internal`) — the
    /// only tables an `INSERT` may target, so the only ones its operand offers.
    pub internal: bool,
    /// Its columns.
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

    /// The column this relation spells `name`, case-insensitively as SQL resolves it.
    pub fn column(&self, name: &str) -> Option<&ColumnSym> {
        self.columns
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(name))
    }
}

/// One statement `PREPARE` left in the session (ED-08) — what `EXECUTE` and `DEALLOCATE` name.
///
/// Session-scoped and engine-side: it comes off `Lang::prepared`, the mirror of DataFusion's
/// own `prepared_plans` (which is `pub(crate)`), and the parameter types are already rendered in
/// the `short_type` vocabulary a column's dtype uses — so the language service never depends on
/// DataFusion's types, exactly as [`FunctionSym`](super::FunctionSym) does not.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PreparedSym {
    /// The statement's name.
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

/// **Everything the language service needs off the engine, read as of one moment** — the sync
/// half of the wiring, from [`Lang::bundle`](crate::Lang::bundle).
///
/// One value rather than five calls because these are five reads of one session that must
/// describe one instant: a completion offering a function the registry no longer holds, against
/// a database list from before a connect, is the drift a single read makes impossible. It is
/// lock-reads only — no I/O, no plan, no dial-out — which is what lets the consumer take it in a
/// side effect on the render thread whenever [`generation`](Self::generation) moves.
///
/// The consumer folds it into a [`Symbols`] with its own rows and its own dialect
/// ([`Symbols::build`]); nothing here is the consumer's to decide.
#[derive(Clone, Default, PartialEq)]
pub struct LangBundle {
    /// The engine's registered functions, **by handle** — see [`Symbols::functions`].
    pub functions: Arc<FunctionCatalog>,
    /// The statements `PREPARE` has left in this session.
    pub prepared: Vec<PreparedSym>,
    /// The file formats the engine reads, with each one's `OPTIONS` keys.
    pub formats: Vec<FormatInfo>,
    /// The project's database sources, derived from the sources snapshot.
    pub databases: Vec<DatabaseSym>,
    /// The catalog generation every field above was read at — **the one invalidation clock**.
    /// Re-take the bundle when [`Catalog::generation`](crate::Catalog::generation) stops matching.
    pub generation: CatalogGen,
}

/// One **data source's catalog**, as a qualified name's first segment (DB-06).
///
/// The two halves come from two places on purpose. The [`name`](Self::name) is the data source's
/// def — so it is offered whether or not the data source is live, exactly as the tree draws a
/// collapsed database node it has never reached. The [`schemas`](Self::schemas) are the
/// connect-time enumeration, scoped by the def's enabled set
/// ([`Sources::listing`](crate::Sources::listing), the one visibility source), so a
/// data source that has not answered offers its name and nothing under it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DatabaseSym {
    /// The data source's name.
    pub name: String,
    /// Whether a write may target a relation in it — the def's own
    /// [`read_only`](strata_model::SourceDef::read_only), inverted.
    ///
    /// The offer at a write position is conditional on it: an `INSERT`/`UPDATE`/`DELETE` target
    /// on a read-only connection is refused by the arm, and offering one would bait the user into
    /// a statement the engine has already decided against.
    pub writable: bool,
    /// The schemas it shows.
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
    /// The schema's name.
    pub name: String,
    /// The relations in it.
    pub relations: Vec<RelationSym>,
}

/// One remote relation. **No columns**: reading them is an introspection round trip, and the
/// completion path does no I/O (§7) — so a three-part qualifier completes no further.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RelationSym {
    /// The relation's name.
    pub name: String,
    /// Whether the server calls it a view — the completion row's glyph and detail.
    pub view: bool,
}

/// A snapshot of everything the analysis layer resolves against, plus the engine setting it
/// has to *read* the buffer with.
#[derive(Clone, Default)]
pub struct Symbols {
    /// Registered tables and saved views (both address columns).
    pub tables: Vec<TableSym>,
    /// The project's database sources, for the qualified offer (DB-06) and the write-target
    /// pools — off the [`LangBundle`], which is where the engine answers for them.
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
    /// The file formats the engine reads — the `STORED AS` word pool and, per word, the
    /// `OPTIONS` keys its reader takes. Engine state like [`functions`](Self::functions), riding
    /// the same snapshot for the same reason.
    pub formats: Vec<FormatInfo>,
    /// The engine's `datafusion.sql_parser.dialect`, for [`lex`](super::lex::lex).
    ///
    /// It rides here because this is already the language service's one snapshot of engine
    /// state, rebuilt by one effect: a completion pass reached from a keystroke has no engine
    /// to ask, and the alternative — a second value threaded to the same call — is a second
    /// thing that can go stale on its own. Empty (the `Default`) resolves to `generic`.
    pub dialect: String,
}

impl Symbols {
    /// **The one constructor** — the consumer's own rows, the engine's [`LangBundle`], and the
    /// dialect from the consumer's own settings.
    ///
    /// Three inputs, from the three places that own them, because the assembly used to be five
    /// arguments gathered from five calls and every one of them was a thing that could go stale
    /// on its own. Re-build when [`LangBundle::generation`] moves.
    ///
    /// Tables are `(name, columns, internal)` triples and views `(name, columns)` pairs (a view
    /// is never internal). The rows are the **store's**, deliberately: the columns are what
    /// registration *learned* (they live on the consumer's project rows, not on the defs), and a
    /// def whose registration **failed** is still offered by name — what the user wrote down is
    /// what the editor knows about, and the failure has its own surface.
    ///
    /// The dialect is the consumer's `datafusion.sql_parser.dialect` setting rather than the
    /// bundle's, because it is a setting the user edits and the engine is not its author.
    pub fn build<'a>(
        tables: impl IntoIterator<Item = (&'a str, &'a [ColumnInfo], bool)>,
        views: impl IntoIterator<Item = (&'a str, &'a [ColumnInfo])>,
        bundle: LangBundle,
        dialect: String,
    ) -> Self {
        let mut out = Vec::new();
        for (name, cols, internal) in tables {
            out.push(TableSym::from_cols(name, false, internal, cols));
        }
        for (name, cols) in views {
            out.push(TableSym::from_cols(name, true, false, cols));
        }
        let LangBundle {
            functions,
            prepared,
            formats,
            databases,
            generation: _,
        } = bundle;
        Symbols {
            tables: out,
            databases,
            functions,
            prepared,
            formats,
            dialect,
        }
    }

    /// The format the `STORED AS` word `name` names, as SQL resolves it.
    pub fn format(&self, name: &str) -> Option<&FormatInfo> {
        self.formats
            .iter()
            .find(|f| f.name.eq_ignore_ascii_case(name))
    }

    /// The data source addressed as `name`, case-insensitively as SQL resolves it.
    pub fn database(&self, name: &str) -> Option<&DatabaseSym> {
        self.databases
            .iter()
            .find(|d| d.name.eq_ignore_ascii_case(name))
    }

    /// The workspace relation spelled `name`, case-insensitively as SQL resolves it.
    pub fn table(&self, name: &str) -> Option<&TableSym> {
        self.tables
            .iter()
            .find(|t| t.name.eq_ignore_ascii_case(name))
    }

    /// Whether the workspace holds a relation spelled `name`.
    pub fn has_table(&self, name: &str) -> bool {
        self.table(name).is_some()
    }
}
