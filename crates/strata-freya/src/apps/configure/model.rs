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

use std::path::Path;

use strata_core::project::resolve_source;
use strata_model::{CsvRead, FileCompression, JsonRead, JsonShape, SourceFormat, TableDef};

use crate::components::form::{one_char, Choice, Control, Group, Make, TextField};

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
    /// Source paths, in the order they were added. A blank row is a row being typed, not a
    /// path — [`nonblank_sources`](Self::nonblank_sources) is what anything downstream reads.
    pub sources: Vec<String>,
    /// Which row the toolbar's remove and browse act on.
    pub selected: usize,
    // --- CSV ---
    pub csv_header: bool,
    pub csv_delimiter: String,
    pub csv_quote: String,
    pub csv_escape: String,
    pub csv_comment: String,
    pub csv_newlines: bool,
    pub csv_truncated: bool,
    pub csv_infer_rows: u32,
    pub csv_compression: FileCompression,
    // --- JSON ---
    pub json_shape: JsonShape,
    pub json_infer_rows: u32,
    pub json_compression: FileCompression,
    // --- Hive ---
    pub hive_on: bool,
    /// The partition columns and the type each is read as, outermost first.
    pub partitions: Vec<(String, String)>,
}

impl Default for ConfigureDraft {
    /// A new table: parquet, one empty path row, every read option at its reader's default.
    fn default() -> Self {
        let csv = CsvRead::default();
        let json = JsonRead::default();
        Self {
            name: String::new(),
            format: FormatId::Parquet,
            sources: vec![String::new()],
            selected: 0,
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
        }
    }
}

impl ConfigureDraft {
    /// Seed the draft from an existing def — every field it holds, so the window opens showing
    /// what is really stored and Save with nothing touched is a no-op.
    ///
    /// The formats it *isn't* in keep their defaults: the def has nothing to say about them.
    pub fn of(def: &TableDef) -> Self {
        let mut draft = Self {
            name: def.name.clone(),
            format: FormatId::of(&def.format),
            sources: match def.sources.is_empty() {
                true => vec![String::new()],
                false => def.sources.clone(),
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
                draft.json_infer_rows = o.infer_rows.unwrap_or(DEFAULT_INFER_ROWS as usize) as u32;
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

    // --- source paths ---

    /// The paths as the **engine** will see them: resolved against the project folder, because
    /// a def's sources are stored project-relative where they sit inside it.
    ///
    /// Anything that asks the filesystem a question about a source has to ask about this, not
    /// about the stored string — `is_dir` on a relative path answers about the process's working
    /// directory, which is not the project's.
    pub fn resolved_sources(&self, root: &Path) -> Vec<String> {
        self.nonblank_sources()
            .iter()
            .map(|p| resolve_source(root, p))
            .collect()
    }

    /// The paths that are actually paths. A blank row is a row being typed.
    pub fn nonblank_sources(&self) -> Vec<String> {
        self.sources
            .iter()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect()
    }

    /// The row the toolbar acts on, clamped — the list shrinks under the selection.
    pub fn selected(&self) -> usize {
        self.selected.min(self.sources.len().saturating_sub(1))
    }

    pub fn add_path(&mut self) {
        self.sources.push(String::new());
        self.selected = self.sources.len() - 1;
    }

    pub fn remove_path(&mut self) {
        if self.sources.is_empty() {
            return;
        }
        let at = self.selected();
        self.sources.remove(at);
        self.selected = at.min(self.sources.len().saturating_sub(1));
    }

    /// Put `paths` into the list at the selection: the first replaces the selected row (or
    /// becomes the first row when the list is empty), the rest are inserted after it.
    ///
    /// Multi-select is the picker's, not a flourish: a table *is* many paths, and picking five
    /// files one dialog at a time is the same five rows with four more dialogs.
    pub fn set_paths(&mut self, paths: Vec<String>) {
        if paths.is_empty() {
            return;
        }
        if self.sources.is_empty() {
            self.sources = paths;
            self.selected = 0;
            return;
        }
        let at = self.selected();
        self.sources.splice(at..=at, paths.iter().cloned());
        self.selected = at + paths.len() - 1;
    }

    /// Whether the Hive section has anything to offer: a partition layout only exists under a
    /// path that resolves to *many* files, so a table of single files is never partitioned.
    ///
    /// **Not gated on parquet**, unlike the canvas. Partition columns are a listing feature, not
    /// a parquet one — DataFusion reads a Hive-partitioned CSV lake perfectly well, and
    /// `TableDef.partition_cols` has always been format-agnostic. Gating the section would hide
    /// a def's own stored columns the moment its format changed.
    pub fn may_partition(&self, root: &Path) -> bool {
        // A def that already carries partition columns always shows them, whatever its paths
        // look like from here — they are the user's decision, and hiding the section would hide
        // a value that is still being saved.
        !self.partitions.is_empty()
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

    // --- the def ---

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
                // Floored at 1: `JsonFormat::infer_schema` breaks out before reading anything
                // at 0, leaving a table with no columns. CSV's 0 means "read all as text";
                // JSON's would mean "no table".
                infer_rows: Some(self.json_infer_rows.max(1) as usize),
                compression: self.json_compression,
            }),
        }
    }

    /// The character the delimiter box names, or `None` when it is blank or unresolvable.
    fn delimiter_char(&self) -> Option<char> {
        one_char("delimiter", &self.csv_delimiter).ok().flatten()
    }

    /// The def this draft describes.
    pub fn def(&self) -> TableDef {
        TableDef {
            name: self.name.trim().to_string(),
            format: self.source_format(),
            sources: self.nonblank_sources(),
            partition_cols: self.effective_partitions(),
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
        if self.nonblank_sources().is_empty() {
            return Some("A table needs at least one source path.".into());
        }
        if let FormatId::Unknown(name) = &self.format {
            return Some(format!(
                "'{name}' is not a format Strata can read. Choose another."
            ));
        }
        if self.format == FormatId::Csv {
            // The box's own complaint when it holds something that is not one character, and
            // "it can't be empty" when it holds nothing.
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

    // --- import options ---

    /// Whether this format has read options at all — parquet and Arrow have none worth showing.
    pub fn has_options(&self) -> bool {
        !self.options().is_empty()
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
                    // Kept **inline**, unlike the two below: this sentence changes with the
                    // switch, so it reports the current state rather than explaining the option
                    // — which is a thing a hover tip cannot be, because it is not there to read
                    // while you decide.
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
                    // One free-text box rather than a pill of the four common separators plus a
                    // custom slot: the export window asks the same question this way, and a
                    // control that means one thing in one window should not mean another next
                    // door. It also takes the escapes a pill cannot.
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
                // A `Select`, like every other closed list of values in this window.
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
            // **Nothing at all** for parquet and Arrow — not even a note saying so. A block
            // headed PARQUET OPTIONS whose only content explains that there are none reads as a
            // section that failed to load. (Export shows a note in the same position, but there
            // it sits among real option groups; here it would be the whole block.) The parquet
            // read options that *are* per-table are their own task.
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
                    hint: Some("Records scanned to infer the schema"),
                    control: Control::Num {
                        value: self.json_infer_rows,
                        // Floored at 1, unlike CSV: zero here is a table with no columns.
                        min: 1,
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
    use super::*;

    fn labels(groups: &[Group<Edit>]) -> Vec<String> {
        groups.iter().map(|g| g.label.clone()).collect()
    }

    fn csv_draft() -> ConfigureDraft {
        ConfigureDraft {
            name: "events".into(),
            format: FormatId::Csv,
            sources: vec!["/data/events.csv".into()],
            ..Default::default()
        }
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
        // Not even a note: a block headed PARQUET OPTIONS whose only content says there are
        // none reads as a section that failed to load.
        for format in [FormatId::Parquet, FormatId::Arrow] {
            let draft = ConfigureDraft {
                format,
                ..csv_draft()
            };
            assert!(draft.options().is_empty());
            assert!(!draft.has_options());
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
        let SourceFormat::Csv(csv) = draft.def().format else {
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
            sources: vec!["/data".into()],
            partition_cols: vec![],
        };
        assert_eq!(ConfigureDraft::of(&def).csv_delimiter, "\\t");
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
        let SourceFormat::Csv(csv) = draft.def().format else {
            panic!("csv");
        };
        assert_eq!(csv.delimiter, ';');

        draft.format = FormatId::Parquet;
        assert_eq!(draft.def().format, SourceFormat::Parquet);
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
        // Opening Configure on a table and pressing Save without touching anything must produce
        // the def that was already there — the whole reason the draft is seeded field by field.
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
            sources: vec!["/data/a".into(), "/data/b".into()],
            partition_cols: vec![("year".into(), "Int32".into())],
        };
        // The Hive section is only offered for a path that resolves to many files; a def that
        // already has partition columns keeps them regardless, which is what this asserts.
        let mut draft = ConfigureDraft::of(&def);
        draft.sources = vec!["/data/year=*/".into()];
        let round = draft.def();
        assert_eq!(round.format, def.format);
        assert_eq!(round.partition_cols, def.partition_cols);
    }

    #[test]
    fn a_legacy_defs_unset_infer_rows_opens_showing_the_engine_default() {
        let def = TableDef {
            name: "t".into(),
            format: SourceFormat::Csv(CsvRead::default()),
            sources: vec!["/data".into()],
            partition_cols: vec![],
        };
        assert_eq!(ConfigureDraft::of(&def).csv_infer_rows, DEFAULT_INFER_ROWS);
    }

    #[test]
    fn an_unreadable_format_opens_as_itself_and_blocks_save() {
        let def = TableDef {
            name: "legacy".into(),
            format: SourceFormat::Unknown("avro".into()),
            sources: vec!["/data".into()],
            partition_cols: vec![],
        };
        let draft = ConfigureDraft::of(&def);
        assert_eq!(draft.format, FormatId::Unknown("avro".into()));
        assert!(draft.blocker().is_some_and(|b| b.contains("avro")));
    }

    #[test]
    fn a_blank_name_or_no_path_blocks_save() {
        let mut draft = ConfigureDraft::default();
        assert!(draft.blocker().is_some_and(|b| b.contains("name")));
        draft.name = "t".into();
        assert!(draft.blocker().is_some_and(|b| b.contains("source path")));
        draft.sources = vec!["   ".into(), "/data".into()];
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
    fn json_infer_rows_never_reaches_the_engine_as_zero() {
        let mut draft = ConfigureDraft {
            format: FormatId::Json,
            ..csv_draft()
        };
        draft.json_infer_rows = 0;
        let SourceFormat::Json(json) = draft.def().format else {
            panic!("json");
        };
        assert_eq!(
            json.infer_rows,
            Some(1),
            "zero would mean no columns at all"
        );
    }

    #[test]
    fn partition_columns_with_the_toggle_off_are_not_partitioning() {
        let mut draft = ConfigureDraft {
            sources: vec!["/data/year=*/".into()],
            partitions: vec![("year".into(), "Utf8".into())],
            ..csv_draft()
        };
        assert!(draft.effective_partitions().is_empty());
        draft.hive_on = true;
        assert_eq!(draft.effective_partitions().len(), 1);
    }

    #[test]
    fn a_partitioned_def_keeps_its_columns_when_its_sources_are_project_relative() {
        // `sources` are stored relative to the *project* folder, so a filesystem probe from
        // here answers about the process's working directory. Gating the saved value on it lost
        // the columns of any table whose path did not happen to resolve.
        let def = TableDef {
            name: "events".into(),
            format: SourceFormat::Parquet,
            sources: vec!["data/events".into()],
            partition_cols: vec![("year".into(), "Int32".into())],
        };
        let draft = ConfigureDraft::of(&def);
        assert!(draft.hive_on, "the def has columns, so the toggle opens on");
        assert!(
            draft.may_partition(Path::new("/nowhere")),
            "and the section shows them"
        );
        assert_eq!(draft.def().partition_cols, def.partition_cols);
    }

    #[test]
    fn the_toolbars_row_actions_keep_the_selection_inside_the_list() {
        let mut draft = ConfigureDraft::default();
        draft.add_path();
        draft.add_path();
        assert_eq!((draft.sources.len(), draft.selected()), (3, 2));
        draft.remove_path();
        draft.remove_path();
        draft.remove_path();
        assert_eq!((draft.sources.len(), draft.selected()), (0, 0));
        // Removing from an empty list is a no-op, not a panic.
        draft.remove_path();
        assert!(draft.sources.is_empty());
    }

    #[test]
    fn a_multi_file_pick_lands_as_one_row_each() {
        let mut draft = ConfigureDraft::default();
        draft.set_paths(vec!["/a".into(), "/b".into(), "/c".into()]);
        assert_eq!(draft.sources, vec!["/a", "/b", "/c"]);
        // …replacing the row it was invoked on, not appending blindly.
        draft.selected = 1;
        draft.set_paths(vec!["/x".into()]);
        assert_eq!(draft.sources, vec!["/a", "/x", "/c"]);
    }

    #[test]
    fn only_a_many_file_path_can_be_partitioned() {
        let root = Path::new("/project");
        let single = ConfigureDraft {
            sources: vec!["/data/one.parquet".into()],
            ..Default::default()
        };
        assert!(!single.may_partition(root));
        for many in ["/data/year=*/", "/data/2024/", "/data/**/*.parquet"] {
            let draft = ConfigureDraft {
                sources: vec![many.into()],
                ..Default::default()
            };
            assert!(draft.may_partition(root), "{many}");
        }
    }

    #[test]
    fn a_relative_source_is_asked_about_where_the_project_actually_is() {
        // `sources` are stored relative to the project folder, so every filesystem question
        // about one has to be resolved first — otherwise it is answered about the process's
        // working directory, and a perfectly ordinary partitioned folder looks like a file.
        let draft = ConfigureDraft {
            sources: vec!["events/year=2024/".into()],
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
}
