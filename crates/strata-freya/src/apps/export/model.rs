//! The export window's data: what is being exported ([`ExportTarget`]), what the user has
//! chosen ([`ExportDraft`]), and the **option groups** those two produce.
//!
//! The groups are the point of D6. The Dioxus modal reached the same screen through hardcoded
//! `match` arms per format; here [`ExportDraft::groups`] returns a `Vec<Group>` and the view
//! renders whatever it is handed, so a new option is a row in a table rather than a new branch
//! in a component — and the whole surface is unit-testable without a renderer.
//!
//! **Every option carries the edit it performs.** A `Choice` holds an [`Edit`], a text field
//! holds `fn(String) -> Edit`. So there is no key/value pairing for a view to get wrong: the
//! only thing a control can do is apply the edit it was built with, and
//! [`ExportDraft::apply`] is exhaustive over `Edit`.
//!
//! **The draft keeps every format's options side by side**, while
//! [`strata_core::engine::export::Format`] keeps only the active format's — deliberately
//! different shapes. Switching to Parquet and back must not forget the delimiter you set, so
//! the draft remembers; the engine spec must not be able to name a delimiter on a Parquet
//! export, so [`ExportDraft::spec`] projects only the format in play.

use strata_core::engine::export::{
    Codec, Compression, Csv, ExportSpec, Format, Json, Parquet, Partition, Scope, Statistics,
    WriterVersion,
};
use strata_model::{Cell, ColumnInfo, Kind, SnapshotId};

use crate::components::form::{self, one_char, Make};

/// The shared option vocabulary (`components::form::options`), at this window's edit type.
/// Aliases rather than re-declarations: the export window was the first consumer of these and
/// P4-11 is the second, so the shapes live in the form module and each surface names its own
/// `Edit`.
pub type Choice = form::Choice<Edit>;
pub type TextField = form::TextField<Edit>;
pub type Group = form::Group<Edit>;
pub type Control = form::Control<Edit>;

/// What this window is exporting — every field read from the run that opened it, and none of
/// it editable. Immutable because the snapshot is: the window pins it for its whole life
/// (SNAPSHOT_SPEC §4), so these facts stay true even if the tab behind re-runs.
#[derive(Clone, PartialEq)]
pub struct ExportTarget {
    pub snapshot: SnapshotId,
    pub columns: Vec<ColumnInfo>,
    /// Rows in the whole snapshot — read from the run, never counted from the grid.
    pub total: usize,
    /// The grid's active sort, so the file matches what is on screen.
    pub sort: Option<(String, bool)>,
    /// The page the grid is showing, for `Scope::Page`.
    pub page: usize,
    pub page_size: usize,
    /// The tab's name, for the window subtitle.
    pub label: String,
    /// The rows already in hand (the page the grid rendered), for the preview. Real values —
    /// never a fabricated sample.
    pub sample: Vec<Vec<Cell>>,
}

impl ExportTarget {
    /// The window's subtitle: the run's name and its shape.
    pub fn subtitle(&self) -> String {
        let cols = self.columns.len();
        let unit = if cols == 1 { "column" } else { "columns" };
        format!("{} · {cols} {unit}", self.label)
    }

    /// The columns a Hive export may partition on: **numeric or string only**. Timestamps,
    /// booleans and nested containers are excluded — a directory name has to be a short,
    /// stable scalar, and a struct has no sensible one.
    pub fn partitionable(&self) -> Vec<&ColumnInfo> {
        self.columns
            .iter()
            .filter(|c| matches!(c.kind, Kind::Num | Kind::Str))
            .collect()
    }
}

/// Which format's card is selected. Separate from
/// [`Format`](strata_core::engine::export::Format) because this one is a *choice* — it has no
/// options attached, and it round-trips through the format cards.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FormatId {
    Csv,
    Json,
    Parquet,
    Arrow,
}

impl FormatId {
    pub const ALL: [FormatId; 4] = [Self::Csv, Self::Json, Self::Parquet, Self::Arrow];

    pub fn name(self) -> &'static str {
        match self {
            Self::Csv => "CSV",
            Self::Json => "JSON",
            Self::Parquet => "Parquet",
            Self::Arrow => "Arrow IPC",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Csv => "Comma-separated text",
            Self::Json => "Newline-delimited (NDJSON)",
            Self::Parquet => "Columnar, compressed",
            Self::Arrow => "Zero-copy Feather",
        }
    }

    /// The destination's extension, before any compression suffix.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Csv => "csv",
            Self::Json => "json",
            Self::Parquet => "parquet",
            Self::Arrow => "arrow",
        }
    }
}

/// Whether the whole snapshot or just the page on screen is written.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ScopeChoice {
    All,
    Page,
}

/// What text stands in for a NULL cell. `Custom` defers to the draft's own custom string,
/// which is why the choice and the text are separate fields.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NullChoice {
    Empty,
    Null,
    NaN,
    Custom,
}

/// A Parquet codec *without* its level — the level lives beside it on the draft, because
/// switching zstd → snappy → zstd must not forget the level you set.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CodecChoice {
    Zstd,
    Snappy,
    Gzip,
    Brotli,
    Lz4,
    Uncompressed,
}

impl CodecChoice {
    /// The level range this codec accepts, or `None` for the codecs that take none — which
    /// is what makes the COMPRESSION LEVEL group appear and disappear.
    pub fn levels(self) -> Option<(u32, u32)> {
        match self {
            Self::Zstd => Some((1, 22)),
            Self::Gzip => Some((1, 9)),
            Self::Brotli => Some((1, 11)),
            Self::Snappy | Self::Lz4 | Self::Uncompressed => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Zstd => "Zstd",
            Self::Snappy => "Snappy",
            Self::Gzip => "Gzip",
            Self::Brotli => "Brotli",
            Self::Lz4 => "LZ4",
            Self::Uncompressed => "None",
        }
    }
}

/// One thing a control can do to the draft. Exhaustive: [`ExportDraft::apply`] matches every
/// variant, and a control is built holding the exact edit it performs — so a control can never
/// write the wrong field.
#[derive(Clone, PartialEq, Debug)]
pub enum Edit {
    Scope(ScopeChoice),
    CsvHeader(bool),
    CsvDelimiter(String),
    CsvNull(NullChoice),
    CsvNullCustom(String),
    CsvQuote(String),
    CsvEscape(String),
    CsvDoubleQuote(bool),
    CsvCompression(Compression),
    JsonCompression(Compression),
    PqCodec(CodecChoice),
    PqLevel(u32),
    PqStatistics(Statistics),
    PqRowGroup(usize),
    PqWriterVersion(WriterVersion),
    PqDictionary(bool),
}

/// Which columns a Hive export fans out on, and whether they stay in the files.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct PartitionDraft {
    pub enabled: bool,
    /// Directory levels, outermost first — the order the user arranged.
    pub columns: Vec<String>,
    pub keep_columns: bool,
    /// The AVAILABLE pane's filter.
    pub filter: String,
}

impl PartitionDraft {
    /// What actually reaches the engine: a selection with the toggle off is no partitioning
    /// at all. One helper so preview, destination and spec can't disagree (the canvas's
    /// `_effParts`, which exists because they once did).
    pub fn effective(&self) -> Vec<String> {
        if self.enabled {
            self.columns.clone()
        } else {
            Vec::new()
        }
    }

    pub fn is_active(&self) -> bool {
        !self.effective().is_empty()
    }
}

/// Everything the user has chosen. Defaults match the canvas's initial state, which in turn
/// match DataFusion's own.
#[derive(Clone, PartialEq, Debug)]
pub struct ExportDraft {
    pub format: FormatId,
    pub scope: ScopeChoice,
    pub csv_header: bool,
    pub csv_delimiter: String,
    pub csv_null: NullChoice,
    pub csv_null_custom: String,
    pub csv_quote: String,
    pub csv_escape: String,
    pub csv_double_quote: bool,
    pub csv_compression: Compression,
    pub json_compression: Compression,
    pub pq_codec: CodecChoice,
    pub pq_level: u32,
    pub pq_statistics: Statistics,
    pub pq_row_group: usize,
    pub pq_writer_version: WriterVersion,
    pub pq_dictionary: bool,
    pub partition: PartitionDraft,
}

impl Default for ExportDraft {
    fn default() -> Self {
        Self {
            format: FormatId::Csv,
            scope: ScopeChoice::All,
            csv_header: true,
            csv_delimiter: ",".into(),
            csv_null: NullChoice::Empty,
            csv_null_custom: "\\N".into(),
            csv_quote: "\"".into(),
            csv_escape: String::new(),
            csv_double_quote: true,
            csv_compression: Compression::None,
            json_compression: Compression::None,
            pq_codec: CodecChoice::Zstd,
            pq_level: 3,
            pq_statistics: Statistics::Page,
            pq_row_group: 1_048_576,
            pq_writer_version: WriterVersion::V1,
            pq_dictionary: true,
            partition: PartitionDraft::default(),
        }
    }
}

/// The CSV compression menu, shared by CSV and JSON (the same whole-file wrapping).
const COMPRESSIONS: [(Compression, &str); 5] = [
    (Compression::None, "None"),
    (Compression::Gzip, "Gzip"),
    (Compression::Zstd, "Zstd"),
    (Compression::Bzip2, "Bzip2"),
    (Compression::Xz, "XZ"),
];

/// Row-group sizes, in **rows**. The canvas labels them in the K/M shorthand the numbers are.
const ROW_GROUPS: [(usize, &str); 4] = [
    (131_072, "128K"),
    (524_288, "512K"),
    (1_048_576, "1M"),
    (2_097_152, "2M"),
];

impl ExportDraft {
    /// Apply one control's edit. Exhaustive over [`Edit`], so adding a variant without a
    /// home here is a compile error rather than a silently ignored control.
    pub fn apply(&mut self, edit: Edit) {
        match edit {
            Edit::Scope(v) => self.scope = v,
            Edit::CsvHeader(v) => self.csv_header = v,
            Edit::CsvDelimiter(v) => self.csv_delimiter = v,
            Edit::CsvNull(v) => self.csv_null = v,
            Edit::CsvNullCustom(v) => self.csv_null_custom = v,
            Edit::CsvQuote(v) => self.csv_quote = v,
            Edit::CsvEscape(v) => self.csv_escape = v,
            Edit::CsvDoubleQuote(v) => self.csv_double_quote = v,
            Edit::CsvCompression(v) => self.csv_compression = v,
            Edit::JsonCompression(v) => self.json_compression = v,
            Edit::PqCodec(v) => self.pq_codec = v,
            Edit::PqLevel(v) => self.pq_level = v,
            Edit::PqStatistics(v) => self.pq_statistics = v,
            Edit::PqRowGroup(v) => self.pq_row_group = v,
            Edit::PqWriterVersion(v) => self.pq_writer_version = v,
            Edit::PqDictionary(v) => self.pq_dictionary = v,
        }
    }

    /// The option groups for the current format — the whole list, in canvas order, flat.
    ///
    /// There is **no ADVANCED section**: the canvas folded it away (`hasAdv: false`), on the
    /// grounds that a format's advanced controls are just more of that format's options.
    pub fn groups(&self, target: &ExportTarget) -> Vec<Group> {
        let mut groups = vec![self.scope_group(target)];
        match self.format {
            FormatId::Csv => groups.extend(self.csv_groups()),
            FormatId::Json => groups.push(Group {
                label: "COMPRESSION".into(),
                hint: None,
                control: compression_select(self.json_compression, Edit::JsonCompression),
            }),
            FormatId::Parquet => groups.extend(self.parquet_groups()),
            // Not an empty list: silence would read as "options are still loading".
            FormatId::Arrow => groups.push(Group {
                label: "FORMAT".into(),
                hint: None,
                control: Control::Note(
                    "Arrow IPC is written schema-faithfully. DataFusion exposes no write \
                     options for it.",
                ),
            }),
        }
        groups
    }

    fn scope_group(&self, target: &ExportTarget) -> Group {
        Group {
            label: "ROWS TO EXPORT".into(),
            hint: None,
            control: Control::Seg {
                options: vec![
                    Choice {
                        label: format!("All · {}", thousands(target.total)),
                        edit: Edit::Scope(ScopeChoice::All),
                        selected: self.scope == ScopeChoice::All,
                    },
                    Choice {
                        label: "This page".into(),
                        edit: Edit::Scope(ScopeChoice::Page),
                        selected: self.scope == ScopeChoice::Page,
                    },
                ],
                custom: None,
            },
        }
    }

    fn csv_groups(&self) -> Vec<Group> {
        vec![
            Group {
                label: "HEADER ROW".into(),
                hint: Some("Write column names as the first row"),
                control: Control::Toggle {
                    on: self.csv_header,
                    edit: Edit::CsvHeader(!self.csv_header),
                    hint: None,
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
            Group {
                label: "NULL VALUES AS".into(),
                hint: None,
                control: Control::Seg {
                    options: vec![
                        null_choice("Empty", NullChoice::Empty, self.csv_null),
                        null_choice("NULL", NullChoice::Null, self.csv_null),
                        null_choice("NaN", NullChoice::NaN, self.csv_null),
                        null_choice("Custom", NullChoice::Custom, self.csv_null),
                    ],
                    custom: (self.csv_null == NullChoice::Custom).then(|| TextField {
                        value: self.csv_null_custom.clone(),
                        placeholder: "text",
                        max_len: 16,
                        make: Make(Edit::CsvNullCustom),
                    }),
                },
            },
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
                hint: Some("Escapes quotes (blank = double-quote)"),
                control: Control::Char(TextField {
                    value: self.csv_escape.clone(),
                    placeholder: "\\",
                    max_len: 1,
                    make: Make(Edit::CsvEscape),
                }),
            },
            Group {
                label: "DOUBLE-QUOTE".into(),
                hint: Some("Escape quotes by doubling (\"\") them"),
                control: Control::Toggle {
                    on: self.csv_double_quote,
                    edit: Edit::CsvDoubleQuote(!self.csv_double_quote),
                    hint: None,
                },
            },
            Group {
                label: "COMPRESSION".into(),
                hint: None,
                control: compression_select(self.csv_compression, Edit::CsvCompression),
            },
        ]
    }

    fn parquet_groups(&self) -> Vec<Group> {
        let mut groups = vec![Group {
            label: "COMPRESSION".into(),
            hint: None,
            control: Control::Select {
                options: [
                    CodecChoice::Zstd,
                    CodecChoice::Snappy,
                    CodecChoice::Gzip,
                    CodecChoice::Brotli,
                    CodecChoice::Lz4,
                    CodecChoice::Uncompressed,
                ]
                .into_iter()
                .map(|c| Choice {
                    label: c.label().into(),
                    edit: Edit::PqCodec(c),
                    selected: c == self.pq_codec,
                })
                .collect(),
            },
        }];

        // The level group exists only for the codecs that take one — the canvas's rule, and
        // the honest one: a level on snappy is a control that changes nothing.
        if let Some((min, max)) = self.pq_codec.levels() {
            groups.push(Group {
                label: format!("COMPRESSION LEVEL ({min}–{max})"),
                hint: Some("Higher = smaller, slower"),
                control: Control::Num {
                    value: self.pq_level.clamp(min, max),
                    min,
                    max,
                    make: Make(Edit::PqLevel),
                },
            });
        }

        groups.push(Group {
            label: "STATISTICS".into(),
            hint: None,
            control: Control::Seg {
                options: [
                    (Statistics::None, "None"),
                    (Statistics::Chunk, "Chunk"),
                    (Statistics::Page, "Page"),
                ]
                .into_iter()
                .map(|(s, label)| Choice {
                    label: label.into(),
                    edit: Edit::PqStatistics(s),
                    selected: s == self.pq_statistics,
                })
                .collect(),
                custom: None,
            },
        });
        groups.push(Group {
            label: "MAX ROW GROUP SIZE".into(),
            hint: Some("Rows per row group — larger scans faster, costs more memory"),
            control: Control::Seg {
                options: ROW_GROUPS
                    .into_iter()
                    .map(|(rows, label)| Choice {
                        label: label.into(),
                        edit: Edit::PqRowGroup(rows),
                        selected: rows == self.pq_row_group,
                    })
                    .collect(),
                custom: None,
            },
        });
        groups.push(Group {
            label: "WRITER VERSION".into(),
            hint: Some("2.0 enables newer encodings; 1.0 reads everywhere"),
            control: Control::Seg {
                options: [(WriterVersion::V1, "1.0"), (WriterVersion::V2, "2.0")]
                    .into_iter()
                    .map(|(v, label)| Choice {
                        label: label.into(),
                        edit: Edit::PqWriterVersion(v),
                        selected: v == self.pq_writer_version,
                    })
                    .collect(),
                custom: None,
            },
        });
        groups.push(Group {
            label: "DICTIONARY ENCODING".into(),
            hint: Some("Encode repeated values via dictionary"),
            control: Control::Toggle {
                on: self.pq_dictionary,
                edit: Edit::PqDictionary(!self.pq_dictionary),
                hint: None,
            },
        });
        groups
    }

    /// The compression suffix the destination picks up, so the filename shown matches the
    /// file written. Parquet and Arrow compress internally, so they add nothing.
    pub fn compression_extension(&self) -> &'static str {
        match self.format {
            FormatId::Csv => self.csv_compression.extension(),
            FormatId::Json => self.json_compression.extension(),
            FormatId::Parquet | FormatId::Arrow => "",
        }
    }

    /// The default filename offered by the save dialog — the tab's name, sanitised, with the
    /// format's extension and any compression suffix.
    pub fn suggested_name(&self, target: &ExportTarget) -> String {
        let stem: String = target
            .label
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let stem = stem.trim_matches('_');
        let stem = if stem.is_empty() { "export" } else { stem };
        // A partitioned export writes a *directory*, so it carries no extension at all.
        if self.partition.is_active() {
            return stem.to_string();
        }
        format!(
            "{stem}.{}{}",
            self.format.extension(),
            self.compression_extension()
        )
    }

    /// Turn the draft into an engine spec for `path`, or explain why it can't be.
    ///
    /// This is where the draft's remember-everything shape narrows to the engine's
    /// one-format-only shape, and where the free-text fields become the characters they
    /// name — a bad delimiter is reported here, before anything is written.
    pub fn spec(&self, target: &ExportTarget, path: String) -> Result<ExportSpec, String> {
        let format = match self.format {
            FormatId::Csv => Format::Csv(Csv {
                header: self.csv_header,
                delimiter: one_char("delimiter", &self.csv_delimiter)?
                    .ok_or("The CSV delimiter can't be empty")?,
                null_value: match self.csv_null {
                    NullChoice::Empty => String::new(),
                    NullChoice::Null => "NULL".into(),
                    NullChoice::NaN => "NaN".into(),
                    NullChoice::Custom => self.csv_null_custom.clone(),
                },
                quote: one_char("quote character", &self.csv_quote)?
                    .ok_or("The CSV quote character can't be empty")?,
                escape: one_char("escape character", &self.csv_escape)?,
                double_quote: self.csv_double_quote,
                compression: self.csv_compression,
            }),
            FormatId::Json => Format::Json(Json {
                compression: self.json_compression,
            }),
            FormatId::Parquet => Format::Parquet(Parquet {
                compression: match self.pq_codec {
                    CodecChoice::Zstd => Codec::Zstd(self.pq_level),
                    CodecChoice::Gzip => Codec::Gzip(self.pq_level),
                    CodecChoice::Brotli => Codec::Brotli(self.pq_level),
                    CodecChoice::Snappy => Codec::Snappy,
                    CodecChoice::Lz4 => Codec::Lz4,
                    CodecChoice::Uncompressed => Codec::Uncompressed,
                },
                statistics: self.pq_statistics,
                max_row_group_size: self.pq_row_group,
                writer_version: self.pq_writer_version,
                dictionary: self.pq_dictionary,
            }),
            FormatId::Arrow => Format::Arrow,
        };

        Ok(ExportSpec {
            path,
            scope: match self.scope {
                ScopeChoice::All => Scope::All,
                ScopeChoice::Page => Scope::Page {
                    page: target.page,
                    page_size: target.page_size,
                },
            },
            sort: target.sort.clone(),
            format,
            partition: Partition {
                columns: self.partition.effective(),
                keep_columns: self.partition.keep_columns,
            },
        })
    }
}

fn null_choice(label: &str, choice: NullChoice, current: NullChoice) -> Choice {
    Choice {
        label: label.into(),
        edit: Edit::CsvNull(choice),
        selected: choice == current,
    }
}

fn compression_select(current: Compression, edit: fn(Compression) -> Edit) -> Control {
    Control::Select {
        options: COMPRESSIONS
            .into_iter()
            .map(|(c, label)| Choice {
                label: label.into(),
                edit: edit(c),
                selected: c == current,
            })
            .collect(),
    }
}

/// Thousands-separated, for the row counts the window quotes.
pub fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use datafusion::arrow::datatypes::{DataType, Field, TimeUnit};
    use strata_core::engine::column_info;

    use super::*;

    fn target() -> ExportTarget {
        ExportTarget {
            snapshot: SnapshotId(1),
            columns: vec![
                col("region", DataType::Utf8),
                col("amount", DataType::Int64),
                col(
                    "created_at",
                    DataType::Timestamp(TimeUnit::Millisecond, None),
                ),
                col("active", DataType::Boolean),
                col(
                    "payload",
                    DataType::Struct(vec![Field::new("a", DataType::Utf8, true)].into()),
                ),
            ],
            total: 48_213,
            sort: None,
            page: 2,
            page_size: 100,
            label: "cross-file join".into(),
            sample: vec![],
        }
    }

    fn col(name: &str, dtype: DataType) -> ColumnInfo {
        column_info(&Field::new(name, dtype, true))
    }

    fn labels(groups: &[Group]) -> Vec<String> {
        groups.iter().map(|g| g.label.clone()).collect()
    }

    #[test]
    fn csv_offers_the_documented_surface_in_canvas_order() {
        let draft = ExportDraft::default();
        assert_eq!(
            labels(&draft.groups(&target())),
            vec![
                "ROWS TO EXPORT",
                "HEADER ROW",
                "DELIMITER",
                "NULL VALUES AS",
                "QUOTE CHARACTER",
                "ESCAPE CHARACTER",
                "DOUBLE-QUOTE",
                "COMPRESSION",
            ]
        );
    }

    #[test]
    fn json_offers_compression_only() {
        let draft = ExportDraft {
            format: FormatId::Json,
            ..Default::default()
        };
        assert_eq!(
            labels(&draft.groups(&target())),
            vec!["ROWS TO EXPORT", "COMPRESSION"]
        );
    }

    #[test]
    fn arrow_offers_no_options_but_says_why() {
        let draft = ExportDraft {
            format: FormatId::Arrow,
            ..Default::default()
        };
        let groups = draft.groups(&target());
        assert_eq!(labels(&groups), vec!["ROWS TO EXPORT", "FORMAT"]);
        assert!(matches!(groups[1].control, Control::Note(_)));
    }

    #[test]
    fn the_parquet_level_appears_only_for_codecs_that_take_one() {
        let mut draft = ExportDraft {
            format: FormatId::Parquet,
            ..Default::default()
        };
        assert!(labels(&draft.groups(&target()))
            .iter()
            .any(|l| l.starts_with("COMPRESSION LEVEL")));

        draft.pq_codec = CodecChoice::Snappy;
        assert!(!labels(&draft.groups(&target()))
            .iter()
            .any(|l| l.starts_with("COMPRESSION LEVEL")));
    }

    #[test]
    fn the_level_label_carries_the_codecs_own_range() {
        let mut draft = ExportDraft {
            format: FormatId::Parquet,
            ..Default::default()
        };
        assert!(labels(&draft.groups(&target())).contains(&"COMPRESSION LEVEL (1–22)".to_string()));
        draft.pq_codec = CodecChoice::Gzip;
        assert!(labels(&draft.groups(&target())).contains(&"COMPRESSION LEVEL (1–9)".to_string()));
    }

    #[test]
    fn the_custom_null_field_shows_only_when_custom_is_picked() {
        let mut draft = ExportDraft::default();
        let groups = draft.groups(&target());
        let null = groups.iter().find(|g| g.label == "NULL VALUES AS").unwrap();
        assert!(matches!(&null.control, Control::Seg { custom: None, .. }));

        draft.csv_null = NullChoice::Custom;
        let groups = draft.groups(&target());
        let null = groups.iter().find(|g| g.label == "NULL VALUES AS").unwrap();
        assert!(matches!(
            &null.control,
            Control::Seg {
                custom: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn every_control_carries_the_edit_it_performs() {
        let mut draft = ExportDraft::default();
        let groups = draft.groups(&target());
        let header = groups.iter().find(|g| g.label == "HEADER ROW").unwrap();
        let Control::Toggle { on, edit, .. } = &header.control else {
            panic!("a toggle");
        };
        assert!(*on);
        draft.apply(edit.clone());
        assert!(!draft.csv_header, "the toggle's edit flips it");
    }

    #[test]
    fn switching_format_and_back_keeps_the_options_you_set() {
        let mut draft = ExportDraft::default();
        draft.apply(Edit::CsvDelimiter(";".into()));
        draft.format = FormatId::Parquet;
        draft.apply(Edit::PqCodec(CodecChoice::Snappy));
        draft.format = FormatId::Csv;
        assert_eq!(draft.csv_delimiter, ";", "the CSV side was not reset");
        assert_eq!(draft.pq_codec, CodecChoice::Snappy);
    }

    #[test]
    fn the_spec_carries_only_the_active_formats_options() {
        let draft = ExportDraft::default();
        let spec = draft.spec(&target(), "/tmp/x.csv".into()).expect("spec");
        assert!(matches!(spec.format, Format::Csv(_)));

        let draft = ExportDraft {
            format: FormatId::Arrow,
            ..Default::default()
        };
        let spec = draft.spec(&target(), "/tmp/x.arrow".into()).expect("spec");
        assert!(matches!(spec.format, Format::Arrow));
    }

    #[test]
    fn a_tab_delimiter_is_written_as_the_escape_and_resolved_for_the_engine() {
        let draft = ExportDraft {
            csv_delimiter: "\\t".into(),
            ..Default::default()
        };
        let spec = draft.spec(&target(), "/tmp/x.csv".into()).expect("spec");
        let Format::Csv(csv) = spec.format else {
            panic!("csv")
        };
        assert_eq!(csv.delimiter, '\t');
    }

    #[test]
    fn a_multi_character_delimiter_is_refused_before_anything_is_written() {
        let draft = ExportDraft {
            csv_delimiter: "||".into(),
            ..Default::default()
        };
        let err = draft.spec(&target(), "/tmp/x.csv".into()).expect_err("bad");
        assert!(err.contains("single character"), "{err}");
    }

    #[test]
    fn a_blank_escape_is_absent_rather_than_an_error() {
        let draft = ExportDraft::default();
        let spec = draft.spec(&target(), "/tmp/x.csv".into()).expect("spec");
        let Format::Csv(csv) = spec.format else {
            panic!("csv")
        };
        assert_eq!(csv.escape, None);
    }

    #[test]
    fn the_page_scope_carries_the_page_the_grid_is_showing() {
        let draft = ExportDraft {
            scope: ScopeChoice::Page,
            ..Default::default()
        };
        let spec = draft.spec(&target(), "/tmp/x.csv".into()).expect("spec");
        assert_eq!(
            spec.scope,
            Scope::Page {
                page: 2,
                page_size: 100
            }
        );
    }

    #[test]
    fn a_selection_with_the_toggle_off_is_not_partitioning() {
        let mut draft = ExportDraft::default();
        draft.partition.columns = vec!["region".into()];
        assert!(!draft.partition.is_active(), "the toggle gates it");
        let spec = draft.spec(&target(), "/tmp/x.csv".into()).expect("spec");
        assert!(spec.partition.columns.is_empty());

        draft.partition.enabled = true;
        let spec = draft.spec(&target(), "/tmp/x.csv".into()).expect("spec");
        assert_eq!(spec.partition.columns, vec!["region".to_string()]);
    }

    #[test]
    fn only_scalar_columns_can_be_partitioned_on() {
        let target = target();
        let names: Vec<&str> = target
            .partitionable()
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(
            names,
            vec!["region", "amount"],
            "no timestamps, booleans or nested containers"
        );
    }

    #[test]
    fn the_suggested_name_carries_the_format_and_its_compression_suffix() {
        // The space becomes `_`; the hyphen is legal in a filename and survives.
        let mut draft = ExportDraft::default();
        assert_eq!(draft.suggested_name(&target()), "cross-file_join.csv");
        draft.csv_compression = Compression::Gzip;
        assert_eq!(draft.suggested_name(&target()), "cross-file_join.csv.gz");
        draft.format = FormatId::Parquet;
        assert_eq!(draft.suggested_name(&target()), "cross-file_join.parquet");
    }

    #[test]
    fn a_partitioned_export_suggests_a_directory_name_with_no_extension() {
        let mut draft = ExportDraft::default();
        draft.partition.enabled = true;
        draft.partition.columns = vec!["region".into()];
        assert_eq!(draft.suggested_name(&target()), "cross-file_join");
    }

    #[test]
    fn the_subtitle_names_the_run_and_its_shape() {
        assert_eq!(target().subtitle(), "cross-file join · 5 columns");
    }

    #[test]
    fn row_counts_are_grouped() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(48_213), "48,213");
        assert_eq!(thousands(1_000_000), "1,000,000");
    }
}
