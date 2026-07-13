//! Catalog of the curated DataFusion engine options surfaced in **Settings ▸ Engine**
//! (W2). Pure metadata — no `datafusion` types — so both the Settings UI (which
//! renders the rows) and [`crate::engine`] (which applies them) depend on it without
//! pulling datafusion into the UI layer.
//!
//! Values are stored as strings in [`crate::config::Settings::engine`], a
//! `BTreeMap<key, value>` that holds **only overrides** — a key absent from the map
//! means "use the built-in default". Setting a value back to its default clears the
//! key (see [`set_override`]), so the MODIFIED badge = "key present in the map".
//!
//! Which keys apply how (DataFusion 54): the nine `execution.*` / `sql_parser.*` /
//! `format.*` / `optimizer.*` keys are real `ConfigOptions` entries (live-settable);
//! the two `datafusion.runtime.*` keys live on the `RuntimeEnv` and only take effect
//! when the engine is (re)built. `crate::engine` owns that split.

use std::collections::BTreeMap;

/// How an option is edited and displayed.
#[derive(Clone, Copy, PartialEq)]
pub enum EngineKind {
    /// Integer, rendered as a `NumberStepper` (numeric-only input + ± buttons).
    /// `min`/`max` clamp; `step` sizes the buttons.
    Int {
        min: i64,
        max: Option<i64>,
        step: i64,
    },
    /// Free text. `placeholder` hints the field; `empty` (if any) is the label shown
    /// when blank; `allow_empty` keeps an explicit blank as a real override.
    Text {
        placeholder: &'static str,
        empty: Option<&'static str>,
        allow_empty: bool,
    },
    Bool,
    Enum(&'static [&'static str]),
}

/// One engine option's metadata.
pub struct EngineOption {
    pub key: &'static str,
    pub group: &'static str,
    pub label: &'static str,
    pub desc: &'static str,
    pub kind: EngineKind,
    /// The built-in default, as the string we store/apply.
    pub default: &'static str,
}

/// The curated set, in display order — grouped by `group`, groups in first-seen order.
pub const OPTIONS: &[EngineOption] = &[
    EngineOption {
        key: "datafusion.execution.target_partitions",
        group: "Execution",
        label: "Target partitions",
        desc: "Parallelism for query execution. 0 uses one partition per CPU core; set to 1 for tiny data to skip repartition overhead.",
        kind: EngineKind::Int {
            min: 0,
            max: None,
            step: 1,
        },
        default: "0",
    },
    EngineOption {
        key: "datafusion.execution.batch_size",
        group: "Execution",
        label: "Batch size",
        desc: "Rows per in-memory batch. Lower it under a tight memory limit; raise it for vectorized throughput on wide scans.",
        kind: EngineKind::Int {
            min: 1,
            max: None,
            step: 512,
        },
        default: "8192",
    },
    EngineOption {
        key: "datafusion.execution.time_zone",
        group: "Execution",
        label: "Session time zone",
        desc: "Time zone used by now() and timestamp functions.",
        kind: EngineKind::Text {
            placeholder: "+00:00",
            empty: None,
            allow_empty: false,
        },
        default: "+00:00",
    },
    EngineOption {
        key: "datafusion.runtime.memory_limit",
        group: "Memory & spill",
        label: "Memory limit",
        desc: "Cap on execution memory before spilling to disk. Suffixes K / M / G, e.g. 2G. Blank means unlimited.",
        kind: EngineKind::Text {
            placeholder: "unlimited",
            empty: Some("unlimited"),
            allow_empty: true,
        },
        default: "",
    },
    EngineOption {
        key: "datafusion.runtime.max_temp_directory_size",
        group: "Memory & spill",
        label: "Max spill directory size",
        desc: "Ceiling on temporary spill files on disk. Suffixes K / M / G.",
        kind: EngineKind::Text {
            placeholder: "100G",
            empty: None,
            allow_empty: false,
        },
        default: "100G",
    },
    EngineOption {
        key: "datafusion.sql_parser.dialect",
        group: "SQL parser",
        label: "SQL dialect",
        desc: "Parser dialect — affects identifier quoting, functions and cast behaviour.",
        kind: EngineKind::Enum(&[
            "generic",
            "postgresql",
            "mysql",
            "sqlite",
            "duckdb",
            "snowflake",
            "bigquery",
            "ansi",
        ]),
        default: "generic",
    },
    EngineOption {
        key: "datafusion.sql_parser.default_null_ordering",
        group: "SQL parser",
        label: "Default NULL ordering",
        desc: "Where NULLs land when ORDER BY doesn't specify. nulls_max matches Postgres.",
        kind: EngineKind::Enum(&["nulls_max", "nulls_min", "nulls_first", "nulls_last"]),
        default: "nulls_max",
    },
    EngineOption {
        key: "datafusion.format.null",
        group: "Result format",
        label: "NULL display",
        desc: "How NULL values render in the results grid.",
        kind: EngineKind::Text {
            placeholder: "(empty)",
            empty: None,
            allow_empty: true,
        },
        default: "NULL",
    },
    EngineOption {
        key: "datafusion.format.date_format",
        group: "Result format",
        label: "Date format",
        desc: "strftime pattern applied to Date columns.",
        kind: EngineKind::Text {
            placeholder: "%Y-%m-%d",
            empty: None,
            allow_empty: false,
        },
        default: "%Y-%m-%d",
    },
    EngineOption {
        key: "datafusion.format.timestamp_format",
        group: "Result format",
        label: "Timestamp format",
        desc: "strftime pattern applied to Timestamp columns.",
        kind: EngineKind::Text {
            placeholder: "%Y-%m-%dT%H:%M:%S%.f",
            empty: None,
            allow_empty: false,
        },
        default: "%Y-%m-%dT%H:%M:%S%.f",
    },
    EngineOption {
        key: "datafusion.optimizer.prefer_hash_join",
        group: "Optimizer",
        label: "Prefer hash join",
        desc: "Favour HashJoin over SortMergeJoin. Faster when memory allows; disable to prefer the more memory-efficient sort-merge join.",
        kind: EngineKind::Bool,
        default: "true",
    },
];

/// Look up an option's metadata by key.
pub fn option(key: &str) -> Option<&'static EngineOption> {
    OPTIONS.iter().find(|o| o.key == key)
}

/// The effective value for `key` given the overrides map — the override if present,
/// else the built-in default. `None` if `key` isn't a known option.
pub fn effective(overrides: &BTreeMap<String, String>, key: &str) -> Option<String> {
    let opt = option(key)?;
    Some(
        overrides
            .get(key)
            .cloned()
            .unwrap_or_else(|| opt.default.to_string()),
    )
}

/// Set `key` to `value` in `overrides`, clearing it when `value` equals the built-in
/// default — the design's "== default → not an override" rule. Unknown keys ignored.
pub fn set_override(overrides: &mut BTreeMap<String, String>, key: &str, value: String) {
    match option(key) {
        Some(opt) if value == opt.default => {
            overrides.remove(key);
        }
        Some(_) => {
            overrides.insert(key.to_string(), value);
        }
        None => {}
    }
}

/// Validate a candidate `value` for `key` before it's committed to the overrides.
/// `Ok(())` = acceptable; `Err(msg)` is a short user-facing reason. A blank is always
/// allowed (means "use the default"); free-text options (time zone, strftime formats,
/// NULL display) accept anything; ints, runtime capacities and enums are checked.
pub fn validate(key: &str, value: &str) -> Result<(), String> {
    let Some(opt) = option(key) else {
        return Ok(());
    };
    let v = value.trim();
    if v.is_empty() {
        return Ok(());
    }
    // The runtime capacities take a size string (2G / 512M / a byte count).
    if key == "datafusion.runtime.memory_limit"
        || key == "datafusion.runtime.max_temp_directory_size"
    {
        return validate_size(v);
    }
    match opt.kind {
        EngineKind::Int { .. } => v
            .parse::<u64>()
            .map(|_| ())
            .map_err(|_| "Enter a whole number (0 or more).".to_string()),
        EngineKind::Enum(choices) => {
            if choices.contains(&v) {
                Ok(())
            } else {
                Err("Choose one of the listed options.".to_string())
            }
        }
        EngineKind::Text { .. } | EngineKind::Bool => Ok(()),
    }
}

/// A capacity string: a non-negative number with an optional `K` / `M` / `G` suffix.
fn validate_size(v: &str) -> Result<(), String> {
    let num = v.strip_suffix(|c: char| "KMGkmg".contains(c)).unwrap_or(v);
    match num.trim().parse::<f64>() {
        Ok(n) if n >= 0.0 => Ok(()),
        _ => Err("Use a size like 2G, 512M, or a plain byte count.".to_string()),
    }
}

/// Groups in display order, each carrying its options (preserves [`OPTIONS`] order).
pub fn groups() -> Vec<(&'static str, Vec<&'static EngineOption>)> {
    let mut out: Vec<(&'static str, Vec<&'static EngineOption>)> = Vec::new();
    for o in OPTIONS {
        if let Some(g) = out.iter_mut().find(|(name, _)| *name == o.group) {
            g.1.push(o);
        } else {
            out.push((o.group, vec![o]));
        }
    }
    out
}
