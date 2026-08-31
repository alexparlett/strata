//! Export one result snapshot to disk — one file, or a Hive directory when partition columns
//! are given.
//!
//! **The spec is the whole of what a caller says; the writing is `statements::copy_job`'s.** This
//! module turns an [`ExportSpec`] into the [`CopyJob`] every write in the engine goes through: the
//! rows as a `DataFrame` over the snapshot, the writer out of the session's own format registry,
//! and the options in DataFusion's own namespaced spelling. Nothing here composes SQL.
//!
//! **The spec is shaped so an impossible export can't be spelled.** Write options belong to
//! the format that has them ([`Format`] carries its own struct), so "a CSV delimiter on a
//! Parquet export" is unrepresentable rather than ignored — and [`Format::Arrow`] is a bare
//! variant because DataFusion exposes **no** Arrow IPC write options at all. Every field
//! below maps to a real DataFusion 54 `COPY … OPTIONS` key; nothing here offers a knob the
//! writer would silently drop.
//!
//! **The snapshot is the source, never a re-run** (`docs/SNAPSHOT_SPEC.md`): an export reads
//! the same immutable table the grid pages, with the same [`ExportSpec::sort`] the grid is
//! showing, so what lands on disk is what was on screen.
//!
//! **Where a write may land is this module's, and every surface that writes one reaches it.**
//! Three of them do — the Export window, a typed `COPY … TO` (`statements::arms::copy`) and the
//! agent's `export_result` (QE-05) — and none may land in storage Strata owns
//! ([`refuse_owned_target`], over the roots [`owned_roots`] gathers) or name a partition column
//! that is not one bare word ([`partition_columns_are_bare_words`]). One copy of a rule the user
//! reads as one.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::env;
use std::fs::{self, Metadata};
use std::path::{is_separator, Component, Path, PathBuf};
use std::sync::Arc;

use datafusion::arrow::record_batch::RecordBatch;
use datafusion::common::file_options::file_type::FileType;
use datafusion::datasource::file_format::format_as_file_type;
use datafusion::execution::SessionState;
use datafusion::logical_expr::{Expr, LogicalPlan};
use datafusion::prelude::*;
use datafusion::sql::sqlparser::dialect::Dialect;

use super::snapshots::snapshot_name;
use crate::snapshots::SnapshotStats;
use crate::sql;
use crate::statements::copy_job::{run_copy, CopyJob, NullEvidence};
use strata_core::project::strata_dir;
use strata_model::SnapshotId;

/// Everything one export needs: where it goes, how much of the snapshot, in what order, in
/// what format, and whether it fans out into a Hive tree.
#[derive(Clone, Debug, PartialEq)]
pub struct ExportSpec {
    /// The destination — a file for a flat export, a directory when `partition` has columns.
    pub path: String,
    /// How much of the snapshot to write.
    pub scope: Scope,
    /// `(column name, ascending)` — the grid's active sort, applied over the **whole**
    /// snapshot before any row window. `None` = snapshot order.
    pub sort: Option<(String, bool)>,
    /// The output format and its write options.
    pub format: Format,
    /// The columns the output fans out over, if any.
    pub partition: Partition,
}

/// What one finished export wrote.
///
/// Every figure is read rather than derived: `rows` is the count `COPY` itself returns and
/// `bytes` is the written file's own size.
///
/// `bytes` is `None` for two reasons. A partitioned export writes a Hive directory, whose own
/// metadata is not the size of the data under it. And the size is read back *after* a write that
/// has already succeeded, so a stat that fails is a fact this call could not learn, never a
/// failed export.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportReport {
    /// Where the write landed.
    pub path: String,
    /// How many rows it wrote.
    pub rows: usize,
    /// How large the file is, where one file was written and could be read back.
    pub bytes: Option<u64>,
}

impl ExportReport {
    /// Reads back what the write at `path` left on disk.
    pub(crate) fn of(path: String, rows: usize) -> Self {
        let bytes = fs::metadata(&path)
            .ok()
            .filter(Metadata::is_file)
            .map(|written| written.len());
        ExportReport { path, rows, bytes }
    }
}

/// How much of the snapshot to write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// Every row.
    All,
    /// One page window, in the grid's own 1-based paging terms.
    Page {
        /// Which page, 1-based.
        page: usize,
        /// How many rows a page holds.
        page_size: usize,
    },
}

/// The output format, each carrying exactly the write options DataFusion honours for it.
#[derive(Clone, Debug, PartialEq)]
pub enum Format {
    /// Delimited text.
    Csv(Csv),
    /// Newline-delimited JSON, which is the only shape DataFusion writes: its `JsonSerializer`
    /// is an `arrow::json::LineDelimitedWriter` with no array mode, so there is no shape option
    /// to spell. `newline_delimited` is a *read* option.
    Json(Json),
    /// Apache Parquet.
    Parquet(Parquet),
    /// Arrow IPC — **no write options exist**, which is why this variant carries nothing.
    Arrow,
    /// A format registered with [`EngineBuilder::with_format`](crate::EngineBuilder::with_format),
    /// written through the writer it brought.
    ///
    /// Its options are strings because they are its own writer's, not ours: there is no options
    /// panel to draw for a format this build does not know the settings of, so they travel in the
    /// `format.*` spelling a typed `COPY` writes them in.
    Extension {
        /// The word the writer is registered under.
        format: String,
        /// That writer's `format.*` options, verbatim.
        options: BTreeMap<String, String>,
    },
}

impl Format {
    /// The word this format is registered under — the `STORED AS` keyword in an editor, and the
    /// key DataFusion resolves the writer under here. One name for both, so a format the editor
    /// can write is a format the window can write.
    fn word(&self) -> &str {
        match self {
            Self::Csv(_) => "CSV",
            Self::Json(_) => "JSON",
            Self::Parquet(_) => "PARQUET",
            Self::Arrow => "ARROW",
            Self::Extension { format, .. } => format,
        }
    }

    /// The writer, out of the session's own file-format registry.
    ///
    /// **The same lookup the `COPY` planner does**, so a typed `STORED AS geojson` and a
    /// `Format::Extension { format: "geojson" }` resolve to one writer or to neither.
    ///
    /// # Errors
    ///
    /// Nothing is registered under this format's word.
    pub(crate) fn file_type(&self, state: &SessionState) -> Result<Arc<dyn FileType>, String> {
        let word = self.word();
        state
            .get_file_format_factory(word)
            .map(format_as_file_type)
            .ok_or_else(|| format!("Can't write '{word}': no writer is registered for it"))
    }

    /// The write options this format contributes, in DataFusion's own namespaced spelling.
    ///
    /// # Errors
    ///
    /// A CSV single-character option that is not one ASCII byte ([`ascii_byte`]).
    pub(crate) fn options(&self) -> Result<HashMap<String, String>, String> {
        let pairs: Vec<(&str, String)> = match self {
            Format::Csv(csv) => {
                let mut pairs = vec![
                    ("has_header", csv.header.to_string()),
                    ("delimiter", ascii_byte("delimiter", csv.delimiter)?),
                    ("quote", ascii_byte("quote character", csv.quote)?),
                    ("double_quote", csv.double_quote.to_string()),
                    ("null_value", csv.null_value.clone()),
                    ("compression", csv.compression.as_option().into()),
                ];
                if let Some(escape) = csv.escape {
                    pairs.push(("escape", ascii_byte("escape character", escape)?));
                }
                pairs
            }
            Format::Json(json) => vec![("compression", json.compression.as_option().into())],
            Format::Parquet(pq) => vec![
                ("compression", pq.compression.as_option()),
                ("statistics_enabled", pq.statistics.as_option().into()),
                (
                    "max_row_group_size",
                    pq.max_row_group_size.max(1).to_string(),
                ),
                ("writer_version", pq.writer_version.as_option().into()),
                ("dictionary_enabled", pq.dictionary.to_string()),
            ],
            Format::Arrow => vec![],
            Format::Extension { options, .. } => {
                return Ok(options
                    .iter()
                    .map(|(key, value)| (namespaced(key), value.clone()))
                    .collect())
            }
        };
        Ok(pairs
            .into_iter()
            .map(|(key, value)| (namespaced(key), value))
            .collect())
    }
}

/// An option key as DataFusion's own `COPY` planner would file it: lowercased, and prefixed with
/// `format.` when it carries no namespace of its own.
///
/// `SqlToRel::parse_options_map`'s rule, restated because a plan-built `COPY` never passes through
/// it — and applied to a registrant's own keys, which may already name a namespace, exactly as to
/// ours.
fn namespaced(key: &str) -> String {
    match key.contains('.') {
        true => key.to_lowercase(),
        false => format!("format.{}", key.to_lowercase()),
    }
}

/// The write options a caller that was offered none gets.
///
/// [`Default`] rather than a constant per surface because it is a property of the format: a
/// reader has to be able to open what was written with nothing said, so the defaults are the
/// self-describing spellings (a header row, a comma, `"` quotes doubled, no compression). The
/// Export window's draft starts on the same values and is free to move: it exists to be edited,
/// while [`crate::SnapshotReads::export_to`] offers no options at all and these are what it
/// writes.
impl Default for Csv {
    fn default() -> Csv {
        Csv {
            header: true,
            delimiter: ',',
            null_value: String::new(),
            quote: '"',
            escape: None,
            double_quote: true,
            compression: Compression::None,
        }
    }
}

impl Default for Json {
    fn default() -> Json {
        Json {
            compression: Compression::None,
        }
    }
}

impl Default for Parquet {
    fn default() -> Parquet {
        Parquet {
            compression: Codec::Zstd(3),
            statistics: Statistics::Page,
            max_row_group_size: 1_048_576,
            writer_version: WriterVersion::V1,
            dictionary: true,
        }
    }
}

/// CSV write options.
#[derive(Clone, Debug, PartialEq)]
pub struct Csv {
    /// Write the column-name row.
    pub header: bool,
    /// Field separator. One ASCII character — the UI resolves `\t` before it gets here.
    pub delimiter: char,
    /// Text written for NULL cells (empty by default).
    pub null_value: String,
    /// Character fields containing the delimiter are wrapped in.
    pub quote: char,
    /// Character that escapes a quote. `None` leaves DataFusion's default.
    pub escape: Option<char>,
    /// Escape quotes by doubling them rather than with `escape`.
    pub double_quote: bool,
    /// Whole-file compression (changes the extension — the UI reflects that).
    pub compression: Compression,
}

/// JSON write options.
#[derive(Clone, Debug, PartialEq)]
pub struct Json {
    /// Whole-file compression.
    pub compression: Compression,
}

/// Parquet write options.
#[derive(Clone, Debug, PartialEq)]
pub struct Parquet {
    /// The column codec.
    pub compression: Codec,
    /// How much column statistics to write.
    pub statistics: Statistics,
    /// Rows per row group — a **row count**, not a byte size.
    pub max_row_group_size: usize,
    /// Which Parquet format version to write.
    pub writer_version: WriterVersion,
    /// Dictionary-encode where it pays.
    pub dictionary: bool,
}

/// Whole-file compression, for the formats where compression wraps the file rather than
/// encoding columns (CSV / JSON). Levelless: the canvas offers the codec only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compression {
    /// Uncompressed.
    None,
    /// gzip, `.gz`.
    Gzip,
    /// Zstandard, `.zst`.
    Zstd,
    /// bzip2, `.bz2`.
    Bzip2,
    /// xz, `.xz`.
    Xz,
}

impl Compression {
    fn as_option(&self) -> &'static str {
        match self {
            Self::None => "uncompressed",
            Self::Gzip => "gzip",
            Self::Zstd => "zstd",
            Self::Bzip2 => "bzip2",
            Self::Xz => "xz",
        }
    }

    /// The suffix this compression adds to the destination's own extension, so a filename
    /// shown to the user matches what is written (`orders.csv` → `orders.csv.gz`).
    pub fn extension(&self) -> &'static str {
        match self {
            Self::None => "",
            Self::Gzip => ".gz",
            Self::Zstd => ".zst",
            Self::Bzip2 => ".bz2",
            Self::Xz => ".xz",
        }
    }
}

/// Parquet's column codec. The level rides with the codecs that take one, so a level can't
/// be set on a codec that would ignore it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    /// Uncompressed.
    Uncompressed,
    /// Snappy.
    Snappy,
    /// LZ4.
    Lz4,
    /// gzip, at the level given.
    Gzip(u32),
    /// Brotli, at the level given.
    Brotli(u32),
    /// Zstandard, at the level given.
    Zstd(u32),
}

impl Codec {
    /// DataFusion takes the level inside the codec string — `zstd(3)`.
    fn as_option(&self) -> String {
        match self {
            Self::Uncompressed => "uncompressed".into(),
            Self::Snappy => "snappy".into(),
            Self::Lz4 => "lz4".into(),
            Self::Gzip(level) => format!("gzip({})", level.clamp(&1, &9)),
            Self::Brotli(level) => format!("brotli({})", level.clamp(&1, &11)),
            Self::Zstd(level) => format!("zstd({})", level.clamp(&1, &22)),
        }
    }
}

/// How much column statistics Parquet writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Statistics {
    /// None at all.
    None,
    /// Per column chunk.
    Chunk,
    /// Per page.
    Page,
}

impl Statistics {
    fn as_option(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Chunk => "chunk",
            Self::Page => "page",
        }
    }
}

/// Parquet format version. 2.0 enables newer encodings; 1.0 is the compatible floor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriterVersion {
    /// 1.0, the compatible floor.
    V1,
    /// 2.0, which enables the newer encodings.
    V2,
}

impl WriterVersion {
    fn as_option(&self) -> &'static str {
        match self {
            Self::V1 => "1.0",
            Self::V2 => "2.0",
        }
    }
}

/// Hive-style partitioning. Empty `columns` = a flat single-file export, which is why this
/// is one struct rather than an `Option` the caller has to keep consistent with `keep_columns`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Partition {
    /// Directory levels, outermost first.
    pub columns: Vec<String>,
    /// Also write the partition columns *inside* the files. Off by default — they live in
    /// the directory names.
    pub keep_columns: bool,
}

impl Partition {
    fn is_flat(&self) -> bool {
        self.columns.is_empty()
    }
}

/// Write one snapshot per `spec`. A plain file path (extension) → one file; partition columns →
/// a Hive-partitioned directory. Returns `(path, rows_written)`.
pub(crate) async fn run_export(
    ctx: &SessionContext,
    snapshot: SnapshotId,
    spec: ExportSpec,
    stats: &SnapshotStats,
    owned: &[Owned],
) -> Result<(String, usize), String> {
    let snap = snapshot_name(snapshot);
    let Ok(table) = ctx.table(snap.as_str()).await else {
        return Err("No results to export — run a query first".to_string());
    };

    partition_columns_are_bare_words(&spec.partition.columns, ctx)?;

    let mut options = spec.format.options()?;
    if !spec.partition.is_flat() {
        options.insert(
            KEEP_PARTITION_COLUMNS.to_string(),
            spec.partition.keep_columns.to_string(),
        );
    }
    let job = CopyJob {
        input: Arc::new(snapshot_rows(table, &spec, stats.ord.as_deref())?),
        target: spec.path.clone(),
        file_type: spec.format.file_type(&ctx.state())?,
        options,
        partition_by: spec.partition.columns.clone(),
    };
    let rows = run_copy(ctx, job, owned, NullEvidence::Snapshot(stats), "Export").await?;
    Ok((spec.path, rows))
}

/// The rows the export writes: the result's columns — **explicitly, never `*`** — over the whole
/// snapshot or one page window, in the grid's order.
///
/// Explicit because the snapshot carries the ordinal column (`docs/SNAPSHOT_SPEC.md` §9), and a
/// write must not put bookkeeping in the user's file. The ordinal is what the read *orders by*
/// instead: alone for an unsorted export, as the tie-break under a user sort — the same rule as
/// `fetch_page`, which is what makes "the file matches what was on screen" true rather than
/// hopeful (an unordered `LIMIT/OFFSET` over a split scan is nondeterministic, measured in §9).
///
/// **Sorted, then windowed, then projected**, which is what a `SELECT … ORDER BY … LIMIT` means:
/// "this page" is the page the user is looking at rather than an arbitrary slice re-ordered
/// afterwards, and the ordinal is still there to sort by when the projection drops it.
/// `NULLS LAST` in both directions matches the grid's own ordering (Rz6).
fn snapshot_rows(
    table: DataFrame,
    spec: &ExportSpec,
    ord: Option<&str>,
) -> Result<LogicalPlan, String> {
    let columns: Vec<Expr> = table
        .schema()
        .fields()
        .iter()
        .map(|f| f.name().as_str())
        .filter(|name| ord != Some(name))
        .map(ident)
        .collect();

    let mut order = Vec::new();
    if let Some((name, asc)) = &spec.sort {
        order.push(ident(name).sort(*asc, false));
    }
    if let Some(ord) = ord {
        order.push(ident(ord).sort(true, false));
    }

    let mut rows = table;
    if !order.is_empty() {
        rows = rows.sort(order).map_err(|e| e.to_string())?;
    }
    if let Scope::Page { page, page_size } = spec.scope {
        let offset = page.saturating_sub(1).saturating_mul(page_size);
        rows = rows
            .limit(offset, Some(page_size))
            .map_err(|e| e.to_string())?;
    }
    rows.select(columns)
        .map(DataFrame::into_unoptimized_plan)
        .map_err(|e| e.to_string())
}

/// Whether a partitioned write also puts the partition columns **inside** the files.
///
/// Sent as a **COPY option**, not as a session `SET`. DataFusion's physical planner reads this
/// exact key out of the statement's own options and only falls back to the session config when it
/// is absent (`physical_planner.rs`, the `Copy` arm), so an export states its own answer and
/// leaves the session's alone. The `SET` this replaces was never restored: invisible while
/// nothing else could read the option, and — the moment `SET` and `SHOW` became statements a user
/// can type (ED-08) — one partitioned export silently rewriting an engine option for every later
/// one, window or typed. Namespaced rather than bare because `TableOptions::set` ignores the whole
/// `execution.` namespace, which is what lets the key reach the planner without the format
/// refusing it as unknown.
const KEEP_PARTITION_COLUMNS: &str = "execution.keep_partition_by_columns";

/// A single-character CSV option as its **byte value**, which is how it must be sent.
///
/// DataFusion parses these `u8` fields by trying `str::parse::<u8>()` *first* and only then
/// falling back to "the one ASCII character" — so the character `9` would arrive as byte 9,
/// a tab. Sending the number always sidesteps that: every character has exactly one reading.
fn ascii_byte(what: &str, c: char) -> Result<String, String> {
    if !c.is_ascii() {
        return Err(format!(
            "The CSV {what} has to be a single ASCII character, not {c:?}"
        ));
    }
    Ok((c as u32).to_string())
}

/// Refuse any partition column the engine's own parser dialect doesn't read as a single
/// bare word.
///
/// `PARTITIONED BY` takes **bare** identifiers, and quoting is not an option: a Hive directory
/// segment is `name=value`, so a name the tokenizer does not read as one word can never equal the
/// segment it was written under. Bare is case-preserving here, so every name that round-trips is
/// accepted and one that cannot be expressed is said so plainly.
///
/// Its own sync function rather than an inline check, because the resolved dialect is not `Send`
/// and [`run_export`] is spawned onto the engine runtime.
///
/// **Shared by the three surfaces that carry a `PARTITIONED BY`** — the Export window, whose
/// columns are the spec's, and the two typed statements, whose columns are
/// `Ident::to_string()`'s output. A quoted `PARTITIONED BY ("order date")` therefore arrives here
/// *with its quotes*, which for a `COPY` is a name matching no field and for a registration is a
/// stored name no folder segment can equal. One clause, one rule, so the wording names
/// **`PARTITIONED BY`** rather than any statement. The bad name is rendered inside single quotes
/// rather than by `Debug`, so it reads as what the user typed instead of as escaped Rust.
pub(crate) fn partition_columns_are_bare_words(
    columns: &[String],
    ctx: &SessionContext,
) -> Result<(), String> {
    let dialect = sql::lex::dialect(ctx.state().config_options().sql_parser.dialect.as_ref());
    match columns.iter().find(|c| !is_bare_word(dialect.as_ref(), c)) {
        Some(bad) => Err(format!(
            "Can't partition by '{bad}': PARTITIONED BY takes unquoted column names, so a \
             partition column has to be a single plain word"
        )),
        None => Ok(()),
    }
}

/// Whether `name` tokenises as a single **unquoted** identifier, asked of the very
/// dialect DataFusion will parse the generated `COPY` with rather than a hardcoded
/// character set (`sql::lex` follows the same setting for the editor).
///
/// The dialect has to be the caller's, not a constant: `generic` reads `region#eu` as one
/// identifier and `postgresql` does not, so a hardcoded one would wave through a partition
/// column that then emits a `PARTITIONED BY` the planner chokes on — the exact parser
/// message this check exists to replace (WJ-04).
fn is_bare_word(dialect: &dyn Dialect, name: &str) -> bool {
    let mut rest = name.chars();
    matches!(rest.next(), Some(c) if dialect.is_identifier_start(c))
        && rest.all(|c| dialect.is_identifier_part(c))
}

/// One root a write must stay out of, and what it holds — the phrase its refusal reads.
pub(crate) type Owned = (PathBuf, &'static str);

/// Every root a write must stay out of: what this engine's stores say they keep their bytes
/// under, plus the project's own `.strata/`.
///
/// **The stores are asked rather than guessed.** Where results live is a
/// [`SnapshotStore`](crate::snapshots::SnapshotStore) and where Strata's own tables live is an
/// [`InternalTableStore`](crate::tables::InternalTableStore); a store with nothing on the
/// filesystem answers nothing and fences nothing.
///
/// The project's own directory is here rather than answered by the table store, because it holds
/// more than tables — the session, the conversations — and is fenced whether or not a store
/// follows it. Order is the order a refusal is worded in, so the nested default
/// (`.strata/tables`, under `.strata/`) reads as the project's own data.
pub(crate) fn owned_roots(
    root: Option<&Path>,
    snapshots: Vec<PathBuf>,
    tables: Vec<PathBuf>,
) -> Vec<Owned> {
    let mut owned: Vec<Owned> = snapshots
        .into_iter()
        .map(|dir| (dir, "holds query results"))
        .collect();
    if let Some(root) = root {
        owned.push((strata_dir(root), "holds this project's own data"));
    }
    owned.extend(
        tables
            .into_iter()
            .map(|dir| (dir, "holds tables Strata owns the data of")),
    );
    owned
}

/// Refuse a write whose target lands in storage Strata owns.
///
/// **What is fenced is where a stray file changes what Strata later reads.** A file under a
/// table's directory is listed by that table's next scan; one under the snapshot spool is read
/// back as a result. Everywhere else on the disk is the user's own, and a write that overwrites
/// their file is the statement doing what it says.
///
/// **Resolved, never compared as text.** A relative target is the process's cwd away from an
/// absolute one, and `'.strata/../.strata/tables'` names the fenced directory without sharing its
/// prefix. The target need not exist yet, so `canonicalize` cannot be asked about it directly: the
/// path is made absolute, its `.` and `..` segments are folded away, and both sides are then
/// anchored on the deepest ancestor that *does* exist — which is what makes a symlinked project
/// folder compare equal to the path the fence was built from.
///
/// `subject` is what the sentence is about, because three surfaces reach this and the user reads a
/// refusal as being about the thing they did: `COPY` for the typed statement, `Export` for the
/// window and for [`check_destination`]'s caller. Only the subject differs — the rule, the roots
/// and the reason are one copy.
pub(crate) fn refuse_owned_target(
    target: &str,
    owned: &[Owned],
    subject: &str,
) -> Result<(), String> {
    let local = match target.split_once("://") {
        Some((scheme, rest)) if is_url_scheme(scheme) => {
            match scheme.eq_ignore_ascii_case("file") {
                true => Cow::Owned(format!("/{}", rest.trim_start_matches('/'))),
                false => return Ok(()),
            }
        }
        _ => Cow::Borrowed(target),
    };
    let path = resolve(Path::new(local.as_ref()));

    for (dir, what) in owned {
        if path.starts_with(resolve(dir)) {
            return Err(format!(
                "{subject} can't write into '{}', which {what}",
                dir.display(),
            ));
        }
    }
    Ok(())
}

/// Whether `s` is shaped like a URL scheme — RFC 3986's `ALPHA *( ALPHA / DIGIT / "+" / "-" / "." )`.
///
/// A path separator can never appear in one, which is the whole point: it is what tells
/// `s3://bucket` from a local file whose name happens to contain `://`.
fn is_url_scheme(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// `path` as an absolute path with `.` and `..` folded away, anchored on the deepest ancestor that
/// exists. See [`refuse_owned_target`] for why each of the three steps is there.
///
/// **The whole path existing is its own case, because `join("")` is not a no-op.** Pushing an
/// empty relative path leaves a trailing separator, and `stat` on `some-file/` is `ENOTDIR` — so a
/// resolved path that names an existing *file* answered `exists() == false`, which
/// [`check_destination`]'s no-overwrite rule reads as a free name. `starts_with` is
/// component-wise and never noticed.
fn resolve(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir().unwrap_or_default().join(path)
    };
    let mut folded = PathBuf::new();
    for part in absolute.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                folded.pop();
            }
            other => folded.push(other),
        }
    }
    let mut existing: &Path = &folded;
    loop {
        if let Ok(real) = existing.canonicalize() {
            let rest = folded.strip_prefix(existing).unwrap_or(Path::new(""));
            return match rest.as_os_str().is_empty() {
                true => real,
                false => real.join(rest),
            };
        }
        match existing.parent() {
            Some(parent) => existing = parent,
            None => return folded,
        }
    }
}

/// The characters that make `ListingTableUrl::parse` read a path as a **glob pattern** rather
/// than a path — `datafusion-datasource`'s own `GLOB_START_CHARS`, restated here because it is
/// private and it decides where a write actually lands.
const GLOB_CHARS: [char; 3] = ['?', '*', '['];

/// Whether `path`'s last segment carries an extension, which is what makes DataFusion write it as
/// **one file** rather than a directory of part files (`FileOutputMode::Automatic`).
///
/// DataFusion's own predicate, restated: its `ListingTableUrl::file_extension` asks whether the
/// last URL segment contains a `.` and does not end with one. Rust's `Path::extension` is *not*
/// the same question — it answers `None` for a dotfile like `.gitignore`, which DataFusion counts
/// as having one — and disagreeing here would refuse a path that would have worked.
fn names_one_file(path: &str) -> bool {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.contains('.') && !name.ends_with('.'))
}

/// Where an export a **caller named** may land (QE-05): an absolute local path naming a file that
/// does not exist yet, in a folder that does, outside the storage Strata owns.
///
/// **The path is the whole fence, because the data is not one.** An agent's `export_result` reads
/// a result it can already page byte for byte, so nothing here protects the *contents*; what is
/// new is that a caller with no file dialog in front of it names the destination. So the rules are
/// about the write: it never lands where Strata reads back what it finds, it never overwrites, and
/// it never makes a folder.
///
/// **The shape refusals are not extra caution — they are what make the other three *true*, and
/// each of them is a thing DataFusion would otherwise do quietly to a path nobody vetted.** A
/// relative path resolves against a process cwd the caller cannot see, so "the parent exists" and
/// "the file does not" would be answered about a folder it never meant; a remote target has no
/// local file to ask either question of, so the no-overwrite promise could not be kept there. The
/// last three are the same rule read off `FileOutputMode::single_file_output` and
/// `ListingTableUrl::parse`, which together decide what the target even *is*:
///
/// - **A glob character (`?`, `*`, `[`) makes the path a pattern**, and `parse` then splits it —
///   the write lands in the directory *before* the glob under a generated name. Measured: an
///   export to `…/report[1].csv` reported success at that path while the rows went to
///   `…/<random>_0.csv` beside it, so the answer named a file that does not exist.
/// - **A trailing separator is a collection**, so DataFusion fans part files into it.
/// - **No extension is a collection too** — `Automatic` mode is single-file only when the last
///   segment carries one. Measured: an export to `…/results` created a *directory* holding
///   `<random>_0.csv`, and `bytes` reported the directory inode's 96 as the file's size.
///
/// The typed `COPY` is unaffected and keeps exactly one of these ([`refuse_owned_target`]): a
/// statement the user typed in their own editor may overwrite their own file and may ask for a
/// directory of part files, which is the statement doing what it says.
pub(crate) fn check_destination(path: &str, owned: &[Owned]) -> Result<(), String> {
    if path
        .split_once("://")
        .is_some_and(|(scheme, _)| is_url_scheme(scheme))
    {
        return Err(format!(
            "Export writes a local file, and '{path}' names a remote location. Give an absolute \
             path on this machine"
        ));
    }
    if let Some(glob) = path.chars().find(|c| GLOB_CHARS.contains(c)) {
        return Err(format!(
            "'{path}' contains '{glob}', which reads as a filename pattern rather than a path. \
             Give a path with no '?', '*' or '[' in it"
        ));
    }
    if !Path::new(path).is_absolute() {
        return Err(format!(
            "Export takes an absolute path, and '{path}' is relative"
        ));
    }
    if path.ends_with(is_separator) {
        return Err(format!(
            "'{path}' names a folder, and an export writes one file. Give the path of the file to \
             write"
        ));
    }
    if !names_one_file(path) {
        return Err(format!(
            "'{path}' has no file extension, so it would be written as a folder of part files \
             rather than one file. Give the file an extension, such as '.csv'"
        ));
    }
    refuse_owned_target(path, owned, "Export")?;

    let resolved = resolve(Path::new(path));
    if resolved.exists() {
        return Err(format!(
            "'{path}' already exists, and an export never overwrites. Give a path that is not taken"
        ));
    }
    match resolved.parent() {
        Some(parent) if parent.is_dir() => Ok(()),
        _ => Err(format!(
            "'{}' does not exist, and an export writes a file rather than the folders above it",
            Path::new(path).parent().unwrap_or(Path::new("")).display()
        )),
    }
}

/// A write returns a single `UInt64` "count" column with the rows it wrote.
///
/// One shape, three writers: DataFusion's `COPY` node answers it, and so does every
/// `DataSinkExec` — which is why `sink`'s remote append reads its answer out of here too.
pub(crate) fn copy_row_count(batches: &[RecordBatch]) -> usize {
    use datafusion::arrow::array::UInt64Array;
    let Some(batch) = batches.first() else {
        return 0;
    };
    if batch.num_columns() == 0 {
        return 0;
    }
    batch
        .column(0)
        .as_any()
        .downcast_ref::<UInt64Array>()
        .filter(|a| !a.is_empty())
        .map(|a| a.value(0) as usize)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::{fs, process};

    use datafusion::arrow::array::{Int64Array, StringArray, UInt64Array};
    use datafusion::arrow::datatypes::{DataType, Field, Schema};
    use datafusion::arrow::record_batch::RecordBatch;

    use crate::builder::test_context;
    use crate::snapshots::{LocalIpcSnapshotStore, MemSnapshotStore};
    use crate::{Engine, RunRows, RunTag, WsId};

    use super::*;

    fn spec(format: Format) -> ExportSpec {
        ExportSpec {
            path: "/tmp/out.csv".into(),
            scope: Scope::All,
            sort: None,
            format,
            partition: Partition::default(),
        }
    }

    fn csv() -> Csv {
        Csv {
            header: true,
            delimiter: ',',
            null_value: String::new(),
            quote: '"',
            escape: None,
            double_quote: true,
            compression: Compression::None,
        }
    }

    /// The roots a project alone owns — what every fence test that is not about a store fences.
    fn project_roots(root: &Path) -> Vec<Owned> {
        owned_roots(Some(root), Vec::new(), Vec::new())
    }

    /// **Every option key is written the way DataFusion's own `COPY` planner would file it.** A
    /// plan-built write never passes through `parse_options_map`, so the keys have to arrive
    /// already lowercased and already namespaced — a bare `HAS_HEADER` reaches no `CsvOptions`
    /// field at all and is silently nothing rather than an error.
    #[test]
    fn a_formats_options_are_namespaced_the_way_the_planner_files_them() {
        let options = Format::Csv(csv()).options().expect("csv options");
        assert_eq!(
            options.get("format.has_header").map(String::as_str),
            Some("true")
        );
        assert!(
            options.keys().all(|key| key.starts_with("format.")),
            "{options:?}"
        );
    }

    /// **A single-character CSV option travels as its byte value.** DataFusion parses these `u8`
    /// fields by trying `str::parse::<u8>()` *first* and only then falling back to "the one ASCII
    /// character", so the character `9` would arrive as byte 9 — a tab. The number has exactly one
    /// reading.
    #[test]
    fn csv_single_char_options_are_sent_as_byte_values() {
        let options = Format::Csv(csv()).options().expect("csv options");
        assert_eq!(
            options.get("format.delimiter").map(String::as_str),
            Some("44")
        );
        assert_eq!(options.get("format.quote").map(String::as_str), Some("34"));
        assert!(!options.contains_key("format.escape"), "{options:?}");

        let tabbed = Format::Csv(Csv {
            delimiter: '\t',
            ..csv()
        })
        .options()
        .expect("csv options");
        assert_eq!(
            tabbed.get("format.delimiter").map(String::as_str),
            Some("9")
        );
    }

    #[test]
    fn a_non_ascii_delimiter_is_refused_in_our_own_words() {
        let err = Format::Csv(Csv {
            delimiter: '£',
            ..csv()
        })
        .options()
        .expect_err("non-ASCII delimiter");
        assert!(err.contains("single ASCII character"), "{err}");
    }

    /// **An option value is a value, so it travels verbatim.** It used to be escaped for a
    /// single-quoted SQL literal, because the option clause was rendered text; a plan carries the
    /// string itself, and doubling the quote now would put the doubled quote in the user's file.
    #[test]
    fn a_quote_in_the_null_text_travels_as_itself() {
        let options = Format::Csv(Csv {
            null_value: "it's null".into(),
            ..csv()
        })
        .options()
        .expect("csv options");
        assert_eq!(
            options.get("format.null_value").map(String::as_str),
            Some("it's null")
        );
    }

    #[test]
    fn a_parquet_level_rides_inside_the_codec_string() {
        let options = Format::Parquet(Parquet {
            compression: Codec::Zstd(9),
            statistics: Statistics::Page,
            max_row_group_size: 1_048_576,
            writer_version: WriterVersion::V1,
            dictionary: true,
        })
        .options()
        .expect("parquet options");
        assert_eq!(
            options.get("format.compression").map(String::as_str),
            Some("zstd(9)")
        );
        assert_eq!(
            options.get("format.max_row_group_size").map(String::as_str),
            Some("1048576")
        );
        assert_eq!(
            options.get("format.writer_version").map(String::as_str),
            Some("1.0")
        );
    }

    #[test]
    fn a_levelless_codec_carries_no_parens() {
        assert_eq!(Codec::Snappy.as_option(), "snappy");
        assert_eq!(Codec::Uncompressed.as_option(), "uncompressed");
    }

    #[test]
    fn arrow_writes_no_options_at_all() {
        assert!(Format::Arrow.options().expect("arrow").is_empty());
    }

    /// **A registrant's own option keys are its own.** They arrive in whatever spelling its writer
    /// reads them in, so a key that already names a namespace keeps it and a bare one is filed
    /// under `format.` — the planner's rule, applied to a caller's strings rather than to ours.
    /// Nothing is refused: a key is a map key now, not grammar spliced into a statement.
    #[test]
    fn a_registered_formats_keys_follow_the_planners_own_namespacing() {
        let options = Format::Extension {
            format: "geojson".into(),
            options: BTreeMap::from([
                ("CRS".into(), "EPSG:4326".into()),
                ("format.precision".into(), "7".into()),
            ]),
        }
        .options()
        .expect("a registrant's options");
        assert_eq!(
            options.get("format.crs").map(String::as_str),
            Some("EPSG:4326")
        );
        assert_eq!(
            options.get("format.precision").map(String::as_str),
            Some("7")
        );
    }

    /// **The writer is the session's, resolved by the same key `STORED AS` resolves.** So a format
    /// the editor can write is one the window can write, and a name nothing is registered under is
    /// refused here rather than deep inside a planner.
    #[test]
    fn a_format_resolves_the_writer_the_stored_as_word_resolves() {
        let state = test_context(&BTreeMap::new()).state();
        for format in [
            Format::Csv(csv()),
            Format::Json(Json::default()),
            Format::Parquet(Parquet::default()),
            Format::Arrow,
        ] {
            format.file_type(&state).expect("a shipped writer");
        }
        assert_eq!(
            Format::Extension {
                format: "geojson".into(),
                options: BTreeMap::new(),
            }
            .file_type(&state)
            .err(),
            Some("Can't write 'geojson': no writer is registered for it".to_string())
        );
    }

    /// **Keep-columns rides in the statement, never in the session.** It is the one option that is
    /// not a format option, and it keeps its `execution.` namespace so the COPY planner reads it
    /// and `TableOptions::set` skips it. The `SET` this replaced was global and unrestored, so one
    /// partitioned export decided the answer for every later one.
    #[test]
    fn keeping_partition_columns_is_a_copy_option_in_its_own_namespace() {
        assert_eq!(
            KEEP_PARTITION_COLUMNS,
            "execution.keep_partition_by_columns"
        );
        assert_eq!(
            namespaced(KEEP_PARTITION_COLUMNS),
            KEEP_PARTITION_COLUMNS,
            "a key that names its own namespace keeps it"
        );
    }

    #[test]
    fn compression_names_the_suffix_it_adds_to_the_destination() {
        assert_eq!(Compression::None.extension(), "");
        assert_eq!(Compression::Gzip.extension(), ".gz");
        assert_eq!(Compression::Zstd.extension(), ".zst");
    }

    /// A snapshot as the export sees one: the result's columns plus the ordinal, last.
    fn snapshot_frame(ctx: &SessionContext) -> DataFrame {
        let schema = Arc::new(Schema::new(vec![
            Field::new("amount", DataType::Int64, true),
            Field::new("name", DataType::Utf8, true),
            Field::new("__strata_ord", DataType::UInt64, false),
        ]));
        let rows = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int64Array::from(vec![3, 1])),
                Arc::new(StringArray::from(vec!["c", "a"])),
                Arc::new(UInt64Array::from(vec![1_u64, 2])),
            ],
        )
        .expect("a snapshot batch");
        ctx.read_batch(rows).expect("a frame over the snapshot")
    }

    /// **The ordinal orders the read and is then projected away.** It is bookkeeping
    /// (`docs/SNAPSHOT_SPEC.md` §9) and must never reach the user's file, but it is also the only
    /// thing that makes an unsorted export deterministic — so it is sorted by first and dropped
    /// last, which is what a `SELECT … ORDER BY` means and what the projection here has to keep
    /// true now that there is no SQL saying it.
    #[test]
    fn the_ordinal_orders_the_read_and_never_lands_in_the_file() {
        let ctx = test_context(&BTreeMap::new());
        let plan = snapshot_rows(
            snapshot_frame(&ctx),
            &spec(Format::Arrow),
            Some("__strata_ord"),
        )
        .expect("a read of the whole snapshot");

        assert_eq!(
            plan.schema()
                .fields()
                .iter()
                .map(|f| f.name().as_str())
                .collect::<Vec<_>>(),
            ["amount", "name"],
        );
        let text = plan.display_indent().to_string();
        assert!(
            text.contains("Sort: ?table?.__strata_ord ASC NULLS LAST"),
            "{text}"
        );
    }

    /// **The window is taken after the sort**, so "this page" is the page the user is looking at
    /// rather than an arbitrary slice re-ordered afterwards — and the user's sort is `NULLS LAST`
    /// in both directions, matching the grid, with the ordinal as the tie-break.
    #[test]
    fn a_page_window_is_taken_after_the_sort() {
        let ctx = test_context(&BTreeMap::new());
        let mut s = spec(Format::Arrow);
        s.sort = Some(("amount".into(), false));
        s.scope = Scope::Page {
            page: 3,
            page_size: 100,
        };
        let text = snapshot_rows(snapshot_frame(&ctx), &s, Some("__strata_ord"))
            .expect("a page read")
            .display_indent()
            .to_string();

        let sort =
            text.find("Sort: ?table?.amount DESC NULLS LAST, ?table?.__strata_ord ASC NULLS LAST");
        let limit = text.find("Limit: skip=200, fetch=100");
        assert!(sort.is_some() && limit.is_some(), "{text}");
        assert!(limit < sort, "the limit sits above the sort: {text}");
    }

    /// A snapshot with no ordinal (an `EXPLAIN`, or duplicate column names) reads unordered and
    /// keeps every column it has — there is nothing to sort by and nothing to hide.
    #[test]
    fn a_snapshot_with_no_ordinal_reads_unordered() {
        let ctx = test_context(&BTreeMap::new());
        let text = snapshot_rows(snapshot_frame(&ctx), &spec(Format::Arrow), None)
            .expect("an unordered read")
            .display_indent()
            .to_string();
        assert!(!text.contains("Sort:"), "{text}");
        assert!(text.contains("__strata_ord"), "{text}");
    }

    #[test]
    fn a_partition_column_that_isnt_one_bare_word_is_refused_before_planning() {
        let d = sql::lex::dialect("generic");
        assert!(is_bare_word(d.as_ref(), "year"));
        assert!(is_bare_word(d.as_ref(), "_2024"));
        assert!(!is_bare_word(d.as_ref(), "order date"));
        assert!(!is_bare_word(d.as_ref(), "2024"));
    }

    /// **And it is the *engine's* dialect that decides.** `region#eu` is one identifier under
    /// `generic` and three tokens under `postgresql`, so a hardcoded dialect here would wave
    /// through a partition column a typed `PARTITIONED BY` could never name.
    #[test]
    fn bare_words_are_judged_by_the_configured_dialect() {
        assert!(is_bare_word(
            sql::lex::dialect("generic").as_ref(),
            "region#eu"
        ));
        assert!(!is_bare_word(
            sql::lex::dialect("postgresql").as_ref(),
            "region#eu"
        ));
    }

    /// **A scheme is a scheme, and a path with `://` in it is a path.** Reading everything before
    /// the first `://` as a scheme waved `…/x://y` through the ownership fence as though it named
    /// an object store, which is how a local target inside `.strata/` could skip the check.
    #[test]
    fn only_a_real_scheme_reads_as_a_url() {
        for yes in ["s3", "gs", "http", "https", "file", "s3a", "x+y", "a-b.c"] {
            assert!(is_url_scheme(yes), "{yes}");
        }
        for no in [
            "",
            "3s",
            "/tmp/a",
            "sales/eu",
            "/proj/.strata/tables/sales/x",
            "a b",
            "a_b",
        ] {
            assert!(!is_url_scheme(no), "{no}");
        }
    }

    /// The ownership fence, over the two shapes that matter: a remote target is not ours to judge,
    /// and a local one carrying `://` is still a local one.
    #[test]
    fn a_local_target_with_a_colon_slash_slash_is_still_fenced() {
        let root = env::temp_dir().join(format!("strata-copy-fence-{}", process::id()));
        let owned = strata_dir(&root).join("tables/sales/x://y");

        refuse_owned_target(&owned.to_string_lossy(), &project_roots(&root), "COPY")
            .expect_err("a local path inside .strata is refused whatever is in its name");
        refuse_owned_target("s3://acme-lake/out.parquet", &project_roots(&root), "COPY")
            .expect("a remote target is not local storage");
        refuse_owned_target(
            &root.join("out.parquet").to_string_lossy(),
            &project_roots(&root.join("elsewhere")),
            "COPY",
        )
        .expect("the user's own file");
    }

    /// **The fence asks the stores; it does not guess.** Where results live is a `SnapshotStore`
    /// and where Strata's own tables live is an `InternalTableStore` — so an engine on a store
    /// rooted somewhere of its own fences *that* root, and one whose store keeps nothing on the
    /// filesystem fences nothing on its account. This used to be the default snapshot store's
    /// shared temp root, named unconditionally: fenced for an engine that never wrote there, and
    /// wide open for one that wrote somewhere else.
    #[test]
    fn the_fence_is_where_the_stores_say_their_bytes_are() {
        let spool = env::temp_dir().join(format!("strata-fence-spool-{}", process::id()));
        let held = env::temp_dir().join(format!("strata-fence-tables-{}", process::id()));
        let owned = owned_roots(None, vec![spool.clone()], vec![held.clone()]);

        let refused = |dir: &Path| {
            refuse_owned_target(&dir.join("out.csv").to_string_lossy(), &owned, "COPY")
                .expect_err("owned storage")
        };
        assert!(refused(&spool).contains("holds query results"));
        assert!(refused(&held).contains("holds tables Strata owns the data of"));

        assert!(
            owned_roots(None, Vec::new(), Vec::new()).is_empty(),
            "a store with nothing on disk fences nothing, and no project fences nothing"
        );
    }

    /// **And the engine is what asks them.** Driven through `Engine` rather than through
    /// `owned_roots` because the claim is that the store an embedder passed is the store the fence
    /// reads — a mem store leaves the machine-shared spool unfenced, and a store rooted elsewhere
    /// fences its own root.
    #[tokio::test]
    async fn an_engines_fence_follows_the_stores_it_was_built_with() {
        let spool = env::temp_dir().join(format!("strata-fence-engine-{}", process::id()));
        let _ = fs::remove_dir_all(&spool);

        let held = Engine::builder()
            .with_snapshot_store(LocalIpcSnapshotStore::new_in(&spool))
            .build();
        assert!(
            held.owned_storage().iter().any(|(dir, _)| dir == &spool),
            "the store's own root is what is fenced"
        );

        let none = Engine::builder()
            .with_snapshot_store(MemSnapshotStore::new())
            .build();
        assert!(
            none.owned_storage().is_empty(),
            "a store with no filesystem storage fences none of it"
        );
        let _ = fs::remove_dir_all(&spool);
    }

    /// **A caller-named destination has to be a new local file, and each refusal says which rule
    /// it broke.** The shape rules are what make the other two answerable: a relative path and a
    /// remote one have no local file to ask "does this already exist" of, and the last three are
    /// the paths DataFusion would not write as one file at all.
    #[test]
    fn a_callers_destination_has_to_name_a_new_local_file() {
        let root = scratch("destination");
        let owned = project_roots(&root);
        let taken = root.join("taken.csv");
        fs::write(&taken, "n\n1\n").unwrap();

        let refused = |path: &str| check_destination(path, &owned).expect_err(path);
        assert!(refused("s3://acme-lake/out.parquet").contains("names a remote location"));
        assert!(refused("file:///tmp/out.csv").contains("names a remote location"));
        assert!(refused("out.csv").contains("is relative"));
        assert!(refused(&format!("{}/", root.display())).contains("names a folder"));
        assert!(refused(&taken.display().to_string()).contains("never overwrites"));
        assert!(
            refused(&root.join("nope/out.csv").display().to_string()).contains("does not exist")
        );

        check_destination(&root.join("fresh.csv").display().to_string(), &owned)
            .expect("a new file beside the project is the caller's own");
        let _ = fs::remove_dir_all(&root);
    }

    /// **The three paths DataFusion writes as something other than the one named file.** Each was
    /// measured against the engine before this rule existed: `…/results` became a *directory* of
    /// part files whose inode size was reported as `bytes`, and `…/report[1].csv` reported success
    /// at a path where no file existed while the rows landed beside it under a generated name.
    ///
    /// A dotfile is deliberately **allowed**: DataFusion counts `.gitignore` as carrying an
    /// extension (its own predicate is "contains a dot and does not end with one"), so refusing it
    /// would decline a path that works.
    #[test]
    fn a_destination_that_would_not_be_one_file_is_refused() {
        let root = scratch("one-file");
        let owned = project_roots(&root);
        let refused = |path: &str| check_destination(path, &owned).expect_err(path);

        assert!(refused(&root.join("results").display().to_string()).contains("no file extension"));
        assert!(refused(&root.join("out.").display().to_string()).contains("no file extension"));
        for globbed in ["report[1].csv", "report?.csv", "report*.csv"] {
            let err = refused(&root.join(globbed).display().to_string());
            assert!(err.contains("filename pattern"), "{globbed}: {err}");
        }
        assert!(refused(&root.join("a*b/out.csv").display().to_string()).contains("pattern"));

        assert!(names_one_file("/tmp/.gitignore"), "a dotfile has one");
        check_destination(&root.join(".gitignore").display().to_string(), &owned)
            .expect("DataFusion reads a dotfile as carrying an extension");
        let _ = fs::remove_dir_all(&root);
    }

    /// **What lands on disk is the result, in result order, with no bookkeeping in it.** The
    /// ordinal column orders the read and is projected away, so a caller's file carries the user's
    /// own columns and the rows in the order the run produced them — not sorted, not re-run.
    #[tokio::test]
    async fn a_callers_export_writes_the_result_in_order_and_never_the_ordinal() {
        let root = scratch("agent-export");
        let eng = Engine::builder().build();
        eng.set_data_dir(&root);
        let snapshot = settled(
            &eng,
            "SELECT * FROM (VALUES (3,'c'),(1,'a'),(2,'b')) AS t(n, s)",
        )
        .await;

        let out = root.join("out.csv");
        let report = eng
            .snapshot(snapshot)
            .export_to(out.display().to_string(), Format::Csv(Csv::default()))
            .await
            .expect("exported");

        assert_eq!(report.path, out.display().to_string());
        assert_eq!(report.rows, 3);
        assert_eq!(report.bytes, Some(fs::metadata(&out).unwrap().len()));
        assert_eq!(fs::read_to_string(&out).unwrap(), "n,s\n3,c\n1,a\n2,b\n");
        let _ = fs::remove_dir_all(&root);
    }

    /// **The two fences a caller-named path needs, driven through the engine.** The owned-storage
    /// one is reached by a path that does *not* share `.strata`'s prefix as text, because the gate
    /// resolves rather than compares; the overwrite one is the genuinely new risk of a path nobody
    /// picked in a dialog, and a refusal leaves the file that is already there exactly as it was.
    #[tokio::test]
    async fn a_callers_export_is_fenced_out_of_owned_storage_and_never_overwrites() {
        let root = scratch("agent-fence");
        let eng = Engine::builder().build();
        eng.set_data_dir(&root);
        let snapshot = settled(&eng, "SELECT 1 AS n").await;
        let export = |path: PathBuf| {
            eng.snapshot(snapshot)
                .export_to(path.display().to_string(), Format::Csv(Csv::default()))
        };

        let sneaky = root.join(".strata/tables/../tables/sales/rows.csv");
        let owned = export(sneaky)
            .await
            .expect_err("inside .strata")
            .to_string();
        assert!(owned.contains("holds this project's own data"), "{owned}");

        let out = root.join("once.csv");
        export(out.clone()).await.expect("the user's own folder");
        let written = fs::read_to_string(&out).unwrap();
        let again = export(out.clone())
            .await
            .expect_err("already there")
            .to_string();
        assert!(again.contains("never overwrites"), "{again}");
        assert_eq!(fs::read_to_string(&out).unwrap(), written);
        let _ = fs::remove_dir_all(&root);
    }

    /// The snapshot one run settled, which is what an export reads.
    async fn settled(eng: &Engine, sql: &str) -> SnapshotId {
        let RunRows { output, .. } = eng
            .ws(WsId(1))
            .query(RunTag(1), sql.into(), 10)
            .await
            .expect("query");
        output.snapshot.expect("a materialized result")
    }

    /// A scratch project folder of our own, per test — the tag is load-bearing because these run
    /// concurrently in one process.
    fn scratch(tag: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("strata_export_{}_{tag}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
