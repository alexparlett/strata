//! Catalog of DataFusion engine config keys surfaced in **Settings ▸ Engine ▸
//! Properties** (design24). A flat, datafusion-free list of known keys — name +
//! built-in default (as the string we store/apply) + a value [`Kind`] (for validation)
//! + a one-line description — used by the Properties editor's autocomplete, inspector,
//! and value validation, and by [`crate::engine`] to apply the overrides.
//!
//! It is **not** a whitelist: any `datafusion.*` key may be entered in the editor;
//! unknown keys are applied best-effort (DataFusion rejects the ones it doesn't know).
//! Overrides live in [`crate::config::Settings::engine`], a `name → value` map.
//!
//! `datafusion.runtime.*` keys reconfigure the `RuntimeEnv` (fixed at engine start), so
//! changing them requires an engine restart — see [`is_restart_key`]. Runtime keys are
//! *not* applied to the live `ConfigOptions` (so DataFusion never gets a chance to reject
//! a bad one until restart) — which is exactly why [`value_error`] validates values in the
//! editor, before Apply, rather than relying on the engine to complain.

use crate::util::{is_byte_size, is_duration, is_time_zone};
use std::collections::BTreeMap;

/// The value shape of a known key — drives editor-side validation ([`value_error`]).
/// Deliberately lenient: DataFusion does the final, authoritative validation when the
/// value is applied; this only catches the clearly-wrong so Apply can flag it inline.
#[derive(Clone, Copy)]
pub enum Kind {
    /// Free-form string — not validated (names, formats, paths, codecs).
    Text,
    /// `true` or `false` (lower-case, as DataFusion parses it).
    Bool,
    /// A whole number `>= min`.
    Int { min: i64 },
    /// A byte size: a number with an optional K/M/G/T (i)(B) suffix, or blank.
    Bytes,
    /// A duration: a number with an optional s/m/h suffix, or blank.
    Duration,
    /// A time zone: a `±HH:MM` offset (e.g. `+00:00`) or a named zone.
    TimeZone,
    /// One of a fixed set of values (compared case-insensitively).
    Enum(&'static [&'static str]),
}

/// One known engine config key: DataFusion name, built-in default (string form), value
/// [`Kind`], and a one-line description.
pub struct EngineKey {
    pub key: &'static str,
    pub default: &'static str,
    pub kind: Kind,
    pub desc: &'static str,
}

/// The curated catalog — the DataFusion `ConfigOptions` + `runtime.*` keys we document,
/// in display order (grouped by namespace).
pub const ENGINE_KEYS: &[EngineKey] = &[
    // catalog — `default_catalog` / `default_schema` are deliberately absent: the app
    // owns those names, see `is_owned_key`.
    EngineKey { key: "datafusion.catalog.information_schema", default: "false", kind: Kind::Bool, desc: "Expose information_schema virtual tables for schema introspection." },
    EngineKey { key: "datafusion.catalog.newlines_in_values", default: "false", kind: Kind::Bool, desc: "Allow newlines inside quoted CSV values (may reduce scan performance)." },
    // execution
    EngineKey { key: "datafusion.execution.batch_size", default: "8192", kind: Kind::Int { min: 1 }, desc: "Rows per in-memory batch. Lower under tight memory; raise for vectorized throughput." },
    EngineKey { key: "datafusion.execution.target_partitions", default: "0", kind: Kind::Int { min: 0 }, desc: "Parallelism for execution. 0 = one partition per CPU core; 1 for tiny data." },
    EngineKey { key: "datafusion.execution.time_zone", default: "+00:00", kind: Kind::TimeZone, desc: "Session time zone used by now() and timestamp functions." },
    EngineKey { key: "datafusion.execution.coalesce_batches", default: "true", kind: Kind::Bool, desc: "Coalesce small batches between operators into larger ones." },
    EngineKey { key: "datafusion.execution.collect_statistics", default: "true", kind: Kind::Bool, desc: "Collect statistics when a table is first created." },
    EngineKey { key: "datafusion.execution.planning_concurrency", default: "0", kind: Kind::Int { min: 0 }, desc: "Fan-out during physical planning. 0 = number of CPU cores." },
    EngineKey { key: "datafusion.execution.spill_compression", default: "uncompressed", kind: Kind::Enum(&["uncompressed", "lz4_frame", "zstd"]), desc: "Codec for spill files: uncompressed, lz4_frame, or zstd." },
    EngineKey { key: "datafusion.execution.sort_spill_reservation_bytes", default: "10485760", kind: Kind::Int { min: 0 }, desc: "Memory reserved for each spillable sort's in-memory merge." },
    EngineKey { key: "datafusion.execution.sort_in_place_threshold_bytes", default: "1048576", kind: Kind::Int { min: 0 }, desc: "Below this size, sort in a single RecordBatch rather than merging." },
    EngineKey { key: "datafusion.execution.meta_fetch_concurrency", default: "32", kind: Kind::Int { min: 1 }, desc: "Files read in parallel when inferring schema and statistics." },
    EngineKey { key: "datafusion.execution.enable_recursive_ctes", default: "true", kind: Kind::Bool, desc: "Support recursive common table expressions." },
    EngineKey { key: "datafusion.execution.keep_partition_by_columns", default: "false", kind: Kind::Bool, desc: "Keep partition_by columns in the output RecordBatches." },
    EngineKey { key: "datafusion.execution.parquet.pruning", default: "true", kind: Kind::Bool, desc: "Skip row groups using predicate + min/max metadata." },
    EngineKey { key: "datafusion.execution.parquet.enable_page_index", default: "true", kind: Kind::Bool, desc: "Use the Parquet Page Index to reduce I/O and rows decoded." },
    EngineKey { key: "datafusion.execution.parquet.pushdown_filters", default: "false", kind: Kind::Bool, desc: "Apply filters during Parquet decoding (late materialization)." },
    EngineKey { key: "datafusion.execution.parquet.reorder_filters", default: "false", kind: Kind::Bool, desc: "Reorder pushed-down filters heuristically to cut evaluation cost." },
    EngineKey { key: "datafusion.execution.parquet.metadata_size_hint", default: "524288", kind: Kind::Int { min: 0 }, desc: "Bytes fetched optimistically for the Parquet footer + metadata." },
    EngineKey { key: "datafusion.execution.parquet.compression", default: "zstd(3)", kind: Kind::Text, desc: "(writing) Default codec: snappy, gzip(level), zstd(level), lz4, lz4_raw." },
    EngineKey { key: "datafusion.execution.parquet.max_row_group_size", default: "1048576", kind: Kind::Int { min: 1 }, desc: "(writing) Target max rows per row group." },
    EngineKey { key: "datafusion.execution.parquet.statistics_enabled", default: "page", kind: Kind::Enum(&["none", "chunk", "page"]), desc: "(writing) Statistics level: none, chunk, or page." },
    // optimizer
    EngineKey { key: "datafusion.optimizer.prefer_hash_join", default: "true", kind: Kind::Bool, desc: "Prefer HashJoin over SortMergeJoin (faster, more memory)." },
    EngineKey { key: "datafusion.optimizer.repartition_joins", default: "true", kind: Kind::Bool, desc: "Repartition on join keys to run joins in parallel." },
    EngineKey { key: "datafusion.optimizer.repartition_aggregations", default: "true", kind: Kind::Bool, desc: "Repartition on aggregate keys to run aggregates in parallel." },
    EngineKey { key: "datafusion.optimizer.repartition_sorts", default: "true", kind: Kind::Bool, desc: "Sort per-partition then merge, rather than coalescing first." },
    EngineKey { key: "datafusion.optimizer.repartition_file_scans", default: "true", kind: Kind::Bool, desc: "Repartition data-source partitions for maximum parallelism." },
    EngineKey { key: "datafusion.optimizer.repartition_file_min_size", default: "10485760", kind: Kind::Int { min: 0 }, desc: "Minimum total file size (bytes) to repartition a file scan." },
    EngineKey { key: "datafusion.optimizer.enable_round_robin_repartition", default: "true", kind: Kind::Bool, desc: "Add round-robin repartitioning to use more CPU cores." },
    EngineKey { key: "datafusion.optimizer.enable_topk_aggregation", default: "true", kind: Kind::Bool, desc: "Perform TopK during aggregations where possible." },
    EngineKey { key: "datafusion.optimizer.enable_dynamic_filter_pushdown", default: "true", kind: Kind::Bool, desc: "Push operator-generated dynamic filters into the scan phase." },
    EngineKey { key: "datafusion.optimizer.skip_failed_rules", default: "false", kind: Kind::Bool, desc: "Warn and continue when an optimizer rule errors, instead of failing." },
    EngineKey { key: "datafusion.optimizer.max_passes", default: "3", kind: Kind::Int { min: 1 }, desc: "How many times the optimizer re-runs over the plan." },
    EngineKey { key: "datafusion.optimizer.default_filter_selectivity", default: "20", kind: Kind::Int { min: 0 }, desc: "Default filter selectivity (0-100) when none can be determined." },
    EngineKey { key: "datafusion.optimizer.hash_join_single_partition_threshold", default: "1048576", kind: Kind::Int { min: 0 }, desc: "Max bytes of one HashJoin side collected into a single partition." },
    // explain
    EngineKey { key: "datafusion.explain.logical_plan_only", default: "false", kind: Kind::Bool, desc: "EXPLAIN prints only the logical plan." },
    EngineKey { key: "datafusion.explain.physical_plan_only", default: "false", kind: Kind::Bool, desc: "EXPLAIN prints only the physical plan." },
    EngineKey { key: "datafusion.explain.show_statistics", default: "false", kind: Kind::Bool, desc: "EXPLAIN prints operator statistics for physical plans." },
    EngineKey { key: "datafusion.explain.show_sizes", default: "true", kind: Kind::Bool, desc: "EXPLAIN prints partition sizes." },
    EngineKey { key: "datafusion.explain.format", default: "indent", kind: Kind::Enum(&["indent", "tree"]), desc: "EXPLAIN display format: indent or tree." },
    // sql_parser
    EngineKey { key: "datafusion.sql_parser.dialect", default: "generic", kind: Kind::Text, desc: "Parser dialect: generic, postgresql, mysql, sqlite, duckdb, snowflake, bigquery, ansi." },
    EngineKey { key: "datafusion.sql_parser.default_null_ordering", default: "nulls_max", kind: Kind::Enum(&["nulls_max", "nulls_min", "nulls_first", "nulls_last"]), desc: "Default NULL ordering: nulls_max, nulls_min, nulls_first, nulls_last." },
    EngineKey { key: "datafusion.sql_parser.enable_ident_normalization", default: "true", kind: Kind::Bool, desc: "Lower-case unquoted identifiers." },
    EngineKey { key: "datafusion.sql_parser.parse_float_as_decimal", default: "false", kind: Kind::Bool, desc: "Parse float literals as DECIMAL." },
    EngineKey { key: "datafusion.sql_parser.support_varchar_with_length", default: "true", kind: Kind::Bool, desc: "Permit VARCHAR(n) lengths (ignored) instead of erroring." },
    EngineKey { key: "datafusion.sql_parser.map_string_types_to_utf8view", default: "true", kind: Kind::Bool, desc: "Map string types to Utf8View during planning." },
    EngineKey { key: "datafusion.sql_parser.recursion_limit", default: "50", kind: Kind::Int { min: 1 }, desc: "Recursion depth limit when parsing complex SQL." },
    // format
    EngineKey { key: "datafusion.format.null", default: "NULL", kind: Kind::Text, desc: "How NULL values render in the results grid." },
    EngineKey { key: "datafusion.format.date_format", default: "%Y-%m-%d", kind: Kind::Text, desc: "strftime pattern for Date columns." },
    EngineKey { key: "datafusion.format.datetime_format", default: "%Y-%m-%dT%H:%M:%S%.f", kind: Kind::Text, desc: "strftime pattern for DateTime columns." },
    EngineKey { key: "datafusion.format.timestamp_format", default: "%Y-%m-%dT%H:%M:%S%.f", kind: Kind::Text, desc: "strftime pattern for Timestamp columns." },
    EngineKey { key: "datafusion.format.time_format", default: "%H:%M:%S%.f", kind: Kind::Text, desc: "strftime pattern for Time columns." },
    EngineKey { key: "datafusion.format.duration_format", default: "pretty", kind: Kind::Enum(&["pretty", "iso8601"]), desc: "Duration rendering: pretty or ISO8601." },
    // runtime (restart-required — configure the RuntimeEnv, fixed at engine start)
    EngineKey { key: "datafusion.runtime.memory_limit", default: "", kind: Kind::Bytes, desc: "Cap on execution memory before spilling. Suffixes K/M/G, e.g. 2G. Blank = unlimited." },
    EngineKey { key: "datafusion.runtime.max_temp_directory_size", default: "100G", kind: Kind::Bytes, desc: "Ceiling on temporary spill files on disk. Suffixes K/M/G." },
    EngineKey { key: "datafusion.runtime.temp_directory", default: "", kind: Kind::Text, desc: "Path to the temporary spill directory." },
    EngineKey { key: "datafusion.runtime.metadata_cache_limit", default: "50M", kind: Kind::Bytes, desc: "Memory for the file-metadata cache (e.g. Parquet metadata)." },
    EngineKey { key: "datafusion.runtime.list_files_cache_limit", default: "1M", kind: Kind::Bytes, desc: "Memory for the list-files cache. Suffixes K/M/G." },
    EngineKey { key: "datafusion.runtime.list_files_cache_ttl", default: "", kind: Kind::Duration, desc: "TTL of list-files cache entries. Units m/s, e.g. 2m." },
];

/// Look up a known key's metadata, or `None` for a custom key.
pub fn key_def(name: &str) -> Option<&'static EngineKey> {
    let name = name.trim();
    ENGINE_KEYS.iter().find(|e| e.key == name)
}

/// Whether `name` is a runtime-level key — these reconfigure the `RuntimeEnv` (fixed at
/// engine start), so changing them requires a restart.
pub fn is_restart_key(name: &str) -> bool {
    name.trim().starts_with("datafusion.runtime.")
}

/// Whether `name` is a key the **app** owns and config may never set: the catalog and
/// schema our tables live in (`crate::engine`'s `CATALOG` / `SCHEMA`).
///
/// `RefreshCatalog` looks tables up by those names, so an override would hide the whole
/// catalog from it — and since the live-apply path re-asserts every override *over*
/// whatever the `SessionConfig` builder set, naming them at build time doesn't hold on
/// its own; they have to be skipped here too. They're absent from [`ENGINE_KEYS`], so
/// they can now only arrive as a hand-typed custom key or a stale saved override — both
/// are skipped on apply and flagged by [`value_error`].
pub fn is_owned_key(name: &str) -> bool {
    matches!(
        name.trim(),
        "datafusion.catalog.default_catalog"
            | "datafusion.catalog.default_schema"
            // Planner source spans feed the editor's diagnostics (P2-18).
            | "datafusion.sql_parser.collect_spans"
    )
}

/// The effective value for `key` given the overrides map — the override if present, else
/// the known built-in default. `None` for a custom key with no override.
pub fn effective(overrides: &BTreeMap<String, String>, key: &str) -> Option<String> {
    if let Some(v) = overrides.get(key) {
        return Some(v.clone());
    }
    key_def(key).map(|e| e.default.to_string())
}

/// Validate `value` against the known `key`'s [`Kind`]. Returns an error message for a
/// clearly-invalid value, or `None` if it's acceptable — including a blank value (which
/// means "unset / use the default") and any custom (non-catalog) key. Lenient by design:
/// DataFusion does the final validation when the value is applied.
pub fn value_error(key: &str, value: &str) -> Option<String> {
    // Reserved regardless of value — better to say so than to accept it and ignore it.
    if is_owned_key(key) {
        return Some("Reserved — Strata names the catalog and schema itself.".to_string());
    }
    let v = value.trim();
    if v.is_empty() {
        return None;
    }
    match key_def(key)?.kind {
        Kind::Text => None,
        Kind::Bool => (v != "true" && v != "false").then(|| "Expected true or false.".to_string()),
        Kind::Int { min } => match v.parse::<i64>() {
            Ok(n) if n >= min => None,
            Ok(_) if min == 0 => Some("Expected a non-negative whole number.".to_string()),
            Ok(_) if min == 1 => Some("Expected a positive whole number.".to_string()),
            Ok(_) => Some(format!("Expected a whole number ≥ {min}.")),
            Err(_) => Some("Expected a whole number.".to_string()),
        },
        Kind::Bytes => (!is_byte_size(v))
            .then(|| "Expected a size like 512M, 2G, or a number of bytes.".to_string()),
        Kind::Duration => {
            (!is_duration(v)).then(|| "Expected a duration like 30s or 2m.".to_string())
        }
        Kind::TimeZone => (!is_time_zone(v))
            .then(|| "Expected a ±HH:MM offset (e.g. +00:00) or a named zone.".to_string()),
        Kind::Enum(opts) => (!opts.iter().any(|o| o.eq_ignore_ascii_case(v)))
            .then(|| format!("Expected one of: {}.", opts.join(", "))),
    }
}

