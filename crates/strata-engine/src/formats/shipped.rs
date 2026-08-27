//! The formats Strata ships, as ordinary registrants.
//!
//! Each is a unit struct with a name and a reader, registered by
//! [`Formats::shipped`](super::Formats::shipped) through the same [`with_format`] an embedder
//! calls — so there is no private path a format can reach the engine by, and each is held to the
//! seam's own contract.
//!
//! **The typed defs are theirs.** `parquet` · `csv` · `json` · `arrow` are the closed first-party
//! vocabulary, so each overrides [`read`](super::FormatProvider::read) to land a statement's
//! `format.*` options on its own struct rather than keeping them as strings — which is what lets
//! a form edit one option at a time. The option tables below **are** those arms: one vocabulary,
//! read by the arm and offered by completion, never a copy kept honest by a test.
//!
//! **None of them brings a writer of its own.** DataFusion writes all four itself, and a factory registered over one of those names would replace the writer every
//! other `COPY` in the session uses — see [`Formats::insert`](super::Formats::insert).
//!
//! [`with_format`]: crate::EngineBuilder::with_format

use std::collections::BTreeMap;
use std::sync::Arc;

use datafusion::datasource::file_format::csv::CsvFormat;
use datafusion::datasource::file_format::file_compression_type::FileCompressionType;
use datafusion::datasource::file_format::parquet::ParquetFormat;
use datafusion::datasource::file_format::FileFormat;

use strata_core::util::one_char;
use strata_model::{CsvRead, FileCompression, JsonRead, JsonShape, SourceFormat};

use crate::arrow_stats::StrataArrowFormat;
use crate::json_poly::PolyJsonFormat;

use super::{FileFormatKind, FormatProvider, OptionKind, OptionOffer, ReadFor};

/// Apache Parquet, with file-level metadata skipped — a table's own schema is what registration
/// infers, and the embedded key/value metadata is not part of it.
#[derive(Debug)]
pub(super) struct Parquet;

impl FileFormatKind for Parquet {
    const NAME: &'static str = "parquet";
}

impl FormatProvider for Parquet {
    fn build(&self, format: &SourceFormat) -> Result<Arc<dyn FileFormat>, String> {
        let SourceFormat::Parquet = format else {
            return Err(mismatch(Self::NAME, format));
        };
        Ok(Arc::new(ParquetFormat::default().with_skip_metadata(true)))
    }

    fn read(
        &self,
        at: ReadFor<'_>,
        options: &BTreeMap<String, String>,
    ) -> Result<SourceFormat, String> {
        no_options(at, options)?;
        Ok(SourceFormat::Parquet)
    }

    fn copy_to(&self) -> bool {
        true
    }
}

/// Arrow IPC, read through [`StrataArrowFormat`] — DataFusion's own reader plus the exact row
/// count its footer already holds.
#[derive(Debug)]
pub(super) struct Arrow;

impl FileFormatKind for Arrow {
    const NAME: &'static str = "arrow";
}

impl FormatProvider for Arrow {
    fn build(&self, format: &SourceFormat) -> Result<Arc<dyn FileFormat>, String> {
        let SourceFormat::Arrow = format else {
            return Err(mismatch(Self::NAME, format));
        };
        Ok(Arc::new(StrataArrowFormat::default()))
    }

    fn read(
        &self,
        at: ReadFor<'_>,
        options: &BTreeMap<String, String>,
    ) -> Result<SourceFormat, String> {
        no_options(at, options)?;
        Ok(SourceFormat::Arrow)
    }

    fn copy_to(&self) -> bool {
        true
    }
}

/// Delimited text, in every setting [`CsvRead`] carries.
#[derive(Debug)]
pub(super) struct Csv;

impl FileFormatKind for Csv {
    const NAME: &'static str = "csv";
}

impl FormatProvider for Csv {
    fn build(&self, format: &SourceFormat) -> Result<Arc<dyn FileFormat>, String> {
        let SourceFormat::Csv(o) = format else {
            return Err(mismatch(Self::NAME, format));
        };
        let mut fmt = CsvFormat::default()
            .with_has_header(o.header)
            .with_delimiter(ascii_byte("delimiter", o.delimiter)?)
            .with_quote(ascii_byte("quote character", o.quote)?)
            .with_newlines_in_values(o.newlines_in_values)
            .with_truncated_rows(o.truncated_rows)
            .with_file_compression_type(compression_type(o.compression));
        if let Some(escape) = o.escape {
            fmt = fmt.with_escape(Some(ascii_byte("escape character", escape)?));
        }
        if let Some(comment) = o.comment {
            fmt = fmt.with_comment(Some(ascii_byte("comment character", comment)?));
        }
        if let Some(rows) = o.infer_rows {
            fmt = fmt.with_schema_infer_max_rec(rows);
        }
        Ok(Arc::new(fmt))
    }

    fn read(
        &self,
        at: ReadFor<'_>,
        options: &BTreeMap<String, String>,
    ) -> Result<SourceFormat, String> {
        let mut read = CsvRead::default();
        apply(CSV_OPTION_KEYS, &mut read, at, options)?;
        Ok(SourceFormat::Csv(read))
    }

    fn copy_to(&self) -> bool {
        true
    }

    fn reader_options(&self) -> Vec<OptionOffer> {
        offers(CSV_OPTION_KEYS)
    }
}

/// JSON, read through [`PolyJsonFormat`] — the union-tolerant inference discriminated documents
/// need, which DataFusion's own reader refuses outright.
#[derive(Debug)]
pub(super) struct Json;

impl FileFormatKind for Json {
    const NAME: &'static str = "json";
}

impl FormatProvider for Json {
    fn build(&self, format: &SourceFormat) -> Result<Arc<dyn FileFormat>, String> {
        let SourceFormat::Json(o) = format else {
            return Err(mismatch(Self::NAME, format));
        };
        Ok(Arc::new(PolyJsonFormat::new(o.clone())))
    }

    fn read(
        &self,
        at: ReadFor<'_>,
        options: &BTreeMap<String, String>,
    ) -> Result<SourceFormat, String> {
        let mut read = JsonRead::default();
        apply(JSON_OPTION_KEYS, &mut read, at, options)?;
        Ok(SourceFormat::Json(read))
    }

    fn copy_to(&self) -> bool {
        true
    }

    fn reader_options(&self) -> Vec<OptionOffer> {
        offers(JSON_OPTION_KEYS)
    }
}

/// The key that chooses between the two JSON layouts — DataFusion's own spelling, and the only
/// thing that tells them apart: newline-delimited is a JSON reader's default, not a format.
const NEWLINE_DELIMITED: &str = "format.newline_delimited";

/// The compression spellings [`compression`] parses — DataFusion's own vocabulary, stated once
/// for the refusal message, the value offer and the coercion alike.
const COMPRESSION_WORDS: &[&str] = &["uncompressed", "gzip", "bzip2", "xz", "zstd"];

/// One `OPTIONS` key of a first-party format: the DataFusion spelling, its value shape, the short
/// detail completion shows, and the coercion-and-def-field its value lands on.
struct OptionKey<T: 'static> {
    key: &'static str,
    kind: OptionKind,
    what: &'static str,
    set: fn(&mut T, &str, &str) -> Result<(), String>,
}

/// The CSV reader's keys — every field of [`CsvRead`] and nothing else, which is what
/// `docs/IMPORT_OPTIONS.md` documents from the other side. The three CSV options DataFusion has
/// and this deliberately lacks (`format.null_regex`, `format.terminator`, `format.double_quote`)
/// reach [`apply`]'s by-name refusal like any other key — [`CsvRead`]'s doc comment is why they
/// are absent, and it is the read path's asymmetry rather than an oversight.
const CSV_OPTION_KEYS: &[OptionKey<CsvRead>] = &[
    OptionKey {
        key: "format.has_header",
        kind: OptionKind::Bool,
        what: "header row",
        set: |o, k, v| {
            o.header = boolean(k, v)?;
            Ok(())
        },
    },
    OptionKey {
        key: "format.delimiter",
        kind: OptionKind::Char,
        what: "delimiter character",
        set: |o, k, v| {
            o.delimiter = character(k, "delimiter", v)?;
            Ok(())
        },
    },
    OptionKey {
        key: "format.quote",
        kind: OptionKind::Char,
        what: "quote character",
        set: |o, k, v| {
            o.quote = character(k, "quote character", v)?;
            Ok(())
        },
    },
    OptionKey {
        key: "format.escape",
        kind: OptionKind::Char,
        what: "escape character",
        set: |o, k, v| {
            o.escape = Some(character(k, "escape character", v)?);
            Ok(())
        },
    },
    OptionKey {
        key: "format.comment",
        kind: OptionKind::Char,
        what: "comment character",
        set: |o, k, v| {
            o.comment = Some(character(k, "comment character", v)?);
            Ok(())
        },
    },
    OptionKey {
        key: "format.newlines_in_values",
        kind: OptionKind::Bool,
        what: "newlines in quoted values",
        set: |o, k, v| {
            o.newlines_in_values = boolean(k, v)?;
            Ok(())
        },
    },
    OptionKey {
        key: "format.truncated_rows",
        kind: OptionKind::Bool,
        what: "tolerate short rows",
        set: |o, k, v| {
            o.truncated_rows = boolean(k, v)?;
            Ok(())
        },
    },
    OptionKey {
        key: "format.schema_infer_max_rec",
        kind: OptionKind::Int,
        what: "rows read to infer the schema",
        set: |o, k, v| {
            o.infer_rows = Some(count(k, v)?);
            Ok(())
        },
    },
    OptionKey {
        key: "format.compression",
        kind: OptionKind::Enum(COMPRESSION_WORDS),
        what: "whole-file compression",
        set: |o, k, v| {
            o.compression = compression(k, v)?;
            Ok(())
        },
    },
];

/// The JSON reader's keys — [`JsonRead`]'s fields exactly, as above.
const JSON_OPTION_KEYS: &[OptionKey<JsonRead>] = &[
    OptionKey {
        key: NEWLINE_DELIMITED,
        kind: OptionKind::Bool,
        what: "newline-delimited shape",
        set: |o, k, v| {
            o.shape = match boolean(k, v)? {
                true => JsonShape::NewlineDelimited,
                false => JsonShape::Array,
            };
            Ok(())
        },
    },
    OptionKey {
        key: "format.schema_infer_max_rec",
        kind: OptionKind::Int,
        what: "rows read to infer the schema",
        set: |o, k, v| {
            let rows = count(k, v)?;
            o.infer_rows = (rows > 0).then_some(rows);
            Ok(())
        },
    },
    OptionKey {
        key: "format.compression",
        kind: OptionKind::Enum(COMPRESSION_WORDS),
        what: "whole-file compression",
        set: |o, k, v| {
            o.compression = compression(k, v)?;
            Ok(())
        },
    },
];

/// Reads every option onto `read` through `keys`, refusing by name where there is none.
///
/// The arm set **is** the table, and the table is the def: every field of [`CsvRead`] and
/// [`JsonRead`] has a DataFusion key there and nothing else does, which is what lets completion
/// offer the same set with zero drift.
fn apply<T>(
    keys: &'static [OptionKey<T>],
    read: &mut T,
    at: ReadFor<'_>,
    options: &BTreeMap<String, String>,
) -> Result<(), String> {
    for (key, value) in options {
        match keys.iter().find(|k| k.key == key) {
            Some(k) => (k.set)(read, key, value)?,
            None => return Err(unsupported(key, at)),
        }
    }
    Ok(())
}

/// The offer rows for a table of keys — the same table, projected.
fn offers<T>(keys: &'static [OptionKey<T>]) -> Vec<OptionOffer> {
    keys.iter()
        .map(|k| OptionOffer {
            key: k.key,
            kind: k.kind,
            what: k.what,
        })
        .collect()
}

/// A key with no field on the format in play. Names the format, because the commonest way to
/// reach this is a CSV option on a parquet table — which is the state [`SourceFormat`] exists to
/// make unwritable.
fn unsupported(key: &str, at: ReadFor<'_>) -> String {
    format!(
        "'{key}' is not a read option for a {} table. Table '{}' is STORED AS {}",
        at.format,
        at.table,
        at.format.to_uppercase()
    )
}

/// The refusal for a format whose reader takes nothing at all.
fn no_options(at: ReadFor<'_>, options: &BTreeMap<String, String>) -> Result<(), String> {
    match options.is_empty() {
        true => Ok(()),
        false => Err(format!(
            "Table '{}' is STORED AS {}, which takes no read options",
            at.table,
            at.format.to_uppercase()
        )),
    }
}

/// A def handed to the wrong registrant, which dispatching on the def's own name makes
/// unreachable.
fn mismatch(name: &str, format: &SourceFormat) -> String {
    format!(
        "The '{name}' reader was asked for a '{}' table",
        format.name()
    )
}

fn boolean(key: &str, value: &str) -> Result<bool, String> {
    match value.to_ascii_lowercase().as_str() {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(format!(
            "The option '{key}' is '{other}'. It takes true or false"
        )),
    }
}

/// A single-character option, through the rule the two windows publish — so `\t` is a tab here
/// exactly as it is in a delimiter box, and a longer string is reported rather than truncated.
fn character(key: &str, what: &str, value: &str) -> Result<char, String> {
    one_char(what, value)?.ok_or_else(|| format!("The option '{key}' has no value"))
}

fn count(key: &str, value: &str) -> Result<usize, String> {
    value
        .parse()
        .map_err(|_| format!("The option '{key}' takes a number of rows"))
}

/// Whole-file compression, in **DataFusion's own spelling** — there is no second vocabulary for
/// it, so the statement takes the words `format.compression` takes everywhere else and the
/// message lists them rather than restating a Strata enum.
fn compression(key: &str, value: &str) -> Result<FileCompression, String> {
    use datafusion::common::parsers::CompressionTypeVariant as V;
    let parsed: FileCompressionType = value.parse().map_err(|_| {
        format!("The option '{key}' is '{value}'. It takes uncompressed, gzip, bzip2, xz or zstd")
    })?;
    Ok(match parsed.get_variant() {
        V::UNCOMPRESSED => FileCompression::None,
        V::GZIP => FileCompression::Gzip,
        V::BZIP2 => FileCompression::Bzip2,
        V::XZ => FileCompression::Xz,
        V::ZSTD => FileCompression::Zstd,
    })
}

/// Our compression vocabulary as DataFusion's.
pub(crate) fn compression_type(c: FileCompression) -> FileCompressionType {
    match c {
        FileCompression::None => FileCompressionType::UNCOMPRESSED,
        FileCompression::Gzip => FileCompressionType::GZIP,
        FileCompression::Bzip2 => FileCompressionType::BZIP2,
        FileCompression::Xz => FileCompressionType::XZ,
        FileCompression::Zstd => FileCompressionType::ZSTD,
    }
}

/// A single-character CSV option as the byte DataFusion's reader takes.
fn ascii_byte(what: &str, c: char) -> Result<u8, String> {
    c.is_ascii()
        .then_some(c as u8)
        .ok_or_else(|| format!("The CSV {what} has to be a single-byte character, not '{c}'."))
}
