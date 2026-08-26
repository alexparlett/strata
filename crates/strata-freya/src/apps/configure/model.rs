//! The Configure window's data: what is being configured ([`ConfigureTarget`]), what the user
//! has chosen ([`ConfigureDraft`]), and the **import-option groups** the chosen format produces.
//!
//! **Options are data.** [`ConfigureDraft::core`] / [`ConfigureDraft::advanced`] return
//! `Vec<Group>` and the view renders whatever it is handed, so a new option is a row in a table
//! rather than a branch in a component. Every option carries the [`Edit`] it performs, so a
//! control cannot write the wrong field and [`ConfigureDraft::apply`] is exhaustive.
//!
//! **The draft keeps every format's options side by side; the def keeps only the active
//! format's.** Switching to Parquet and back must not forget the delimiter you set, so the draft
//! remembers; but a parquet `TableDef` must not be able to name a delimiter, so
//! [`ConfigureDraft::def`] projects only the format in play — the same split the export window
//! makes for the same reason.
//!
//! Which options exist at all is [`strata_model::CsvRead`]'s subject: the bar is that an option
//! reaches the read, in both halves of it, and three that look available do not.

use std::collections::BTreeMap;
use std::path::Path;

use strata_core::project::{relativize, resolve_source};
use strata_core::util::one_char;
use strata_engine::export::quote_col;
use strata_engine::{duplicate_column, fold_ident};
use strata_model::{
    ConnectionDef, CsvRead, FileCompression, JsonRead, JsonShape, ProviderId, SourceFormat,
    TableDef, TableOrigin,
};

use crate::components::form::{Choice, Control, Group, Make, TextField};

/// DataFusion's own `DEFAULT_SCHEMA_INFER_MAX_RECORD`.
///
/// The def stores `Option<usize>`, where `None` means "the engine's default" — but a number
/// field has no way to show "unset", and a box that showed one would be a box the user cannot
/// type back into. So the field starts here and always reports a number: identical behaviour,
/// with nothing the control cannot express. A legacy def that says `None` opens showing this.
pub const DEFAULT_INFER_ROWS: u32 = 1000;

/// The most rows a schema inference will be asked to read. Not a DataFusion limit — a bound on
/// the box, so a mistyped figure cannot turn `Register` into a full-file read.
const MAX_INFER_ROWS: u32 = 1_000_000;

/// The Arrow types a Hive partition column may be read as — the canvas's four.
///
/// Partition values live in *directory names*, so they arrive as text and are cast on read.
/// `Utf8` is what DataFusion infers on its own, which is why the surface warns while a column
/// is left in it: `WHERE year = 2024` against a `Utf8` partition needs a cast.
pub const PARTITION_TYPES: [&str; 4] = ["Utf8", "Int32", "Int64", "Date32"];

/// What this window is configuring: a new table, or an existing one by name.
///
/// The name is the identity (tables and views share one namespace), and it is also what makes
/// this window single-instance per target — two windows on one def would both write it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ConfigureTarget {
    New,
    Edit(String),
}

impl ConfigureTarget {
    /// The window's title.
    pub fn title(&self) -> String {
        match self {
            Self::New => "New table".into(),
            Self::Edit(name) => format!("Configure {name}"),
        }
    }

    /// The name this window opened on, if any — what a rename is measured against.
    pub fn editing(&self) -> Option<&str> {
        match self {
            Self::New => None,
            Self::Edit(name) => Some(name),
        }
    }
}

/// Which reader the format picker is on.
///
/// Four, not the canvas's five: there is no Avro in this build. [`Unknown`](Self::Unknown) is
/// never *offered* — it is what an existing def whose format has no reader opens as, so the
/// window shows what the def really says and Save stays blocked until a real format is picked.
/// Quietly opening such a table as parquet is exactly the silent mis-read the typed format
/// exists to prevent.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum FormatId {
    Parquet,
    Csv,
    Json,
    Arrow,
    Unknown(String),
}

impl FormatId {
    /// The formats the picker offers.
    pub const OFFERED: [FormatId; 4] = [Self::Parquet, Self::Csv, Self::Json, Self::Arrow];

    pub fn label(&self) -> String {
        match self {
            Self::Parquet => "PARQUET".into(),
            Self::Csv => "CSV".into(),
            Self::Json => "JSON".into(),
            Self::Arrow => "ARROW".into(),
            Self::Unknown(name) => name.to_uppercase(),
        }
    }

    fn of(format: &SourceFormat) -> Self {
        match format {
            SourceFormat::Parquet => Self::Parquet,
            SourceFormat::Csv(_) => Self::Csv,
            SourceFormat::Json(_) => Self::Json,
            SourceFormat::Arrow => Self::Arrow,
            SourceFormat::Unknown(name) => Self::Unknown(name.clone()),
        }
    }
}

/// One thing a control can do to the draft. Exhaustive: [`ConfigureDraft::apply`] matches every
/// variant, and a control is built holding the exact edit it performs.
#[derive(Clone, PartialEq, Debug)]
pub enum Edit {
    CsvHeader(bool),
    CsvDelimiter(String),
    CsvQuote(String),
    CsvEscape(String),
    CsvComment(String),
    CsvNewlines(bool),
    CsvTruncated(bool),
    CsvInferRows(u32),
    CsvCompression(FileCompression),
    JsonShape(JsonShape),
    JsonInferRows(u32),
    JsonCompression(FileCompression),
}

/// Everything the user has chosen.
#[derive(Clone, PartialEq, Debug)]
pub struct ConfigureDraft {
    pub name: String,
    pub format: FormatId,
    /// **LOCATION** — where this table's data is (W7 · 04, IT-01).
    ///
    /// A three-way answer rather than flags: `Local` and `Remote` differ in *which files* are
    /// read, `Internal` in whether the user brings any. Two bools would make "remote and internal"
    /// expressible, and every reader would have to know which one wins.
    ///
    /// Its own field rather than "is a connection chosen": the toggle has to be operable before
    /// there is anything to choose, and a project with no connections for the picked provider is
    /// exactly where the picker's empty line has something to say. It is also what makes the
    /// def's own [`TableDef::connection`] unambiguous — see [`store`](Self::store).
    pub location: Where,
    /// **TYPE** — which provider the CONNECTION picker is filtered to. Only meaningful while
    /// [`remote`](Self::remote), and kept in step with the chosen connection by
    /// [`set_provider`](Self::set_provider).
    ///
    /// **A filter, never the table's provider.** The two agree by construction while the chosen
    /// connection is one the project has — which is the only state the picker can *produce* — but
    /// a def naming a connection that has since been forgotten opens on the first provider
    /// whatever its URL says, so a forgotten `gs://` bucket shows the S3 segment ([`of`](Self::of)
    /// says why it is not re-derived from the scheme). Nothing may read this as a fact about the
    /// table: what the table reads through is [`store`](Self::store), and while the two disagree
    /// Save is blocked naming the missing URL (`views::footer`).
    pub provider: ProviderId,
    /// The chosen connection, by its [`name`](ConnectionDef::named) — how the project addresses one.
    ///
    /// Kept across a flip back to Local, like every format's options are kept across a
    /// format switch: looking at the local arm and coming back must not forget which bucket was
    /// picked.
    pub connection: Option<String>,
    /// The **local disk's** source paths, in the order they were added — never seeded, and a
    /// blank row is a row being typed.
    pub local_sources: Vec<String>,
    /// The **object store's** source path, of which a remote table has exactly one (spec §4).
    ///
    /// A field of its own, kept like a format's options are across a switch: the two locations are
    /// written against different roots, so one list holding both would either lose what was typed
    /// or carry `/data/events.parquet` under a bucket as though it were relative to it.
    /// [`nonblank_sources`](Self::nonblank_sources) projects the one in play.
    pub remote_source: String,
    pub csv_header: bool,
    pub csv_delimiter: String,
    pub csv_quote: String,
    pub csv_escape: String,
    pub csv_comment: String,
    pub csv_newlines: bool,
    pub csv_truncated: bool,
    pub csv_infer_rows: u32,
    pub csv_compression: FileCompression,
    pub json_shape: JsonShape,
    pub json_infer_rows: u32,
    pub json_compression: FileCompression,
    pub hive_on: bool,
    /// The partition columns and the type each is read as, outermost first.
    pub partitions: Vec<(String, String)>,
    /// The declared columns of a table Strata will store (IT-01) — only meaningful on
    /// [`Where::Internal`], and kept across a flip away from it exactly as a connection and a
    /// format's options are.
    ///
    /// Position-addressed like [`local_sources`](Self::local_sources), and edited through the same
    /// two-way-synced rows, because it is the same control: a list of text fields with a
    /// selection and a `+`/`−` toolbar.
    pub columns: Vec<ColumnDraft>,
}

/// **LOCATION** — the three answers to "where is this table's data".
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Where {
    /// Files on this machine's disk.
    #[default]
    Local,
    /// Files in one of the project's object stores (W7 · 04).
    Remote,
    /// No files of the user's at all: Strata writes and owns the data, under the project's
    /// `.strata/tables/` (IT-01). The one LOCATION that **creates** rather than registers, so it
    /// is the one whose Save composes a statement instead of a def.
    Internal,
}

/// What the planner said about one type spelling — [`Ok`] is the Arrow type in the spelling the
/// grid and the inspector will show, [`Err`] is the refusal in the planner's own words.
pub type Verdict = Result<String, String>;

/// Every type spelling this window has asked about, keyed by the **trimmed text** the user typed.
///
/// Keyed by text rather than by row because the answer is a pure function of it on this session:
/// two `VARCHAR` rows are one question, and a row retyped back to a spelling it already had
/// answers instantly.
pub type Probes = BTreeMap<String, Verdict>;

/// One declared column of a internal table: what it is called, and the SQL type as typed.
///
/// The type is **text**, not a pick from a list: there is no Arrow → SQL inverse to author an
/// offer from, so what it means is asked of the planner per row
/// ([`Engine::column_type`](strata_engine::Engine::column_type)) rather than declared here.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct ColumnDraft {
    pub name: String,
    pub sql_type: String,
}

impl ColumnDraft {
    /// The name with its surrounding space removed — what every question about this column is
    /// asked with, and what the composed statement quotes.
    pub fn name(&self) -> &str {
        self.name.trim()
    }

    /// The type text, likewise — and the key its planner verdict is cached under.
    pub fn sql_type(&self) -> &str {
        self.sql_type.trim()
    }

    /// Nothing typed in either box. Not a fault — it is a row you have just added — and it
    /// composes nothing.
    pub fn is_blank(&self) -> bool {
        self.name().is_empty() && self.sql_type().is_empty()
    }
}

impl Default for ConfigureDraft {
    /// A new table: parquet, no path rows, every read option at its reader's default.
    ///
    /// The list opens **empty**, on its own empty state, rather than on a blank row: a row that
    /// was never added is a path the user has to notice is not one, and Browse fills the list
    /// from nothing exactly as it fills it from a selection ([`Self::set_paths`]). Nothing later
    /// seeds one either, [`set_location`](Self::set_location) included.
    fn default() -> Self {
        let csv = CsvRead::default();
        let json = JsonRead::default();
        Self {
            name: String::new(),
            format: FormatId::Parquet,
            location: Where::Local,
            provider: ProviderId::S3,
            connection: None,
            local_sources: Vec::new(),
            remote_source: String::new(),
            csv_header: csv.header,
            csv_delimiter: csv.delimiter.to_string(),
            csv_quote: csv.quote.to_string(),
            csv_escape: String::new(),
            csv_comment: String::new(),
            csv_newlines: csv.newlines_in_values,
            csv_truncated: csv.truncated_rows,
            csv_infer_rows: DEFAULT_INFER_ROWS,
            csv_compression: csv.compression,
            json_shape: json.shape,
            json_infer_rows: DEFAULT_INFER_ROWS,
            json_compression: json.compression,
            hive_on: false,
            partitions: Vec::new(),
            columns: Vec::new(),
        }
    }
}

impl ConfigureDraft {
    /// Seed the draft from an existing def — every field it holds, so the window opens showing
    /// what is really stored and Save with nothing touched is a no-op.
    ///
    /// The formats it *isn't* in keep their defaults, and so does the location it is not in: a
    /// def's sources belong to the one it names, and the other opens empty.
    ///
    /// `connections` is here for one field: which provider serves a connection is the connection's
    /// fact rather than the table's, so the TYPE segment is resolved from it rather than re-derived
    /// from the URL's scheme. A def naming a connection this project no longer has keeps the
    /// reference and opens on the first provider, with `Save` blocked until it is re-pointed —
    /// the same treatment a format with no reader gets.
    ///
    /// A def naming a **database** connection gets that treatment through the filter below: a table
    /// reads files, so the TYPE pill offers only [`ProviderId::OBJECT_STORES`], and a draft opening
    /// on a provider the pill cannot render would show no segment selected.
    pub fn of(def: &TableDef, connections: &[ConnectionDef]) -> Self {
        let provider = def
            .connection
            .as_deref()
            .and_then(|url| connections.iter().find(|c| c.named() == url))
            .map(|c| c.provider.id())
            .filter(|id| id.is_object_store())
            .unwrap_or(ProviderId::S3);
        let remote = def.connection.is_some();
        let mut draft = Self {
            name: def.name.clone(),
            format: FormatId::of(&def.format),
            location: match remote {
                true => Where::Remote,
                false => Where::Local,
            },
            provider,
            connection: def.connection.clone(),
            local_sources: match remote {
                true => Vec::new(),
                false => def.sources.clone(),
            },
            remote_source: match remote {
                true => def.sources.first().cloned().unwrap_or_default(),
                false => String::new(),
            },
            hive_on: !def.partition_cols.is_empty(),
            partitions: def.partition_cols.clone(),
            ..Default::default()
        };
        match &def.format {
            SourceFormat::Csv(o) => {
                draft.csv_header = o.header;
                draft.csv_delimiter = escaped(o.delimiter);
                draft.csv_quote = o.quote.to_string();
                draft.csv_escape = o.escape.map(String::from).unwrap_or_default();
                draft.csv_comment = o.comment.map(String::from).unwrap_or_default();
                draft.csv_newlines = o.newlines_in_values;
                draft.csv_truncated = o.truncated_rows;
                draft.csv_infer_rows = o.infer_rows.unwrap_or(DEFAULT_INFER_ROWS as usize) as u32;
                draft.csv_compression = o.compression;
            }
            SourceFormat::Json(o) => {
                draft.json_shape = o.shape;
                draft.json_infer_rows = o.infer_rows.unwrap_or(0) as u32;
                draft.json_compression = o.compression;
            }
            SourceFormat::Parquet | SourceFormat::Arrow | SourceFormat::Unknown(_) => {}
        }
        draft
    }

    /// Apply one control's edit.
    pub fn apply(&mut self, edit: Edit) {
        match edit {
            Edit::CsvHeader(v) => self.csv_header = v,
            Edit::CsvDelimiter(v) => self.csv_delimiter = v,
            Edit::CsvQuote(v) => self.csv_quote = v,
            Edit::CsvEscape(v) => self.csv_escape = v,
            Edit::CsvComment(v) => self.csv_comment = v,
            Edit::CsvNewlines(v) => self.csv_newlines = v,
            Edit::CsvTruncated(v) => self.csv_truncated = v,
            Edit::CsvInferRows(v) => self.csv_infer_rows = v,
            Edit::CsvCompression(v) => self.csv_compression = v,
            Edit::JsonShape(v) => self.json_shape = v,
            Edit::JsonInferRows(v) => self.json_infer_rows = v,
            Edit::JsonCompression(v) => self.json_compression = v,
        }
    }

    /// The connection this draft's sources are relative to, or `None` for the local disk —
    /// exactly what [`TableDef::connection`] records.
    ///
    /// **The one place the two fields combine**, so nothing downstream has to remember that a
    /// connection kept across a flip back to Local is not the location: `remote` says where
    /// the table reads from, `connection` says which bucket that would be.
    pub fn store(&self) -> Option<&str> {
        match self.location {
            Where::Remote => self.connection.as_deref(),
            Where::Local | Where::Internal => None,
        }
    }

    /// Whether LOCATION is on Remote — the question every section that hides itself asks.
    pub fn remote(&self) -> bool {
        self.location == Where::Remote
    }

    /// Whether LOCATION is on Internal: the table has no sources, no format and no partitions,
    /// and Save composes a `CREATE TABLE` rather than writing a def.
    pub fn internal(&self) -> bool {
        self.location == Where::Internal
    }

    /// Flip **LOCATION**, and settle what that means for the rest of the draft: a move to the
    /// object store picks the provider's first connection when none is chosen yet, and either
    /// direction clears the detected partition columns, exactly as every other path mutator does
    /// — they describe the layout of a location this draft no longer points at.
    ///
    /// **No path moves with the flip, and none is invented** — each location keeps its own
    /// ([`local_sources`](Self::local_sources) / [`remote_source`](Self::remote_source)).
    /// Carrying the first local path over was the rule before, and it wrote
    /// `/data/events.parquet` under a bucket that had nothing to do with it — or, from an empty
    /// list, put a blank row in the one section whose toolbar is absent, so the path a remote
    /// table has was a row nobody added and nobody could remove.
    pub fn set_location(&mut self, location: Where, connections: &[ConnectionDef]) {
        if self.location == location {
            return;
        }
        self.location = location;
        self.partitions.clear();
        match location {
            Where::Internal => {
                if self.columns.is_empty() {
                    self.columns.push(ColumnDraft::default());
                }
            }
            Where::Local => {}
            Where::Remote => {
                if self.connection.is_none() {
                    self.connection = first_connection(connections, self.provider);
                }
            }
        }
    }

    /// Append a blank column row and hand back its index — the paths toolbar's `add_path`, on the
    /// other list.
    pub fn add_column(&mut self) -> usize {
        self.columns.push(ColumnDraft::default());
        self.columns.len() - 1
    }

    /// Remove the column at `at`, answering with the row that takes its place. One row always
    /// remains, so there is somewhere to type: a internal table with no columns is not something
    /// this window can compose, and an empty list would strand the user on `+`.
    pub fn remove_column(&mut self, at: usize) -> usize {
        if at < self.columns.len() && self.columns.len() > 1 {
            self.columns.remove(at);
        } else if self.columns.len() == 1 {
            self.columns[0] = ColumnDraft::default();
        }
        at.min(self.columns.len().saturating_sub(1))
    }

    pub fn set_column_name(&mut self, at: usize, name: String) {
        if let Some(column) = self.columns.get_mut(at) {
            column.name = name;
        }
    }

    pub fn set_column_type(&mut self, at: usize, sql_type: String) {
        if let Some(column) = self.columns.get_mut(at) {
            column.sql_type = sql_type;
        }
    }

    /// The columns that will compose one — everything that is not wholly blank.
    pub fn declared_columns(&self) -> impl Iterator<Item = &ColumnDraft> {
        self.columns.iter().filter(|column| !column.is_blank())
    }

    /// The distinct type spellings `probes` has not answered for yet, in row order — the probe
    /// driver's whole work list, and a projection rather than a queue, so a row retyped while a
    /// pass is in flight simply changes what is pending.
    pub fn unprobed(&self, probes: &Probes) -> Vec<String> {
        let mut pending: Vec<String> = Vec::new();
        for column in self.declared_columns() {
            let typed = column.sql_type();
            if typed.is_empty() || probes.contains_key(typed) {
                continue;
            }
            if !pending.iter().any(|held| held == typed) {
                pending.push(typed.to_string());
            }
        }
        pending
    }

    /// Why each faulty column row cannot be created, **by row index**.
    ///
    /// Four kinds, in the order a row hits them: a type with no column to apply it to, a name
    /// another row already claims (**both** rows are marked, because either is the one to fix), a
    /// name with no type yet, and the planner's own refusal of the type that was typed. A row
    /// whose type has not been answered for yet carries nothing — the verdict is a beat away, and
    /// a message that appears and vanishes per keystroke is worse than none.
    ///
    /// Only the first of those four is this window's own sentence; the rest are the create arm's,
    /// **reached** rather than restated ([`duplicate_column`], and the planner verbatim), so the
    /// form cannot be a second drifting copy of the engine's rules.
    pub fn column_faults(&self, probes: &Probes) -> BTreeMap<usize, String> {
        let mut faults = BTreeMap::new();
        let mut claimed: BTreeMap<String, usize> = BTreeMap::new();
        for (at, column) in self.columns.iter().enumerate() {
            if column.is_blank() {
                continue;
            }
            let name = column.name();
            if name.is_empty() {
                faults.insert(at, "Enter a column name.".to_string());
                continue;
            }
            let folded = fold_ident(name);
            if let Some(first) = claimed.get(&folded) {
                let message = duplicate_column(name);
                faults.insert(*first, message.clone());
                faults.insert(at, message);
                continue;
            }
            claimed.insert(folded, at);
            let typed = column.sql_type();
            if typed.is_empty() {
                faults.insert(at, "Enter a column type.".to_string());
                continue;
            }
            if let Some(Err(refusal)) = probes.get(typed) {
                faults.insert(at, refusal.clone());
            }
        }
        faults
    }

    /// The `CREATE TABLE` a internal table's Save runs, or `None` when there is nothing to compose
    /// yet.
    ///
    /// Names are quoted **verbatim** ([`quote_col`], not the engine's `quote_ident`): what the
    /// user typed into a box is the name they meant, so `Region` is a column called `Region`
    /// rather than one silently folded to `region`. The engine still folds for *identity*, which
    /// is why [`column_faults`](Self::column_faults) refuses `Region` beside `REGION`.
    ///
    /// The types are passed through as typed. They are the one part of the statement this window
    /// does not author, which is exactly why each box is validated per row rather than at Save.
    pub fn create_statement(&self) -> Option<String> {
        let name = self.name.trim();
        if name.is_empty() {
            return None;
        }
        let columns: Vec<String> = self
            .declared_columns()
            .filter(|column| !column.name().is_empty() && !column.sql_type().is_empty())
            .map(|column| format!("  {} {}", quote_col(column.name()), column.sql_type()))
            .collect();
        if columns.is_empty() {
            return None;
        }
        Some(format!(
            "CREATE TABLE {} (\n{}\n);",
            quote_col(name),
            columns.join(",\n")
        ))
    }

    /// Pick a **TYPE**, and with it a connection that provider actually serves — its first,
    /// unless the one already chosen is one of them. The picker below only ever offers this
    /// provider's connections, so leaving a foreign one selected would be a selection with no
    /// row to show it.
    pub fn set_provider(&mut self, provider: ProviderId, connections: &[ConnectionDef]) {
        if self.provider == provider {
            return;
        }
        self.provider = provider;
        let serves = self
            .connection
            .as_deref()
            .and_then(|url| connections.iter().find(|c| c.named() == url))
            .is_some_and(|c| c.provider.id() == provider);
        if !serves {
            self.connection = first_connection(connections, provider);
        }
    }

    /// The paths as **this machine** sees them: resolved against the project folder, where a
    /// def's sources are stored project-relative.
    ///
    /// Anything that asks the filesystem a question about a source has to ask about this, not
    /// about the stored string — `is_dir` on a relative path answers about the process's working
    /// directory, which is not the project's.
    ///
    /// A **remote** source comes back as it is stored. What address it resolves to is composed
    /// where the store is registered, so a draft that answered would be keeping a second copy of
    /// the registry's answer; and every filesystem question a remote path is asked answers
    /// `false` regardless, which is correct — a bucket has no directories to stat.
    pub fn resolved_sources(&self, root: &Path) -> Vec<String> {
        let root = match self.remote() {
            true => None,
            false => Some(root),
        };
        self.nonblank_sources()
            .iter()
            .map(|p| match root {
                Some(root) => resolve_source(root, None, p),
                None => p.clone(),
            })
            .collect()
    }

    /// The paths that are actually paths, for the LOCATION in play — the one place the two source
    /// fields are projected, as [`store`](Self::store) is for the connection.
    pub fn nonblank_sources(&self) -> Vec<String> {
        match self.location {
            Where::Local => self
                .local_sources
                .iter()
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect(),
            Where::Remote => match self.remote_source.trim() {
                "" => Vec::new(),
                path => vec![path.to_string()],
            },
            Where::Internal => Vec::new(),
        }
    }

    /// How many rows the path list holds — the local list's length, and always one on a
    /// connection, that arm being a single box rather than a list anything adds to.
    ///
    /// Whether the section draws them is `views::paths`'s question, which is why an internal
    /// table answers here as the local one it will be again if the LOCATION moves back.
    pub fn path_count(&self) -> usize {
        match self.location {
            Where::Local | Where::Internal => self.local_sources.len(),
            Where::Remote => 1,
        }
    }

    /// Row `at` as the list shows it — what a box holds, so blank rows included.
    pub fn path_at(&self, at: usize) -> String {
        match self.location {
            Where::Remote => match at {
                0 => self.remote_source.clone(),
                _ => String::new(),
            },
            Where::Local | Where::Internal => {
                self.local_sources.get(at).cloned().unwrap_or_default()
            }
        }
    }

    /// The field row `at` writes into, so a row's two-way sync cannot reach the wrong one.
    fn path_slot(&mut self, at: usize) -> Option<&mut String> {
        match self.location {
            Where::Remote => (at == 0).then_some(&mut self.remote_source),
            Where::Local | Where::Internal => self.local_sources.get_mut(at),
        }
    }

    /// Clamp a selection to the list — it shrinks under the caller.
    pub fn clamp_selection(&self, selected: usize) -> usize {
        selected.min(self.path_count().saturating_sub(1))
    }

    /// Add a blank row to the local list; returns the index to select. The toolbar it comes from
    /// is drawn on that arm only.
    pub fn add_path(&mut self) -> usize {
        self.local_sources.push(String::new());
        self.local_sources.len() - 1
    }

    /// Remove row `at` from the local list; returns the index to select afterwards.
    pub fn remove_path(&mut self, at: usize) -> usize {
        if self.local_sources.is_empty() {
            return 0;
        }
        let at = at.min(self.local_sources.len() - 1);
        self.local_sources.remove(at);
        self.partitions.clear();
        at.min(self.local_sources.len().saturating_sub(1))
    }

    /// Put `paths` into the local list at the selection: the first replaces the selected row (or
    /// becomes the first row when the list is empty), the rest are inserted after it.
    ///
    /// Multi-select is the picker's, not a flourish: a table *is* many paths, and picking five
    /// files one dialog at a time is the same five rows with four more dialogs.
    /// Returns the index to select afterwards.
    ///
    /// **Clears the detected partition columns**, as every path mutator does. They describe the
    /// layout of the paths that were there when the switch was flipped; keeping them across a
    /// re-point means Save writing one lake's keys onto another's, which registers as
    /// "no files match the partition columns" at best and mislabels a column at worst.
    pub fn set_paths(&mut self, at: usize, paths: Vec<String>) -> usize {
        if paths.is_empty() {
            return at;
        }
        self.partitions.clear();
        if self.local_sources.is_empty() {
            self.local_sources = paths;
            return 0;
        }
        let at = at.min(self.local_sources.len() - 1);
        self.local_sources.splice(at..=at, paths.iter().cloned());
        at + paths.len() - 1
    }

    /// Type into row `at` — the local list's row, or the remote box. Clears the detected
    /// partitions for the reason above.
    pub fn set_path(&mut self, at: usize, path: String) {
        if let Some(slot) = self.path_slot(at) {
            if *slot != path {
                *slot = path;
                self.partitions.clear();
            }
        }
    }

    /// Whether the Hive section has anything to offer: a partition layout only exists under a
    /// path that resolves to *many* files, so a table of single files is never partitioned.
    ///
    /// **Not gated on parquet**, unlike the canvas. Partition columns are a listing feature, not
    /// a parquet one — DataFusion reads a Hive-partitioned CSV lake perfectly well, and
    /// `TableDef.partition_cols` has always been format-agnostic. Gating the section would hide
    /// a def's own stored columns the moment its format changed.
    pub fn may_partition(&self, root: &Path) -> bool {
        if self.internal() {
            return false;
        }
        self.hive_on
            || !self.partitions.is_empty()
            || self.resolved_sources(root).iter().any(|p| {
                p.contains('*') || p.contains('?') || p.ends_with('/') || Path::new(p).is_dir()
            })
    }

    /// The partition columns that reach the def: a detected list with the toggle off is not
    /// partitioning at all. One helper, so the def and the surface cannot disagree.
    ///
    /// Gated on the **toggle only**, never on [`may_partition`](Self::may_partition). That
    /// predicate asks the filesystem about the path as *typed*, and a def's sources are stored
    /// project-relative where they sit inside the project folder — so `is_dir` answers about the
    /// process's working directory, not the project's. Letting it gate the saved value meant a
    /// perfectly good partitioned table silently lost its partition columns the moment it was
    /// opened in this window and saved. What the section *offers* may depend on the disk; what
    /// the user already decided may not.
    pub fn effective_partitions(&self) -> Vec<(String, String)> {
        match self.hive_on {
            true => self.partitions.clone(),
            false => Vec::new(),
        }
    }

    /// Whether any partition column is still being read as text — the canvas's cast warning.
    pub fn partitions_are_text(&self) -> bool {
        self.hive_on && self.partitions.iter().any(|(_, ty)| ty == "Utf8")
    }

    /// The reader and its options for the format in play — and *only* that format's.
    fn source_format(&self) -> SourceFormat {
        match &self.format {
            FormatId::Parquet => SourceFormat::Parquet,
            FormatId::Arrow => SourceFormat::Arrow,
            FormatId::Unknown(name) => SourceFormat::Unknown(name.clone()),
            FormatId::Csv => SourceFormat::Csv(CsvRead {
                header: self.csv_header,
                delimiter: self.delimiter_char().unwrap_or(','),
                quote: first_char(&self.csv_quote).unwrap_or('"'),
                escape: first_char(&self.csv_escape),
                comment: first_char(&self.csv_comment),
                newlines_in_values: self.csv_newlines,
                truncated_rows: self.csv_truncated,
                infer_rows: Some(self.csv_infer_rows as usize),
                compression: self.csv_compression,
            }),
            FormatId::Json => SourceFormat::Json(JsonRead {
                shape: self.json_shape,
                infer_rows: (self.json_infer_rows > 0).then_some(self.json_infer_rows as usize),
                compression: self.json_compression,
            }),
        }
    }

    /// The character the delimiter box names, or `None` when it is blank or unresolvable.
    fn delimiter_char(&self) -> Option<char> {
        one_char("delimiter", &self.csv_delimiter).ok().flatten()
    }

    /// The def this draft describes.
    ///
    /// A **local** def's sources are stored **project-relative** where they sit inside `root`
    /// (`project::relativize`), which is what [`TableDef`]'s own doc promises and what
    /// `resolve_source` assumes when reading them back. Without it a project that is moved,
    /// synced, or opened on another machine loses every table the picker wrote.
    ///
    /// A def over a **connection** stores its source exactly as typed: it is already relative —
    /// to that bucket rather than to this folder — and `relativize` measures against a path the
    /// bucket has nothing to do with.
    pub fn def(&self, root: &Path) -> TableDef {
        TableDef {
            name: self.name.trim().to_string(),
            format: self.source_format(),
            connection: self.store().map(str::to_string),
            sources: match self.store() {
                Some(_) => self.nonblank_sources(),
                None => self
                    .nonblank_sources()
                    .iter()
                    .map(|p| relativize(root, p))
                    .collect(),
            },
            partition_cols: self.effective_partitions(),
            origin: TableOrigin::External,
        }
    }

    /// Why this draft cannot be saved yet, or `None` when it can.
    ///
    /// Only what the *draft* can answer. Whether the files are readable is the register's
    /// question and is deliberately not pre-flighted here (D9): the Register is the check.
    pub fn blocker(&self) -> Option<String> {
        if self.name.trim().is_empty() {
            return Some("A table needs a name.".into());
        }
        if self.internal() {
            return self
                .declared_columns()
                .next()
                .is_none()
                .then(|| "A table needs at least one column.".into());
        }
        if self.remote() && self.connection.is_none() {
            return Some("A remote table needs a connection to read through.".into());
        }
        if self.nonblank_sources().is_empty() {
            return Some("A table needs at least one source path.".into());
        }
        if let FormatId::Unknown(name) = &self.format {
            return Some(format!(
                "'{name}' is not a format Strata can read. Choose another."
            ));
        }
        if self.hive_on && self.partitions.is_empty() {
            return Some("No key=value folders were found in the source paths.".into());
        }
        if self.format == FormatId::Csv {
            match one_char("delimiter", &self.csv_delimiter) {
                Err(why) => return Some(why),
                Ok(None) => return Some("The CSV delimiter can't be empty.".into()),
                Ok(Some(_)) => {}
            }
            if first_char(&self.csv_quote).is_none() {
                return Some("The CSV quote character can't be empty.".into());
            }
        }
        None
    }

    /// The label over the import block.
    pub fn options_label(&self) -> String {
        format!("{} OPTIONS", self.format.label())
    }

    /// The format's options — **one flat list**, in canvas order.
    ///
    /// There is no ADVANCED disclosure, though this window's canvas draws one. The export
    /// window's canvas folded its own away on the grounds that a format's advanced controls are
    /// just more of that format's options, and that reasoning does not stop being true here:
    /// the split would only be one more thing to open before a CSV's quote character can be
    /// reached, in a window whose whole subject is how a file is read.
    pub fn options(&self) -> Vec<Group<Edit>> {
        if self.internal() {
            return Vec::new();
        }
        let mut groups = self.core();
        groups.extend(self.advanced());
        groups
    }

    /// The options the canvas showed outright.
    fn core(&self) -> Vec<Group<Edit>> {
        match self.format {
            FormatId::Csv => vec![
                Group {
                    label: "HEADER ROW".into(),
                    hint: None,
                    control: Control::Toggle {
                        on: self.csv_header,
                        edit: Edit::CsvHeader(!self.csv_header),
                        hint: Some(
                            match self.csv_header {
                                true => "The first row holds column names",
                                false => "Columns are named column_1, column_2 and so on",
                            }
                            .into(),
                        ),
                    },
                },
                Group {
                    label: "DELIMITER".into(),
                    hint: Some("Field separator (use \\t for tab)"),
                    control: Control::Text(TextField {
                        value: self.csv_delimiter.clone(),
                        placeholder: ",",
                        max_len: 8,
                        make: Make(Edit::CsvDelimiter),
                    }),
                },
            ],
            FormatId::Json => vec![Group {
                label: "SHAPE".into(),
                hint: Some("How the records are laid out in the file"),
                control: Control::Select {
                    options: [
                        (JsonShape::NewlineDelimited, "One record per line"),
                        (JsonShape::Array, "JSON array"),
                    ]
                    .into_iter()
                    .map(|(shape, label)| Choice {
                        label: label.into(),
                        selected: shape == self.json_shape,
                        edit: Edit::JsonShape(shape),
                    })
                    .collect(),
                },
            }],
            FormatId::Parquet | FormatId::Arrow | FormatId::Unknown(_) => Vec::new(),
        }
    }

    /// The options the canvas put behind its disclosure — kept as a separate builder only
    /// because the two halves read in a different order from the canvas's own list.
    fn advanced(&self) -> Vec<Group<Edit>> {
        match self.format {
            FormatId::Csv => vec![
                Group {
                    label: "QUOTE CHARACTER".into(),
                    hint: Some("Wraps fields containing the delimiter"),
                    control: Control::Char(TextField {
                        value: self.csv_quote.clone(),
                        placeholder: "\"",
                        max_len: 1,
                        make: Make(Edit::CsvQuote),
                    }),
                },
                Group {
                    label: "ESCAPE CHARACTER".into(),
                    hint: Some("Escapes a quote inside a quoted field (blank = none)"),
                    control: Control::Char(TextField {
                        value: self.csv_escape.clone(),
                        placeholder: "\\",
                        max_len: 1,
                        make: Make(Edit::CsvEscape),
                    }),
                },
                Group {
                    label: "COMMENT CHARACTER".into(),
                    hint: Some("Skip lines starting with this character (blank = none)"),
                    control: Control::Char(TextField {
                        value: self.csv_comment.clone(),
                        placeholder: "#",
                        max_len: 1,
                        make: Make(Edit::CsvComment),
                    }),
                },
                Group {
                    label: "NEWLINES IN VALUES".into(),
                    hint: Some(
                        "Allow quoted fields to contain line breaks. Files are then read whole \
                         rather than split and read in parallel",
                    ),
                    control: Control::Toggle {
                        on: self.csv_newlines,
                        edit: Edit::CsvNewlines(!self.csv_newlines),
                        hint: None,
                    },
                },
                Group {
                    label: "RAGGED ROWS".into(),
                    hint: Some(
                        "Pad rows and files that are short of a column with nulls, instead of \
                         failing the read",
                    ),
                    control: Control::Toggle {
                        on: self.csv_truncated,
                        edit: Edit::CsvTruncated(!self.csv_truncated),
                        hint: None,
                    },
                },
                Group {
                    label: "SCHEMA-INFER ROWS".into(),
                    hint: Some("Rows scanned to infer column types. 0 reads every column as text"),
                    control: Control::Num {
                        value: self.csv_infer_rows,
                        min: 0,
                        max: MAX_INFER_ROWS,
                        make: Make(Edit::CsvInferRows),
                    },
                },
                compression_group(self.csv_compression, Edit::CsvCompression),
            ],
            FormatId::Json => vec![
                Group {
                    label: "SCHEMA-INFER ROWS".into(),
                    hint: Some("Records scanned to infer the schema. 0 scans every record"),
                    control: Control::Num {
                        value: self.json_infer_rows,
                        min: 0,
                        max: MAX_INFER_ROWS,
                        make: Make(Edit::JsonInferRows),
                    },
                },
                compression_group(self.json_compression, Edit::JsonCompression),
            ],
            FormatId::Parquet | FormatId::Arrow | FormatId::Unknown(_) => Vec::new(),
        }
    }
}

/// The connections one provider serves, by name and in the order the project
/// keeps them — what the CONNECTION picker offers, and where "this provider has none" is
/// answered.
///
/// The URL is both the value and the label: it is the project's identity for a connection (the
/// pane, the registration outcome and the forget confirm all name one this way), and a bucket
/// alone cannot tell `s3://lake` from `gs://lake`.
///
/// `provider` is always one of [`ProviderId::OBJECT_STORES`] here, because the TYPE pill above
/// this picker is the only thing that sets it and that is what it offers: a table reads *files*,
/// and a database connection registers no object store to read them from.
pub fn connections_for(connections: &[ConnectionDef], provider: ProviderId) -> Vec<String> {
    connections
        .iter()
        .filter(|c| c.provider.id() == provider)
        .map(ConnectionDef::named)
        .collect()
}

/// The connection a provider is picked *on* — its first, or none at all.
fn first_connection(connections: &[ConnectionDef], provider: ProviderId) -> Option<String> {
    connections_for(connections, provider).into_iter().next()
}

/// The compression dropdown, shared by CSV and JSON — the same whole-file wrapping, and the same
/// effect on the listing's file extension.
///
/// A `Select`, like the export window's, rather than a pill: five codecs is more than a pill
/// reads well at, and the same question should not be asked two ways in two windows.
fn compression_group(current: FileCompression, edit: fn(FileCompression) -> Edit) -> Group<Edit> {
    Group {
        label: "COMPRESSION".into(),
        hint: Some("Whole-file compression. The source files carry the matching suffix"),
        control: Control::Select {
            options: FileCompression::ALL
                .into_iter()
                .map(|c| Choice {
                    label: c.label().into(),
                    selected: c == current,
                    edit: edit(c),
                })
                .collect(),
        },
    }
}

/// How a character reads *in* a delimiter box: the escapes [`one_char`] understands, so a tab
/// opens as `\t` rather than as an invisible cell.
fn escaped(c: char) -> String {
    match c {
        '\t' => "\\t".into(),
        '\n' => "\\n".into(),
        '\\' => "\\\\".into(),
        other => other.to_string(),
    }
}

/// The one character a single-character field names, or `None` when it is blank.
///
/// The field is capped at one character by the control itself, so this reads what the box
/// shows rather than silently truncating something longer.
fn first_char(raw: &str) -> Option<char> {
    raw.chars().next()
}

#[cfg(test)]
mod tests {
    use strata_model::{GcsStore, Provider, S3Store};

    use super::*;

    fn labels(groups: &[Group<Edit>]) -> Vec<String> {
        groups.iter().map(|g| g.label.clone()).collect()
    }

    fn csv_draft() -> ConfigureDraft {
        ConfigureDraft {
            name: "events".into(),
            format: FormatId::Csv,
            local_sources: vec!["/data/events.csv".into()],
            ..Default::default()
        }
    }

    /// Two S3 buckets and one GCS, in the order a project keeps them — enough for the picker's
    /// three questions: which this provider serves, which is first, and what a switch does to a
    /// choice the new provider does not serve.
    fn connections() -> Vec<ConnectionDef> {
        ["acme-lake", "cold-store"]
            .into_iter()
            .map(|address| ConnectionDef {
                address: address.into(),
                name: address.replace('-', "_"),
                provider: Provider::S3(S3Store::default()),
                client_config: Default::default(),
            })
            .chain(std::iter::once(ConnectionDef {
                address: "warehouse".into(),
                name: "warehouse".into(),
                provider: Provider::Gcs(GcsStore::default()),
                client_config: Default::default(),
            }))
            .collect()
    }

    #[test]
    fn csv_offers_one_flat_list_in_canvas_order() {
        assert_eq!(
            labels(&csv_draft().options()),
            vec![
                "HEADER ROW",
                "DELIMITER",
                "QUOTE CHARACTER",
                "ESCAPE CHARACTER",
                "COMMENT CHARACTER",
                "NEWLINES IN VALUES",
                "RAGGED ROWS",
                "SCHEMA-INFER ROWS",
                "COMPRESSION",
            ]
        );
    }

    #[test]
    fn json_leads_with_shape_which_is_the_option_the_canvas_never_had() {
        let draft = ConfigureDraft {
            format: FormatId::Json,
            ..csv_draft()
        };
        assert_eq!(
            labels(&draft.options()),
            vec!["SHAPE", "SCHEMA-INFER ROWS", "COMPRESSION"]
        );
    }

    #[test]
    fn parquet_and_arrow_show_no_import_block_at_all() {
        for format in [FormatId::Parquet, FormatId::Arrow] {
            let draft = ConfigureDraft {
                format,
                ..csv_draft()
            };
            assert!(draft.options().is_empty());
        }
    }

    #[test]
    fn the_delimiter_is_one_text_box_that_takes_the_escapes() {
        let mut draft = csv_draft();
        let groups = draft.options();
        let g = groups.iter().find(|g| g.label == "DELIMITER").unwrap();
        assert!(
            matches!(g.control, Control::Text(_)),
            "one box, as in export"
        );

        draft.apply(Edit::CsvDelimiter("\\t".into()));
        let SourceFormat::Csv(csv) = draft.def(Path::new("/project")).format else {
            panic!("csv")
        };
        assert_eq!(csv.delimiter, '\t', "the escape resolves");
    }

    #[test]
    fn a_stored_tab_delimiter_opens_as_its_escape_rather_than_an_invisible_box() {
        let def = TableDef {
            name: "t".into(),
            format: SourceFormat::Csv(CsvRead {
                delimiter: '\t',
                ..Default::default()
            }),
            connection: None,
            sources: vec!["/data".into()],
            partition_cols: vec![],
            origin: TableOrigin::External,
        };
        assert_eq!(ConfigureDraft::of(&def, &[]).csv_delimiter, "\\t");
    }

    #[test]
    fn a_delimiter_that_is_not_one_character_is_reported_rather_than_truncated() {
        let mut draft = csv_draft();
        draft.apply(Edit::CsvDelimiter("||".into()));
        assert!(draft
            .blocker()
            .is_some_and(|b| b.contains("single character")));
    }

    #[test]
    fn every_control_carries_the_edit_it_performs() {
        let mut draft = csv_draft();
        let groups = draft.options();
        let header = groups.iter().find(|g| g.label == "HEADER ROW").unwrap();
        let Control::Toggle { on, edit, .. } = &header.control else {
            panic!("a toggle");
        };
        assert!(*on);
        draft.apply(edit.clone());
        assert!(!draft.csv_header);
    }

    #[test]
    fn the_def_carries_only_the_active_formats_options() {
        let mut draft = csv_draft();
        draft.apply(Edit::CsvDelimiter(";".into()));
        let SourceFormat::Csv(csv) = draft.def(Path::new("/project")).format else {
            panic!("csv");
        };
        assert_eq!(csv.delimiter, ';');

        draft.format = FormatId::Parquet;
        assert_eq!(
            draft.def(Path::new("/project")).format,
            SourceFormat::Parquet
        );
    }

    #[test]
    fn switching_format_and_back_keeps_the_options_you_set() {
        let mut draft = csv_draft();
        draft.apply(Edit::CsvDelimiter("|".into()));
        draft.format = FormatId::Json;
        draft.apply(Edit::JsonShape(JsonShape::Array));
        draft.format = FormatId::Csv;
        assert_eq!(draft.csv_delimiter, "|");
        assert_eq!(draft.json_shape, JsonShape::Array);
    }

    #[test]
    fn a_def_round_trips_through_the_draft_unchanged() {
        let def = TableDef {
            name: "events".into(),
            format: SourceFormat::Csv(CsvRead {
                header: false,
                delimiter: '|',
                quote: '\'',
                escape: Some('\\'),
                comment: Some('#'),
                newlines_in_values: true,
                truncated_rows: true,
                infer_rows: Some(50),
                compression: FileCompression::Gzip,
            }),
            connection: None,
            sources: vec!["/data/a".into(), "/data/b".into()],
            partition_cols: vec![("year".into(), "Int32".into())],
            origin: TableOrigin::External,
        };
        let mut draft = ConfigureDraft::of(&def, &[]);
        draft.local_sources = vec!["/data/year=*/".into()];
        let round = draft.def(Path::new("/project"));
        assert_eq!(round.format, def.format);
        assert_eq!(round.partition_cols, def.partition_cols);
    }

    #[test]
    fn a_legacy_defs_unset_infer_rows_opens_showing_the_engine_default() {
        let def = TableDef {
            name: "t".into(),
            format: SourceFormat::Csv(CsvRead::default()),
            connection: None,
            sources: vec!["/data".into()],
            partition_cols: vec![],
            origin: TableOrigin::External,
        };
        assert_eq!(
            ConfigureDraft::of(&def, &[]).csv_infer_rows,
            DEFAULT_INFER_ROWS
        );
    }

    #[test]
    fn an_unreadable_format_opens_as_itself_and_blocks_save() {
        let def = TableDef {
            name: "legacy".into(),
            format: SourceFormat::Unknown("avro".into()),
            connection: None,
            sources: vec!["/data".into()],
            partition_cols: vec![],
            origin: TableOrigin::External,
        };
        let draft = ConfigureDraft::of(&def, &[]);
        assert_eq!(draft.format, FormatId::Unknown("avro".into()));
        assert!(draft.blocker().is_some_and(|b| b.contains("avro")));
    }

    #[test]
    fn a_blank_name_or_no_path_blocks_save() {
        let mut draft = ConfigureDraft::default();
        assert!(draft.blocker().is_some_and(|b| b.contains("name")));
        draft.name = "t".into();
        assert!(draft.blocker().is_some_and(|b| b.contains("source path")));
        draft.local_sources = vec!["   ".into(), "/data".into()];
        assert_eq!(draft.blocker(), None);
    }

    #[test]
    fn an_emptied_delimiter_blocks_save_rather_than_defaulting() {
        let mut draft = csv_draft();
        draft.apply(Edit::CsvDelimiter(String::new()));
        assert!(draft.blocker().is_some_and(|b| b.contains("delimiter")));
        draft.apply(Edit::CsvDelimiter(";".into()));
        assert_eq!(draft.blocker(), None);
    }

    #[test]
    fn json_zero_infer_rows_means_scan_everything() {
        let mut draft = ConfigureDraft {
            format: FormatId::Json,
            ..csv_draft()
        };
        draft.json_infer_rows = 0;
        let SourceFormat::Json(json) = draft.def(Path::new("/project")).format else {
            panic!("json");
        };
        assert_eq!(
            json.infer_rows, None,
            "0 is the unbounded scan, not a request for no columns"
        );
    }

    /// The round trip that was broken: an unbounded def has to survive being opened and saved.
    /// It used to come back `Some(1000)` — a silent cap from a dialog the user only looked at.
    #[test]
    fn an_unbounded_json_def_survives_a_no_op_save() {
        let def = TableDef {
            name: "t".into(),
            format: SourceFormat::Json(JsonRead::default()),
            connection: None,
            sources: vec!["/data".into()],
            partition_cols: vec![],
            origin: TableOrigin::External,
        };
        assert!(matches!(def.format, SourceFormat::Json(ref o) if o.infer_rows.is_none()));

        let draft = ConfigureDraft::of(&def, &[]);
        assert_eq!(
            draft.json_infer_rows, 0,
            "unset opens as the unbounded scan"
        );
        let SourceFormat::Json(saved) = draft.def(Path::new("/project")).format else {
            panic!("json");
        };
        assert_eq!(saved.infer_rows, None, "and saving it back changes nothing");
    }

    #[test]
    fn partition_columns_with_the_toggle_off_are_not_partitioning() {
        let mut draft = ConfigureDraft {
            local_sources: vec!["/data/year=*/".into()],
            partitions: vec![("year".into(), "Utf8".into())],
            ..csv_draft()
        };
        assert!(draft.effective_partitions().is_empty());
        draft.hive_on = true;
        assert_eq!(draft.effective_partitions().len(), 1);
    }

    #[test]
    fn a_partitioned_def_keeps_its_columns_when_its_sources_are_project_relative() {
        let def = TableDef {
            name: "events".into(),
            format: SourceFormat::Parquet,
            connection: None,
            sources: vec!["data/events".into()],
            partition_cols: vec![("year".into(), "Int32".into())],
            origin: TableOrigin::External,
        };
        let draft = ConfigureDraft::of(&def, &[]);
        assert!(draft.hive_on, "the def has columns, so the toggle opens on");
        assert!(
            draft.may_partition(Path::new("/nowhere")),
            "and the section shows them"
        );
        assert_eq!(
            draft.def(Path::new("/project")).partition_cols,
            def.partition_cols
        );
    }

    #[test]
    fn the_toolbars_row_actions_keep_the_selection_inside_the_list() {
        let mut draft = ConfigureDraft::default();
        assert!(
            draft.local_sources.is_empty(),
            "a new table has no path rows"
        );
        assert_eq!(draft.add_path(), 0);
        assert_eq!(draft.add_path(), 1);
        assert_eq!(draft.add_path(), 2);
        assert_eq!(
            (draft.local_sources.len(), draft.clamp_selection(2)),
            (3, 2)
        );
        let mut at = 2;
        for _ in 0..3 {
            at = draft.remove_path(at);
        }
        assert_eq!((draft.local_sources.len(), at), (0, 0));
        assert_eq!(draft.remove_path(0), 0);
        assert!(draft.local_sources.is_empty());
    }

    #[test]
    fn a_multi_file_pick_lands_as_one_row_each() {
        let mut draft = ConfigureDraft::default();
        draft.set_paths(0, vec!["/a".into(), "/b".into(), "/c".into()]);
        assert_eq!(draft.local_sources, vec!["/a", "/b", "/c"]);
        draft.set_paths(1, vec!["/x".into()]);
        assert_eq!(draft.local_sources, vec!["/a", "/x", "/c"]);
    }

    #[test]
    fn the_hive_switch_stays_reachable_once_it_is_on() {
        let root = Path::new("/project");
        let mut draft = ConfigureDraft {
            local_sources: vec!["/data/one.parquet".into()],
            hive_on: true,
            ..csv_draft()
        };
        assert!(draft.may_partition(root), "the switch must stay on screen");
        assert!(draft.blocker().is_some_and(|b| b.contains("key=value")));
        draft.hive_on = false;
        assert!(draft.blocker().is_none(), "and switching it off frees Save");
    }

    #[test]
    fn only_a_many_file_path_can_be_partitioned() {
        let root = Path::new("/project");
        let single = ConfigureDraft {
            local_sources: vec!["/data/one.parquet".into()],
            ..Default::default()
        };
        assert!(!single.may_partition(root));
        for many in ["/data/year=*/", "/data/2024/", "/data/**/*.parquet"] {
            let draft = ConfigureDraft {
                local_sources: vec![many.into()],
                ..Default::default()
            };
            assert!(draft.may_partition(root), "{many}");
        }
    }

    #[test]
    fn a_relative_source_is_asked_about_where_the_project_actually_is() {
        let draft = ConfigureDraft {
            local_sources: vec!["events/year=2024/".into()],
            ..Default::default()
        };
        assert_eq!(
            draft.resolved_sources(Path::new("/project")),
            vec!["/project/events/year=2024/"]
        );
        assert!(draft.may_partition(Path::new("/project")));
    }

    #[test]
    fn the_cast_warning_is_up_while_any_column_is_still_text() {
        let mut draft = ConfigureDraft {
            hive_on: true,
            partitions: vec![("year".into(), "Int32".into())],
            ..Default::default()
        };
        assert!(!draft.partitions_are_text());
        draft.partitions.push(("month".into(), "Utf8".into()));
        assert!(draft.partitions_are_text());
    }

    #[test]
    fn the_title_names_what_is_being_configured() {
        assert_eq!(ConfigureTarget::New.title(), "New table");
        assert_eq!(
            ConfigureTarget::Edit("events".into()).title(),
            "Configure events"
        );
    }

    /// **The headline**: a table over a connection writes the connection's URL and a
    /// bucket-relative source, and that pair resolves to the address the engine reads.
    ///
    /// The source is stored **as typed** — `relativize` measures against the project folder,
    /// which a bucket has nothing to do with, and a path it happened to mangle would register as
    /// a different object.
    #[test]
    fn a_table_over_a_connection_stores_the_url_and_a_bucket_relative_path() {
        let root = Path::new("/project");
        let mut draft = csv_draft();
        draft.set_location(Where::Remote, &connections());
        draft.set_path(0, "events/2024/**/*.parquet".into());

        let def = draft.def(root);
        assert_eq!(def.connection.as_deref(), Some("acme_lake"));
        assert_eq!(def.sources, ["events/2024/**/*.parquet"]);
        assert_eq!(
            draft.resolved_sources(root),
            ["events/2024/**/*.parquet"],
            "a remote source stays as it is stored — what it resolves to is composed where the \
             store is registered"
        );
    }

    /// **Each location keeps its own paths, and neither is written for the user** — the flip
    /// picks the provider's first connection and settles nothing else, so an empty box is what
    /// blocks Save.
    #[test]
    fn each_location_keeps_its_own_paths_and_neither_is_seeded() {
        let mut draft = ConfigureDraft {
            local_sources: vec!["   ".into(), "/data/a.csv".into(), "/data/b.csv".into()],
            ..csv_draft()
        };
        draft.set_location(Where::Remote, &connections());

        assert_eq!(draft.connection.as_deref(), Some("acme_lake"));
        assert_eq!(
            draft.path_count(),
            1,
            "the one box that arm is built around"
        );
        assert_eq!(draft.path_at(0), "", "and nothing typed into it");
        assert!(draft.nonblank_sources().is_empty());
        assert!(draft
            .blocker()
            .is_some_and(|why| why.contains("source path")));

        draft.set_path(0, "events/".into());
        assert_eq!(draft.nonblank_sources(), ["events/"]);
        assert_eq!(
            draft.local_sources,
            ["   ", "/data/a.csv", "/data/b.csv"],
            "the disk's list is whole, blank row and all"
        );

        draft.set_location(Where::Local, &connections());
        assert_eq!(draft.nonblank_sources(), ["/data/a.csv", "/data/b.csv"]);
        assert_eq!(draft.remote_source, "events/");
    }

    /// Picking a **TYPE** re-points the connection unless that provider already serves the one
    /// chosen — the picker only offers its own, so a foreign selection would be a choice with no
    /// row to show it.
    #[test]
    fn picking_a_provider_lands_on_one_of_its_connections() {
        let connections = connections();
        let mut draft = csv_draft();
        draft.set_location(Where::Remote, &connections);
        assert_eq!(draft.connection.as_deref(), Some("acme_lake"));

        draft.set_provider(ProviderId::Gcs, &connections);
        assert_eq!(draft.connection.as_deref(), Some("warehouse"));

        draft.set_provider(ProviderId::S3, &connections);
        draft.connection = Some("cold_store".into());
        draft.set_provider(ProviderId::S3, &connections);
        assert_eq!(draft.connection.as_deref(), Some("cold_store"));

        draft.set_provider(ProviderId::Http, &connections);
        assert_eq!(draft.connection, None);
        assert!(draft
            .blocker()
            .is_some_and(|why| why.contains("needs a connection")));
    }

    /// A connection is **remembered** across a flip back to the local disk, and does not reach
    /// the def while it is not the table's location — the two fields say different things.
    #[test]
    fn a_remembered_connection_is_not_a_location() {
        let connections = connections();
        let mut draft = csv_draft();
        draft.set_location(Where::Remote, &connections);
        draft.set_location(Where::Local, &connections);

        assert_eq!(
            draft.connection.as_deref(),
            Some("acme_lake"),
            "coming back must not have to pick it again"
        );
        assert_eq!(draft.store(), None, "but the table reads from disk");
        assert_eq!(draft.def(Path::new("/project")).connection, None);
    }

    /// A def naming a connection opens on it, **on that connection's provider** — which is the
    /// connection's fact, read off the project's list rather than parsed out of the URL.
    #[test]
    fn a_remote_def_opens_on_its_connections_provider() {
        let def = TableDef {
            name: "events".into(),
            format: SourceFormat::Parquet,
            connection: Some("warehouse".into()),
            sources: vec!["events/".into()],
            partition_cols: vec![],
            origin: TableOrigin::External,
        };
        let draft = ConfigureDraft::of(&def, &connections());
        assert!(draft.remote());
        assert_eq!(draft.provider, ProviderId::Gcs);
        assert_eq!(draft.connection.as_deref(), Some("warehouse"));
        assert_eq!(draft.def(Path::new("/project")), def);
    }

    /// A def whose connection this project no longer has **keeps the reference**: rewriting it to
    /// "local disk" would silently re-point the table at a relative path on the user's own machine.
    /// The window says so and blocks Save instead (`views::footer`).
    #[test]
    fn a_def_over_a_forgotten_connection_keeps_naming_it() {
        let def = TableDef {
            name: "events".into(),
            format: SourceFormat::Parquet,
            connection: Some("s3://gone".into()),
            sources: vec!["events/".into()],
            partition_cols: vec![],
            origin: TableOrigin::External,
        };
        let draft = ConfigureDraft::of(&def, &connections());
        assert!(draft.remote());
        assert_eq!(draft.connection.as_deref(), Some("s3://gone"));
        assert_eq!(draft.def(Path::new("/project")), def);
    }

    /// The picker offers one provider's connections, by URL and in the project's order — and
    /// answers "none" for a provider with none, which is what the empty line under it says.
    #[test]
    fn the_picker_offers_one_providers_connections() {
        let connections = connections();
        assert_eq!(
            connections_for(&connections, ProviderId::S3),
            ["acme_lake", "cold_store"]
        );
        assert_eq!(
            connections_for(&connections, ProviderId::Gcs),
            ["warehouse"]
        );
        assert!(connections_for(&connections, ProviderId::Http).is_empty());
    }
}

/// LOCATION ▸ **Internal** (IT-01) — the draft half: what the third answer does to the rest of
/// the form, what it composes, and what it refuses.
#[cfg(test)]
mod internal_tests {
    use super::*;

    /// An internal draft with the given columns, in order.
    fn draft(name: &str, columns: &[(&str, &str)]) -> ConfigureDraft {
        let mut draft = ConfigureDraft {
            name: name.into(),
            ..Default::default()
        };
        draft.set_location(Where::Internal, &[]);
        draft.columns.clear();
        for (column, sql_type) in columns {
            let at = draft.add_column();
            draft.set_column_name(at, (*column).into());
            draft.set_column_type(at, (*sql_type).into());
        }
        draft
    }

    /// Everything asked about, answered `Ok` with the given Arrow spelling.
    fn answered(pairs: &[(&str, &str)]) -> Probes {
        pairs
            .iter()
            .map(|(sql, arrow)| ((*sql).to_string(), Ok((*arrow).to_string())))
            .collect()
    }

    /// **The third answer starts with a row to type into**, and takes the file questions off the
    /// table — the whole reason this is a LOCATION rather than a surface of its own.
    #[test]
    fn moving_to_internal_opens_a_column_and_silences_the_file_sections() {
        let mut draft = ConfigureDraft {
            name: "t".into(),
            local_sources: vec!["/data/t.parquet".into()],
            hive_on: true,
            partitions: vec![("year".into(), "Int32".into())],
            ..Default::default()
        };
        draft.set_location(Where::Internal, &[]);

        assert!(draft.internal());
        assert!(!draft.remote());
        assert_eq!(draft.columns.len(), 1, "somewhere to type");
        assert!(draft.options().is_empty(), "nothing to say about reading");
        assert!(!draft.may_partition(Path::new("/tmp")));
        assert_eq!(draft.store(), None);
        assert_eq!(draft.local_sources, vec!["/data/t.parquet".to_string()]);
    }

    #[test]
    fn an_internal_draft_composes_the_statement_save_runs() {
        let draft = draft("sales", &[("region", "VARCHAR"), ("amount", "DOUBLE")]);
        assert_eq!(
            draft.create_statement().expect("composes"),
            "CREATE TABLE \"sales\" (\n  \"region\" VARCHAR,\n  \"amount\" DOUBLE\n);"
        );
    }

    /// Names are quoted verbatim, embedded quotes doubled — what was typed is what the column is
    /// called, reserved words and capitals included.
    #[test]
    fn names_are_quoted_exactly_as_they_were_typed() {
        let draft = draft("My Table", &[("Order", "INT"), ("say \"hi\"", "VARCHAR")]);
        assert_eq!(
            draft.create_statement().expect("composes"),
            "CREATE TABLE \"My Table\" (\n  \"Order\" INT,\n  \"say \"\"hi\"\"\" VARCHAR\n);"
        );
    }

    #[test]
    fn nothing_composes_until_there_is_a_name_and_a_whole_column() {
        assert_eq!(draft("", &[("a", "INT")]).create_statement(), None);
        assert_eq!(draft("t", &[("a", "")]).create_statement(), None);
        assert_eq!(draft("t", &[("", "INT")]).create_statement(), None);
        assert_eq!(draft("t", &[]).create_statement(), None);
    }

    #[test]
    fn a_half_typed_row_names_the_box_that_is_empty() {
        let draft = draft("t", &[("a", "INT"), ("", "VARCHAR"), ("c", "")]);
        let faults = draft.column_faults(&answered(&[("INT", "Int32"), ("VARCHAR", "Utf8")]));
        assert_eq!(faults[&1], "Enter a column name.");
        assert_eq!(faults[&2], "Enter a column type.");
        assert!(!faults.contains_key(&0));
    }

    /// The duplicate rule is the **engine's** fold and its wording is the create arm's — a form
    /// that answered either in its own terms would be a second copy of one rule. The arm names
    /// the *second* spelling, because its fold errors on the field that repeats.
    #[test]
    fn a_repeated_name_is_the_create_arms_own_refusal_and_marks_both_rows() {
        let probes = answered(&[("VARCHAR", "Utf8")]);
        let repeated = draft("t", &[("Region", "VARCHAR"), ("region", "VARCHAR")]);
        let faults = repeated.column_faults(&probes);
        assert_eq!(faults.len(), 2, "either row is the one to fix");
        assert_eq!(faults[&0], duplicate_column("region"));
        assert_eq!(faults[&1], duplicate_column("region"));

        let spaced = draft("t", &[("my col", "VARCHAR"), ("MY COL", "VARCHAR")]);
        assert!(spaced.column_faults(&probes).is_empty());
    }

    /// The planner's refusal reaches the row as written; nothing paraphrases it.
    #[test]
    fn a_refused_type_carries_the_planners_own_words() {
        let draft = draft("t", &[("size", "FLOAT64")]);
        let probes: Probes = [(
            "FLOAT64".to_string(),
            Err("Unsupported SQL type FLOAT64".to_string()),
        )]
        .into_iter()
        .collect();
        assert_eq!(
            draft.column_faults(&probes)[&0],
            "Unsupported SQL type FLOAT64"
        );
    }

    /// The probe work list is the distinct unanswered spellings — two `VARCHAR` rows are one
    /// question, and an answered one is never asked again.
    #[test]
    fn the_probe_work_list_is_the_distinct_unanswered_types() {
        let draft = draft(
            "t",
            &[
                ("a", "VARCHAR"),
                ("b", " VARCHAR "),
                ("c", "INT"),
                ("d", ""),
            ],
        );
        assert_eq!(draft.unprobed(&Probes::new()), ["VARCHAR", "INT"]);
        assert_eq!(draft.unprobed(&answered(&[("VARCHAR", "Utf8")])), ["INT"]);
        assert!(draft
            .unprobed(&answered(&[("VARCHAR", "Utf8"), ("INT", "Int32")]))
            .is_empty());
    }

    /// The draft's own blocker stops at the column count; what a *type* means is the planner's,
    /// so the footer asks that (`views::footer::column_fault`).
    #[test]
    fn the_blocker_names_the_next_thing_to_do() {
        assert_eq!(
            draft("", &[("a", "INT")]).blocker().as_deref(),
            Some("A table needs a name.")
        );
        assert_eq!(
            draft("t", &[]).blocker().as_deref(),
            Some("A table needs at least one column.")
        );
        assert_eq!(draft("t", &[("a", "INT")]).blocker(), None);
    }

    /// One row always remains, so there is somewhere to type — removing the last empties it.
    #[test]
    fn the_column_list_always_has_a_row() {
        let mut draft = draft("t", &[("a", "INT")]);
        assert_eq!(draft.remove_column(0), 0);
        assert_eq!(draft.columns.len(), 1);
        assert!(draft.columns[0].is_blank());
    }
}
