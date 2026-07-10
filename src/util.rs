//! Small shared helpers: column-type classification/colours, name derivation,
//! byte formatting.

use std::collections::BTreeSet;
use std::path::Path;

/// The visual "kind" of a column, driving dot/type/cell colours (matches the
/// Strata type→colour map).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Str,
    Num,
    Bool,
    Ts,
    Struct,
    List,
    Map,
}

impl Kind {
    /// Infer a kind from an Arrow `DataType` debug string (e.g. "Int64",
    /// "Utf8", "Timestamp(...)", "Struct(...)", "List(...)", "Map(...)").
    pub fn from_arrow(dtype: &str) -> Kind {
        let d = dtype;
        if d.starts_with("Struct") {
            Kind::Struct
        } else if d.starts_with("List")
            || d.starts_with("LargeList")
            || d.starts_with("FixedSizeList")
        {
            Kind::List
        } else if d.starts_with("Map") {
            Kind::Map
        } else if d.starts_with("Boolean") {
            Kind::Bool
        } else if d.starts_with("Timestamp") || d.starts_with("Date") || d.starts_with("Time") {
            Kind::Ts
        } else if d.starts_with("Int")
            || d.starts_with("UInt")
            || d.starts_with("Float")
            || d.starts_with("Decimal")
        {
            Kind::Num
        } else {
            Kind::Str
        }
    }

    /// CSS class for the small square dot (`d-num`, ...).
    pub fn dot_class(self) -> &'static str {
        match self {
            Kind::Str => "d-str",
            Kind::Num => "d-num",
            Kind::Bool => "d-bool",
            Kind::Ts => "d-ts",
            Kind::Struct => "d-struct",
            Kind::List => "d-list",
            Kind::Map => "d-map",
        }
    }

    /// CSS colour for the type swatch/dot (`var(--t-num)`, ...) — for the `Dot` component's
    /// `color` prop (inline fill, so it beats the base dot styling).
    pub fn dot_color(self) -> &'static str {
        match self {
            Kind::Str => "var(--t-str)",
            Kind::Num => "var(--t-num)",
            Kind::Bool => "var(--t-bool)",
            Kind::Ts => "var(--t-ts)",
            Kind::Struct => "var(--t-struct)",
            Kind::List => "var(--t-list)",
            Kind::Map => "var(--t-map)",
        }
    }

    /// CSS class for coloured type text (`t-num`, ...).
    pub fn text_class(self) -> &'static str {
        match self {
            Kind::Str => "t-str",
            Kind::Num => "t-num",
            Kind::Bool => "t-bool",
            Kind::Ts => "t-ts",
            Kind::Struct => "t-struct",
            Kind::List => "t-list",
            Kind::Map => "t-map",
        }
    }

    /// Extra CSS class for a result cell (`num`/`bool`/`ts`/`nested`), if any.
    pub fn cell_class(self) -> &'static str {
        match self {
            Kind::Num => "num",
            Kind::Bool => "bool",
            Kind::Ts => "ts",
            Kind::Struct | Kind::List | Kind::Map => "nested",
            Kind::Str => "",
        }
    }

    pub fn is_nested(self) -> bool {
        matches!(self, Kind::Struct | Kind::List | Kind::Map)
    }
}

/// A stable FNV-1a hash of the **trimmed** SQL — the tab dirty-tracking baseline.
/// Cheaper than storing/comparing whole strings, and deterministic across runs so
/// a persisted baseline still matches after reload.
pub fn sql_hash(sql: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in sql.trim().as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Human-readable byte size (e.g. `1.4 MB`).
pub fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut f = n as f64;
    let mut i = 0;
    while f >= 1024.0 && i < UNITS.len() - 1 {
        f /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else {
        format!("{f:.1} {}", UNITS[i])
    }
}

/// Turn a file/dir name into a valid, unique lower_snake SQL identifier.
pub fn derive_table_name(path: &Path, existing: &BTreeSet<String>) -> String {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("table");
    let mut base: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    if base.is_empty() {
        base = "table".into();
    }
    if base.chars().next().map_or(true, |c| c.is_ascii_digit()) {
        base = format!("t_{base}");
    }
    let mut name = base.clone();
    let mut i = 2;
    while existing.contains(&name) {
        name = format!("{base}_{i}");
        i += 1;
    }
    name
}
