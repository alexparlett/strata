//! Catalog references and row descriptors: which [`CatalogKind`] section a row is in,
//! what a pending removal ([`RemoveKind`] / [`RemoveTarget`]) targets, and a [`ColRef`]
//! that names one column. Also the persisted **catalog definitions** ([`TableDef`] /
//! [`ViewDef`] / [`SavedQuery`]) — exactly what `.strata/project.json` stores, nothing
//! more. What registration *learns* about a def (columns, row counts, status, profiles)
//! is runtime state and lives with the UI's project store, wrapped around these — not
//! here as skipped fields.

use serde::{Deserialize, Deserializer, Serialize};
use uuid::Uuid;

/// What a pending removal targets — drives the confirm dialog's wording and the
/// engine command sent on confirm.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RemoveKind {
    Table,
    View,
}

#[derive(Clone)]
pub struct RemoveTarget {
    pub kind: RemoveKind,
    pub name: String,
}

/// Which catalog section a right-clicked row belongs to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CatalogKind {
    Table,
    View,
    Query,
}

/// A reference to one column in the catalog — **what kind of thing owns it, its owner's
/// name, and its path within it**. Each part earns its place:
///
/// - **kind** — tables and views are separate collections. Without it, resolving a
///   reference means searching both and hoping the name only lands in one.
/// - **path**, not a name — `["address", "city"]`. A name alone can't say *which* `city`,
///   the top-level one or the one inside `address`, and the sidebar renders both. Keying
///   by name meant a nested column resolved to an unrelated top-level one.
///
/// A struct rather than a `"view::orders.address.city"` URN for the same reason the path
/// is a `Vec`: names come from the user's files and may contain dots, `::`, or anything
/// else. A string that has to be parsed back is a bug waiting to be rediscovered (cf.
/// `ident` vs `col` in [`crate::profile`]).
#[derive(Clone, PartialEq, Debug)]
pub struct ColRef {
    /// `Table` or `View` — says which collection owns it, so resolving is one lookup.
    pub kind: CatalogKind,
    /// The owning table or view.
    pub owner: String,
    /// Path within the owner. A top-level column is a one-segment path.
    pub path: Vec<String>,
}

impl ColRef {
    /// A nested *field* — a struct's child. A position, not a type: a top-level column
    /// whose type is a struct is not one.
    pub fn is_child(&self) -> bool {
        self.path.len() > 1
    }

    /// The leaf's own name. The path is how it's found, not what it's called.
    pub fn name(&self) -> &str {
        self.path.last().map(String::as_str).unwrap_or_default()
    }
}

// ---------------------------------------------------------------------------
// Catalog definitions (persisted to `.strata/project.json`)
// ---------------------------------------------------------------------------

/// Accept partition columns as either the legacy name-only `["year","month"]`
/// (→ typed `Utf8`) or the current typed `[["year","Int32"], …]` form, so old project
/// files keep loading. Serialization always emits the typed form.
fn de_partition_cols<'de, D>(d: D) -> Result<Vec<(String, String)>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Col {
        Named(String),
        Typed(String, String),
    }
    Ok(Vec::<Col>::deserialize(d)?
        .into_iter()
        .map(|c| match c {
            Col::Named(n) => (n, "Utf8".to_string()),
            Col::Typed(n, t) => (n, t),
        })
        .collect())
}

// ---------------------------------------------------------------------------
// Source format + its read options
// ---------------------------------------------------------------------------

/// Whole-file compression wrapping a text source. Read-effective for CSV and JSON; parquet and
/// Arrow carry their compression *inside* the file, so neither offers this.
///
/// The extension matters as much as the codec: a gzipped CSV is `events.csv.gz`, and a listing
/// filtered on `.csv` matches none of them — see [`SourceFormat::extension`].
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum FileCompression {
    #[default]
    None,
    Gzip,
    Bzip2,
    Xz,
    Zstd,
}

impl FileCompression {
    pub const ALL: [FileCompression; 5] =
        [Self::None, Self::Gzip, Self::Bzip2, Self::Xz, Self::Zstd];

    /// What this codec adds to the file name — DataFusion's own suffixes.
    pub fn extension(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Gzip => ".gz",
            Self::Bzip2 => ".bz2",
            Self::Xz => ".xz",
            Self::Zstd => ".zst",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Gzip => "gzip",
            Self::Bzip2 => "bzip2",
            Self::Xz => "xz",
            Self::Zstd => "zstd",
        }
    }
}

/// Which JSON layout the file is in.
///
/// **Both are readable** (DataFusion 54's `JsonFormat::with_newline_delimited`), which is why
/// this is an option rather than a rule the reader enforces. One caveat rides with
/// [`Array`](Self::Array): DataFusion cannot range-split such a file, and `JsonSource` does not
/// declare that, so a file over `datafusion.optimizer.repartition_file_min_size` (10 MB) fails
/// its *scan* with a `NotImplemented`. Loud, self-describing, and only above that size.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "snake_case")]
pub enum JsonShape {
    /// One record per line (NDJSON) — DataFusion's default.
    #[default]
    NewlineDelimited,
    /// One whole-document array: `[{…},{…}]`.
    Array,
}

/// How a CSV source is read.
///
/// **Every field here reaches the read path**, in both halves of it — inference and scan. That
/// is the bar, and it excluded three options that look available and are not:
///
/// - `null_regex` (a "NULL value" text) is wired into `CsvFormat`'s *inference* only;
///   `CsvSource::builder` never puts it on the reader. Setting it re-types a column and then
///   fails the scan parsing the very token it was told was null — strictly worse than leaving
///   it off, where the column simply infers as text.
/// - `terminator` is the mirror image: wired at scan, absent from inference, so the schema and
///   the rows would be read by different rules.
/// - `double_quote`, `quote_style`, `null_value`, the date/time formats, the whitespace pair
///   and `compression_level` are the **writer's**; no read path references them.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct CsvRead {
    /// The first row holds column names.
    pub header: bool,
    pub delimiter: char,
    pub quote: char,
    /// Escapes a quote inside a quoted field. Absent = none.
    pub escape: Option<char>,
    /// Lines starting with this are skipped. Absent = none.
    pub comment: Option<char>,
    /// Allow line breaks inside quoted fields. Costs the parallel file split
    /// (`CsvSource::supports_repartitioning`), which is why it is off by default.
    pub newlines_in_values: bool,
    /// Read files whose rows (or whose *schemas*, across paths) have different column counts:
    /// the union of the columns found, padded with nulls. Without it a ragged file — or one
    /// path with a column the others lack — fails the register outright.
    pub truncated_rows: bool,
    /// Rows scanned to infer types. `None` = the engine's default; `Some(0)` means "read
    /// everything as text" (DataFusion's own `disable_inference` arm).
    pub infer_rows: Option<usize>,
    pub compression: FileCompression,
}

impl Default for CsvRead {
    /// DataFusion's defaults, exactly — so a def written before read options existed registers
    /// the way it always did.
    fn default() -> Self {
        Self {
            header: true,
            delimiter: ',',
            quote: '"',
            escape: None,
            comment: None,
            newlines_in_values: false,
            truncated_rows: false,
            infer_rows: None,
            compression: FileCompression::default(),
        }
    }
}

/// How a JSON source is read.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(default)]
pub struct JsonRead {
    pub shape: JsonShape,
    /// Records scanned to infer the schema. **`None` = scan every record**, which is the default
    /// and is deliberate: the reader exists to notice a type conflict, and a capped scan that
    /// misses one types the column wrong and then fails at *query* time on a table the catalog
    /// called healthy (`engine::json_poly::format::infer_schema`).
    ///
    /// `Some(0)` is refused by the engine — it would infer a schema with no columns — so the
    /// Configure pane spends 0 as its "scan everything" sentinel and writes `None` for it.
    pub infer_rows: Option<usize>,
    pub compression: FileCompression,
}

/// **A table's reader, and the options that reader takes** — one field, not a format string
/// beside an options bag.
///
/// The two are not independent: a [`CsvRead`] means nothing to the parquet reader, and a def
/// carrying both would have a state where they disagree — a delimiter set on a parquet table,
/// silently ignored, and shown by whatever surface renders the options. Here the format *is* the
/// options, so that state cannot be written down.
///
/// [`Unknown`](Self::Unknown) is not a fallback: it is a def naming a reader this build does not
/// have (a legacy `"avro"`), kept so one such row cannot stop the whole project file loading, and
/// failing loudly at its own registration rather than being read as something else.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SourceFormat {
    #[default]
    Parquet,
    Csv(CsvRead),
    Json(JsonRead),
    Arrow,
    /// A format string with no reader in this build, kept verbatim.
    Unknown(String),
}

impl SourceFormat {
    /// The format's own name — what the catalog labels a row with, and what a legacy
    /// `"format": "csv"` held.
    pub fn name(&self) -> &str {
        match self {
            Self::Parquet => "parquet",
            Self::Csv(_) => "csv",
            Self::Json(_) => "json",
            Self::Arrow => "arrow",
            Self::Unknown(name) => name,
        }
    }

    /// The default options for a named format. An unrecognised name becomes
    /// [`Unknown`](Self::Unknown) rather than parquet — reading one format's files as another
    /// is the silent failure this enum exists to prevent.
    pub fn from_name(name: &str) -> Self {
        match name {
            "parquet" => Self::Parquet,
            "csv" => Self::Csv(CsvRead::default()),
            "json" => Self::Json(JsonRead::default()),
            "arrow" => Self::Arrow,
            other => Self::Unknown(other.to_string()),
        }
    }

    /// The file extension a listing filters on — the format's own **plus** any compression
    /// suffix, because that is what the files are actually called (`events.csv.gz`).
    pub fn extension(&self) -> String {
        let compression = match self {
            Self::Csv(o) => o.compression,
            Self::Json(o) => o.compression,
            Self::Parquet | Self::Arrow | Self::Unknown(_) => FileCompression::None,
        };
        format!(".{}{}", self.name(), compression.extension())
    }
}

/// Write a format the way it will read back.
///
/// The four readers emit the tagged form. [`Unknown`](SourceFormat::Unknown) emits the **bare
/// string** it arrived as — which it has to, twice over. Serde cannot serialize an internally
/// tagged *newtype* variant holding a string at all (it fails at runtime, which would have made
/// every `save_defs` of a project containing one legacy `avro` def fail, taking the whole
/// project file with it). And it is the honest form anyway: a def this build cannot read should
/// come back out of `project.json` exactly as it went in, so a Strata save never mangles a
/// table some other tool wrote.
fn se_format<S>(format: &SourceFormat, s: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    match format {
        SourceFormat::Unknown(name) => s.serialize_str(name),
        known => known.serialize(s),
    }
}

/// Accept a format as either the legacy bare `"csv"` (→ that reader's defaults) or the current
/// tagged `{"type":"csv", …}` form, so old project files keep loading.
fn de_format<'de, D>(d: D) -> Result<SourceFormat, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Named(String),
        Typed(SourceFormat),
    }
    Ok(match Raw::deserialize(d)? {
        Raw::Named(name) => SourceFormat::from_name(&name),
        Raw::Typed(format) => format,
    })
}

/// One logical table definition (a DataFusion `ListingTable` over many source paths).
/// `sources` are stored project-relative where they sit inside the project folder.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct TableDef {
    pub name: String,
    /// The reader and its options — see [`SourceFormat`].
    #[serde(deserialize_with = "de_format", serialize_with = "se_format")]
    pub format: SourceFormat,
    pub sources: Vec<String>,
    /// Hive partition columns as `(name, arrow_type)` — the persisted source of truth for
    /// deterministic reload (types aren't re-detected).
    #[serde(default, deserialize_with = "de_partition_cols")]
    pub partition_cols: Vec<(String, String)>,
}

#[cfg(test)]
mod format_tests {
    use super::*;

    fn parse(json: &str) -> TableDef {
        serde_json::from_str(json).expect("a table def")
    }

    #[test]
    fn a_legacy_bare_format_string_loads_with_that_readers_defaults() {
        let def = parse(r#"{"name":"t","format":"csv","sources":["/data"]}"#);
        assert_eq!(def.format, SourceFormat::Csv(CsvRead::default()));
        assert_eq!(def.format.name(), "csv");
    }

    #[test]
    fn a_legacy_def_registers_exactly_as_it_did_before_read_options_existed() {
        // The whole point of matching DataFusion's defaults: nothing about an existing
        // project's tables changes because this field arrived.
        let csv = CsvRead::default();
        assert!(csv.header);
        assert_eq!(csv.delimiter, ',');
        assert_eq!(csv.quote, '"');
        assert_eq!(csv.escape, None);
        assert_eq!(csv.comment, None);
        assert!(!csv.newlines_in_values);
        assert!(!csv.truncated_rows);
        assert_eq!(csv.infer_rows, None);
        assert_eq!(csv.compression, FileCompression::None);
        assert_eq!(JsonRead::default().shape, JsonShape::NewlineDelimited);
    }

    #[test]
    fn the_tagged_form_round_trips_with_its_options() {
        let def = TableDef {
            name: "events".into(),
            format: SourceFormat::Csv(CsvRead {
                delimiter: ';',
                truncated_rows: true,
                compression: FileCompression::Gzip,
                ..Default::default()
            }),
            sources: vec!["/data".into()],
            partition_cols: vec![],
        };
        let round: TableDef = parse(&serde_json::to_string(&def).expect("serialize"));
        assert_eq!(round.format, def.format);
    }

    #[test]
    fn an_unreadable_format_survives_a_save_as_the_string_it_arrived_as() {
        // Serde cannot serialize an internally tagged newtype variant holding a string, so the
        // derived impl would fail here — and `save_defs` serializes the whole `ProjectDefs`, so
        // one such def would block every write of that project file.
        let def = TableDef {
            name: "legacy".into(),
            format: SourceFormat::Unknown("avro".into()),
            sources: vec!["/data".into()],
            partition_cols: vec![],
        };
        let json = serde_json::to_string(&def).expect("an unreadable format still serializes");
        assert!(json.contains(r#""format":"avro""#), "{json}");
        assert_eq!(parse(&json).format, def.format, "and reads back unchanged");
    }

    #[test]
    fn an_unreadable_format_is_kept_verbatim_rather_than_read_as_parquet() {
        // A legacy `avro` def must not quietly become a parquet table — one row that cannot
        // register is recoverable, a table read with the wrong reader is not.
        let def = parse(r#"{"name":"t","format":"avro","sources":["/data"]}"#);
        assert_eq!(def.format, SourceFormat::Unknown("avro".into()));
        assert_eq!(def.format.name(), "avro");
    }

    #[test]
    fn the_listing_extension_carries_the_compression_suffix() {
        assert_eq!(SourceFormat::Parquet.extension(), ".parquet");
        assert_eq!(SourceFormat::Csv(CsvRead::default()).extension(), ".csv");
        assert_eq!(
            SourceFormat::Csv(CsvRead {
                compression: FileCompression::Gzip,
                ..Default::default()
            })
            .extension(),
            ".csv.gz"
        );
        assert_eq!(
            SourceFormat::Json(JsonRead {
                compression: FileCompression::Zstd,
                ..Default::default()
            })
            .extension(),
            ".json.zst"
        );
    }
}

/// A saved, query-backed catalog view definition (a real DataFusion `CREATE VIEW`).
/// Views are addressed by `name` — that *is* their engine/SQL identity.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct ViewDef {
    pub name: String,
    pub sql: String,
}

/// A named SQL snippet stored in the project — distinct from a [`ViewDef`] (which is a
/// real DataFusion view). Re-opened in a query tab, not queryable by name — so unlike a
/// view, its `name` is only a label, and identity is the stable `id` (what a tab's
/// save-target origin holds; renaming can't dangle it). Files written before ids get one
/// minted per load; it sticks on the next save.
#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub struct SavedQuery {
    #[serde(default = "Uuid::new_v4")]
    pub id: Uuid,
    pub name: String,
    pub sql: String,
    pub meta: String,
}
