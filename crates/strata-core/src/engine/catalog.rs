//! Catalog side of the engine: registering external tables, reading their free
//! (footer) statistics, view-dependency extraction (D10), and full-scan profiling (D4).

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use datafusion::arrow::datatypes::{DataType, Field};
use datafusion::common::stats::Precision;
use datafusion::common::ScalarValue;
use datafusion::prelude::*;

use strata_model::{
    ColumnInfo, CsvRead, FileCompression, JsonRead, JsonShape, Kind, SourceFormat, Stat, StatKey,
};

use crate::profile::{aggregates, decode, profile_sql, CatalogProfile};

/// What a (re)registration learned about a table: its columns, plus the free row count
/// (`None` when the source doesn't report one).
#[derive(Clone, Debug, PartialEq)]
pub struct TableMeta {
    pub columns: Vec<ColumnInfo>,
    pub rows: Option<u64>,
}

/// Everything needed to register one external table: its name, source paths, the reader and
/// its options, and Hive partition columns.
#[derive(Clone, Debug)]
pub struct TableSpec {
    pub name: String,
    pub paths: Vec<String>,
    /// The reader *and* the options it takes — see [`SourceFormat`]. One field, so a CSV
    /// delimiter cannot be named on a parquet table.
    pub format: SourceFormat,
    pub partitions: Vec<(String, String)>,
}

/// What creating a view learned: its columns and what it reads (D10). `tables` /
/// `aliases` come straight from [`PlanDeps`] — `aliases` is raw (view inlines mixed
/// with table-alias / CTE noise); the caller keeps only the names that are actually
/// views.
#[derive(Clone, Debug, PartialEq)]
pub struct ViewMeta {
    pub columns: Vec<ColumnInfo>,
    /// Base tables the view scans.
    pub tables: Vec<String>,
    /// Every `SubqueryAlias` name in its plan (see [`PlanDeps::aliases`]).
    pub aliases: Vec<String>,
}

// ---- external table registration ----

/// Register (or **re**-register) one external table from its spec, returning its
/// inferred schema + free metadata.
///
/// This is also the catalog **re-scan** step (D5 / P3-03): it deregisters whatever is
/// registered under `spec.name` and builds a *fresh* `ListingTable` from a
/// re-`infer_schema`d config, because re-registering the same provider wouldn't re-infer
/// anything. The spec is the source of truth on every pass — paths, format and partition
/// columns come from the project's def, so a re-scan also picks up a def that changed and
/// retries a table whose first registration failed.
///
/// Only the *inferred schema* is frozen at registration; file sets, row counts and
/// partition values are already live, since we run no `ListFilesCache` and DataFusion
/// re-`LIST`s per scan.
pub async fn register_external(
    ctx: &SessionContext,
    spec: &TableSpec,
) -> Result<TableMeta, String> {
    use datafusion::datasource::file_format::arrow::ArrowFormat;
    use datafusion::datasource::file_format::parquet::ParquetFormat;
    use datafusion::datasource::file_format::FileFormat;
    use datafusion::datasource::listing::{ListingOptions, ListingTable, ListingTableConfig};

    let _ = ctx.deregister_table(spec.name.as_str());

    let mut urls = Vec::new();
    for p in source_paths(spec) {
        urls.push(listing_url(p)?);
    }
    if urls.is_empty() {
        return Err("No source paths".into());
    }

    // The reader, dressed in the def's own options — and **exhaustive**: a format with no
    // reader in this build fails its registration by name. The arm this replaces was a
    // `_ =>` fallthrough onto parquet, so a legacy `avro` def was read as parquet and said
    // nothing about it.
    let fmt: Arc<dyn FileFormat> = match &spec.format {
        SourceFormat::Csv(o) => Arc::new(csv_format(o)?),
        SourceFormat::Json(o) => Arc::new(json_format(o)),
        SourceFormat::Arrow => Arc::new(ArrowFormat),
        SourceFormat::Parquet => Arc::new(ParquetFormat::default().with_skip_metadata(true)),
        SourceFormat::Unknown(name) => {
            return Err(format!(
                "Table '{}' is defined as '{name}', which Strata cannot read.",
                spec.name
            ))
        }
    };
    // The listing's filter is the format's extension **plus its compression suffix**: a
    // gzipped CSV is `events.csv.gz`, and a filter of `.csv` matches none of them — which
    // DataFusion reports as an empty location, with the files sitting right there.
    let ext = spec.format.extension();
    let ext = ext.as_str();
    // `with_session_config_options` *before* any explicit option: it carries the
    // session's `collect_statistics` (and `target_partitions`) onto the options and
    // would otherwise clobber them.
    //
    // It is not optional. `ListingOptions::new` hardcodes `collect_stat: false`, and a
    // hand-built `ListingTable` never picks the `datafusion.execution.collect_statistics`
    // key up on its own — `ListingTableConfig::with_listing_options` does no such wiring.
    // Without this, every footer statistic comes back `Absent` while the engine setting
    // claims to be on. It's baked in at `try_new`, so a registered table can't be fixed
    // after the fact — `rebuild_listing` inherits it by cloning `lt.options()`.
    let mut opts = ListingOptions::new(fmt)
        .with_session_config_options(&ctx.copied_config())
        .with_file_extension(ext);
    if !spec.partitions.is_empty() {
        let cols = spec
            .partitions
            .iter()
            .map(|(n, ty)| (n.clone(), parse_dtype(ty)))
            .collect();
        opts = opts.with_table_partition_cols(cols);
    }

    let config = ListingTableConfig::new_with_multi_paths(urls)
        .with_listing_options(opts)
        .infer_schema(&ctx.state())
        .await
        .map_err(|e| register_error(spec, ext, &e.to_string()))?;
    let table =
        ListingTable::try_new(config).map_err(|e| register_error(spec, ext, &e.to_string()))?;
    ctx.register_table(spec.name.as_str(), Arc::new(table))
        .map_err(|e| register_error(spec, ext, &e.to_string()))?;

    table_meta(ctx, spec.name.as_str()).await
}

// ---- Hive partition detection (P4-11) ----

/// How many levels down the listing below will look.
///
/// Not a safety valve for a runaway walk: each level is a **network round trip** against
/// whatever store the table lives on, so this is the cost ceiling. Real Hive layouts are
/// `year=/month=/day=/hour=` at their deepest, so four is the practical maximum and this is
/// comfortably past it while still bounding a pathological tree (or a symlink cycle on a local
/// disk) to a fixed number of requests.
const MAX_PARTITION_DEPTH: usize = 8;

/// The Hive partition **keys** under `paths`, outermost first — `["year", "month"]` for a lake
/// laid out as `…/year=2024/month=03/*.parquet`.
///
/// The Configure window's Hive section says it "found `key=value` folders in the source paths",
/// so it has to have looked. Two ways a path can say so:
///
/// - the path **globs** the keys itself (`/data/year=*/month=*/`), in which case the pattern is
///   the answer and nothing needs listing at all;
/// - otherwise it is **listed**, one level at a time, following the first `key=value` prefix at
///   each level for as long as they keep appearing.
///
/// The listing goes through the session's **object store**, not `std::fs`. That is the whole
/// reason this lives in the engine: a source is a `ListingTableUrl`, and the store behind it is
/// a local disk today and an S3 or GCS bucket once connections land (W7). `list_with_delimiter`
/// is the same call for both, and `common_prefixes` is what "a directory" means to a store that
/// has no directories. A `std::fs::read_dir` walk would have had to be rewritten from scratch
/// for the first remote table.
///
/// Empty when neither finds anything — which is what keeps the section off a table that isn't
/// partitioned, rather than offering a toggle over an empty list.
pub async fn detect_partitions(ctx: &SessionContext, paths: &[String]) -> Vec<String> {
    for path in paths.iter().filter(|p| !p.trim().is_empty()) {
        let named = keys_in_pattern(path);
        if !named.is_empty() {
            return named;
        }
        let listed = keys_in_store(ctx, path.trim()).await;
        if !listed.is_empty() {
            return listed;
        }
    }
    Vec::new()
}

/// The `key=` segments the path itself spells out **as globs**, in order.
///
/// A *literal* `key=value` segment is deliberately not one of them, and the distinction is not
/// cosmetic: a source path is the listing **root**, so a literal `…/year=2024/` means the
/// `year=` level is already consumed and only what is *below* it is still partitioned.
/// Declaring `year` there produces a table that registers with a plausible schema and then
/// returns **zero rows for every query** — DataFusion's `parse_partitions_for_path` needs the
/// relative segment's key to equal the column name, the relative segment is `month=03`, and so
/// every file is filtered out silently. A glob (`year=*`) does not consume the level and is the
/// only form that genuinely declares a column here.
fn keys_in_pattern(path: &str) -> Vec<String> {
    path.split(['/', '\\'])
        .filter(|seg| seg.contains('*') || seg.contains('?'))
        .filter_map(partition_key)
        .map(str::to_string)
        .collect()
}

/// The `key=` levels found by listing down from `path`, following the first partition-shaped
/// prefix at each level. Silent on any store error: a path that cannot be listed is a path with
/// no partitions to offer, and the *register* is where an unreadable source is reported.
async fn keys_in_store(ctx: &SessionContext, path: &str) -> Vec<String> {
    let Ok(url) = listing_url(path) else {
        return Vec::new();
    };
    let Ok(store) = ctx.runtime_env().object_store(&url) else {
        return Vec::new();
    };

    let mut keys = Vec::new();
    let mut prefix = url.prefix().clone();
    for _ in 0..MAX_PARTITION_DEPTH {
        let Ok(listed) = store.list_with_delimiter(Some(&prefix)).await else {
            break;
        };
        // Sorted, so the level's key is the same on every run: a store's listing order is its
        // own, and a level holding one stray non-partition prefix would otherwise answer
        // differently each time.
        let mut prefixes = listed.common_prefixes;
        prefixes.sort();
        let found = prefixes.into_iter().find_map(|p| {
            // The key is read out before `p` moves into the pair — a `Path`'s parts borrow it.
            let key = partition_key(p.parts().next_back()?.as_ref())?.to_string();
            Some((key, p))
        });
        let Some((key, next)) = found else {
            break;
        };
        keys.push(key);
        prefix = next;
    }
    keys
}

/// The key in a `key=value` path segment — `None` for anything else. The key has to be an
/// identifier: it becomes a column name.
fn partition_key(segment: &str) -> Option<&str> {
    let (key, _) = segment.split_once('=')?;
    let mut chars = key.chars();
    let first = chars.next()?;
    (first.is_ascii_alphabetic() || first == '_')
        .then_some(key)
        .filter(|_| chars.all(|c| c.is_ascii_alphanumeric() || c == '_'))
}

/// One CSV option that has to be a **byte**, because that is what DataFusion's reader takes.
///
/// Reported rather than truncated: a delimiter the user typed as `→` is not `\xe2`, and a
/// reader configured with the first byte of a multi-byte character splits fields in the middle
/// of the next one.
fn ascii_byte(what: &str, c: char) -> Result<u8, String> {
    // **ASCII, not "fits in a u8".** `'é' as u32` is 0xE9, which converts to a byte quite
    // happily — but 'é' is *encoded* as 0xC3 0xA9, so that byte appears nowhere in the file and
    // is itself a UTF-8 lead byte, so the reader would split records mid-character. Only a
    // character whose UTF-8 encoding is one byte can be one of these options.
    c.is_ascii()
        .then_some(c as u8)
        .ok_or_else(|| format!("The CSV {what} has to be a single-byte character, not '{c}'."))
}

/// Dress a `CsvFormat` in the def's options.
///
/// Every option set here reaches **both** halves of the read — `infer_schema` and the scan. The
/// ones DataFusion only wires into one of the two are deliberately absent; [`CsvRead`] records
/// which and why.
fn csv_format(o: &CsvRead) -> Result<datafusion::datasource::file_format::csv::CsvFormat, String> {
    use datafusion::datasource::file_format::csv::CsvFormat;

    let mut fmt = CsvFormat::default()
        .with_has_header(o.header)
        .with_delimiter(ascii_byte("delimiter", o.delimiter)?)
        .with_quote(ascii_byte("quote character", o.quote)?)
        .with_newlines_in_values(o.newlines_in_values)
        .with_truncated_rows(o.truncated_rows)
        .with_file_compression_type(compression(o.compression));
    if let Some(escape) = o.escape {
        fmt = fmt.with_escape(Some(ascii_byte("escape character", escape)?));
    }
    if let Some(comment) = o.comment {
        fmt = fmt.with_comment(Some(ascii_byte("comment character", comment)?));
    }
    if let Some(rows) = o.infer_rows {
        fmt = fmt.with_schema_infer_max_rec(rows);
    }
    Ok(fmt)
}

/// Dress a `JsonFormat` in the def's options.
fn json_format(o: &JsonRead) -> datafusion::datasource::file_format::json::JsonFormat {
    use datafusion::datasource::file_format::json::JsonFormat;

    let mut fmt = JsonFormat::default()
        .with_newline_delimited(o.shape == JsonShape::NewlineDelimited)
        .with_file_compression_type(compression(o.compression));
    if let Some(rows) = o.infer_rows {
        fmt = fmt.with_schema_infer_max_rec(rows);
    }
    fmt
}

/// Our compression vocabulary as DataFusion's.
fn compression(
    c: FileCompression,
) -> datafusion::datasource::file_format::file_compression_type::FileCompressionType {
    use datafusion::datasource::file_format::file_compression_type::FileCompressionType as F;
    match c {
        FileCompression::None => F::UNCOMPRESSED,
        FileCompression::Gzip => F::GZIP,
        FileCompression::Bzip2 => F::BZIP2,
        FileCompression::Xz => F::XZ,
        FileCompression::Zstd => F::ZSTD,
    }
}

/// The spec's non-blank source paths.
fn source_paths(spec: &TableSpec) -> impl Iterator<Item = &str> {
    spec.paths
        .iter()
        .map(String::as_str)
        .filter(|p| !p.trim().is_empty())
}

/// One source path as DataFusion sees it. A directory has to end in `/` or
/// `ListingTableUrl` reads it as a single file — so the same normalization has to happen
/// wherever a path is turned into a URL, which is why registration and the failure
/// messages both come through here rather than each rolling their own.
fn listing_url(p: &str) -> Result<datafusion::datasource::listing::ListingTableUrl, String> {
    use datafusion::datasource::listing::ListingTableUrl;
    let mut loc = p.to_string();
    if Path::new(&loc).is_dir() && !loc.ends_with('/') {
        loc.push('/');
    }
    ListingTableUrl::parse(&loc).map_err(|e| e.to_string())
}

// ---- registration failure messages (P3-07) ----

/// The longest engine text we'll pass through. A failure reaches the user as a catalog
/// row's tooltip *and* its a11y label, and DataFusion is willing to interpolate an entire
/// parsed document into an error (the JSON array case below did exactly that), so an
/// unrecognised message is capped rather than trusted.
const MAX_PASSTHROUGH: usize = 240;

/// Whether a source path is a glob rather than a name that can be looked up on disk.
fn is_glob(p: &str) -> bool {
    p.contains('*') || p.contains('?') || p.contains('[')
}

/// Translate a registration failure into something the user can act on.
///
/// Only failures we actually recognise are rewritten; anything else passes through as
/// DataFusion wrote it (capped — see [`MAX_PASSTHROUGH`]). Translating an unfamiliar error
/// would mean guessing at its cause, and a confident wrong diagnosis is worse than a raw
/// one the user can search for.
fn register_error(spec: &TableSpec, ext: &str, raw: &str) -> String {
    if let Some(m) = json_shape_error(spec, raw) {
        return m;
    }
    if let Some(m) = no_files_error(spec, ext, raw) {
        return m;
    }
    if raw.chars().count() > MAX_PASSTHROUGH {
        let cut: String = raw.chars().take(MAX_PASSTHROUGH).collect();
        return format!("{cut}…");
    }
    raw.to_string()
}

/// A JSON source is read in one of two **shapes** (`JsonShape`), and reading it in the wrong
/// one fails with wording that says neither what is wrong nor that it is a shape problem at
/// all: Arrow's `Not valid JSON: EOF while parsing an object at line 1 column 1` for a file
/// that usually *is* valid JSON.
///
/// The advice names the setting rather than stating a rule. It used to say "JSON sources must
/// be newline-delimited, one record per line", which was true of the reader as it was
/// configured and false of the reader as a whole — DataFusion 54 reads a whole-document array
/// perfectly well, so the fix is a shape to change, not a file to rewrite.
///
/// A genuine syntax error lands here too and is **not** rewritten into a shape complaint —
/// it keeps Arrow's diagnosis, which points at the offending line and column. The two are
/// told apart by Arrow running out of input mid-record (a record that doesn't end on its
/// line) versus rejecting what it read.
fn json_shape_error(spec: &TableSpec, raw: &str) -> Option<String> {
    // Only a JSON table can have a JSON read problem. Without this, any failure whose text
    // merely mentions `Json error:` would be rewritten into a confident "Cannot read … as
    // JSON" for a table that isn't JSON — the mis-attribution this mapping exists to remove.
    let SourceFormat::Json(options) = &spec.format else {
        return None;
    };
    let detail = raw.split("Json error: ").nth(1)?;
    let name = &spec.name;
    // Only worth suggesting while the def is still in the other shape. Told to read an array
    // and still failing, the shape is not the problem, and repeating the advice would send the
    // user to a setting that already says what it should.
    let fix = match options.shape {
        JsonShape::NewlineDelimited => {
            " Set the JSON shape to array in Table Config, or use newline-delimited JSON."
        }
        JsonShape::Array => "",
    };

    if let Some(found) = detail.strip_prefix("Expected JSON record to be an object, found ") {
        // Never the value itself — this is the arm whose text carried the whole parsed
        // document. Only the type word, which ends at the first space or bracket.
        let kind = found
            .split([' ', '[', '{', '('])
            .next()
            .unwrap_or("value")
            .trim();
        return Some(if kind == "Array" {
            format!("Cannot read '{name}' as JSON: the source is a JSON array.{fix}")
        } else {
            format!("Cannot read '{name}' as JSON: a top-level {kind} is not a record.{fix}")
        });
    }

    let syntax = detail
        .strip_prefix("Not valid JSON: ")
        .unwrap_or(detail)
        .trim();
    if syntax.starts_with("EOF while parsing") {
        return Some(format!(
            "Cannot read '{name}' as JSON: a record does not end on its line.{fix}"
        ));
    }
    Some(format!(
        "Cannot read '{name}' as JSON: {}",
        syntax.trim_end_matches('.')
    ))
}

/// DataFusion answers "no files found" identically for every reason a listing can come
/// back empty — a path that isn't there, a file whose extension doesn't match the table's
/// format, a directory holding nothing readable, and a partitioned spec whose files are
/// all filtered out — and then calls the location empty, which for four of those five is
/// false. Each is a different fix, so each gets its own sentence.
///
/// The path is recovered by matching DataFusion's URL against the spec's own paths through
/// [`listing_url`], so multi-path tables name the source that actually failed rather than
/// the first one. A path that is a glob, or that lives in an object store (W7), can't be
/// resolved on disk and only gets what is certain: nothing matched it.
fn no_files_error(spec: &TableSpec, ext: &str, raw: &str) -> Option<String> {
    let token = raw
        .split("No files found at ")
        .nth(1)?
        .split_whitespace()
        .next()?;
    // Exactly the one sentence-ending dot DataFusion adds — `trim_end_matches` would eat a
    // trailing dot that belongs to the path itself.
    let url = token.strip_suffix('.').unwrap_or(token);
    let path = source_paths(spec)
        .find(|p| listing_url(p).is_ok_and(|u| u.to_string() == url))
        .unwrap_or(url);

    if path.contains("://") || is_glob(path) {
        return Some(format!("No files matched '{path}'."));
    }

    let on_disk = Path::new(path);
    if !on_disk.exists() {
        return Some(format!("No source at '{path}'."));
    }
    if on_disk.is_file() {
        // `with_file_extension` is a suffix match, so this asks exactly what DataFusion
        // asked when it skipped the file.
        return Some(if path.ends_with(ext) {
            format!("No files matched '{path}'.")
        } else {
            format!(
                "Table '{}' reads {ext} files, and '{path}' is not one.",
                spec.name
            )
        });
    }
    // With no partition columns nothing was filtered, so DataFusion's empty listing is
    // trustworthy and the directory can be called empty without looking. Only a partitioned
    // spec earns the walk — measured case: files under an unkeyed `2024/` where a Hive
    // partition needs `year=2024/`, where saying the directory holds nothing would repeat
    // DataFusion's own falsehood with the files sitting right there. An inconclusive walk
    // counts as "don't claim emptiness": on a lake big enough to exhaust the budget, the
    // partition columns are the likelier answer, and they're the only claim still supported.
    if !spec.partitions.is_empty() && holds_ext(on_disk, ext) != Some(false) {
        let cols: Vec<&str> = spec.partitions.iter().map(|(n, _)| n.as_str()).collect();
        return Some(format!(
            "No {ext} files under '{path}' match the partition columns '{}'.",
            cols.join("', '")
        ));
    }
    Some(format!("No {ext} files under '{path}'."))
}

/// Whether `dir` holds any file with extension `ext` (`".parquet"`), stopping at the first
/// hit — `None` when the entry budget ran out first and the answer is therefore unknown.
/// The distinction matters: a bare `false` on a lake too big to walk is indistinguishable
/// from a genuinely empty directory, and the caller turns that into a claim about the
/// user's data.
///
/// Bounded, and only ever walked on the failure path — a partitioned lake is deep, and this
/// exists to answer one yes/no question, not to enumerate it. The budget also bounds the
/// walk against a symlink cycle.
fn holds_ext(dir: &Path, ext: &str) -> Option<bool> {
    const MAX_ENTRIES: usize = 4096;
    let mut seen = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            seen += 1;
            if seen > MAX_ENTRIES {
                return None;
            }
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.to_string_lossy().ends_with(ext) {
                return Some(true);
            }
        }
    }
    Some(false)
}

// ---- schema helpers ----

/// What a view plan reads (D10): its **base tables** and the **names it inlines**.
///
/// Asks the planner, not the SQL text, which matters three ways:
///
/// - **Views are already resolved away.** DataFusion 54 inlines a view at plan-*build*
///   time, so a plan from `ctx.table("a_view")` scans the view's base tables directly.
///   That's transitive for free — a view over a view was inlined when the inner one was
///   planned, so `C → B → A → orders` collapses to a single tree carrying `orders` at
///   the leaf and `A`, `B` as the inliner's alias markers on the way down. Reading the
///   SQL would stop at `FROM b`.
/// - **`apply_with_subqueries`, not `apply`.** Plain `apply` visits only direct
///   children, so a view with `WHERE id IN (SELECT id FROM other)` would silently drop
///   `other` — and a *missed* dependency is the failure that matters: a stale profile
///   nobody invalidates, or an entry dropped without warning.
/// - **`.table()`, not `to_string()`.** A `TableReference` renders as written — `t`
///   here, `public.t` there — so `to_string()` yields two keys for one thing. The engine
///   owns a single schema, so the bare name is the identity.
pub struct PlanDeps {
    /// Base tables scanned — for profile invalidation and the table-drop warning.
    pub tables: Vec<String>,
    /// Every `SubqueryAlias` name, which for an inlined sub-view is the view's own name.
    /// Raw: also includes plain table aliases (`FROM t AS x`) and CTE names, since those
    /// are indistinguishable from a view inline in the plan. The UI keeps only the ones
    /// that are actually views. Recursion is automatic — a chain leaves one alias per
    /// hop in the tree, so this is the transitive set of referenced views.
    pub aliases: Vec<String>,
}

pub fn plan_deps(plan: &datafusion::logical_expr::LogicalPlan) -> PlanDeps {
    use datafusion::common::tree_node::TreeNodeRecursion;
    use datafusion::logical_expr::LogicalPlan;
    let mut tables = BTreeSet::new();
    let mut aliases = BTreeSet::new();
    let _ = plan.apply_with_subqueries(|node| {
        match node {
            LogicalPlan::TableScan(scan) => {
                // A source still carrying its own plan is a view that *didn't* inline —
                // only reachable if filters were pushed at build time, which our path
                // never does. Recording it would name the view instead of what it reads.
                if scan.source.get_logical_plan().is_none() {
                    tables.insert(scan.table_name.table().to_string());
                }
            }
            LogicalPlan::SubqueryAlias(a) => {
                aliases.insert(a.alias.table().to_string());
            }
            _ => {}
        }
        Ok(TreeNodeRecursion::Continue)
    });
    PlanDeps {
        tables: tables.into_iter().collect(),
        aliases: aliases.into_iter().collect(),
    }
}

/// Profile `name` — one full scan, every column at once (see [`crate::profile`]).
///
/// Spawned onto the engine's own runtime by [`Engine::profile`](super::Engine::profile), which
/// owns the abort handle: blocking is fine in here, since this is *meant* to be the expensive
/// thing the user opted into, and the UI stays live either way.
pub async fn run_profile(ctx: &SessionContext, name: &str) -> Result<CatalogProfile, String> {
    let df = ctx.table(name).await.map_err(|e| e.to_string())?;
    let columns: Vec<ColumnInfo> = df
        .schema()
        .fields()
        .iter()
        .map(|f| column_info(f))
        .collect();
    let (exprs, slots) = aggregates(&columns);
    // Render *before* executing, from the same `Expr`s that are about to run, so "view
    // as query" can't drift from the facts it produced. Not `plan_to_sql` on the whole
    // plan: that inlines a view's body and names no view (see `profile_sql`).
    let sql = profile_sql(name, &exprs);
    let batches = df
        .aggregate(vec![], exprs)
        .map_err(|e| e.to_string())?
        .collect()
        .await
        .map_err(|e| e.to_string())?;
    let batch = batches.first().ok_or("profile returned no batches")?;
    let mut profile = decode(&slots, batch, &columns)?;
    profile.sql = sql;
    Ok(profile)
}

/// A table's columns plus its **free** metadata — the row count and per-column
/// min/max/nulls, read from the source's own footers. One metadata read per file, no
/// data pages. Everything lands `None` for a source that reports nothing (CSV/JSON),
/// which the inspector renders as an absent row rather than a guess.
async fn table_meta(ctx: &SessionContext, name: &str) -> Result<TableMeta, String> {
    let df = ctx.table(name).await.map_err(|e| e.to_string())?;
    // `|f| column_info(f)`, not `column_info`: `fields()` yields `&Arc<Field>` and the
    // deref coercion to `&Field` only happens at a call site.
    let mut columns: Vec<ColumnInfo> = df
        .schema()
        .fields()
        .iter()
        .map(|f| column_info(f))
        .collect();
    let rows = free_stats(ctx, name, &mut columns).await;
    Ok(TableMeta { columns, rows })
}

/// Fold the source's free statistics onto `columns`, returning the row count. Best
/// effort throughout: anything unavailable simply stays `None`.
async fn free_stats(ctx: &SessionContext, name: &str, columns: &mut [ColumnInfo]) -> Option<u64> {
    use datafusion::datasource::listing::ListingTable;
    let provider = ctx.table_provider(name).await.ok()?;
    // Only a `ListingTable` has files whose footers can be read — a view has none.
    let lt = provider.downcast_ref::<ListingTable>()?;
    let state = ctx.state();
    // `limit: None` — a limit would make the aggregate inexact.
    let stats = lt
        .list_files_for_scan(&state, &[], None)
        .await
        .ok()?
        .statistics;
    let rows = stats.num_rows.get_value().map(|n| *n as u64);
    // Zip rather than index: DataFusion promises one entry per *table*-schema field, but
    // a table with no files short-circuits to `file_schema`, which omits the partition
    // columns — indexing would then misattribute every stat.
    for (col, cs) in columns.iter_mut().zip(stats.column_statistics.iter()) {
        // Push only what's actually there — an absent fact is an absent row, not a
        // blank one. Display order.
        let nulls = match cs.null_count.get_value() {
            // `Exact(num_rows)` is *also* DataFusion's "no stats for this column"
            // fallback, so an all-null column and an unknown one are indistinguishable.
            // Say nothing; the profile answers it for real with a COUNT ... FILTER.
            Some(n) if Some(*n as u64) == rows => None,
            Some(n) => Some(Stat {
                key: StatKey::Nulls,
                text: n.to_string(),
                exact: true,
            }),
            None => None,
        };
        col.stats = [
            nulls,
            stat_of(StatKey::Min, &cs.min_value),
            stat_of(StatKey::Max, &cs.max_value),
        ]
        .into_iter()
        .flatten()
        .collect();
    }
    rows
}

/// A `Precision<ScalarValue>` as a display [`Stat`]. `Absent` → `None` (say nothing).
/// A null value means the column is in the arrow schema but absent from the source's
/// own (schema evolution) — also nothing to report. `Inexact` carries through flagged.
fn stat_of(key: StatKey, p: &Precision<ScalarValue>) -> Option<Stat> {
    let v = p.get_value()?;
    if v.is_null() {
        return None;
    }
    Some(Stat {
        key,
        text: v.to_string(),
        exact: p.is_exact().unwrap_or(false),
    })
}

pub fn column_info(field: &Field) -> ColumnInfo {
    let dtype = short_type(field.data_type());
    ColumnInfo {
        name: field.name().clone(),
        kind: Kind::from_arrow(&dtype),
        dtype,
        nullable: field.is_nullable(),
        children: nested_children(field.data_type()),
        // Filled by `free_stats` where the source has metadata to read; a nested child
        // never gets any — footers describe leaves, and we don't traverse into them.
        stats: Vec::new(),
    }
}

fn nested_children(dt: &DataType) -> Vec<ColumnInfo> {
    match dt {
        DataType::Struct(fields) => fields.iter().map(|f| column_info(f)).collect(),
        DataType::List(f) | DataType::LargeList(f) | DataType::FixedSizeList(f, _) => {
            vec![column_info(f)]
        }
        DataType::Map(entries, _) => nested_children(entries.data_type()),
        _ => Vec::new(),
    }
}

fn short_type(dt: &DataType) -> String {
    let full = format!("{dt:?}");
    let base: String = full.split(['(', '<']).next().unwrap_or(&full).to_string();
    match base.as_str() {
        "LargeUtf8" => "Utf8".into(),
        "LargeList" | "FixedSizeList" => "List".into(),
        other => other.to_string(),
    }
}

fn parse_dtype(label: &str) -> DataType {
    match label {
        "Int8" => DataType::Int8,
        "Int16" => DataType::Int16,
        "Int32" => DataType::Int32,
        "Int64" => DataType::Int64,
        "Float32" => DataType::Float32,
        "Float64" => DataType::Float64,
        "Date" | "Date32" => DataType::Date32,
        _ => DataType::Utf8,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `raw` below is a **measured** string: what `Engine::register` actually
    /// returned for that source on DataFusion 54. They are the point of these tests — the
    /// mapping keys off engine wording, so if an upgrade rewords a failure the arm stops
    /// matching and the message silently reverts to pass-through. A test holding the old
    /// wording is what catches that.
    fn spec(name: &str, paths: &[&str], format: &str) -> TableSpec {
        TableSpec {
            name: name.into(),
            paths: paths.iter().map(|s| s.to_string()).collect(),
            format: SourceFormat::from_name(format),
            partitions: Vec::new(),
        }
    }

    fn tmp(sub: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir()
            .join("strata_register_error_tests")
            .join(sub);
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    // ---- Hive partition detection ----

    /// Detection now lists through the session's object store rather than `std::fs`, so these
    /// go through a real `SessionContext` — which for a local path is the store DataFusion
    /// registers for `file://`, the same code path a bucket will take.
    fn detect(paths: &[&str]) -> Vec<String> {
        let ctx = SessionContext::new();
        let paths: Vec<String> = paths.iter().map(|p| p.to_string()).collect();
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(detect_partitions(&ctx, &paths))
    }

    #[test]
    fn a_path_that_globs_its_partition_keys_needs_no_listing() {
        assert_eq!(
            detect(&["/data/events/year=*/month=*/*.parquet"]),
            vec!["year", "month"]
        );
    }

    #[test]
    fn a_literal_partition_segment_is_a_root_and_declares_nothing() {
        // The level is already consumed by the path, so declaring it registers a table that
        // silently returns no rows at all — see `keys_in_pattern`. Nothing on disk here, so
        // the store walk finds nothing either and the honest answer is "no columns".
        assert!(detect(&["/data/events/year=2024/month=03/"]).is_empty());
    }

    #[test]
    fn a_directory_is_listed_for_its_partition_levels() {
        let root = tmp("hive_walk");
        std::fs::create_dir_all(root.join("year=2024/month=03")).unwrap();
        std::fs::create_dir_all(root.join("year=2025/month=01")).unwrap();
        assert_eq!(
            detect(&[&format!("{}/", root.to_string_lossy())]),
            vec!["year", "month"]
        );
    }

    #[test]
    fn an_unpartitioned_directory_finds_nothing_rather_than_guessing() {
        let root = tmp("hive_flat");
        std::fs::create_dir_all(root.join("2024/03")).unwrap();
        assert!(detect(&[&format!("{}/", root.to_string_lossy())]).is_empty());
    }

    #[test]
    fn a_segment_whose_key_is_not_an_identifier_is_not_a_partition() {
        // `=` shows up in perfectly ordinary names; only an identifier can become a column.
        assert!(detect(&["/data/a b=1/x.parquet"]).is_empty());
        assert!(detect(&["/data/=1/x.parquet"]).is_empty());
        assert!(detect(&["/data/2024=1/x.parquet"]).is_empty());
    }

    // ---- JSON shapes ----

    /// A JSON spec already set to read whole-document arrays.
    fn array_spec(name: &str) -> TableSpec {
        TableSpec {
            format: SourceFormat::Json(JsonRead {
                shape: JsonShape::Array,
                ..Default::default()
            }),
            ..spec(name, &[], "json")
        }
    }

    #[test]
    fn a_pretty_printed_record_is_named_as_a_shape_problem() {
        // The file is valid JSON; Arrow's own "Not valid JSON" is the wrong story.
        let raw = "Arrow error: Json error: Not valid JSON: EOF while parsing an object at line 1 column 1";
        assert_eq!(
            register_error(&spec("signups", &[], "json"), ".json", raw),
            "Cannot read 'signups' as JSON: a record does not end on its line. \
             Set the JSON shape to array in Table Config, or use newline-delimited JSON."
        );
    }

    #[test]
    fn a_top_level_array_never_carries_the_parsed_document() {
        let raw = "Arrow error: Json error: Expected JSON record to be an object, found Array \
                   [Object {\"a\": Number(1)}, Object {\"a\": Number(2)}]";
        let msg = register_error(&spec("signups", &[], "json"), ".json", raw);
        assert_eq!(
            msg,
            "Cannot read 'signups' as JSON: the source is a JSON array. \
             Set the JSON shape to array in Table Config, or use newline-delimited JSON."
        );
        assert!(
            !msg.contains("Number("),
            "the parsed value never reaches the user"
        );
    }

    #[test]
    fn a_non_object_record_names_what_was_found() {
        let raw = "Arrow error: Json error: Expected JSON record to be an object, found Number 3";
        assert_eq!(
            register_error(&spec("nums", &[], "json"), ".json", raw),
            "Cannot read 'nums' as JSON: a top-level Number is not a record. \
             Set the JSON shape to array in Table Config, or use newline-delimited JSON."
        );
    }

    #[test]
    fn a_table_already_reading_arrays_is_not_told_to_set_the_shape_it_has() {
        // The advice is only a fix while the def is in the other shape. Repeating it here
        // sends the user to a setting that already says what it should.
        let raw = "Arrow error: Json error: Expected JSON record to be an object, found Number 3";
        assert_eq!(
            register_error(&array_spec("nums"), ".json", raw),
            "Cannot read 'nums' as JSON: a top-level Number is not a record."
        );
    }

    #[test]
    fn a_syntax_error_keeps_arrows_diagnosis() {
        // A genuinely malformed file is *not* a shape problem, and Arrow's line:column is
        // the useful part. Rewriting this into "must be newline-delimited" would be the
        // same confident-but-wrong diagnosis this mapping exists to remove.
        let raw =
            "Arrow error: Json error: Not valid JSON: key must be a string at line 1 column 9";
        assert_eq!(
            register_error(&spec("bad", &[], "json"), ".json", raw),
            "Cannot read 'bad' as JSON: key must be a string at line 1 column 9"
        );
    }

    // ---- empty listings ----

    /// DataFusion names the failing source as the **URL** it built, not the string the user
    /// typed, so every fake error here is built through `listing_url` — the same call the
    /// recovery uses. Hand-writing a bare path would quietly bypass that lookup and test the
    /// `unwrap_or(url)` fallback instead.
    fn no_files_at(source: &str) -> String {
        format!(
            "Error during planning: No files found at {}. Cannot infer schema from an empty \
             location; either add data files or declare an explicit schema for the table.",
            listing_url(source).unwrap()
        )
    }

    fn no_files_at_path(path: &std::path::Path) -> String {
        no_files_at(&path.display().to_string())
    }

    #[test]
    fn a_missing_path_says_so_rather_than_calling_it_empty() {
        let missing = tmp("missing").join("nope.parquet");
        let p = missing.display().to_string();
        assert_eq!(
            register_error(
                &spec("t", &[&p], "parquet"),
                ".parquet",
                &no_files_at_path(&missing)
            ),
            format!("No source at '{p}'.")
        );
    }

    #[test]
    fn a_file_whose_extension_does_not_match_says_which_extension_is_read() {
        let dir = tmp("ext");
        let csv = dir.join("regions.csv");
        std::fs::write(&csv, "a,b\n1,2\n").unwrap();
        let p = csv.display().to_string();
        assert_eq!(
            register_error(
                &spec("regions", &[&p], "parquet"),
                ".parquet",
                &no_files_at_path(&csv)
            ),
            format!("Table 'regions' reads .parquet files, and '{p}' is not one.")
        );
    }

    #[test]
    fn an_empty_directory_is_reported_as_holding_nothing_readable() {
        let dir = tmp("bare");
        std::fs::write(dir.join("notes.txt"), "hello").unwrap();
        let p = dir.display().to_string();
        assert_eq!(
            register_error(
                &spec("t", &[&p], "parquet"),
                ".parquet",
                &no_files_at_path(&dir)
            ),
            format!("No .parquet files under '{p}'.")
        );
    }

    #[test]
    fn a_directory_that_does_hold_files_blames_the_partition_columns() {
        // The measured case: `<dir>/2024/data.csv` declared with partition column `year`.
        // Hive partitions must be `key=value/` directories, so an unkeyed `2024/` matches
        // nothing and DataFusion calls the location empty — with the files right there.
        // Depth mismatches do *not* land here: DataFusion pairs partition columns to
        // directory levels positionally, so declaring two columns over a flat directory,
        // or one over a two-level tree, simply registers.
        let dir = tmp("parts");
        let leaf = dir.join("2024");
        std::fs::create_dir_all(&leaf).unwrap();
        std::fs::write(leaf.join("data.csv"), "a,b\n1,2\n").unwrap();
        let p = dir.display().to_string();
        let mut s = spec("events", &[&p], "csv");
        s.partitions = vec![("year".into(), "Utf8".into())];
        assert_eq!(
            register_error(&s, ".csv", &no_files_at_path(&dir)),
            format!("No .csv files under '{p}' match the partition columns 'year'.")
        );
    }

    #[test]
    fn a_glob_only_claims_that_nothing_matched() {
        // Through `no_files_at`, so the glob is recovered by the same URL lookup the real
        // path takes — and the message quotes the user's own glob, not the URL.
        let g = format!("{}/**/*.parquet", tmp("glob").display());
        assert_eq!(
            register_error(&spec("t", &[&g], "parquet"), ".parquet", &no_files_at(&g)),
            format!("No files matched '{g}'.")
        );
    }

    #[test]
    fn a_source_whose_name_ends_in_a_dot_is_still_recovered() {
        // DataFusion ends the sentence with a dot, so the URL of `report.` arrives as
        // `…/report..`. Stripping every trailing dot would resolve to a different file.
        let dir = tmp("dotted");
        let odd = dir.join("report.");
        std::fs::write(&odd, "x").unwrap();
        let p = odd.display().to_string();
        assert_eq!(
            register_error(
                &spec("t", &[&p], "parquet"),
                ".parquet",
                &no_files_at_path(&odd)
            ),
            format!("Table 't' reads .parquet files, and '{p}' is not one.")
        );
    }

    #[test]
    fn a_json_error_on_a_non_json_table_is_not_rewritten_as_a_json_problem() {
        // The table is parquet; whatever produced this text, "cannot read it as JSON" is a
        // claim about a file that was never being read as JSON.
        let raw = "Arrow error: Json error: Not valid JSON: EOF while parsing an object at line 1 column 1";
        assert_eq!(
            register_error(&spec("events", &[], "parquet"), ".parquet", raw),
            raw
        );
    }

    #[test]
    fn an_unwalkable_directory_never_claims_to_be_empty() {
        // `holds_ext` gives up past its entry budget. The caller must not turn "I stopped
        // looking" into "there is nothing here" — on a partitioned spec the partition
        // columns are the only claim still supported.
        let dir = tmp("huge");
        for i in 0..5_000 {
            std::fs::write(dir.join(format!("f{i}.txt")), "x").unwrap();
        }
        assert_eq!(holds_ext(&dir, ".parquet"), None, "the budget ran out");
        let p = dir.display().to_string();
        let mut s = spec("lake", &[&p], "parquet");
        s.partitions = vec![("year".into(), "Utf8".into())];
        assert_eq!(
            register_error(&s, ".parquet", &no_files_at_path(&dir)),
            format!("No .parquet files under '{p}' match the partition columns 'year'.")
        );
    }

    #[test]
    fn a_multi_path_table_names_the_source_that_failed() {
        let dir = tmp("multi");
        let good = dir.join("a.parquet");
        std::fs::write(&good, "x").unwrap();
        let missing = dir.join("b.parquet");
        let (g, m) = (good.display().to_string(), missing.display().to_string());
        assert_eq!(
            register_error(
                &spec("t", &[&g, &m], "parquet"),
                ".parquet",
                &no_files_at_path(&missing)
            ),
            format!("No source at '{m}'.")
        );
    }

    // ---- pass-through ----

    #[test]
    fn an_unrecognised_failure_is_left_exactly_as_datafusion_wrote_it() {
        let raw = "Parquet error: Parquet error: Invalid Parquet file. Corrupt footer";
        assert_eq!(
            register_error(&spec("t", &[], "parquet"), ".parquet", raw),
            raw
        );
    }

    #[test]
    fn a_runaway_message_is_capped() {
        let raw = "Internal error: ".to_string() + &"x".repeat(5_000);
        let msg = register_error(&spec("t", &[], "parquet"), ".parquet", &raw);
        assert_eq!(
            msg.chars().count(),
            MAX_PASSTHROUGH + 1,
            "capped, plus the marker"
        );
        assert!(msg.ends_with('…'));
        assert!(
            msg.starts_with("Internal error: "),
            "the useful head survives"
        );
    }
}
