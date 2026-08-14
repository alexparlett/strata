//! The **wire shapes** — what a tool takes and what it answers, as JSON.
//!
//! Kept apart from [`crate::host`]'s types on purpose: a host type models the states out of
//! existence, while a wire type is flat with empty collections and absent facts omitted, because
//! that is what reads well to a model. The projections between them are the `from_*` functions
//! here, so no tool assembles a response by hand.
//!
//! Two conventions hold throughout:
//!
//! - **A cell is `null` or a string.** Rows arrive already formatted by the engine's `CellFormat`,
//!   so numbers come back as strings and a null becomes JSON `null` rather than the configured NULL
//!   rendering, which is presentation.
//! - **A query-session handle is its `QuerySessionId` as text** — the session's own `Uuid`, the
//!   same one the engine uses as its `WsId`, never a parallel id scheme.

use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use strata_core::engine::export::{Csv, ExportReport, Format, Json, Parquet};
use strata_core::engine::plan::QueryPlan;
use strata_core::engine::sql::{FunctionCatalog, FunctionSym};
use strata_core::util::{clip, collapse_sql};
use strata_model::{Cell, ColumnInfo, Diagnostic, Kind, QueryOutput, Severity, Stat, StatKey};

use crate::host::{CatalogEntry, Project, QuerySessionInfo, QuerySessionState, RegState, RunMode};

/// A result's columns, shared rather than copied.
///
/// One result is described by its `run` and by every `read_page` after it, and a schema here
/// is not small — the app already has a file whose one column carries 19,311 nested fields.
/// Converting `ColumnInfo` to [`ColumnWire`] is recursive work per field, so it happens once
/// per result and each response holds a refcount. `Arc<T>` serializes and schematizes exactly
/// as `T` does, so nothing about the wire format changes.
pub type Columns = Arc<Vec<ColumnWire>>;

/// Convert a result schema once, for sharing as [`Columns`].
pub fn columns(info: &[ColumnInfo]) -> Columns {
    Arc::new(info.iter().map(ColumnWire::from).collect())
}

/// The disambiguator every project-scoped tool takes: a project's root path or its name.
/// Only needed when more than one project is open — the error lists them.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ProjectParams {
    #[serde(default)]
    pub project: Option<String>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct DescribeTableParams {
    /// The table or view to describe. Saved queries are not in this namespace.
    pub name: String,
    /// A nested column to descend to: name segments exactly as a previous answer printed
    /// them, outermost first. Never a dotted path in one string — field names come from the
    /// user's files and may contain dots.
    #[serde(default)]
    pub path: Option<Vec<String>>,
    /// Case-insensitive substring over field names, searched through the whole tree (under
    /// 'path' when both are given). Matches come back as paths this tool accepts back.
    #[serde(default)]
    pub matching: Option<String>,
    /// 1-based window over the described columns (or the addressed column's children, or
    /// the matches), 50 per page.
    #[serde(default)]
    pub page: Option<usize>,
    #[serde(default)]
    pub project: Option<String>,
}

/// `list_tables` on the wire. Its own struct rather than [`ProjectParams`] because the
/// narrowing belongs to the catalog listing alone — the session tools that share
/// `ProjectParams` must not grow a 'matching' nobody reads.
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ListTablesParams {
    /// Case-insensitive substring over entry names.
    #[serde(default)]
    pub matching: Option<String>,
    /// 1-based page over the (filtered) catalog, 50 entries per page.
    #[serde(default)]
    pub page: Option<usize>,
    #[serde(default)]
    pub project: Option<String>,
}

/// `list_functions` on the wire — [`ListTablesParams`]'s reason, restated.
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ListFunctionsParams {
    /// Case-insensitive substring over function names. A match set small enough comes back
    /// in full detail; the unfiltered registry lists names only.
    #[serde(default)]
    pub matching: Option<String>,
    #[serde(default)]
    pub project: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ValidateParams {
    /// The SQL to check. Never executed.
    pub sql: String,
    #[serde(default)]
    pub project: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct QuerySessionParams {
    /// A handle from `open_query_session` or `list_query_sessions`.
    pub query_session: String,
    #[serde(default)]
    pub project: Option<String>,
}

/// `run` on the wire. `mode` is a parameter rather than a second tool because the two share
/// every other argument and the query session they run in.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct RunParams {
    /// A handle from `open_query_session` or `list_query_sessions`. The run replaces
    /// whatever that session last produced.
    pub query_session: String,
    /// The statement to run. Read-only: SELECT, EXPLAIN, SHOW and DESCRIBE only.
    pub sql: String,
    /// `run` (default) executes and returns page 1; `explain` returns the plan and
    /// materializes nothing.
    #[serde(default)]
    pub mode: Option<Mode>,
    /// Rows in the returned page. Defaults to the app's row-limit setting; capped at 10000.
    /// The query itself is never rewritten — the total is exact and `read_page` reads the
    /// rest.
    #[serde(default)]
    pub page_size: Option<usize>,
    #[serde(default)]
    pub project: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    Run,
    Explain,
}

impl From<Mode> for RunMode {
    fn from(mode: Mode) -> RunMode {
        match mode {
            Mode::Run => RunMode::Run,
            Mode::Explain => RunMode::Explain,
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReadPageParams {
    /// A query-session handle whose last run settled with rows.
    pub query_session: String,
    /// 1-based page number over that session's settled snapshot.
    pub page: usize,
    /// Order the whole snapshot before the page window is taken.
    #[serde(default)]
    pub sort: Option<Sort>,
    #[serde(default)]
    pub project: Option<String>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct Sort {
    pub column: String,
    /// Ascending when absent.
    #[serde(default = "yes")]
    pub ascending: bool,
}

fn yes() -> bool {
    true
}

/// `export_result` on the wire. Its own struct rather than [`ReadPageParams`] because the
/// destination belongs to the write alone.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ExportResultParams {
    /// A query-session handle whose last run settled with rows.
    pub query_session: String,
    /// Absolute path of the file to write, with a file extension. It must not exist, and the
    /// folder above it must.
    pub path: String,
    /// What to write. Each format is written with its own defaults: a CSV carries a header row
    /// and comma-separated fields, none of them are compressed.
    pub format: ExportFormat,
    #[serde(default)]
    pub project: Option<String>,
}

/// What an export writes — the four the engine's writer supports, named as a reader would ask
/// for them (`ndjson` rather than DataFusion's `JSON`, which is what its JSON writer actually
/// emits here).
///
/// No write options ride with the choice: the tool writes each format's self-describing
/// defaults ([`Format`]'s own), because a caller with no dialog in front of it has nothing to
/// preview an unusual delimiter against. The Export window is where those are chosen.
#[derive(Clone, Copy, Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    Csv,
    Ndjson,
    Parquet,
    Arrow,
}

impl From<ExportFormat> for Format {
    fn from(format: ExportFormat) -> Format {
        match format {
            ExportFormat::Csv => Format::Csv(Csv::default()),
            ExportFormat::Ndjson => Format::Json(Json::default()),
            ExportFormat::Parquet => Format::Parquet(Parquet::default()),
            ExportFormat::Arrow => Format::Arrow,
        }
    }
}

/// What `export_result` wrote. Every figure is the engine's own: `rows` is what `COPY` counted
/// and `bytes` is the file's own size, absent only when it could not be read back after a write
/// that had already succeeded.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ExportResult {
    pub query_session: String,
    pub path: String,
    pub rows: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
}

impl From<(String, ExportReport)> for ExportResult {
    fn from((query_session, report): (String, ExportReport)) -> ExportResult {
        ExportResult {
            query_session,
            path: report.path,
            rows: report.rows,
            bytes: report.bytes,
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ProjectsResult {
    pub projects: Vec<ProjectWire>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ProjectWire {
    pub name: String,
    pub root: String,
}

impl From<Project> for ProjectWire {
    fn from(p: Project) -> ProjectWire {
        ProjectWire {
            name: p.name,
            root: p.root.display().to_string(),
        }
    }
}

/// Entries per `list_tables` page — with the per-entry bounds below, what keeps a page's
/// size deterministic. The common catalog is one page, answered complete with no paging
/// fields at all.
pub const TABLES_PAGE: usize = 50;

/// Characters a view's one-line SQL preview keeps. The clip is visible (a trailing ellipsis
/// character), and the full text is `describe_table`'s to return.
const SQL_PREVIEW: usize = 160;

/// Source paths a table row lists before the count stands in for the rest.
const SOURCES_SHOWN: usize = 3;

#[derive(Debug, Serialize, JsonSchema)]
pub struct TablesResult {
    /// Entries the catalog holds (or 'matching' matched), before paging.
    pub total: usize,
    pub entries: Vec<EntryWire>,
    /// Catalogs the project's database connections have registered. Nothing in 'entries'
    /// describes them and nothing is meant to: a database answers for itself, so its relations
    /// are not defs of this project and 'matching' does not reach them. Read one by three-part
    /// name ('pg.public.orders'), list them with SHOW TABLES, and read one relation's schema
    /// with `describe_table` under that same name.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub databases: Vec<String>,
    /// Present only when the answer is one window of more.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_size: Option<usize>,
}

/// Whether `name` contains the **already-lowercased** needle, case-insensitively — the one
/// spelling of the rule every 'matching' parameter applies (`list_tables`,
/// `list_functions`, `describe_table`'s field search), so the three cannot drift.
pub(crate) fn name_matches(name: &str, lowered: &str) -> bool {
    name.to_lowercase().contains(lowered)
}

/// One page of a list answer, under the one window rule every paged tool shares.
///
/// 1-based, saturating (a page number is wire input, and the largest expressible one is an
/// empty window, never a wrapped skip) — and `page`/`page_size` are present exactly when
/// the answer shows fewer than the total, so the caller's own request cannot forge the
/// "more exists" signal: a requested page of a complete list answers with no paging fields
/// at all.
pub(crate) struct Windowed<T> {
    pub shown: Vec<T>,
    pub total: usize,
    pub page: Option<usize>,
    pub page_size: Option<usize>,
}

pub(crate) fn windowed<T>(items: Vec<T>, page: Option<usize>, per: usize) -> Windowed<T> {
    let total = items.len();
    let at = page.unwrap_or(1).max(1);
    let shown: Vec<T> = items
        .into_iter()
        .skip((at - 1).saturating_mul(per))
        .take(per)
        .collect();
    let more = shown.len() < total;
    Windowed {
        shown,
        total,
        page: more.then_some(at),
        page_size: more.then_some(per),
    }
}

/// The `list_tables` projection: filter by name, then window — totals first, so a narrowed
/// answer always states what it matched against.
///
/// `databases` rides outside the window and outside `total` on purpose: they are not entries, and
/// counting them into a total the caller pages through would promise pages that do not exist.
/// They are **not** filtered by `matching` either — a narrowed listing that dropped the
/// connections would read as a project with none.
pub fn tables_result(
    entries: Vec<CatalogEntry>,
    databases: Vec<String>,
    matching: Option<&str>,
    page: Option<usize>,
) -> TablesResult {
    let needle = matching.map(str::to_lowercase);
    let matched: Vec<CatalogEntry> = entries
        .into_iter()
        .filter(|e| match &needle {
            Some(m) => name_matches(e.name(), m),
            None => true,
        })
        .collect();
    let w = windowed(matched, page, TABLES_PAGE);
    TablesResult {
        total: w.total,
        entries: w.shown.into_iter().map(entry_wire).collect(),
        databases,
        page: w.page,
        page_size: w.page_size,
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EntryWire {
    Table {
        name: String,
        format: String,
        /// The first few source paths as stored; `sources_total` counts the whole set when
        /// more were elided.
        sources: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        sources_total: Option<usize>,
        state: StateWire,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    View {
        name: String,
        /// A one-line preview, clipped visibly. The full text is `describe_table`'s answer.
        sql: String,
        state: StateWire,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// A saved query's SQL stays **whole**, deliberately: `describe_table` does not answer
    /// for saved queries, so a preview here would make the full text unreachable — the one
    /// per-entry bound honesty forbids.
    SavedQuery {
        id: String,
        name: String,
        sql: String,
    },
}

/// One catalog row bounded for the listing — a function rather than a `From`, because this
/// projection is deliberately lossy (the preview, the source cap) and a `From` reads as a
/// total, lossless conversion.
fn entry_wire(entry: CatalogEntry) -> EntryWire {
    match entry {
        CatalogEntry::Table {
            name,
            format,
            mut sources,
            reg,
        } => {
            let (state, error) = split_reg(reg);
            let total = sources.len();
            sources.truncate(SOURCES_SHOWN);
            EntryWire::Table {
                name,
                format,
                sources,
                sources_total: (total > SOURCES_SHOWN).then_some(total),
                state,
                error,
            }
        }
        CatalogEntry::View { name, sql, reg } => {
            let (state, error) = split_reg(reg);
            EntryWire::View {
                name,
                sql: clip(&collapse_sql(&sql), SQL_PREVIEW).into_owned(),
                state,
                error,
            }
        }
        CatalogEntry::Query { id, name, sql } => EntryWire::SavedQuery {
            id: id.to_string(),
            name,
            sql,
        },
    }
}

#[derive(Clone, Copy, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StateWire {
    Pending,
    Ready,
    Failed,
}

fn split_reg(reg: RegState) -> (StateWire, Option<String>) {
    match reg {
        RegState::Pending => (StateWire::Pending, None),
        RegState::Ready => (StateWire::Ready, None),
        RegState::Failed(e) => (StateWire::Failed, Some(e)),
    }
}

/// `describe_table`'s answer, flattened: a table's facts and a view's are different sets,
/// and a def the engine refused has neither, so everything but the name and the state is
/// omitted when it does not apply.
///
/// The schema portion is **bounded** (`crate::describe`), and the convention over every
/// bound is: an answer with no totals in it is a complete answer. `columns_total`,
/// `children_total` and `keys_total` on a column, `matched_total`, `matched_keys` on a match
/// and `page` appear exactly where something was elided, collapsed or searched, and each
/// names what the shown part was cut from.
#[derive(Debug, Serialize, JsonSchema)]
pub struct DescribeResult {
    pub name: String,
    pub state: StateWire,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<EntryKindWire>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The database connection this relation lives in, for one that is not a def of the
    /// project's — its catalog name, which is also the first part of 'name'. Absent for
    /// everything in the workspace.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<String>,
    /// Source paths the def holds in total; present only when 'sources' shows fewer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sources_total: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sql: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub partitions: Vec<PartitionWire>,
    /// The row count the source reports for free. Absent when it reports none — never
    /// counted, never derived.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rows: Option<u64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<ColumnWire>,
    /// Top-level columns the schema holds in total; present only when 'columns' shows
    /// fewer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub columns_total: Option<usize>,
    /// Base tables a view scans.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub reads: Vec<String>,
    /// Fields 'matching' found, each addressed by a path this tool accepts back.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub matches: Vec<MatchWire>,
    /// How many fields 'matching' matched — stated even at zero, so an empty answer cannot
    /// read as an unsearched one. Absent only when no 'matching' was given. Counts every
    /// matching field, including the ones a collapsed row stands for (`matched_keys`), so it
    /// can exceed the number of rows; 'page' is what says whether more rows exist.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched_total: Option<usize>,
    /// Present only when the answer is one window of more.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_size: Option<usize>,
}

/// One field 'matching' found: where it is — as the path `describe_table` accepts back,
/// never a dotted string — and what it is.
#[derive(Debug, Serialize, JsonSchema)]
pub struct MatchWire {
    pub path: Vec<String>,
    pub dtype: String,
    pub kind: KindWire,
    /// How many real fields this one row stands for, when its path runs through a collapsed
    /// key set (the `<key>` segment). Absent when the row is one field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched_keys: Option<usize>,
}

#[derive(Clone, Copy, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EntryKindWire {
    Table,
    View,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PartitionWire {
    pub name: String,
    pub dtype: String,
}

/// One column, nested children and all. `kind` is the visual family the app groups types
/// into; `dtype` is the Arrow type as the engine reports it.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ColumnWire {
    pub name: String,
    pub dtype: String,
    pub kind: KindWire,
    pub nullable: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<ColumnWire>,
    /// Direct children this column has in total; present only when 'children' shows fewer.
    /// Reach the rest with `describe_table`'s 'path' and 'page', or 'matching'.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub children_total: Option<usize>,
    /// On a **collapsed key set**: how many same-shaped siblings this one entry stands for.
    /// Its name is the placeholder `<key>` rather than any of theirs, because the keys are
    /// data and the shape below them is the answer. Present only on such an entry — which is
    /// what tells it apart from a field a file really named `<key>`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keys_total: Option<usize>,
    /// A few of that set's real keys, exactly as the file spells them — what `describe_table`'s
    /// 'path' takes to descend into one of them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub key_examples: Vec<String>,
    /// Facts the source reports **for free** — read at registration, never computed. Empty
    /// for every format without metadata to read, which is every format but Parquet and
    /// Arrow. Profiling is deliberately not exposed (the spec's "The policy gate").
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stats: Vec<StatWire>,
}

/// The **whole** subtree — the projection a result schema uses ([`Columns`]), where the
/// snapshot the model queries really does hold every field. A `describe_table` answer never
/// uses this: its walk is `crate::describe`'s, bounded.
impl From<&ColumnInfo> for ColumnWire {
    fn from(c: &ColumnInfo) -> ColumnWire {
        ColumnWire {
            name: c.name.clone(),
            dtype: c.dtype.clone(),
            kind: c.kind.into(),
            nullable: c.nullable,
            children: c.children.iter().map(ColumnWire::from).collect(),
            children_total: None,
            keys_total: None,
            key_examples: Vec::new(),
            stats: c.stats.iter().map(StatWire::from).collect(),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum KindWire {
    Str,
    Num,
    Bool,
    Ts,
    Struct,
    List,
    Map,
}

impl From<Kind> for KindWire {
    fn from(kind: Kind) -> KindWire {
        match kind {
            Kind::Str => KindWire::Str,
            Kind::Num => KindWire::Num,
            Kind::Bool => KindWire::Bool,
            Kind::Ts => KindWire::Ts,
            Kind::Struct => KindWire::Struct,
            Kind::List => KindWire::List,
            Kind::Map => KindWire::Map,
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct StatWire {
    pub key: StatKeyWire,
    pub value: String,
    /// False when the source truncated the value, making it a bound rather than the value.
    pub exact: bool,
}

impl From<&Stat> for StatWire {
    fn from(s: &Stat) -> StatWire {
        StatWire {
            key: s.key.into(),
            value: s.text.clone(),
            exact: s.exact,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StatKeyWire {
    Nulls,
    Min,
    Max,
    Distinct,
    Mean,
    Median,
}

impl From<StatKey> for StatKeyWire {
    fn from(key: StatKey) -> StatKeyWire {
        match key {
            StatKey::Nulls => StatKeyWire::Nulls,
            StatKey::Min => StatKeyWire::Min,
            StatKey::Max => StatKeyWire::Max,
            StatKey::Distinct => StatKeyWire::Distinct,
            StatKey::Mean => StatKeyWire::Mean,
            StatKey::Median => StatKeyWire::Median,
        }
    }
}

/// The most functions one answer describes in **full** (signatures, returns, description).
/// One rule for filtered and unfiltered alike: at or under it the set comes back detailed,
/// over it names only — so a small project's registry is complete in one answer, and the
/// 319-function default registry (63,729 bytes detailed, 2.66x the assistant's result cap)
/// answers with every name and a note pointing at 'matching'.
pub const FUNCTION_DETAIL: usize = 30;

#[derive(Debug, Serialize, JsonSchema)]
pub struct FunctionsResult {
    /// Functions this answer covers — the whole registry, or what 'matching' matched.
    /// Always stated, so "the 12 date functions" and "12 of the 40 date functions" cannot
    /// read the same.
    pub total: usize,
    pub scalar: Vec<FunctionWire>,
    pub aggregate: Vec<FunctionWire>,
    pub window: Vec<FunctionWire>,
    /// Present when detail was withheld; names the recovery.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// The `list_functions` projection: filter by name, count, then decide detail **once** for
/// the whole answer — never per category, or one answer would mix two shapes.
pub fn functions_result(catalog: &FunctionCatalog, matching: Option<&str>) -> FunctionsResult {
    let needle = matching.map(str::to_lowercase);
    let keep = |f: &&FunctionSym| match &needle {
        Some(m) => name_matches(&f.name, m),
        None => true,
    };
    let scalar: Vec<&FunctionSym> = catalog.scalar.iter().filter(keep).collect();
    let aggregate: Vec<&FunctionSym> = catalog.aggregate.iter().filter(keep).collect();
    let window: Vec<&FunctionSym> = catalog.window.iter().filter(keep).collect();
    let total = scalar.len() + aggregate.len() + window.len();
    let detailed = total <= FUNCTION_DETAIL;
    FunctionsResult {
        total,
        scalar: function_rows(scalar, detailed),
        aggregate: function_rows(aggregate, detailed),
        window: function_rows(window, detailed),
        note: (!detailed).then(|| {
            format!(
                "Names only: {total} functions is too many to describe in full. Narrow with \
                 'matching' to at most {FUNCTION_DETAIL} matches to read signatures, return \
                 types and descriptions."
            )
        }),
    }
}

/// One category's rows, at the detail level the whole answer decided on. A names-only row
/// is `{"name": …}` and nothing else.
fn function_rows(fs: Vec<&FunctionSym>, detailed: bool) -> Vec<FunctionWire> {
    fs.into_iter()
        .map(|f| {
            if detailed {
                FunctionWire::from(f)
            } else {
                FunctionWire {
                    name: f.name.clone(),
                    signatures: None,
                    returns: None,
                    description: None,
                }
            }
        })
        .collect()
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct FunctionWire {
    pub name: String,
    /// One entry per overload, each an ordered list of parameter labels. A trailing `…`
    /// marks a variadic tail. **Absent only in a names-only answer** — a detailed row
    /// whose registry declares no arity carries an empty list, so "detail withheld" and
    /// "no declared arity" cannot read as the same absence.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signatures: Option<Vec<Vec<String>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub returns: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl From<&FunctionSym> for FunctionWire {
    fn from(f: &FunctionSym) -> FunctionWire {
        FunctionWire {
            name: f.name.clone(),
            signatures: Some(f.signatures.clone()),
            returns: f.ret.clone(),
            description: f.description.clone(),
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ValidateResult {
    pub diagnostics: Vec<DiagnosticWire>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct DiagnosticWire {
    pub severity: SeverityWire,
    pub message: String,
    /// `line L:C`, when the diagnostic has a position.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loc: Option<String>,
    /// Byte range into the submitted SQL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span: Option<[usize; 2]>,
}

impl From<&Diagnostic> for DiagnosticWire {
    fn from(d: &Diagnostic) -> DiagnosticWire {
        DiagnosticWire {
            severity: match d.severity {
                Severity::Error => SeverityWire::Error,
                Severity::Warning => SeverityWire::Warning,
                Severity::Info => SeverityWire::Info,
            },
            message: d.message.clone(),
            loc: d.loc.clone(),
            span: d.span.as_ref().map(|s| [s.start, s.end]),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SeverityWire {
    Error,
    Warning,
    Info,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct QuerySessionsResult {
    pub query_sessions: Vec<QuerySessionWire>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct QuerySessionWire {
    pub query_session: String,
    pub state: QuerySessionStateWire,
}

impl From<QuerySessionInfo> for QuerySessionWire {
    fn from(s: QuerySessionInfo) -> QuerySessionWire {
        QuerySessionWire {
            query_session: s.session.0.to_string(),
            state: match s.state {
                QuerySessionState::Empty => QuerySessionStateWire::Empty,
                QuerySessionState::Running => QuerySessionStateWire::Running,
                QuerySessionState::Settled => QuerySessionStateWire::Settled,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum QuerySessionStateWire {
    Empty,
    Running,
    Settled,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct QuerySessionResult {
    pub query_session: String,
}

/// What a `run` settled as. **A stop is a status, not an error**: a cancel in the app or a
/// supersede by a newer press is news the user already has, and the only thing that knows a
/// stop from a fault is `strata_core::engine::stopped_on_purpose`.
///
/// **`extend` is load-bearing, not decoration.** This is the vocabulary's one `outputSchema`
/// that is a sum rather than a struct, and schemars emits an internally-tagged enum as a bare
/// `oneOf` with no `"type"` at the top. MCP says an output schema describes the *object* a tool
/// returns in `structuredContent`, and a client that checks so rejects this tool — and, because
/// it validates the `tools/list` response as a whole, **every other tool with it**. The
/// symptom is the worst kind: the server connects, reports healthy, answers `tools/list` with
/// all ten, and the client shows none, with nothing anywhere saying why. Adding `type` beside
/// `oneOf` is plain JSON Schema (an instance must satisfy both) and describes exactly what
/// every variant already is — the three of them are objects; only the tag differs.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
#[schemars(extend("type" = "object"))]
pub enum RunResult {
    Ok {
        query_session: String,
        columns: Columns,
        /// Page 1. A cell is `null` or its formatted text.
        rows: Vec<Vec<Option<String>>>,
        /// Exact — the snapshot knows, and no `LIMIT` was injected to make it otherwise.
        total: usize,
        page: usize,
        page_size: usize,
        elapsed_ms: u64,
    },
    Plan {
        query_session: String,
        /// True when the statement was `EXPLAIN ANALYZE`, so the physical plan carries
        /// per-operator metrics.
        analyze: bool,
        logical: String,
        physical: String,
    },
    Stopped {
        query_session: String,
        reason: String,
    },
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PageResult {
    pub query_session: String,
    pub columns: Columns,
    pub rows: Vec<Vec<Option<String>>>,
    pub total: usize,
    pub page: usize,
    pub page_size: usize,
}

/// The plan trees as text — what `EXPLAIN` prints, which is the form every SQL tool shows
/// and the one an agent can read. The app's structured `PlanNode` list exists to be
/// *drawn* (it carries accent colours and time-share bars); over the wire it would be the
/// same tree twice, once in a shape nothing off-screen can use.
pub fn plan_result(query_session: String, plan: QueryPlan) -> RunResult {
    RunResult::Plan {
        query_session,
        analyze: plan.analyze,
        logical: plan.logical_text,
        physical: plan.physical_text,
    }
}

/// `columns` is passed in rather than derived here, so the caller can hand the same
/// [`Columns`] to the cache `read_page` will answer from — one conversion per result.
pub fn rows_result(query_session: String, columns: Columns, output: QueryOutput) -> RunResult {
    RunResult::Ok {
        query_session,
        columns,
        rows: cells(&output.rows),
        total: output.total,
        page: output.page,
        page_size: output.page_size,
        elapsed_ms: output.elapsed_ms as u64,
    }
}

/// A null cell becomes JSON `null`; everything else is the text the grid shows.
pub fn cells(rows: &[Vec<Cell>]) -> Vec<Vec<Option<String>>> {
    rows.iter()
        .map(|row| {
            row.iter()
                .map(|c| (!c.null).then(|| c.text.clone()))
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;

    #[test]
    fn a_null_cell_is_json_null_not_the_null_rendering() {
        let rows = vec![vec![
            Cell {
                text: "NULL".into(),
                null: true,
            },
            Cell {
                text: "7".into(),
                null: false,
            },
        ]];
        assert_eq!(cells(&rows), vec![vec![None, Some("7".to_string())]]);
    }

    /// One detail rule, filtered and unfiltered alike: a set inside [`FUNCTION_DETAIL`] is
    /// full, a larger one is names-only with the recovery named — and the total is always
    /// stated, so "the 12 date functions" and "12 of the 40" cannot read the same.
    #[test]
    fn the_function_detail_rule_is_one_rule_not_two_modes() {
        let sym = |name: &str| FunctionSym {
            name: name.into(),
            signatures: vec![vec!["Float64".into()]],
            ret: Some("Float64".into()),
            description: Some("does a thing".into()),
            ..FunctionSym::default()
        };
        let small = FunctionCatalog {
            scalar: vec![sym("date_trunc"), sym("abs")],
            aggregate: vec![sym("count")],
            window: Vec::new(),
        };
        let detailed = functions_result(&small, None);
        assert_eq!(detailed.total, 3);
        assert!(detailed.note.is_none());
        assert!(detailed.scalar[0].description.is_some());

        let big = FunctionCatalog {
            scalar: (0..40).map(|i| sym(&format!("fn_{i}"))).collect(),
            aggregate: Vec::new(),
            window: Vec::new(),
        };
        let names_only = functions_result(&big, None);
        assert_eq!(names_only.total, 40);
        assert!(names_only
            .note
            .as_deref()
            .is_some_and(|n| n.contains("'matching'")));
        let row = serde_json::to_value(&names_only.scalar[0]).unwrap();
        assert_eq!(row, serde_json::json!({"name": "fn_0"}), "names only");

        let narrowed = functions_result(&big, Some("FN_1"));
        assert_eq!(narrowed.total, 11, "case-insensitive substring");
        assert!(narrowed.note.is_none());
        assert!(narrowed.scalar[0].description.is_some());

        let no_arity = FunctionCatalog {
            scalar: vec![FunctionSym {
                name: "my_udf".into(),
                ..FunctionSym::default()
            }],
            aggregate: Vec::new(),
            window: Vec::new(),
        };
        let detailed_row =
            serde_json::to_value(&functions_result(&no_arity, None).scalar[0]).unwrap();
        assert_eq!(detailed_row["signatures"], serde_json::json!([]));
    }

    /// Per-entry bounds plus paging: a small catalog is complete with no paging fields, a
    /// windowed one states the total, and 'matching' filters every kind by name.
    #[test]
    fn a_catalog_is_filtered_windowed_and_counted() {
        let entries: Vec<CatalogEntry> = (0..60)
            .map(|i| CatalogEntry::Table {
                name: format!("t{i:02}"),
                format: "parquet".into(),
                sources: vec!["a.parquet".into()],
                reg: RegState::Ready,
            })
            .collect();

        let paged = tables_result(entries.clone(), Vec::new(), None, None);
        assert_eq!(paged.total, 60);
        assert_eq!(paged.entries.len(), TABLES_PAGE);
        assert_eq!(paged.page, Some(1));
        assert_eq!(paged.page_size, Some(TABLES_PAGE));

        let second = tables_result(entries.clone(), Vec::new(), None, Some(2));
        assert_eq!(second.entries.len(), 10);

        let matched = tables_result(entries.clone(), Vec::new(), Some("T05"), None);
        assert_eq!(matched.total, 1, "case-insensitive substring");
        assert_eq!(matched.page, None, "one page is a complete answer");

        let small = tables_result(
            entries.into_iter().take(3).collect(),
            Vec::new(),
            None,
            None,
        );
        assert_eq!(small.total, 3);
        assert_eq!(small.page, None);
        assert_eq!(small.page_size, None);
    }

    /// **The database catalogs ride beside the entries, not among them** (DB-03): outside the
    /// total, outside the window, and outside 'matching' — a narrowed listing that dropped them
    /// would read as a project with no database connections.
    #[test]
    fn database_catalogs_are_named_beside_the_entries() {
        let entries = vec![CatalogEntry::Table {
            name: "people".into(),
            format: "csv".into(),
            sources: vec!["people.csv".into()],
            reg: RegState::Ready,
        }];
        let listed = tables_result(
            entries,
            vec!["pg".to_string(), "warehouse".to_string()],
            Some("nothing matches this"),
            None,
        );
        assert_eq!(listed.total, 0, "the filter emptied the entries");
        assert_eq!(listed.databases, vec!["pg", "warehouse"]);
    }

    /// A view row carries a preview a reader can tell is one; a saved query's SQL stays
    /// whole because no other tool returns it; a table's source list is capped with its
    /// total stated.
    #[test]
    fn per_entry_bounds_are_honest_about_what_they_cut() {
        let long_sql = format!("SELECT   a,\n  b\nFROM t WHERE x = '{}'", "y".repeat(300));
        let rows = tables_result(
            vec![
                CatalogEntry::View {
                    name: "wide".into(),
                    sql: long_sql.clone(),
                    reg: RegState::Ready,
                },
                CatalogEntry::Query {
                    id: Uuid::nil(),
                    name: "parked".into(),
                    sql: long_sql.clone(),
                },
                CatalogEntry::Table {
                    name: "sharded".into(),
                    format: "parquet".into(),
                    sources: (0..10).map(|i| format!("part-{i}.parquet")).collect(),
                    reg: RegState::Ready,
                },
            ],
            Vec::new(),
            None,
            None,
        );
        match &rows.entries[..] {
            [EntryWire::View { sql: preview, .. }, EntryWire::SavedQuery { sql: whole, .. }, EntryWire::Table {
                sources,
                sources_total,
                ..
            }] => {
                assert!(preview.len() < long_sql.len());
                assert!(!preview.contains('\n'), "one line: {preview}");
                assert!(preview.ends_with('…'), "the clip is visible: {preview}");
                assert_eq!(whole, &long_sql, "a saved query's SQL has no other home");
                assert_eq!(sources.len(), 3);
                assert_eq!(sources_total, &Some(10));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_saved_query_row_carries_no_registration_state() {
        let wire = entry_wire(CatalogEntry::Query {
            id: Uuid::nil(),
            name: "top sellers".into(),
            sql: "SELECT 1".into(),
        });
        let json = serde_json::to_value(&wire).unwrap();
        assert_eq!(json["kind"], "saved_query");
        assert!(json.get("state").is_none(), "{json}");
    }

    /// **Every `outputSchema` says `"type": "object"`.**
    ///
    /// MCP's output schema describes the object a tool returns in `structuredContent`, and a
    /// client that checks so drops a tool that does not say it — then, validating the
    /// `tools/list` response as a whole, drops *every* tool with it. That is how this was
    /// found: the server connected, reported healthy, answered `tools/list` with all ten, and
    /// a fresh Claude Code session showed none, with nothing anywhere naming the cause.
    ///
    /// [`RunResult`] is the one that can fail it, because it is the one sum type — schemars
    /// emits an internally-tagged enum as a bare `oneOf`. The test covers every result shape
    /// rather than that one, since the next sum added would fail identically and silently.
    #[test]
    fn every_result_schema_describes_an_object() {
        fn object_schema<T: JsonSchema>(named: &str) {
            let schema = serde_json::to_value(schemars::schema_for!(T)).unwrap();
            assert_eq!(
                schema.get("type").and_then(|t| t.as_str()),
                Some("object"),
                "{named}'s output schema must say it is an object: {schema}"
            );
        }
        object_schema::<ProjectsResult>("ProjectsResult");
        object_schema::<TablesResult>("TablesResult");
        object_schema::<DescribeResult>("DescribeResult");
        object_schema::<FunctionsResult>("FunctionsResult");
        object_schema::<ValidateResult>("ValidateResult");
        object_schema::<QuerySessionResult>("QuerySessionResult");
        object_schema::<QuerySessionsResult>("QuerySessionsResult");
        object_schema::<RunResult>("RunResult");
        object_schema::<PageResult>("PageResult");
        object_schema::<ExportResult>("ExportResult");
    }
}
