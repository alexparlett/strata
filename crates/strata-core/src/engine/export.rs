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

use datafusion::arrow::array::Array;
use datafusion::arrow::datatypes::Schema;
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::prelude::*;
use datafusion::sql::sqlparser::dialect::Dialect;

use super::query::snapshot_name;
use crate::engine::sql;
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
    stats: &crate::engine::query::SnapshotStats,
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
        let offset = page.saturating_sub(1) * page_size;
        sql.push_str(&format!(" LIMIT {page_size} OFFSET {offset}"));
    }
    sql
}

/// A **result column name** rendered into SQL: double-quoted verbatim, embedded quotes
/// doubled. Deliberately not the crate's `quote_ident`, which folds a bare word to
/// lowercase — right for catalog names (that fold is their registered identity), wrong for
/// a result column, whose name is exactly what the user's query produced. (Replaces the
/// old local escape that the `ORDER BY` used; same rendering, one name.)
fn quote_col(name: impl AsRef<str>) -> String {
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
    schema: &datafusion::arrow::datatypes::Schema,
    stats: &crate::engine::query::SnapshotStats,
) -> Result<(), String> {
    for name in columns {
        let index = schema
            .fields()
            .iter()
            .position(|f| f.name() == name)
            .ok_or_else(|| format!("Can't partition by '{name}': the result has no such column"))?;
        // A missing entry is not "zero nulls" — it means the count is unavailable, which under
        // the exact-zero rule is a reason to decline just as a positive count is.
        if stats.nulls.get(index).copied() != Some(0) {
            return Err(partition_null_refusal(name));
        }
    }
    Ok(())
}

/// Refuse any partition column the engine's own parser dialect doesn't read as a single
/// bare word.
///
/// `PARTITIONED BY` takes **bare** identifiers, and quoting is not an option: DataFusion 54's
/// COPY parser re-renders each one with `Ident::to_string()`, so a quoted name reaches the
/// planner with its quotes still attached and matches no field. Bare is also case-preserving
/// here (that parser doesn't normalise), so every name the tokenizer reads as one word
/// round-trips — and one it doesn't simply can't be expressed, which is worth saying plainly
/// instead of emitting a statement that fails with a parser message about a stray token.
///
/// Its own (sync) function rather than an inline check, because the resolved dialect is not
/// `Send` and [`run_export`] is spawned onto the engine runtime — a `Box<dyn Dialect>` held
/// across one of its awaits would not compile.
///
/// **Shared with the typed `COPY` arm** (`ddl::copy`), which asks it of the very strings
/// `CopyToStatement::partitioned_by` holds — those are `Ident::to_string()`'s output, so a
/// quoted `PARTITIONED BY ("order date")` arrives here *with its quotes*, which is exactly the
/// name that would then match no field. The bad name is rendered inside single quotes rather
/// than by `Debug` so that case reads as what the user typed instead of as escaped Rust.
pub(super) fn partition_columns_are_bare_words(
    columns: &[String],
    ctx: &SessionContext,
) -> Result<(), String> {
    let dialect = sql::lex::dialect(ctx.state().config_options().sql_parser.dialect.as_ref());
    match columns.iter().find(|c| !is_bare_word(dialect.as_ref(), c)) {
        Some(bad) => Err(format!(
            "Can't partition by '{bad}': COPY takes unquoted column names, so a partition \
             column has to be a single plain word"
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
    use datafusion::arrow::datatypes::{DataType, Field};

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
        // The snapshot file carries the ordinal; the SELECT names the user's columns
        // explicitly and orders by the ordinal, so the file matches the grid without ever
        // containing the bookkeeping.
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
    /// ordinal (a `UInt64` — `row_number()`'s output type), which the SELECT must exclude.
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
        // ',' is 44 and '"' is 34 — never the characters themselves, so a digit delimiter
        // can't be read as a control byte.
        assert!(opts.contains("'DELIMITER' '44'"), "{opts}");
        assert!(opts.contains("'QUOTE' '34'"), "{opts}");
        // No escape was chosen, so the key is absent rather than sent empty.
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
}
