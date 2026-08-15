//! Catalog references and row descriptors, plus the persisted **catalog definitions**
//! ([`TableDef`] / [`ViewDef`] / [`SavedQuery`]) — exactly what `.strata/project.json` stores.
//!
//! What registration *learns* about a def (columns, row counts, status, profiles) is runtime state
//! and lives in the project store wrapped around these, never here as skipped fields.

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
///
/// `Hash` because it rides a freya-query cache key (a profile is *of* a table or a view), and a
/// key's identity has to be hashable all the way down.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum CatalogKind {
    Table,
    View,
    Query,
}

/// One relation inside a database connection's catalog, addressed the way SQL addresses it: the
/// catalog the connection registered, the server's own schema, and the relation.
///
/// The **catalog name** rather than the connection's URL, because every question asked of a remote
/// relation is asked in SQL — its columns come from resolving `catalog.schema.relation`, and a
/// profile's `FROM` renders those same three segments. A URL can say neither, and carrying both
/// would be two statements of one identity that can disagree.
///
/// Each segment is kept in the server's own spelling, so a renderer decides the quoting once
/// (`sql::qualified`); a joined string would have to be parsed back, and these names come from a
/// server rather than from us.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct RemoteRef {
    pub connection: String,
    pub schema: String,
    pub relation: String,
}

impl RemoteRef {
    /// The three segments as a person reads them — a panel header, never SQL.
    pub fn label(&self) -> String {
        format!("{}.{}.{}", self.connection, self.schema, self.relation)
    }
}

/// Whose columns a [`ColRef`] addresses.
///
/// Two arms because a remote relation is not a workspace def and never becomes one: it has no
/// stored def, no `Reg` row and no one-segment name, so a [`CatalogKind`] beside a `String` cannot
/// say where it is. Which arm it is also decides where a profile request is *kept* — the workspace
/// entry's own row, or the window's satellite — so the two questions are answered by one value.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ColOwner {
    /// A workspace table or view, by name — their shared engine/SQL identity.
    Entry { kind: CatalogKind, name: String },
    /// A relation inside a database connection's catalog.
    Remote(RemoteRef),
}

impl ColOwner {
    /// What the owner is called, as a panel prints it.
    pub fn label(&self) -> String {
        match self {
            ColOwner::Entry { name, .. } => name.clone(),
            ColOwner::Remote(relation) => relation.label(),
        }
    }

    /// The workspace collection that owns it, or `None` for a remote relation — which is the
    /// question "does the project store have a row for this".
    pub fn kind(&self) -> Option<CatalogKind> {
        match self {
            ColOwner::Entry { kind, .. } => Some(*kind),
            ColOwner::Remote(_) => None,
        }
    }
}

/// A reference to one column in the catalog: what owns it, and its **path** within that owner.
///
/// A path rather than a name because a name alone cannot say *which* `city`, the top-level one or
/// the one inside `address`, and the sidebar renders both. A struct rather than a
/// `"view::orders.address.city"` URN because names come from the user's files and may contain dots
/// or `::`.
///
/// An **empty path is the owner itself** — the state a remote relation is selected in before its
/// columns have been read, since only an introspection can name one. The panel resolves it to the
/// owner's first column, which is the standing-on-the-first-column rule a workspace reveal applies
/// up front, moved to where the columns are actually known.
#[derive(Clone, PartialEq, Debug)]
pub struct ColRef {
    pub owner: ColOwner,
    /// Path within the owner. A top-level column is a one-segment path.
    pub path: Vec<String>,
}

impl ColRef {
    /// A column of a workspace table or view.
    pub fn entry(kind: CatalogKind, name: impl Into<String>, path: Vec<String>) -> Self {
        Self {
            owner: ColOwner::Entry {
                kind,
                name: name.into(),
            },
            path,
        }
    }

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
/// One caveat rides with [`Array`](Self::Array): DataFusion cannot range-split such a file and
/// `JsonSource` does not declare that, so a file over `repartition_file_min_size` (10 MB) fails its
/// *scan* with a `NotImplemented`. Loud, self-describing, and only above that size.
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
/// **Every field here reaches both halves of the read path** — inference and scan. That bar
/// excluded `null_regex` (inference only, so setting it re-types a column and then fails the scan),
/// `terminator` (scan only, so schema and rows would be read by different rules), and the writer's
/// options, which no read path references.
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
    /// Read files whose rows (or whose *schemas*, across paths) have different column counts: the
    /// union of the columns found, padded with nulls. Without it a ragged file fails the register.
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
    /// Records scanned to infer the schema. **`None` = scan every record**, the default: the
    /// reader exists to notice a type conflict, and a capped scan that misses one types the column
    /// wrong and then fails at *query* time on a table the catalog called healthy.
    ///
    /// `Some(0)` is refused by the engine, so the Configure pane spends 0 as its "scan everything"
    /// sentinel and writes `None` for it.
    pub infer_rows: Option<usize>,
    pub compression: FileCompression,
}

/// **A table's reader, and the options that reader takes** — one field, not a format string beside
/// an options bag, so a delimiter set on a parquet table is a state that cannot be written down.
///
/// [`Unknown`](Self::Unknown) is not a fallback: it is a def naming a reader this build does not
/// have, kept so one such row cannot stop the whole project file loading, and failing loudly at its
/// own registration rather than being read as something else.
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
/// The four readers emit the tagged form; [`Unknown`](SourceFormat::Unknown) emits the **bare
/// string** it arrived as. Serde cannot serialize an internally tagged newtype variant holding a
/// string at all, so the derived impl would fail every `save_defs` of a project containing one —
/// and it is the honest form anyway, so a Strata save never mangles a table another tool wrote.
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

/// **Whose files a table def points at** (ED-04) — a flag on [`TableDef`], not a second type.
///
/// Both origins share one namespace, one list in `project.json` and one catalog section, because
/// they are the same kind of thing to everything that reads a def. The flag answers exactly three
/// questions: may a write statement target it, does dropping it delete data, can Configure edit it.
/// A def written before this field existed is [`External`](Self::External).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum TableOrigin {
    /// The user's own files, registered by Table Config or a typed `CREATE EXTERNAL TABLE`.
    /// Strata reads them and never writes them.
    #[default]
    External,
    /// Strata's own: written by `CREATE TABLE` / CTAS into the project's `.strata/tables/`,
    /// which is gitignored — so the **def** travels with the project and the data does not.
    Internal,
}

impl TableOrigin {
    pub fn is_internal(self) -> bool {
        matches!(self, TableOrigin::Internal)
    }
}

/// One logical table definition (a DataFusion `ListingTable` over many source paths).
/// `sources` are stored project-relative where they sit inside the project folder — unless the
/// def names a [`connection`](Self::connection), which is what they are relative to instead.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct TableDef {
    pub name: String,
    /// The reader and its options — see [`SourceFormat`].
    #[serde(deserialize_with = "de_format", serialize_with = "se_format")]
    pub format: SourceFormat,
    /// **Which object store [`sources`](Self::sources) are read from**, as the
    /// [`ConnectionDef::url`](crate::ConnectionDef::url) that identifies it — `s3://acme-lake`.
    /// `None` is the local disk.
    ///
    /// A *reference*, not a copy: the bucket, its provider and its credentials belong to the
    /// connection. The URL rather than the bucket for the reason the registry keys on it —
    /// `s3://lake` and `gs://lake` are two stores over one bucket name.
    ///
    /// The **one** field that says a table is remote: a source is bucket-relative exactly when this
    /// is `Some`, and `strata_core::project::resolve_source` is the single place that composes the
    /// two.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection: Option<String>,
    pub sources: Vec<String>,
    /// Hive partition columns as `(name, arrow_type)` — the persisted source of truth for
    /// deterministic reload (types aren't re-detected).
    #[serde(default, deserialize_with = "de_partition_cols")]
    pub partition_cols: Vec<(String, String)>,
    /// Whose files [`sources`](Self::sources) names — see [`TableOrigin`].
    #[serde(default)]
    pub origin: TableOrigin,
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
            connection: None,
            sources: vec!["/data".into()],
            partition_cols: vec![],
            origin: TableOrigin::External,
        };
        let round: TableDef = parse(&serde_json::to_string(&def).expect("serialize"));
        assert_eq!(round.format, def.format);
    }

    /// A def written before origins existed is external, and an internal one round-trips.
    #[test]
    fn an_origin_defaults_to_external_and_round_trips() {
        assert_eq!(
            parse(r#"{"name":"t","format":"csv","sources":["/data"]}"#).origin,
            TableOrigin::External
        );
        let def = TableDef {
            name: "daily".into(),
            format: SourceFormat::Arrow,
            connection: None,
            sources: vec![".strata/tables/daily/".into()],
            partition_cols: vec![],
            origin: TableOrigin::Internal,
        };
        let json = serde_json::to_string(&def).expect("serialize");
        assert!(json.contains(r#""origin":"internal""#), "{json}");
        assert_eq!(parse(&json), def);
    }

    /// A def written before connections existed reads from the **local disk**; a remote one
    /// round-trips, and a local one writes no key at all.
    #[test]
    fn a_connection_defaults_to_the_local_disk_and_round_trips() {
        assert_eq!(
            parse(r#"{"name":"t","format":"csv","sources":["/data"]}"#).connection,
            None
        );
        let local = TableDef {
            name: "events".into(),
            format: SourceFormat::Parquet,
            connection: None,
            sources: vec!["data/events/".into()],
            partition_cols: vec![],
            origin: TableOrigin::External,
        };
        let json = serde_json::to_string(&local).expect("serialize");
        assert!(!json.contains("connection"), "{json}");

        let remote = TableDef {
            connection: Some("s3://acme-lake".into()),
            sources: vec!["events/2024/**/*.parquet".into()],
            ..local
        };
        let json = serde_json::to_string(&remote).expect("serialize");
        assert!(json.contains(r#""connection":"s3://acme-lake""#), "{json}");
        assert_eq!(parse(&json), remote);
    }

    #[test]
    fn an_unreadable_format_survives_a_save_as_the_string_it_arrived_as() {
        let def = TableDef {
            name: "legacy".into(),
            format: SourceFormat::Unknown("avro".into()),
            connection: None,
            sources: vec!["/data".into()],
            partition_cols: vec![],
            origin: TableOrigin::External,
        };
        let json = serde_json::to_string(&def).expect("an unreadable format still serializes");
        assert!(json.contains(r#""format":"avro""#), "{json}");
        assert_eq!(parse(&json).format, def.format, "and reads back unchanged");
    }

    #[test]
    fn an_unreadable_format_is_kept_verbatim_rather_than_read_as_parquet() {
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

/// A saved, query-backed catalog view definition (a real DataFusion `CREATE VIEW`). Addressed by
/// `name` — that *is* its engine/SQL identity.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct ViewDef {
    pub name: String,
    pub sql: String,
}

/// A named SQL snippet stored in the project, re-opened in a query tab and not queryable by name.
///
/// So unlike a [`ViewDef`], its `name` is only a label and identity is the stable `id`, which a
/// tab's save-target origin holds and a rename cannot dangle. Files written before ids get one
/// minted per load; it sticks on the next save.
#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub struct SavedQuery {
    #[serde(default = "Uuid::new_v4")]
    pub id: Uuid,
    pub name: String,
    pub sql: String,
    pub meta: String,
}
