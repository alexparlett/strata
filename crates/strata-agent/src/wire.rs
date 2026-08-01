//! The **wire shapes** — what a tool takes and what it answers, as JSON.
//!
//! Kept apart from [`crate::host`]'s types on purpose. A host type models the states out of
//! existence (four `Described` variants, no `Option` soup); a wire type is flat, with empty
//! collections and absent facts omitted, because that is what reads well to a model and
//! keeps a response small. The projections between them are the `from_*` functions here, so
//! no tool assembles a response by hand.
//!
//! Two conventions hold throughout:
//!
//! - **A cell is `null` or a string.** Rows arrive already formatted by the engine's
//!   `CellFormat` — the same text the grid shows — so numbers come back as strings and a
//!   null becomes JSON `null` rather than the configured NULL rendering, which is
//!   presentation.
//! - **A query-session handle is its `QuerySessionId` as text.** Not a parallel id scheme:
//!   it is the session's own `Uuid` — the same one the engine uses as its `WsId` — so a
//!   handle from `open_query_session` and one from `list_query_sessions` are the same thing.

use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use strata_core::engine::plan::QueryPlan;
use strata_core::engine::sql::{FunctionCatalog, FunctionSym};
use strata_model::{Cell, ColumnInfo, Diagnostic, Kind, QueryOutput, Severity, Stat, StatKey};

use crate::host::{
    CatalogEntry, Described, Project, QuerySessionInfo, QuerySessionState, RegState, RunMode,
};

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

// ---------------------------------------------------------------------------
// Parameters
// ---------------------------------------------------------------------------

/// The disambiguator every project-scoped tool takes: a project's root path or its name.
/// Only needed when more than one project is open — the error lists them.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ProjectParams {
    #[serde(default)]
    pub project: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DescribeTableParams {
    /// The table or view to describe. Saved queries are not in this namespace.
    pub name: String,
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

// ---------------------------------------------------------------------------
// Results
// ---------------------------------------------------------------------------

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

#[derive(Debug, Serialize, JsonSchema)]
pub struct TablesResult {
    pub entries: Vec<EntryWire>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EntryWire {
    Table {
        name: String,
        format: String,
        sources: Vec<String>,
        state: StateWire,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    View {
        name: String,
        sql: String,
        state: StateWire,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    SavedQuery {
        id: String,
        name: String,
        sql: String,
    },
}

impl From<CatalogEntry> for EntryWire {
    fn from(entry: CatalogEntry) -> EntryWire {
        match entry {
            CatalogEntry::Table {
                name,
                format,
                sources,
                reg,
            } => {
                let (state, error) = split_reg(reg);
                EntryWire::Table {
                    name,
                    format,
                    sources,
                    state,
                    error,
                }
            }
            CatalogEntry::View { name, sql, reg } => {
                let (state, error) = split_reg(reg);
                EntryWire::View {
                    name,
                    sql,
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
#[derive(Debug, Serialize, JsonSchema)]
pub struct DescribeResult {
    pub name: String,
    pub state: StateWire,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<EntryKindWire>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<String>,
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
    /// Base tables a view scans.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub reads: Vec<String>,
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

impl From<Described> for DescribeResult {
    fn from(described: Described) -> DescribeResult {
        let blank = |name: String, state: StateWire| DescribeResult {
            name,
            state,
            kind: None,
            error: None,
            format: None,
            sources: Vec::new(),
            sql: None,
            partitions: Vec::new(),
            rows: None,
            columns: Vec::new(),
            reads: Vec::new(),
        };
        match described {
            Described::Table {
                name,
                format,
                sources,
                partitions,
                rows,
                columns,
            } => DescribeResult {
                kind: Some(EntryKindWire::Table),
                format: Some(format),
                sources,
                partitions: partitions
                    .into_iter()
                    .map(|(name, dtype)| PartitionWire { name, dtype })
                    .collect(),
                rows,
                columns: columns.iter().map(ColumnWire::from).collect(),
                ..blank(name, StateWire::Ready)
            },
            Described::View {
                name,
                sql,
                columns,
                reads,
            } => DescribeResult {
                kind: Some(EntryKindWire::View),
                sql: Some(sql),
                columns: columns.iter().map(ColumnWire::from).collect(),
                reads,
                ..blank(name, StateWire::Ready)
            },
            Described::Failed { name, error } => DescribeResult {
                error: Some(error),
                ..blank(name, StateWire::Failed)
            },
            Described::Pending { name } => blank(name, StateWire::Pending),
        }
    }
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
    /// Facts the source reports **for free** — read at registration, never computed. Empty
    /// for every format without metadata to read, which is every format but Parquet and
    /// Arrow. Profiling is deliberately not exposed (spec §6).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stats: Vec<StatWire>,
}

impl From<&ColumnInfo> for ColumnWire {
    fn from(c: &ColumnInfo) -> ColumnWire {
        ColumnWire {
            name: c.name.clone(),
            dtype: c.dtype.clone(),
            kind: c.kind.into(),
            nullable: c.nullable,
            children: c.children.iter().map(ColumnWire::from).collect(),
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

#[derive(Debug, Serialize, JsonSchema)]
pub struct FunctionsResult {
    pub scalar: Vec<FunctionWire>,
    pub aggregate: Vec<FunctionWire>,
    pub window: Vec<FunctionWire>,
}

impl From<&FunctionCatalog> for FunctionsResult {
    fn from(c: &FunctionCatalog) -> FunctionsResult {
        FunctionsResult {
            scalar: c.scalar.iter().map(FunctionWire::from).collect(),
            aggregate: c.aggregate.iter().map(FunctionWire::from).collect(),
            window: c.window.iter().map(FunctionWire::from).collect(),
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct FunctionWire {
    pub name: String,
    /// One entry per overload, each an ordered list of parameter labels. A trailing `…`
    /// marks a variadic tail; an empty outer list means the registry declares no arity.
    pub signatures: Vec<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub returns: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl From<&FunctionSym> for FunctionWire {
    fn from(f: &FunctionSym) -> FunctionWire {
        FunctionWire {
            name: f.name.clone(),
            signatures: f.signatures.clone(),
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

    /// A failed def has no schema, and the flattening must not invent one.
    #[test]
    fn a_failed_description_carries_only_its_name_state_and_error() {
        let wire = DescribeResult::from(Described::Failed {
            name: "orders".into(),
            error: "No source paths".into(),
        });
        let json = serde_json::to_value(&wire).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "name": "orders",
                "state": "failed",
                "error": "No source paths",
            })
        );
    }

    #[test]
    fn a_saved_query_row_carries_no_registration_state() {
        let wire = EntryWire::from(CatalogEntry::Query {
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
        fn object_schema<T: schemars::JsonSchema>(named: &str) {
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
    }
}
