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
    ChartRole, ColumnInfo, CsvRead, FileCompression, JsonShape, Kind, SourceFormat, Stat, StatKey,
};

use crate::engine::arrow_stats::StrataArrowFormat;
use crate::engine::json_poly::PolyJsonFormat;
use crate::engine::query::is_snapshot_name;
use crate::engine::sql::Blocked;
use crate::engine::{fold_ident, CATALOG, SCHEMA};
use crate::profile::{aggregates, decode, profile_sql, CatalogProfile};

/// What a (re)registration learned about a table: its columns, plus the free row count
/// (`None` when the source doesn't report one).
#[derive(Clone, Debug, PartialEq)]
pub struct TableMeta {
    pub columns: Vec<ColumnInfo>,
    pub rows: Option<u64>,
}

/// Everything needed to register one table: its name, source paths, the reader and
/// its options, Hive partition columns, and whose files those are.
#[derive(Clone, Debug)]
pub struct TableSpec {
    pub name: String,
    pub paths: Vec<String>,
    /// The reader *and* the options it takes — see [`SourceFormat`]. One field, so a CSV
    /// delimiter cannot be named on a parquet table.
    pub format: SourceFormat,
    pub partitions: Vec<(String, String)>,
    /// [`TableOrigin::Internal`](strata_model::TableOrigin::Internal) — the data under
    /// [`paths`](Self::paths) is Strata's, spooled into the project's `.strata/tables/` by a
    /// `CREATE TABLE` (ED-04).
    ///
    /// The registration path itself is **identical** either way; this is carried so two things
    /// downstream can be true. A failure to list the files reads differently (`.strata/tables`
    /// is gitignored, so "no source at that path" is the wrong story in a fresh clone — see
    /// [`no_files_error`]), and the engine records which providers a write statement may target
    /// ([`Engine::is_internal`](super::Engine::is_internal)).
    pub internal: bool,
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
/// partition values are already live, because DataFusion re-`LIST`s per scan and this engine
/// runs **no** `ListFilesCache` — which is a setting, not an absence: DataFusion 54 turns that
/// cache on by default with an infinite TTL, and `build_runtime` turns it back off (see
/// `config::ENGINE_KEYS`). With it on, this function returns the previous listing and a re-scan
/// answers with the files that were there last time.
pub async fn register_external(
    ctx: &SessionContext,
    spec: &TableSpec,
) -> Result<TableMeta, String> {
    use datafusion::datasource::file_format::parquet::ParquetFormat;
    use datafusion::datasource::file_format::FileFormat;
    use datafusion::datasource::listing::{ListingOptions, ListingTable, ListingTableConfig};

    // **The reserved namespace, backstopped at the funnel** (`docs/STATEMENTS_SPEC.md` §4). The
    // router already refuses a `__snap_` target in a typed statement, but a def reaches here from
    // three other directions — Table Config, a hand-edited `project.json`, a project written by
    // an older build — and a table registered into that namespace would be invisible to every
    // catalog reader (the schema provider hides the prefix) and would cost a *Run* the first
    // time a snapshot wanted the same name.
    if is_snapshot_name(&spec.name) {
        return Err(Blocked::ReservedName.editor_message());
    }

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
        SourceFormat::Json(o) => Arc::new(PolyJsonFormat::new(o.clone())),
        // Not stock `ArrowFormat`: its `infer_stats` answers unknown, so an internal table —
        // whose whole data set Strata wrote — could not say how many rows it holds. See
        // [`StrataArrowFormat`].
        SourceFormat::Arrow => Arc::new(StrataArrowFormat::default()),
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
    // claims to be on. It's baked in at `try_new`, so a registered table can't be fixed after
    // the fact — a re-scan is a re-registration through here, which is the only way it moves.
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

    let config = match ListingTableConfig::new_with_multi_paths(urls)
        .with_listing_options(opts)
        .infer_schema(&ctx.state())
        .await
    {
        Ok(config) => config,
        Err(e) => {
            tracing::error!("Failed to infer schema: {}", e);
            let raw = e.to_string();
            // **The one question the message cannot answer on its own**, asked here because
            // asking it is `async` and a message mapper is not: does the location actually hold
            // files of this format, under names the partition columns did not match? See
            // [`store_holds_ext`] — it costs a listing, only on the failure path, and only for a
            // partitioned source whose listing came back empty.
            let holds = holds_under_partitions(ctx, spec, ext, &raw).await;
            return Err(register_error(spec, ext, &raw, holds));
        }
    };
    // **The runtime's per-file statistics cache, handed to the table that needs it.**
    //
    // Without it `collected_statistics` is `None`, so `do_collect_statistics_and_ordering`
    // misses every time and re-reads every file's footer — on **every scan**, because
    // [`free_stats`] reaches it through `list_files_for_scan`, and on every registration,
    // because that is what [`table_meta`] answers a row's count from. An internal table is one
    // appended IPC file per `INSERT` (ED-05), and an `INSERT` asks for a re-scan, so file *k*'s
    // arrival re-read *k* footers: quadratic in the number of writes, before anyone had queried
    // anything.
    //
    // The `RuntimeEnv` builds a 20 MiB `DefaultFileStatisticsCache` by default and we never
    // asked for it. **Not the same call as the list-files cache**, which `ENGINE_KEYS` defaults
    // to `0` on purpose: that one answers "which files are there", and a re-scan means asking
    // again. This one answers "what is in *this* file", keyed per object and invalidated by
    // `is_valid_for` on size and mtime — so a re-listing still finds new files, a replaced file
    // still re-reads, and only an unchanged file is spared.
    let table = ListingTable::try_new(config)
        .map_err(|e| register_error(spec, ext, &e.to_string(), None))?
        .with_cache(ctx.runtime_env().cache_manager.get_file_statistic_cache());
    ctx.register_table(spec.name.as_str(), Arc::new(table))
        .map_err(|e| {
            tracing::error!("Failed to register table: {}", e);
            register_error(spec, ext, &e.to_string(), None)
        })?;

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
/// prefix at each level. Silent to the *caller* on any store error: a path that cannot be listed
/// is a path with no partitions to offer, and the *register* is where an unreadable source is
/// reported.
///
/// **Silent to the caller is not silent to the log.** Four different facts leave this function as
/// the same empty vec — an unparseable URL, a bucket with no registered store, a listing the
/// store refused, and a genuine "nothing here is `key=value`" — and the Hive toggle renders all
/// four as "not partitioned". That is the right *answer* (there is nothing to offer either way)
/// and a terrible *diagnosis*: a user whose lake is sitting right there gets no way to tell a
/// broken connection from a layout this does not recognise. So each exit says which it was, at
/// `info` because detection runs on a toggle press rather than in any loop.
async fn keys_in_store(ctx: &SessionContext, path: &str) -> Vec<String> {
    let Ok(url) = listing_url(path) else {
        tracing::info!("no partitions under '{path}': not a source URL");
        return Vec::new();
    };
    let Ok(store) = ctx.runtime_env().object_store(&url) else {
        // The connection's `connect` never ran, or ran and was refused — a refused connection
        // registers nothing at all (`store::connect` is all-or-nothing).
        tracing::info!("no partitions under '{path}': no object store is registered for it");
        return Vec::new();
    };

    let mut keys = Vec::new();
    let mut prefix = url.prefix().clone();
    for depth in 0..MAX_PARTITION_DEPTH {
        let listed = match store.list_with_delimiter(Some(&prefix)).await {
            Ok(listed) => listed,
            Err(e) => {
                tracing::info!("partition scan of '{path}' stopped at '{prefix}': {e}");
                break;
            }
        };
        // Sorted, so the level's key is the same on every run: a store's listing order is its
        // own, and a level holding one stray non-partition prefix would otherwise answer
        // differently each time.
        let mut prefixes = listed.common_prefixes;
        prefixes.sort();
        let found = prefixes.iter().find_map(|p| {
            // The key is read out before `p` is cloned into the pair — a `Path`'s parts borrow it.
            let key = partition_key(p.parts().next_back()?.as_ref())?.to_string();
            Some((key, p.clone()))
        });
        let Some((key, next)) = found else {
            // **Rendered here and nowhere else.** The decisive datum when a lake that *looks*
            // partitioned finds nothing is what the store actually answered at this level — no
            // prefixes at all (wrong path, or files with no folders under it) reads very
            // differently from prefixes that are simply not `key=value`. Built inside this arm
            // rather than beside the listing, because on the success path it is a `String` per
            // folder, per level, that nothing ever reads: a first level holding twenty thousand
            // prefixes allocated twenty thousand of them to throw away.
            let seen: Vec<String> = prefixes.iter().map(ToString::to_string).collect();
            tracing::info!(
                "partition scan of '{path}' found no key=value folder at depth {depth} \
                 under '{prefix}'; the store listed {} folder(s) there: {seen:?}",
                seen.len()
            );
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

// The JSON arm above is `json_poly::PolyJsonFormat`, not arrow's `JsonFormat`.
//
// Arrow's schema inference admits five type combinations and errors on every other pair, so a
// field that is a string in one record and an object in the next fails the whole registration —
// `Expected object json type, found: Array(Scalar({Utf8, Boolean}))`, naming neither the key nor
// the file. Ours stringifies exactly those paths and infers everything else identically, so it is
// a superset rather than a behaviour change (`json_poly::infer`, asserted against arrow's own
// inference in that module's tests).
//
// The def's options are carried by the format itself, so there is no `json_format` dresser left:
// `PolyJsonFormat::new` takes the whole `JsonRead`.

/// Our compression vocabulary as DataFusion's.
pub(super) fn compression(
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

/// The wrapper names DataFusion prepends as a failure crosses each crate boundary, in the
/// spelling it writes them.
///
/// Every one of these names a **layer the message travelled through**, not a cause — which is
/// what makes stripping them safe on the pass-through path, beside the mappers that do diagnose.
/// Unrecognised text is left exactly as it arrived, so the list being incomplete costs noise
/// rather than meaning.
const LAYERS: &[&str] = &["External error", "Object Store error", "Execution error"];

/// `object_store`'s own store wrapper, which is a **format with an open store name** rather than
/// a list: `#[error("Generic {} error: {}", store, source)]` (`object_store/src/lib.rs`), where
/// the name is each backend's private `STORE` const — `S3`, `GCS`, `HTTP`, `MicrosoftAzure`,
/// `LocalFileSystem`, `InMemory`, and whatever the crate adds next.
///
/// Matched as the pattern it is, because enumerating it is how the list goes stale: the first
/// version of this file *did* enumerate, and shipped `GoogleCloudStorage` (the crate says `GCS`)
/// while omitting `HTTP` — one of Strata's three providers, and the one whose tables are the only
/// place an HTTP connection's reachability is tested at all, since `store::reachable` exempts it.
///
/// [`STORE_WORDS`] is what keeps the pattern from being greedy: a store name is one word, or two
/// where the crate writes `HTTP client`, never a sentence. Without it a message that merely opens
/// with "Generic " would have everything up to its first ` error: ` cut away — which is not a
/// message DataFusion writes, but it is the difference between a rule and a coincidence.
const STORE_PREFIX: &str = "Generic ";
const STORE_SUFFIX: &str = " error: ";
const STORE_WORDS: usize = 2;

/// `object_store`'s retry bookkeeping: which request was made, against what, how long it took and
/// how many attempts it was given — figures about the client, not about what is wrong.
///
/// The shape is `RetryError`'s `Display` (`object_store/src/client/retry.rs`):
///
/// ```text
/// Error performing {METHOD} {uri} in {elapsed:?}[, after {n} retries, max_retries: {m},
/// retry_timeout: {t:?} ] - {cause}
/// ```
///
/// so the cut is at the **first** ` - `: everything before it is that bookkeeping, and neither a
/// `Duration`'s `Debug` nor a URI can contain the separator (a URI cannot hold a raw space).
const RETRY_PREFIX: &str = "Error performing ";
const RETRY_CAUSE: &str = " - ";

/// Whether a source path is a glob rather than a name that can be looked up on disk.
fn is_glob(p: &str) -> bool {
    p.contains('*') || p.contains('?') || p.contains('[')
}

/// The most entries either walk will look at before giving up — [`store_holds_ext`]'s listing and
/// [`holds_ext`]'s directory walk alike.
///
/// **One number, because it settles one question for both**: does this location hold a file of
/// this format, asked on a failure path where the answer is a sentence rather than data. A lake
/// big enough to exhaust it is a lake whose partition columns are the likelier answer anyway, and
/// the two paths have to agree about when they stop knowing that.
const MAX_ENTRIES: usize = 4096;

/// Whether the **remote** source that failed holds a file of this format after all — the
/// question [`holds_ext`] answers for a local directory, asked of an object store instead.
///
/// `None` for everything that is not exactly that case: a local path (the sync walk covers it),
/// a glob (a pattern is not a place to list), an unpartitioned def (nothing was filtered, so
/// DataFusion's empty listing is already trustworthy) and any failure that is not an empty
/// location. So the cost is one listing, on a failure, for a partitioned remote table — and
/// never on the happy path.
///
/// It lists through the **session's own store**, exactly as `detect_partitions` does: a bucket
/// has no directories to walk, and the S3 client that answered the registration is the one that
/// can say what is under a prefix. A listing error is `None` rather than `false` — unknown and
/// empty are different answers, and only one of them is safe to turn into a claim about the
/// user's data.
async fn holds_under_partitions(
    ctx: &SessionContext,
    spec: &TableSpec,
    ext: &str,
    raw: &str,
) -> Option<bool> {
    if spec.partitions.is_empty() {
        return None;
    }
    let path = failing_source(spec, raw)?;
    if !path.contains("://") || is_glob(path) {
        return None;
    }
    store_holds_ext(ctx, path, ext).await
}

/// Whether any object under `path` ends in `ext`, stopping at the first hit — `None` when the
/// store could not be listed, or when the budget ran out before an answer.
///
/// `ObjectStore::list` is recursive (no delimiter), which is what this wants: the question is
/// whether the files are *anywhere* under the prefix, not how they are laid out.
async fn store_holds_ext(ctx: &SessionContext, path: &str, ext: &str) -> Option<bool> {
    use futures::StreamExt;
    use object_store::ObjectStore;

    let url = listing_url(path).ok()?;
    let store = ctx.runtime_env().object_store(&url).ok()?;
    let mut listed = store.list(Some(url.prefix()));
    let mut seen = 0;
    while let Some(object) = listed.next().await {
        let object = object.ok()?;
        seen += 1;
        if seen > MAX_ENTRIES {
            return None;
        }
        if object.location.as_ref().ends_with(ext) {
            return Some(true);
        }
    }
    Some(false)
}

/// The location DataFusion called empty — the URL out of a "No files found at <url>" failure, and
/// `None` for any other failure.
///
/// **The one place that message is parsed.** Three callers need it and they run in different
/// worlds: the guard that says this mapping applies at all, the store listing (async), and the
/// message itself. A second copy of the split would be a second copy of the trailing-dot rule
/// below, which decides which file the message is about.
fn failing_location(raw: &str) -> Option<&str> {
    let token = raw
        .split("No files found at ")
        .nth(1)?
        .split_whitespace()
        .next()?;
    // Exactly the one sentence-ending dot DataFusion adds — `trim_end_matches` would eat a
    // trailing dot that belongs to the path itself.
    Some(token.strip_suffix('.').unwrap_or(token))
}

/// Which of `spec`'s sources that location is, **in the user's own spelling** — `None` when it
/// names none of them. Matched through [`listing_url`], so the lookup is the same normalization
/// registration itself performed.
fn failing_source<'a>(spec: &'a TableSpec, raw: &str) -> Option<&'a str> {
    let url = failing_location(raw)?;
    source_paths(spec).find(|p| listing_url(p).is_ok_and(|u| u.to_string() == url))
}

/// Translate a registration failure into something the user can act on.
///
/// Only failures we actually recognise are rewritten; anything else passes through as
/// DataFusion wrote it, [unwrapped](readable) but **whole**. Translating an unfamiliar error
/// would mean guessing at its cause, and a confident wrong diagnosis is worse than a raw
/// one the user can search for.
///
/// **Nothing is capped here, and that is a change.** The pass-through used to be cut at 240
/// characters with a trailing `…`, on account of the one surface that could not hold a sentence:
/// a catalog row's tooltip and its a11y label. That put a **narrow surface's limit into the
/// string every consumer reads** — so the Problems drawer, which wraps, and its copy button,
/// which exists precisely so a message can be pasted into a search, both handed back a sentence
/// cut mid-clause. An unreachable bucket reports well past 240 characters and names its cause in
/// the last clause, so the cut kept the bookkeeping and threw away the answer.
///
/// The limit now lives with the surface that has it (`catalog::entry`'s `TIP_CHARS`, which says
/// where the rest is), and what leaves here is whole.
fn register_error(spec: &TableSpec, ext: &str, raw: &str, holds: Option<bool>) -> String {
    if let Some(m) = json_shape_error(spec, raw) {
        return m;
    }
    if let Some(m) = no_files_error(spec, ext, raw, holds) {
        return m;
    }
    readable(raw)
}

/// `raw` with the wrapper stack peeled off, down to the sentence that says what is wrong.
///
/// DataFusion and `object_store` report a failure by **prepending a name per crate boundary it
/// crossed**, so a bucket that will not answer arrives as one line carrying three layers and the
/// client's retry settings before it reaches the point:
///
/// ```text
/// External error: Object Store error: Generic S3 error: Error performing GET
/// http://127.0.0.1:4566/lake/a.parquet in 5.383s, after 10 retries, max_retries: 10,
/// retry_timeout: 180s  - HTTP error: error sending request for url (…): connection refused
/// ```
///
/// That is what reads as a stack trace, and the whole of what the user needs is the tail. Peeling
/// is **not** diagnosis — a layer name says where the message has been, never what happened — so
/// this belongs on the pass-through path rather than among the mappers above, and what it hands
/// back is still DataFusion's own words.
///
/// **Every literal here is checked against the crate that writes it**, named at each constant.
/// The first version of this function was written from a doc comment instead
/// (`store::is_bare_redirect`'s, since corrected) and matched three strings `object_store` has
/// never emitted, so it stripped nothing at all on the one path it exists for — while its unit
/// test, whose fixture had been written to match the code rather than copied from the crate,
/// passed.
///
/// It loops because the layers nest, and it stops the moment it stops recognising one. Peeling
/// everything away is treated as not recognising the message at all: a string that *is* only
/// wrappers has no tail to keep, and the raw line is more use than an empty row.
pub(crate) fn readable(raw: &str) -> String {
    let mut s = raw.trim();
    loop {
        if let Some(rest) = LAYERS
            .iter()
            .find_map(|layer| s.strip_prefix(layer).and_then(|r| r.strip_prefix(':')))
            .map(str::trim_start)
        {
            s = rest;
            continue;
        }
        // `Generic {store} error: …`, matched as a pattern — see [`STORE_PREFIX`]. The *first*
        // ` error: ` is the wrapper's, because the format puts it immediately after the store
        // name, and the name is bounded so the pattern cannot eat a sentence.
        if let Some(rest) = s.strip_prefix(STORE_PREFIX).and_then(|r| {
            let (store, rest) = r.split_once(STORE_SUFFIX)?;
            (store.split_whitespace().count() <= STORE_WORDS).then_some(rest)
        }) {
            s = rest.trim_start();
            continue;
        }
        // Only where `object_store` writes it — leading — so a message that merely *contains*
        // ` - ` keeps it.
        if let Some((_, rest)) = s
            .strip_prefix(RETRY_PREFIX)
            .and_then(|r| r.split_once(RETRY_CAUSE))
        {
            s = rest.trim_start();
            continue;
        }
        break;
    }
    match s.is_empty() {
        true => raw.trim().to_string(),
        false => s.to_string(),
    }
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

    // Note for anyone reading the history: main briefly translated arrow's
    // `Expected object json type, found: …` family here, into "a field has more than one type
    // across records". That message is unreachable on this branch — `engine::json_poly` replaced
    // arrow's JSON inference outright and *handles* the conflict rather than failing on it, so
    // nothing can produce the string. The arm was dropped in the merge rather than kept as a
    // translation for an error that cannot occur (and a test over a hardcoded `raw` string that
    // would pass forever regardless).

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
fn no_files_error(spec: &TableSpec, ext: &str, raw: &str, holds: Option<bool>) -> Option<String> {
    // The location DataFusion named, and the guard that this mapping applies at all. `holds` is
    // present only for the case that earned a listing — a partitioned **remote** source (see
    // [`holds_under_partitions`]); everything else is `None` and the arms below decide for
    // themselves, exactly as they did.
    let location = failing_location(raw)?;
    // **An internal table's empty listing is a different fact entirely** (ED-04), and every
    // sentence below would be a lie about it: the path is `.strata/tables/<slug>/`, which the
    // user never typed, cannot fix, and did not lose — `tables/` is gitignored, so a colleague
    // who clones the project gets the def and none of the data. Say that, and do not invite
    // them to go and repair a path.
    if spec.internal {
        return Some(format!(
            "Table '{}' has no data in this copy of the project. An internal table's data is \
             local to the machine that created it.",
            spec.name
        ));
    }
    // Falling back to the URL as DataFusion spelled it, when it names no source of ours — a
    // normalization this build does not perform, or a path the def no longer carries.
    let path = failing_source(spec, raw).unwrap_or(location);

    if path.contains("://") || is_glob(path) {
        // **A remote prefix was listed** for exactly this (`holds_under_partitions`), so the
        // partition columns are blamed on the same terms as a local directory's: the files are
        // there, or the walk could not settle it, and either way they are the likelier answer.
        // A glob brings no listing (a pattern is not a place), so it keeps the one claim it can
        // make.
        if !spec.partitions.is_empty() && holds != Some(false) && !is_glob(path) {
            return Some(partition_mismatch(spec, ext, path));
        }
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
        return Some(partition_mismatch(spec, ext, path));
    }
    Some(format!("No {ext} files under '{path}'."))
}

/// The one sentence both stores' walks arrive at: the files are under `path`, and the partition
/// columns are what did not match them. One copy, because a local directory and a bucket prefix
/// have earned the same claim by then — `holds_ext` for one, `store_holds_ext` for the other.
fn partition_mismatch(spec: &TableSpec, ext: &str, path: &str) -> String {
    let cols: Vec<&str> = spec.partitions.iter().map(|(n, _)| n.as_str()).collect();
    format!(
        "No {ext} files under '{path}' match the partition columns '{}'.",
        cols.join("', '")
    )
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

/// The registered views whose plans read the table `name` — the readers a drop leaves invalid
/// (ED-05), sorted, and **named rather than cascaded**.
///
/// The plan a view carries was inlined when it was created, so a view reading `orders` through
/// another view names `orders` at its leaf and is found here with no recursion of ours.
pub async fn dependent_views(ctx: &SessionContext, name: &str) -> Vec<String> {
    readers(ctx, name, |deps| &deps.tables).await
}

/// The registered views whose plans read the **view** `name` — the same question a rung up
/// (ED-06), and the answer the catalog pane's own view-drop confirm shows.
///
/// A different half of [`PlanDeps`], because a view is not a leaf: the inliner leaves the view's
/// *name* behind as a `SubqueryAlias` and its base tables at the leaves, so a reader of `orders`
/// and a reader of the view over `orders` are told apart by which list the name is in. That is
/// exactly the split the store keeps (`ViewInfo::deps` vs `view_deps`), which is what makes the
/// typed drop's report and the pane's warning the same fact.
///
/// **The aliases half is raw, and this over-reports on purpose.** A `SubqueryAlias` is what the
/// inliner leaves *and* what `FROM t AS v` and a CTE named `v` leave, and the plan cannot tell
/// them apart — so dropping the view `v` also names a view that merely aliased something else `v`.
/// Kept, for two reasons. It is the safe direction: a **missed** reader is a destructive action
/// reported as consequence-free, where a spare one is a name the user can look at. And it is not
/// a divergence from the pane, whose filter (`ProjectState::view_registered`) keeps an alias only
/// where a view row of that name exists — always true of the name being dropped, so the filter
/// cannot subtract this case and the two surfaces still say one thing. Telling the two apart
/// would mean comparing the aliased subtree against the view's own registered plan, which is a
/// change to what `PlanDeps` *is* and would have to move both surfaces at once.
pub async fn dependents_of_view(ctx: &SessionContext, name: &str) -> Vec<String> {
    readers(ctx, name, |deps| &deps.aliases).await
}

/// The registered views whose `deps` name `target`, sorted.
///
/// Asked of the providers, because a drop's report is about what is registered at the moment it
/// happens: a view is anything in the schema still carrying a plan
/// ([`TableProvider::get_logical_plan`](datafusion::catalog::TableProvider::get_logical_plan) is
/// `None` for every base table). That is the same walk [`plan_deps`] does for `ViewMeta`, run
/// against the same trees; the catalog pane's before-the-fact warning reads the store's recorded
/// copy of it, which is the same fact from before the drop.
async fn readers(
    ctx: &SessionContext,
    name: &str,
    deps: fn(&PlanDeps) -> &Vec<String>,
) -> Vec<String> {
    let Some(schema) = ctx.catalog(CATALOG).and_then(|c| c.schema(SCHEMA)) else {
        return Vec::new();
    };
    let target = fold_ident(name);
    let mut readers = Vec::new();
    // `table_names()` is the provider's own sorted key list, so the report's order is stable
    // without a sort of ours — and it already hides the result snapshots.
    for table in schema.table_names() {
        // Nothing is left invalid by its own drop, and a view cannot read itself — but an alias
        // shares a namespace with nothing, so `FROM orders AS v` inside `v` would otherwise
        // report `v` as its own reader.
        if fold_ident(&table) == target {
            continue;
        }
        let Ok(Some(provider)) = schema.table(&table).await else {
            continue;
        };
        let Some(plan) = provider.get_logical_plan() else {
            continue;
        };
        if deps(&plan_deps(&plan)).contains(&target) {
            readers.push(table);
        }
    }
    readers
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
pub(super) async fn table_meta(ctx: &SessionContext, name: &str) -> Result<TableMeta, String> {
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

/// What a chart may encode a column of this type as (`docs/CHART_SPEC.md` §3) — matched on
/// the `DataType` itself, here, because this is the last place that still has one: everything
/// downstream sees a [`ColumnInfo`], and a type's *spelling* is a rendering of a type rather
/// than the type.
///
/// The measure arm is [`DataType::is_numeric`] rather than a list of variants, because that is
/// the very predicate [`super::chart`]'s read gates a Y on — so the encoder cannot offer a
/// measure the read would then refuse. The two time arms together mirror the same module's
/// `positions`, which is what gives those columns a place on an axis; they are *two* because a
/// date stride bins a calendar instant and means nothing to a time of day (see
/// [`ChartRole::Instant`]). A dictionary is a **dimension**
/// whatever it encodes: it is a category by construction, and a dictionary of numbers is not a
/// measure the read accepts. Anything else — nested, binary, interval, a variant Arrow grows
/// later — is [`ChartRole::Other`] and is offered nowhere, which is the safe default in the
/// direction that matters.
pub fn chart_role(dt: &DataType) -> ChartRole {
    if dt.is_numeric() {
        return ChartRole::Measure;
    }
    match dt {
        DataType::Date32 | DataType::Date64 | DataType::Timestamp(_, _) => ChartRole::Instant,
        DataType::Time32(_) | DataType::Time64(_) => ChartRole::Clock,
        DataType::Utf8
        | DataType::LargeUtf8
        | DataType::Utf8View
        | DataType::Boolean
        | DataType::Dictionary(_, _) => ChartRole::Dimension,
        _ => ChartRole::Other,
    }
}

pub fn column_info(field: &Field) -> ColumnInfo {
    let dtype = short_type(field.data_type());
    ColumnInfo {
        name: field.name().clone(),
        kind: Kind::from_arrow(&dtype),
        role: chart_role(field.data_type()),
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

/// The type spelling every surface shows — the grid's header, the inspector, and the value tree's
/// rows (`value_tree::cell_children`). One function, so a node and its column cannot disagree.
///
/// The composite types are matched **by variant, not by their `Debug` text**, which matters more
/// than it looks: `DataType`'s `Debug` is *recursive*, so `format!("{dt:?}")` on a struct renders
/// its entire subtree just to have the first word taken off the front. On `config.json`'s
/// `contentBlocks` (19,311 keys, each a struct) that one call cost **18ms** — and `column_info`
/// makes it per field, all the way down. Every remaining variant's `Debug` is a single term, so
/// those still take the parameters off the front generically rather than being enumerated here.
pub(super) fn short_type(dt: &DataType) -> String {
    match dt {
        DataType::Struct(_) => "Struct".into(),
        DataType::List(_) | DataType::LargeList(_) | DataType::FixedSizeList(..) => "List".into(),
        DataType::ListView(_) => "ListView".into(),
        DataType::LargeListView(_) => "LargeListView".into(),
        DataType::Map(..) => "Map".into(),
        DataType::Union(..) => "Union".into(),
        DataType::Dictionary(..) => "Dictionary".into(),
        DataType::RunEndEncoded(..) => "RunEndEncoded".into(),
        DataType::LargeUtf8 => "Utf8".into(),
        leaf => {
            let full = format!("{leaf:?}");
            full.split(['(', '<']).next().unwrap_or(&full).to_string()
        }
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

    /// The message for a failure **nothing listed** — every case the sync mapper decides on its
    /// own. The listed case (a partitioned remote source, `holds_under_partitions`) is the MinIO
    /// test's, because it needs a store with objects in it to be a real answer.
    fn message(spec: &TableSpec, ext: &str, raw: &str) -> String {
        register_error(spec, ext, raw, None)
    }
    use strata_model::JsonRead;

    use super::*;

    /// Every `raw` below is a **measured** string: what `Engine::register` actually
    /// returned for that source on DataFusion 54. They are the point of these tests — the
    /// mapping keys off engine wording, so if an upgrade rewords a failure the arm stops
    /// matching and the message silently reverts to pass-through. A test holding the old
    /// wording is what catches that.
    fn spec(name: &str, paths: &[&str], format: &str) -> TableSpec {
        TableSpec {
            name: name.into(),
            paths: paths.iter().map(ToString::to_string).collect(),
            format: SourceFormat::from_name(format),
            partitions: Vec::new(),
            internal: false,
        }
    }

    /// **Namespaced by process, like every other scratch helper in this crate.**
    ///
    /// It wipes the directory on entry, so a path shared with another test process is one run
    /// deleting the fixtures another is mid-assertion over — which failed
    /// `an_unpartitioned_directory_finds_nothing_rather_than_guessing` once in a full
    /// `cargo test --workspace` while passing in isolation and in every other run of the same
    /// tree. A test that fails for reasons that are not about the code is worse than no test.
    fn tmp(sub: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "strata_register_error_tests_{}_{sub}",
            std::process::id()
        ));
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
        let paths: Vec<String> = paths.iter().map(ToString::to_string).collect();
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
            message(&spec("signups", &[], "json"), ".json", raw),
            "Cannot read 'signups' as JSON: a record does not end on its line. \
             Set the JSON shape to array in Table Config, or use newline-delimited JSON."
        );
    }

    #[test]
    fn a_top_level_array_never_carries_the_parsed_document() {
        let raw = "Arrow error: Json error: Expected JSON record to be an object, found Array \
                   [Object {\"a\": Number(1)}, Object {\"a\": Number(2)}]";
        let msg = message(&spec("signups", &[], "json"), ".json", raw);
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
            message(&spec("nums", &[], "json"), ".json", raw),
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
            message(&array_spec("nums"), ".json", raw),
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
            message(&spec("bad", &[], "json"), ".json", raw),
            "Cannot read 'bad' as JSON: key must be a string at line 1 column 9"
        );
    }

    /// The conflict main translated is now **read**, not reported. This is the replacement for
    /// `a_field_with_conflicting_types_is_named_as_a_schema_conflict`: rather than asserting the
    /// wording of an error, assert that the case producing it registers.
    /// (`engine::tests::a_polymorphic_json_field_registers_as_text_and_queries` is the end-to-end
    /// version, through `Engine::register` and a real `SELECT`.)
    #[test]
    fn a_field_with_conflicting_types_is_read_rather_than_refused() {
        use serde_json::json;
        let schema = crate::engine::json_poly::infer(
            [
                json!({"content": "plain"}),
                json!({"content": {"kind": "block"}}),
                json!({"content": ["a", true]}),
            ]
            .iter(),
        )
        .expect("a conflicted field no longer fails inference");
        assert_eq!(
            schema
                .field_with_name("content")
                .expect("content")
                .data_type(),
            &DataType::Utf8
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

    fn no_files_at_path(path: &Path) -> String {
        no_files_at(&path.display().to_string())
    }

    #[test]
    fn a_missing_path_says_so_rather_than_calling_it_empty() {
        let missing = tmp("missing").join("nope.parquet");
        let p = missing.display().to_string();
        assert_eq!(
            message(
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
            message(
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
            message(
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
            message(&s, ".csv", &no_files_at_path(&dir)),
            format!("No .csv files under '{p}' match the partition columns 'year'.")
        );
    }

    #[test]
    fn a_glob_only_claims_that_nothing_matched() {
        // Through `no_files_at`, so the glob is recovered by the same URL lookup the real
        // path takes — and the message quotes the user's own glob, not the URL.
        let g = format!("{}/**/*.parquet", tmp("glob").display());
        assert_eq!(
            message(&spec("t", &[&g], "parquet"), ".parquet", &no_files_at(&g)),
            format!("No files matched '{g}'.")
        );
    }

    /// **A remote source gets the local directory's three answers**, off a listing instead of a
    /// walk (W7 · 04). This is the arm around that listing: what the message says once the
    /// answer is in, or when there was none.
    ///
    /// The two that matter are the partitioned ones, and they are the same rule `holds_ext`
    /// follows for a directory — blame the columns unless the store *settled* that there is
    /// nothing there, because an unsettled answer over a lake big enough to exhaust the budget
    /// makes the columns the likelier story. `Some(true)` is what the MinIO test produces for
    /// real, against files sitting under `2024/` where the def asks for `year=`.
    #[test]
    fn a_partitioned_remote_source_blames_the_columns_when_the_store_found_files() {
        let url = "s3://acme-lake/events/";
        let mut s = spec("events", &[url], "csv");
        assert_eq!(
            register_error(&s, ".csv", &no_files_at(url), None),
            format!("No files matched '{url}'."),
            "an unpartitioned table earns no listing and has nothing more to say"
        );

        s.partitions = vec![
            ("year".into(), "Int32".into()),
            ("month".into(), "Int32".into()),
        ];
        let blamed =
            format!("No .csv files under '{url}' match the partition columns 'year', 'month'.");
        assert_eq!(
            register_error(&s, ".csv", &no_files_at(url), Some(true)),
            blamed,
            "the store found .csv files under the prefix, so the columns are what missed them"
        );
        assert_eq!(
            register_error(&s, ".csv", &no_files_at(url), None),
            blamed,
            "and an unsettled listing counts as 'do not claim emptiness', exactly as a walk does"
        );
        assert_eq!(
            register_error(&s, ".csv", &no_files_at(url), Some(false)),
            format!("No files matched '{url}'."),
            "but a prefix the store says is empty is not the columns' fault"
        );
    }

    /// A **glob** brings no listing — a pattern is not a place — so it keeps the one claim it can
    /// make even when the def is partitioned.
    #[test]
    fn a_partitioned_glob_still_only_claims_that_nothing_matched() {
        let g = "s3://acme-lake/events/**/*.csv";
        let mut s = spec("events", &[g], "csv");
        s.partitions = vec![("year".into(), "Int32".into())];
        assert_eq!(
            register_error(&s, ".csv", &no_files_at(g), None),
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
            message(
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
            message(&spec("events", &[], "parquet"), ".parquet", raw),
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
            message(&s, ".parquet", &no_files_at_path(&dir)),
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
            message(
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
        assert_eq!(message(&spec("t", &[], "parquet"), ".parquet", raw), raw);
    }

    /// **The fault this file's cap used to cause.** An unreachable bucket reports well past 240
    /// characters, and the clause naming the cause is the last one — so a cut at 240 kept the
    /// bookkeeping and threw away the answer, in the drawer and on the clipboard alike. Nothing
    /// is cut now; the wrappers come off instead.
    ///
    /// **The fixture is `object_store`'s own `Display` output, assembled from the crate rather
    /// than from prose about it** (`RetryError` in `client/retry.rs`, `Error::Generic` in
    /// `lib.rs`, both 0.13.2) — note the uppercase method, the URI, and the ` - ` before the
    /// cause. The first version of this test invented a plausible-looking string instead, which
    /// made it pass over a `readable` that stripped nothing.
    #[test]
    fn an_unreachable_bucket_keeps_the_clause_that_names_the_cause() {
        let raw = "External error: Object Store error: Generic S3 error: Error performing GET \
                   http://127.0.0.1:4566/lake/a.parquet in 5.383s, after 10 retries, \
                   max_retries: 10, retry_timeout: 180s  - HTTP error: error sending request for \
                   url (http://127.0.0.1:4566/lake/a.parquet): connection refused";
        assert!(raw.chars().count() > 240, "the case only bites when long");
        assert_eq!(
            message(&spec("t", &[], "parquet"), ".parquet", raw),
            "HTTP error: error sending request for url \
             (http://127.0.0.1:4566/lake/a.parquet): connection refused"
        );
    }

    /// A request that never had to be retried omits the retry clause entirely (`RetryError`
    /// writes it only when `retries != 0`), so the bookkeeping still has to come off the short
    /// form — this is the shape a first-attempt 403 or a refused connection takes.
    #[test]
    fn the_bookkeeping_comes_off_a_request_that_was_not_retried() {
        let raw = "Object Store error: Generic S3 error: Error performing GET \
                   http://127.0.0.1:4566/lake/a.parquet in 0.031s - Server returned non-2xx \
                   status code: 403 Forbidden: ";
        assert_eq!(
            message(&spec("t", &[], "parquet"), ".parquet", raw),
            "Server returned non-2xx status code: 403 Forbidden:"
        );
    }

    /// The store wrapper is a **format**, not a list, so a backend nobody enumerated unwraps too.
    /// `GCS` and `HTTP` are the two the enumerated version got wrong — one misspelt, one missing —
    /// and `HTTP client` is why the store name is not narrowed to a single token.
    #[test]
    fn any_backends_store_wrapper_comes_off() {
        for store in [
            "S3",
            "GCS",
            "HTTP",
            "HTTP client",
            "MicrosoftAzure",
            "LocalFileSystem",
            "Wasbs",
        ] {
            let raw = format!("Generic {store} error: something specific went wrong");
            assert_eq!(
                message(&spec("t", &[], "parquet"), ".parquet", &raw),
                "something specific went wrong",
                "the {store} wrapper"
            );
        }
    }

    /// …and the store name is bounded, so a message that merely opens with "Generic " keeps
    /// everything in front of its first ` error: ` rather than having it cut away.
    #[test]
    fn a_generic_opening_is_not_a_store_wrapper() {
        let raw = "Generic failure while reading the footer error: unexpected end of file";
        assert_eq!(message(&spec("t", &[], "parquet"), ".parquet", raw), raw);
    }

    /// A runaway message is passed through **whole**. Every surface that shows it wraps and
    /// scrolls, and its copy button exists so a message worth searching for can be pasted — both
    /// of which a cap defeats.
    #[test]
    fn a_runaway_message_is_not_cut() {
        let raw = "Internal error: ".to_string() + &"x".repeat(5_000);
        assert_eq!(message(&spec("t", &[], "parquet"), ".parquet", &raw), raw);
    }

    /// Peeling stops at the first thing it does not recognise, so a real message that happens to
    /// sit under one wrapper keeps all of itself.
    #[test]
    fn only_the_wrappers_come_off() {
        assert_eq!(
            message(
                &spec("t", &[], "parquet"),
                ".parquet",
                "Object Store error: Parquet error: Invalid Parquet file. Corrupt footer"
            ),
            "Parquet error: Invalid Parquet file. Corrupt footer"
        );
    }

    /// A message that is *only* wrappers has no tail to keep, so the raw line survives rather
    /// than the row going blank.
    #[test]
    fn a_message_that_is_all_wrapper_is_left_alone() {
        let raw = "External error: Object Store error:";
        assert_eq!(message(&spec("t", &[], "parquet"), ".parquet", raw), raw);
    }

    /// The retry clause is dropped only where `object_store` writes it — **leading**. ` - ` is
    /// ordinary punctuation, so a message that merely contains one keeps everything in front of
    /// it; without the prefix guard this cut would silently edit the user's own data out of a
    /// diagnosis.
    #[test]
    fn a_dash_mid_message_is_not_a_retry_wrapper() {
        let raw = "Arrow error: column 'order - id' is not nullable";
        assert_eq!(message(&spec("t", &[], "parquet"), ".parquet", raw), raw);
    }
}
