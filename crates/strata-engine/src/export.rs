//! Export one result snapshot to disk via `COPY … TO` (one file, or a Hive
//! directory when partition columns are given).
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
//! **The gates an export answers to live here, and the statements reach them.** Three surfaces
//! write a result to a path — the Export window, a typed `COPY … TO` (`ddl::copy`), and the
//! agent's `export_result` (QE-05) — and none of them may land in storage Strata owns. So
//! [`refuse_owned_target`], [`partition_columns_are_bare_words`] and [`partition_null_refusal`]
//! are all this module's, called by whichever surface reaches them, rather than each having its
//! own copy of a rule the user reads as one.

use std::borrow::Cow;
use std::env;
use std::path::{is_separator, Component, Path, PathBuf};

use datafusion::arrow::array::Array;
use datafusion::arrow::datatypes::Schema;
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::prelude::*;
use datafusion::sql::sqlparser::dialect::Dialect;

use super::query::{snapshot_name, snapshots_root};
use crate::sql;
use strata_core::project::strata_dir;
use strata_model::SnapshotId;

/// Everything one export needs: where it goes, how much of the snapshot, in what order, in
/// what format, and whether it fans out into a Hive tree.
#[derive(Clone, Debug, PartialEq)]
pub struct ExportSpec {
    /// The destination — a file for a flat export, a directory when `partition` has columns.
    pub path: String,
    pub scope: Scope,
    /// `(column name, ascending)` — the grid's active sort, applied over the **whole**
    /// snapshot before any row window. `None` = snapshot order.
    pub sort: Option<(String, bool)>,
    pub format: Format,
    pub partition: Partition,
}

/// What one finished export wrote, for a caller with no window to show it in
/// ([`SnapshotReads::export_to`](crate::SnapshotReads::export_to)).
///
/// Every figure is read rather than derived: `rows` is the count `COPY` itself returns and
/// `bytes` is the written file's own size. `bytes` is optional because the size is read back
/// *after* a write that has already succeeded — a stat that fails is a fact this call could not
/// learn, never a failed export.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportReport {
    pub path: String,
    pub rows: usize,
    pub bytes: Option<u64>,
}

/// How much of the snapshot to write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// Every row.
    All,
    /// One page window, in the grid's own 1-based paging terms.
    Page { page: usize, page_size: usize },
}

/// The output format, each carrying exactly the write options DataFusion honours for it.
#[derive(Clone, Debug, PartialEq)]
pub enum Format {
    Csv(Csv),
    /// Newline-delimited JSON. DataFusion's writer can also emit a JSON array
    /// (`newline_delimited`), but the canvas offers NDJSON only, so the option isn't spelled.
    Json(Json),
    Parquet(Parquet),
    /// Arrow IPC — **no write options exist**, which is why this variant carries nothing.
    Arrow,
}

impl Format {
    /// The `STORED AS` keyword.
    fn stored_as(&self) -> &'static str {
        match self {
            Self::Csv(_) => "CSV",
            Self::Json(_) => "JSON",
            Self::Parquet(_) => "PARQUET",
            Self::Arrow => "ARROW",
        }
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

#[derive(Clone, Debug, PartialEq)]
pub struct Json {
    pub compression: Compression,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Parquet {
    pub compression: Codec,
    pub statistics: Statistics,
    /// Rows per row group — a **row count**, not a byte size.
    pub max_row_group_size: usize,
    pub writer_version: WriterVersion,
    pub dictionary: bool,
}

/// Whole-file compression, for the formats where compression wraps the file rather than
/// encoding columns (CSV / JSON). Levelless: the canvas offers the codec only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compression {
    None,
    Gzip,
    Zstd,
    Bzip2,
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
    Uncompressed,
    Snappy,
    Lz4,
    Gzip(u32),
    Brotli(u32),
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
    None,
    Chunk,
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
    V1,
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

/// Export one snapshot via `COPY (…) TO … STORED AS`. A plain file path (extension)
/// → one file; partition columns → a Hive-partitioned directory.
/// Returns `(path, rows_written)`.
pub async fn run_export(
    ctx: &SessionContext,
    snapshot: SnapshotId,
    spec: ExportSpec,
    stats: &crate::query::SnapshotStats,
) -> Result<(String, usize), String> {
    let snap = snapshot_name(snapshot);
    let Ok(table) = ctx.table(snap.as_str()).await else {
        return Err("No results to export — run a query first".to_string());
    };
    let schema = table.schema().inner().clone();

    let select = select_sql(&snap, &spec, &schema, stats.ord.as_deref());

    partition_columns_are_bare_words(&spec.partition.columns, ctx)?;
    partition_columns_have_no_nulls(&spec.partition.columns, &schema, stats)?;

    let mut options = format_pairs(&spec.format)?;
    let part_clause = if spec.partition.is_flat() {
        String::new()
    } else {
        options.push((
            KEEP_PARTITION_COLUMNS,
            spec.partition.keep_columns.to_string(),
        ));
        format!(" PARTITIONED BY ({})", spec.partition.columns.join(", "))
    };
    let opts = options_clause(&options);

    let esc = quote_literal(&spec.path);
    let stored = spec.format.stored_as();
    let stmt = format!("COPY ({select}) TO '{esc}' STORED AS {stored}{part_clause}{opts}");

    let df = ctx.sql(&stmt).await.map_err(|e| e.to_string())?;
    let batches = df.collect().await.map_err(|e| e.to_string())?;
    Ok((spec.path, copy_row_count(&batches)))
}

/// The `SELECT` the COPY wraps: the result's columns — **explicitly, never `*`** — over the
/// whole snapshot or one page window, in the grid's order.
///
/// Explicit because the snapshot file carries the ordinal column
/// (`docs/SNAPSHOT_SPEC.md` §9), and a `COPY` must not write bookkeeping into the user's
/// file. The ordinal is what the read *orders by* instead: alone for an unsorted export, as
/// the tie-break under a user sort — the same rule as `fetch_page`, which is what makes "the
/// file matches what was on screen" true rather than hopeful (an unordered `LIMIT/OFFSET`
/// over a split scan is nondeterministic, measured in §9).
///
/// The sort goes **before** the window, so "this page" means the page the user is looking
/// at rather than an arbitrary slice re-ordered afterwards. `NULLS LAST` in both directions
/// matches the grid's own ordering (Rz6).
fn select_sql(snap: &str, spec: &ExportSpec, schema: &Schema, ord: Option<&str>) -> String {
    let columns = schema
        .fields()
        .iter()
        .map(|f| f.name().as_str())
        .filter(|name| ord != Some(*name))
        .map(quote_col)
        .collect::<Vec<_>>()
        .join(", ");
    let mut sql = format!("SELECT {columns} FROM {snap}");
    let mut order = Vec::new();
    if let Some((name, asc)) = &spec.sort {
        let dir = if *asc { "ASC" } else { "DESC" };
        order.push(format!("{} {dir} NULLS LAST", quote_col(name)));
    }
    if let Some(ord) = ord {
        order.push(quote_col(ord));
    }
    if !order.is_empty() {
        sql.push_str(&format!(" ORDER BY {}", order.join(", ")));
    }
    if let Scope::Page { page, page_size } = spec.scope {
        let offset = page.saturating_sub(1).saturating_mul(page_size);
        sql.push_str(&format!(" LIMIT {page_size} OFFSET {offset}"));
    }
    sql
}

/// A **result column name** rendered into SQL: double-quoted verbatim, embedded quotes
/// doubled. Deliberately not the crate's `quote_ident`, which folds a bare word to
/// lowercase — right for catalog names (that fold is their registered identity), wrong for
/// a result column, whose name is exactly what the user's query produced. (Replaces the
/// old local escape that the `ORDER BY` used; same rendering, one name.)
///
/// `pub` for the Shape panel (Chart 09), which composes SQL over result columns on exactly
/// these terms.
pub fn quote_col(name: impl AsRef<str>) -> String {
    format!("\"{}\"", name.as_ref().replace('"', "\"\""))
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

/// The `'key' 'value'` pairs a format contributes to ` OPTIONS (…)`.
///
/// Keys are bare and uppercase: DataFusion's COPY planner lowercases them and applies the
/// `format.` prefix itself, so these resolve onto `CsvOptions` / `JsonOptions` /
/// `TableParquetOptions` field names. A key that carries its own namespace
/// ([`KEEP_PARTITION_COLUMNS`]) keeps it — the planner only prefixes a key with no dot in it.
fn format_pairs(format: &Format) -> Result<Vec<(&'static str, String)>, String> {
    let pairs: Vec<(&'static str, String)> = match format {
        Format::Csv(csv) => {
            let mut pairs = vec![
                ("HAS_HEADER", csv.header.to_string()),
                ("DELIMITER", ascii_byte("delimiter", csv.delimiter)?),
                ("QUOTE", ascii_byte("quote character", csv.quote)?),
                ("DOUBLE_QUOTE", csv.double_quote.to_string()),
                ("NULL_VALUE", quote_literal(&csv.null_value)),
                ("COMPRESSION", csv.compression.as_option().into()),
            ];
            if let Some(escape) = csv.escape {
                pairs.push(("ESCAPE", ascii_byte("escape character", escape)?));
            }
            pairs
        }
        Format::Json(json) => vec![("COMPRESSION", json.compression.as_option().into())],
        Format::Parquet(pq) => vec![
            ("COMPRESSION", pq.compression.as_option()),
            ("STATISTICS_ENABLED", pq.statistics.as_option().into()),
            (
                "MAX_ROW_GROUP_SIZE",
                pq.max_row_group_size.max(1).to_string(),
            ),
            ("WRITER_VERSION", pq.writer_version.as_option().into()),
            ("DICTIONARY_ENABLED", pq.dictionary.to_string()),
        ],
        Format::Arrow => vec![],
    };
    Ok(pairs)
}

/// The ` OPTIONS (…)` clause for a set of pairs, or an empty string for none.
fn options_clause(pairs: &[(&str, String)]) -> String {
    if pairs.is_empty() {
        return String::new();
    }
    let body = pairs
        .iter()
        .map(|(key, value)| format!("'{key}' '{value}'"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(" OPTIONS ({body})")
}

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

/// Escape a value for a single-quoted SQL literal. Every option value and the path land
/// inside `'…'`, so an embedded quote would otherwise close the literal early.
fn quote_literal(raw: &str) -> String {
    raw.replace('\'', "''")
}

/// Why a partition column containing NULLs is refused, in the one wording both surfaces use.
///
/// Shared with the typed `COPY` arm (`ddl::copy`), which reaches the same conclusion by a
/// different route — a pre-flight count over the statement's own source, since a typed COPY has
/// no snapshot behind it and therefore none of the write pass's free counts. Two mechanisms, one
/// sentence: the fact the user is told is the same fact, and a second phrasing of it would read
/// like a second rule.
pub(super) fn partition_null_refusal(name: &str) -> String {
    format!(
        "Can't partition by '{name}': it contains NULL values, and a NULL has no folder name — \
         those rows would be written under another value and read back wrong. Partition by a \
         column with no NULLs, or filter them out of the query first"
    )
}

/// Refuse a partitioned export whose partition columns contain NULLs.
///
/// **Why this is a hard block and not a warning.** A directory name cannot hold a NULL, and
/// DataFusion 54 does not use the Hive convention (`__HIVE_DEFAULT_PARTITION__`) for one: it
/// files the row under a *neighbouring* value's directory instead, so it reads back claiming a
/// value it never had. That is silent data corruption, in the user's own output, discoverable
/// only by comparing against the source — so the export declines rather than warns.
///
/// **Answered from what the write pass counted, not by scanning and not from a footer.** The
/// snapshot is Arrow IPC, which carries no column statistics at all — but nothing was ever gained
/// by asking the file. `query::materialize` streams every batch to write it, and
/// `Array::null_count` is a stored field, so the exact per-column count is a running sum over
/// data already in hand ([`query::SnapshotStats`], held for the snapshot's lifetime in
/// `Lifecycle`). Free to produce, and a slice index to read.
///
/// The rule is "proceed only on an exact zero". `stats` is exact by construction — it counted
/// every row that was written — so there is no "unknown" reading to disambiguate, which the
/// footer route did have.
fn partition_columns_have_no_nulls(
    columns: &[String],
    schema: &Schema,
    stats: &crate::query::SnapshotStats,
) -> Result<(), String> {
    for name in columns {
        let index = schema
            .fields()
            .iter()
            .position(|f| f.name() == name)
            .ok_or_else(|| format!("Can't partition by '{name}': the result has no such column"))?;
        if stats.nulls.get(index).copied() != Some(0) {
            return Err(partition_null_refusal(name));
        }
    }
    Ok(())
}

/// Refuse any partition column the engine's own parser dialect doesn't read as a single
/// bare word.
///
/// `PARTITIONED BY` takes **bare** identifiers, and quoting is not an option: the COPY parser
/// re-renders each with `Ident::to_string()`, so a quoted name reaches the planner with its quotes
/// attached and matches no field. Bare is case-preserving here, so every name the tokenizer reads
/// as one word round-trips and one it does not simply cannot be expressed — worth saying plainly
/// rather than emitting a statement that fails on a stray token.
///
/// Its own sync function rather than an inline check, because the resolved dialect is not `Send`
/// and [`run_export`] is spawned onto the engine runtime.
///
/// **Shared with the two typed statements that carry a `PARTITIONED BY`** — `ddl::copy`, which
/// asks it of the very strings `CopyToStatement::partitioned_by` holds, and `ddl::external`,
/// whose `CreateExternalTable::table_partition_cols` are built the same way. Both are
/// `Ident::to_string()`'s output, so a quoted `PARTITIONED BY ("order date")` arrives here *with
/// its quotes* — which for a COPY is a name that matches no field, and for a registration is a
/// partition column whose stored name can never equal a `key=` folder segment. One clause, one
/// rule, so the wording names **`PARTITIONED BY`** rather than either statement. The bad name is
/// rendered inside single quotes rather than by `Debug` so that case reads as what the user typed
/// instead of as escaped Rust.
pub(super) fn partition_columns_are_bare_words(
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

/// Refuse a write whose target lands in storage Strata owns — the project's `.strata/` directory
/// (internal table data, the session, the conversations) or the snapshot spool.
///
/// **The two fenced roots are the two places a stray file changes what Strata later reads.** A
/// file under `.strata/tables/<slug>/` is listed by that table's next scan; one under the snapshot
/// spool is read back as a result. Everywhere else on the disk is the user's own, and a write that
/// overwrites their file is the statement doing what it says.
///
/// **Resolved, never compared as text.** A relative target is the process's cwd away from an
/// absolute one, and `'.strata/../.strata/tables'` names the fenced directory without sharing its
/// prefix. The target need not exist yet, so `canonicalize` cannot be asked about it directly: the
/// path is made absolute, its `.` and `..` segments are folded away, and both sides are then
/// anchored on the deepest ancestor that *does* exist — which is what makes a symlinked project
/// folder compare equal to the path the fence was built from.
///
/// `subject` is what the sentence is about, because two surfaces reach this and the user reads a
/// refusal as being about the thing they did: `COPY` for the typed statement (`ddl::copy`),
/// `Export` for [`check_destination`]'s caller. Only the subject differs — the rule, the roots and
/// the reason are one copy.
pub(super) fn refuse_owned_target(
    target: &str,
    root: Option<&Path>,
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

    let mut fenced = vec![(PathBuf::from(snapshots_root()), "holds query results")];
    if let Some(root) = root {
        fenced.push((strata_dir(root), "holds this project's own data"));
    }
    for (dir, what) in fenced {
        if path.starts_with(resolve(&dir)) {
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
pub(super) fn check_destination(path: &str, root: Option<&Path>) -> Result<(), String> {
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
    refuse_owned_target(path, root, "Export")?;

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

/// `COPY … TO` returns a single `UInt64` "count" column with the rows written.
///
/// Shared with `ddl::tables`, whose CTAS spool is a `COPY` too: the row count in its report and
/// the one in an export's are the same fact read out of the same shape.
pub(super) fn copy_row_count(batches: &[RecordBatch]) -> usize {
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

    use datafusion::arrow::datatypes::{DataType, Field};

    use crate::{Engine, RunTag, WsId};

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

    /// The ` OPTIONS (…)` a format alone contributes — what [`run_export`] then appends the
    /// partition option to.
    fn format_options(format: &Format) -> Result<String, String> {
        Ok(options_clause(&format_pairs(format)?))
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

    #[test]
    fn scope_all_reads_the_whole_snapshot_in_snapshot_order() {
        assert_eq!(
            select_sql(
                "__snap_1",
                &spec(Format::Arrow),
                &result_schema(),
                Some("__strata_ord")
            ),
            "SELECT \"amount\", \"name\" FROM __snap_1 ORDER BY \"__strata_ord\""
        );
    }

    #[test]
    fn a_page_window_is_taken_after_the_sort() {
        let mut s = spec(Format::Arrow);
        s.sort = Some(("amount".into(), false));
        s.scope = Scope::Page {
            page: 3,
            page_size: 100,
        };
        assert_eq!(
            select_sql("__snap_7", &s, &result_schema(), Some("__strata_ord")),
            "SELECT \"amount\", \"name\" FROM __snap_7 ORDER BY \"amount\" DESC NULLS LAST, \
             \"__strata_ord\" LIMIT 100 OFFSET 200"
        );
    }

    #[test]
    fn a_quote_in_a_sorted_column_name_cant_break_out_of_the_identifier() {
        let mut s = spec(Format::Arrow);
        s.sort = Some((r#"we"ird"#.into(), true));
        assert!(select_sql("__snap_1", &s, &result_schema(), None)
            .contains(r#"ORDER BY "we""ird" ASC"#));
    }

    /// The snapshot table's schema as `run_export` sees it: the user's columns plus the
    /// ordinal (a `UInt64`, the type the spool writer numbers with), which the SELECT must
    /// exclude.
    fn result_schema() -> Schema {
        Schema::new(vec![
            Field::new("amount", DataType::Int64, true),
            Field::new("name", DataType::Utf8, true),
            Field::new("__strata_ord", DataType::UInt64, false),
        ])
    }

    #[test]
    fn csv_single_char_options_are_sent_as_byte_values() {
        let opts = format_options(&Format::Csv(csv())).expect("csv options");
        assert!(opts.contains("'DELIMITER' '44'"), "{opts}");
        assert!(opts.contains("'QUOTE' '34'"), "{opts}");
        assert!(!opts.contains("ESCAPE"), "{opts}");
    }

    #[test]
    fn a_tab_delimiter_survives_as_a_byte() {
        let opts = format_options(&Format::Csv(Csv {
            delimiter: '\t',
            ..csv()
        }))
        .expect("csv options");
        assert!(opts.contains("'DELIMITER' '9'"), "{opts}");
    }

    #[test]
    fn a_non_ascii_delimiter_is_refused_in_our_own_words() {
        let err = format_options(&Format::Csv(Csv {
            delimiter: '£',
            ..csv()
        }))
        .expect_err("non-ASCII delimiter");
        assert!(err.contains("single ASCII character"), "{err}");
    }

    #[test]
    fn a_quote_in_the_null_text_cant_close_the_literal() {
        let opts = format_options(&Format::Csv(Csv {
            null_value: "it's null".into(),
            ..csv()
        }))
        .expect("csv options");
        assert!(opts.contains("'NULL_VALUE' 'it''s null'"), "{opts}");
    }

    #[test]
    fn a_parquet_level_rides_inside_the_codec_string() {
        let opts = format_options(&Format::Parquet(Parquet {
            compression: Codec::Zstd(9),
            statistics: Statistics::Page,
            max_row_group_size: 1_048_576,
            writer_version: WriterVersion::V1,
            dictionary: true,
        }))
        .expect("parquet options");
        assert!(opts.contains("'COMPRESSION' 'zstd(9)'"), "{opts}");
        assert!(opts.contains("'MAX_ROW_GROUP_SIZE' '1048576'"), "{opts}");
        assert!(opts.contains("'WRITER_VERSION' '1.0'"), "{opts}");
    }

    #[test]
    fn a_levelless_codec_carries_no_parens() {
        assert_eq!(Codec::Snappy.as_option(), "snappy");
        assert_eq!(Codec::Uncompressed.as_option(), "uncompressed");
    }

    #[test]
    fn arrow_writes_no_options_clause_at_all() {
        assert_eq!(format_options(&Format::Arrow).expect("arrow"), "");
    }

    /// **Keep-columns rides in the statement, never in the session.** It is the one option that
    /// is not a format option, and it keeps its `execution.` namespace so the COPY planner reads
    /// it and `TableOptions::set` skips it. The `SET` this replaced was global and unrestored, so
    /// one partitioned export decided the answer for every later one.
    #[test]
    fn keeping_partition_columns_is_a_copy_option_in_its_own_namespace() {
        let mut pairs = format_pairs(&Format::Arrow).expect("arrow");
        pairs.push((KEEP_PARTITION_COLUMNS, true.to_string()));
        assert_eq!(
            options_clause(&pairs),
            " OPTIONS ('execution.keep_partition_by_columns' 'true')"
        );
    }

    #[test]
    fn compression_names_the_suffix_it_adds_to_the_destination() {
        assert_eq!(Compression::None.extension(), "");
        assert_eq!(Compression::Gzip.extension(), ".gz");
        assert_eq!(Compression::Zstd.extension(), ".zst");
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
    /// `generic` and three tokens under `postgresql`, so a hardcoded dialect here would emit a
    /// `PARTITIONED BY` the planner rejects with the very parser message this check replaces.
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

        refuse_owned_target(&owned.to_string_lossy(), Some(&root), "COPY")
            .expect_err("a local path inside .strata is refused whatever is in its name");
        refuse_owned_target("s3://acme-lake/out.parquet", Some(&root), "COPY")
            .expect("a remote target is not local storage");
        refuse_owned_target(
            &root.join("out.parquet").to_string_lossy(),
            Some(&root.join("elsewhere")),
            "COPY",
        )
        .expect("the user's own file");
    }

    /// **A caller-named destination has to be a new local file, and each refusal says which rule
    /// it broke.** The shape rules are what make the other two answerable: a relative path and a
    /// remote one have no local file to ask "does this already exist" of, and the last three are
    /// the paths DataFusion would not write as one file at all.
    #[test]
    fn a_callers_destination_has_to_name_a_new_local_file() {
        let root = scratch("destination");
        let taken = root.join("taken.csv");
        fs::write(&taken, "n\n1\n").unwrap();

        let refused = |path: &str| check_destination(path, Some(&root)).expect_err(path);
        assert!(refused("s3://acme-lake/out.parquet").contains("names a remote location"));
        assert!(refused("file:///tmp/out.csv").contains("names a remote location"));
        assert!(refused("out.csv").contains("is relative"));
        assert!(refused(&format!("{}/", root.display())).contains("names a folder"));
        assert!(refused(&taken.display().to_string()).contains("never overwrites"));
        assert!(
            refused(&root.join("nope/out.csv").display().to_string()).contains("does not exist")
        );

        check_destination(&root.join("fresh.csv").display().to_string(), Some(&root))
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
        let refused = |path: &str| check_destination(path, Some(&root)).expect_err(path);

        assert!(refused(&root.join("results").display().to_string()).contains("no file extension"));
        assert!(refused(&root.join("out.").display().to_string()).contains("no file extension"));
        for globbed in ["report[1].csv", "report?.csv", "report*.csv"] {
            let err = refused(&root.join(globbed).display().to_string());
            assert!(err.contains("filename pattern"), "{globbed}: {err}");
        }
        assert!(refused(&root.join("a*b/out.csv").display().to_string()).contains("pattern"));

        assert!(names_one_file("/tmp/.gitignore"), "a dotfile has one");
        check_destination(&root.join(".gitignore").display().to_string(), Some(&root))
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
        let owned = export(sneaky).await.expect_err("inside .strata");
        assert!(owned.contains("holds this project's own data"), "{owned}");

        let out = root.join("once.csv");
        export(out.clone()).await.expect("the user's own folder");
        let written = fs::read_to_string(&out).unwrap();
        let again = export(out.clone()).await.expect_err("already there");
        assert!(again.contains("never overwrites"), "{again}");
        assert_eq!(fs::read_to_string(&out).unwrap(), written);
        let _ = fs::remove_dir_all(&root);
    }

    /// The snapshot one run settled, which is what an export reads.
    async fn settled(eng: &Engine, sql: &str) -> SnapshotId {
        let (output, _) = eng
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
