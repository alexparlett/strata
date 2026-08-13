//! **Typed `CREATE EXTERNAL TABLE`** (ED-10) — the second gesture into the funnel Table Config
//! already uses. `docs/STATEMENTS_SPEC.md` §6.7.
//!
//! The statement becomes a [`TableDef`] and goes through
//! [`register_external`](crate::engine::catalog::register_external), so the store, the persist
//! funnel, replay and the headless host need no new code and the settle is CTAS's exactly. Table
//! Config and typed DDL are two gestures at one registration path, as ⌘S and typed `CREATE VIEW`
//! are for views: either can edit the row the other made, and `ConfigureDraft::of` opens on a
//! typed def like any other.
//!
//! **DataFusion's own `ListingTableFactory` is not used**, for the reason the whole workstream
//! gives: it registers a provider behind the store's back, and the **def** is the durable artifact.
//! A table existing only in a `SessionContext` would vanish on restart and appear in no catalog
//! row, no `project.json`, and no clone of the project. So the statement is read, not planned.
//!
//! **`OPTIONS` is two vocabularies wearing one syntax.** In `datafusion-cli` it carries both the
//! reader's settings and the object store's; Strata keeps those in different files, because the
//! reader's belong to the def ([`SourceFormat`] *is* its options) and the store's to a
//! [`ConnectionDef`](strata_model::ConnectionDef), which holds a reference to credentials and never
//! a credential. So the split is by namespace:
//!
//! - a `format.` key the def has a field for is **read** onto it;
//! - a client option or store namespace is **refused toward Connections** on the key alone —
//!   [`store_key`] never looks at the value, because that value may be a secret and a refusal is a
//!   sentence the user then copies and pastes;
//! - anything else is refused **by name**, which keeps the mechanism total rather than a list of
//!   the keys we thought of.
//!
//! A refused statement is not recorded, so a pasted key does not outlive its buffer.

use std::collections::HashSet;
use std::path::Path;

use datafusion::datasource::file_format::file_compression_type::FileCompressionType;
use datafusion::logical_expr::TableType;
use datafusion::prelude::SessionContext;
use datafusion::sql::parser::{CreateExternalTable, Statement as DFStatement};
use datafusion::sql::sqlparser::ast::{ColumnDef, DataType as SqlType, Value};
use datafusion::sql::TableReference;

use crate::engine::catalog::register_external;
use crate::engine::export::partition_columns_are_bare_words;
use crate::engine::store::client_key;
use crate::engine::{Connections, InternalTables};
use crate::project::{relativize, split_remote};
use crate::register::table_spec;
use crate::util::{one_char, plural};
use strata_model::{
    CsvRead, FileCompression, JsonRead, JsonShape, SourceFormat, TableDef, TableOrigin,
};

use super::{bare_name, elsewhere, existing, DataRoot, StatementOutcome, StoreEffect};

/// What [`bare_name`] calls the objects this statement creates.
const WHAT: &str = "Tables";

/// The statement's own label, for the refusals that name it.
const LABEL: &str = "CREATE EXTERNAL TABLE";

/// Register an external table from a typed `CREATE EXTERNAL TABLE`.
///
/// Everything is resolved before anything is registered — the clauses, the format, the location,
/// the partition columns, the options and the name — so a statement that is going to be refused
/// leaves the catalog exactly as it found it. The registration itself is the only step that can
/// fail late, and it fails the way every other table's registration does, in
/// `register_external`'s own words.
pub async fn create(
    ctx: &SessionContext,
    stmt: DFStatement,
    root: &DataRoot,
    internal: &InternalTables,
    connections: &Connections,
) -> Result<StatementOutcome, String> {
    let DFStatement::CreateExternalTable(create) = stmt else {
        return Err(format!("{LABEL} did not parse as a table"));
    };
    let CreateExternalTable {
        name,
        columns,
        file_type,
        location,
        table_partition_cols,
        order_exprs,
        if_not_exists,
        or_replace,
        temporary,
        unbounded,
        options,
        constraints,
    } = create;

    let Some(root) = root.as_deref() else {
        return Err(format!("{LABEL} needs a project folder to store the table"));
    };

    if temporary {
        return Err(format!("{LABEL} does not support TEMPORARY"));
    }
    if unbounded {
        return Err(format!("{LABEL} does not support UNBOUNDED"));
    }
    if !order_exprs.is_empty() {
        return Err(format!("{LABEL} does not support WITH ORDER"));
    }
    if !constraints.is_empty() {
        return Err("Table constraints are not supported".into());
    }

    if name.0.len() > 3 {
        return Err(elsewhere(WHAT));
    }
    let name = bare_name(&TableReference::parse_str(&name.to_string()), WHAT)?;
    let format = read_format(&file_type, &name, &options)?;
    let (connection, source) = source_of(root, &location, connections)?;
    let partitions = partition_cols(ctx, &columns, &table_partition_cols)?;

    let replacing = match existing(ctx, &name).await {
        Some(TableType::View) => return Err(format!("'{name}' is a view")),
        Some(_) if if_not_exists => {
            return Ok(StatementOutcome {
                message: format!("Table '{name}' already exists"),
                count: None,
                effect: None,
            })
        }
        Some(_) if !or_replace => return Err(format!("Table '{name}' already exists")),
        Some(_) if internal.contains(&name) => {
            return Err(format!(
                "'{name}' is a table Strata stores in this project. Drop it first"
            ))
        }
        taken => taken.is_some(),
    };

    let def = TableDef {
        name: name.clone(),
        format,
        connection,
        sources: vec![source],
        partition_cols: partitions,
        origin: TableOrigin::External,
    };
    let meta = register_external(ctx, &table_spec(root, &def)).await?;

    let verb = if replacing { "replaced" } else { "created" };
    Ok(StatementOutcome {
        message: format!(
            "Table '{name}' {verb}, {}",
            plural(meta.columns.len(), "column")
        ),
        count: None,
        effect: Some(StoreEffect::TableUpserted { def, meta }),
    })
}

/// The format words `STORED AS` takes — [`read_format`]'s own match arms as data, for
/// completion's format-word pool (ED-11). One table, owned by the module whose arms it
/// mirrors, kept honest by `stored_as_formats_parse_through_read_format`: every entry
/// parses through [`read_format`] and a non-member does not.
pub(crate) const STORED_AS_FORMATS: &[&str] = &["PARQUET", "CSV", "JSON", "NDJSON", "ARROW"];

/// The value shape of one `OPTIONS` key — what completion may offer at the key's value
/// position (ED-11), mirroring the `SET` value design: `Bool` offers `true` / `false`,
/// `Enum` its words, and `Char` / `Int` nothing (the values are the user's own).
#[derive(Clone, Copy)]
pub(crate) enum OptionKind {
    Bool,
    Char,
    Int,
    Enum(&'static [&'static str]),
}

/// The compression spellings [`compression`] parses — DataFusion's own vocabulary,
/// stated once for the refusal message, the value offer and the coercion alike.
const COMPRESSION_WORDS: &[&str] = &["uncompressed", "gzip", "bzip2", "xz", "zstd"];

/// One `OPTIONS` key of a format: the DataFusion spelling, its value shape, the short
/// detail completion shows, and the coercion-and-def-field its value lands on. The
/// table **is** [`apply`]'s arm set — one vocabulary, consumed by the arm and by the
/// offer, never a copy kept honest by test.
pub(crate) struct OptionKey<T: 'static> {
    pub(crate) key: &'static str,
    pub(crate) kind: OptionKind,
    pub(crate) what: &'static str,
    pub(crate) set: fn(&mut T, &str, &Value) -> Result<(), String>,
}

/// The keys completion may offer for the format word `STORED AS` names — the same
/// tables [`apply`] consumes, projected per format: NDJSON drops
/// `format.newline_delimited` (which [`read_format`] refuses toward `STORED AS
/// JSON`), and a format with no options — or no format written yet — answers empty,
/// matching the arm's refusal by name. Owned here, beside the arms it mirrors, so
/// the offer cannot drift from dispatch; `option_keys_for_agrees_with_apply` holds
/// the projection and each key's declared kind against the arms themselves.
pub(crate) fn option_keys_for(format_word: &str) -> Vec<(&'static str, OptionKind, &'static str)> {
    fn rows<T>(keys: &'static [OptionKey<T>]) -> Vec<(&'static str, OptionKind, &'static str)> {
        keys.iter().map(|k| (k.key, k.kind, k.what)).collect()
    }
    match format_word {
        "CSV" => rows(CSV_OPTION_KEYS),
        "JSON" => rows(JSON_OPTION_KEYS),
        "NDJSON" => rows(JSON_OPTION_KEYS)
            .into_iter()
            .filter(|(k, ..)| *k != "format.newline_delimited")
            .collect(),
        _ => Vec::new(),
    }
}

/// The CSV reader's keys — every field of [`CsvRead`] and nothing else, which is what
/// `docs/IMPORT_OPTIONS.md` documents from the other side. The three CSV options
/// DataFusion has and this deliberately lacks (`format.null_regex`, `format.terminator`,
/// `format.double_quote`) reach [`apply`]'s by-name refusal like any other key.
pub(crate) const CSV_OPTION_KEYS: &[OptionKey<CsvRead>] = &[
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

/// The JSON reader's keys — [`JsonRead`]'s fields exactly, as above. Completion drops
/// `format.newline_delimited` from the NDJSON offer itself, because [`read_format`]
/// refuses it there toward `STORED AS JSON`.
pub(crate) const JSON_OPTION_KEYS: &[OptionKey<JsonRead>] = &[
    OptionKey {
        key: "format.newline_delimited",
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

/// The reader `STORED AS` names, dressed in the options that reader takes.
///
/// **Exhaustive by name, never a fallthrough** (P4-11): a format with no reader in this build has
/// to fail here rather than reach [`SourceFormat::Unknown`], which exists to keep a *legacy def*
/// loading and is not something a statement may mint. `AVRO` is DataFusion's own table factory
/// name and lands here for exactly that reason.
///
/// `NDJSON` and `JSON` are both DataFusion spellings of the one reader Strata has, and the shape
/// is [`JsonRead::shape`]. `NDJSON` therefore *states* the shape, which is why
/// `format.newline_delimited` is refused on it rather than allowed to contradict it.
fn read_format(
    file_type: &str,
    name: &str,
    options: &[(String, Value)],
) -> Result<SourceFormat, String> {
    let (mut format, ndjson) = match file_type {
        "PARQUET" => (SourceFormat::Parquet, false),
        "ARROW" => (SourceFormat::Arrow, false),
        "CSV" => (SourceFormat::Csv(CsvRead::default()), false),
        "JSON" => (SourceFormat::Json(JsonRead::default()), false),
        "NDJSON" => (SourceFormat::Json(JsonRead::default()), true),
        other => {
            return Err(format!(
                "STORED AS {other} is not a format Strata reads. Use PARQUET, CSV, JSON or ARROW"
            ))
        }
    };
    let mut seen = HashSet::new();
    for (key, value) in options {
        let key = key.to_ascii_lowercase();
        if !seen.insert(key.clone()) {
            return Err(format!("The option '{key}' is set twice"));
        }
        if let Some(surface) = store_key(&key) {
            return Err(format!(
                "'{key}' is an object store setting, not a table read option. {surface}"
            ));
        }
        if ndjson && key == "format.newline_delimited" {
            return Err(
                "STORED AS NDJSON is newline-delimited JSON. Use STORED AS JSON to set \
                 'format.newline_delimited'"
                    .into(),
            );
        }
        apply(&mut format, name, &key, value)?;
    }
    Ok(format)
}

/// Which surface owns `key`, when it is one the object store takes rather than the reader — and
/// `None` for every other key, so the caller's refusal-by-name stays the total answer.
///
/// The namespaces are `object_store`'s own provider prefixes plus the client options Strata
/// already publishes ([`crate::engine::store::CLIENT_KEYS`], shared rather than re-listed). It is
/// an enumeration used **only to choose a better sentence** — every key it does not recognise is
/// still refused by the caller — so it is not a gate that can let something through by omission.
fn store_key(key: &str) -> Option<&'static str> {
    const STORE_NAMESPACES: [&str; 5] = ["aws.", "s3.", "gcp.", "google.", "azure."];
    if STORE_NAMESPACES.iter().any(|ns| key.starts_with(ns)) || client_key(key).is_some() {
        return Some("A bucket, its region and where its credentials come from belong to a connection. Add one in Connections");
    }
    None
}

/// Read one `OPTIONS` entry onto the def's own field, refusing by name where there is none.
///
/// The arm set **is** the table ([`CSV_OPTION_KEYS`] / [`JSON_OPTION_KEYS`]) and the table is
/// the def: every field of [`CsvRead`] and [`JsonRead`] has a DataFusion key there and nothing
/// else does, which is what `docs/IMPORT_OPTIONS.md` documents from the other side — and what
/// lets completion offer the same set with zero drift. The three CSV options DataFusion has and
/// this deliberately lacks (`format.null_regex`, `format.terminator`, `format.double_quote`)
/// reach the by-name refusal like any other key — [`CsvRead`]'s doc comment is why they are
/// absent, and it is the read path's asymmetry rather than an oversight.
fn apply(format: &mut SourceFormat, name: &str, key: &str, value: &Value) -> Result<(), String> {
    match format {
        SourceFormat::Csv(o) => match CSV_OPTION_KEYS.iter().find(|k| k.key == key) {
            Some(k) => (k.set)(o, key, value),
            None => Err(unsupported(key, format, name)),
        },
        SourceFormat::Json(o) => match JSON_OPTION_KEYS.iter().find(|k| k.key == key) {
            Some(k) => (k.set)(o, key, value),
            None => Err(unsupported(key, format, name)),
        },
        SourceFormat::Parquet | SourceFormat::Arrow | SourceFormat::Unknown(_) => Err(format!(
            "Table '{name}' is STORED AS {}, which takes no read options",
            format.name().to_uppercase()
        )),
    }
}

/// A key with no field on the format in play. Names the format, because the commonest way to
/// reach this is a CSV option on a parquet table — which is the state [`SourceFormat`] exists to
/// make unwritable.
fn unsupported(key: &str, format: &SourceFormat, name: &str) -> String {
    format!(
        "'{key}' is not a read option for a {} table. Table '{name}' is STORED AS {}",
        format.name(),
        format.name().to_uppercase()
    )
}

/// An option's value as text. The parser produces exactly these four
/// (`DFParser::parse_option_value`); anything else is a sqlparser variant it cannot reach.
fn text(key: &str, value: &Value) -> Result<String, String> {
    match value {
        Value::SingleQuotedString(s)
        | Value::DoubleQuotedString(s)
        | Value::EscapedStringLiteral(s) => Ok(s.clone()),
        Value::Number(n, _) => Ok(n.clone()),
        _ => Err(format!("The option '{key}' needs a string or number value")),
    }
}

fn boolean(key: &str, value: &Value) -> Result<bool, String> {
    match text(key, value)?.to_ascii_lowercase().as_str() {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(format!(
            "The option '{key}' is '{other}'. It takes true or false"
        )),
    }
}

/// A single-character option, through the rule the two windows publish — so `\t` is a tab here
/// exactly as it is in a delimiter box, and a longer string is reported rather than truncated.
fn character(key: &str, what: &str, value: &Value) -> Result<char, String> {
    one_char(what, &text(key, value)?)?.ok_or_else(|| format!("The option '{key}' has no value"))
}

fn count(key: &str, value: &Value) -> Result<usize, String> {
    text(key, value)?
        .parse()
        .map_err(|_| format!("The option '{key}' takes a number of rows"))
}

/// Whole-file compression, in **DataFusion's own spelling** — there is no second vocabulary for
/// it, so the statement takes the words `format.compression` takes everywhere else and the
/// message lists them rather than restating a Strata enum.
fn compression(key: &str, value: &Value) -> Result<FileCompression, String> {
    use datafusion::common::parsers::CompressionTypeVariant as V;
    let raw = text(key, value)?;
    let parsed: FileCompressionType = raw.parse().map_err(|_| {
        format!("The option '{key}' is '{raw}'. It takes uncompressed, gzip, bzip2, xz or zstd")
    })?;
    Ok(match parsed.get_variant() {
        V::UNCOMPRESSED => FileCompression::None,
        V::GZIP => FileCompression::Gzip,
        V::BZIP2 => FileCompression::Bzip2,
        V::XZ => FileCompression::Xz,
        V::ZSTD => FileCompression::Zstd,
    })
}

/// The def's `(connection, source)` for a `LOCATION`, which is the pair
/// [`resolve_source`](crate::project::resolve_source) composes, arrived at from the composed
/// string.
///
/// A location with a scheme names an object store, and that store has to be a **connection this
/// project has**: a connection carries a provider, a region and where its credentials come from,
/// none of which a `CREATE EXTERNAL TABLE` says and one of which it must never carry. So the
/// statement *references* one, exactly as `TableDef::connection` does, and a bucket with no
/// connection is refused on the terms Configure's Save is blocked on rather than left to fail at
/// registration with "No suitable object store found" — the message the connections-first phase
/// exists to keep off a table row.
///
/// A location with no scheme is a path, and takes the local rule: stored project-relative where
/// it sits inside `root` (portable, which is what `project.json` promises), absolute otherwise.
fn source_of(
    root: &Path,
    location: &str,
    connections: &Connections,
) -> Result<(Option<String>, String), String> {
    let location = location.trim();
    if location.is_empty() {
        return Err("LOCATION has no path".into());
    }
    let Some((url, source)) = split_remote(location) else {
        return Ok((None, relativize(root, location)));
    };
    if url.starts_with("file:") {
        return Err("LOCATION takes a path, not a file:// URL".into());
    }
    let Some(url) = connections.resolve(&url) else {
        return Err(format!(
            "'{url}' is not a connection in this project. Add it in Connections"
        ));
    };
    if source.trim().is_empty() {
        return Err(format!(
            "LOCATION '{location}' names the bucket. Add the path inside it that holds the files"
        ));
    }
    Ok((Some(url), source))
}

/// The def's `(name, arrow type)` partition columns.
///
/// Two forms reach here, and DataFusion's parser folds them into one shape: bare names
/// (`PARTITIONED BY (year, month)`) leave `columns` empty, while column definitions
/// (`PARTITIONED BY (year INT)`) push their `ColumnDef`s into `columns` *as well*. So a `columns`
/// entry naming a partition column supplies its type, and one that does not is a **data** column —
/// which is refused, because the schema is inferred from the files and a declared one would be a
/// second, unenforced statement of it.
///
/// A partition with no declared type is `Utf8`, which is what DataFusion infers on its own and
/// what the Configure window defaults to — with the same consequence its standing warning names:
/// partition values are read as text, so `WHERE year = 2024` needs a cast until the column is
/// given its real type.
fn partition_cols(
    ctx: &SessionContext,
    columns: &[ColumnDef],
    names: &[String],
) -> Result<Vec<(String, String)>, String> {
    partition_columns_are_bare_words(names, ctx)?;

    for (i, name) in names.iter().enumerate() {
        if names[..i].contains(name) {
            return Err(format!("Partition column '{name}' is listed twice"));
        }
    }

    let mut declared = Vec::new();
    for column in columns {
        let rendered = column.name.to_string();
        if !names.contains(&rendered) {
            return Err(format!(
                "Schemas are inferred. Remove the column list, or move '{rendered}' into \
                 PARTITIONED BY"
            ));
        }
        if !column.options.is_empty() {
            return Err(format!(
                "Partition column '{rendered}' cannot carry column options"
            ));
        }
        declared.push((
            rendered,
            partition_type(&column.name.value, &column.data_type)?,
        ));
    }

    Ok(names
        .iter()
        .map(|name| {
            let declared = declared
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, ty)| ty.clone());
            (name.clone(), declared.unwrap_or_else(|| "Utf8".to_string()))
        })
        .collect())
}

/// One partition column's declared type as the def's Arrow name — the four the Configure window
/// offers (`PARTITION_TYPES`), in SQL spelling.
///
/// Deliberately a short list rather than every integer width sqlparser can parse: a def's type
/// string is what `catalog::parse_dtype` reads *and* what Configure renders in a picker offering
/// exactly these four, so a def carrying anything else would open on a control that cannot show
/// it. `INT8` is left out on top of that — it is Postgres's eight-*byte* integer and Arrow's
/// eight-*bit* one, and a spelling that means two different types is not one to guess at.
fn partition_type(name: &str, ty: &SqlType) -> Result<String, String> {
    let arrow = match ty {
        SqlType::Varchar(_) | SqlType::Text | SqlType::String(_) | SqlType::Char(_) => "Utf8",
        SqlType::Int(_) | SqlType::Integer(_) => "Int32",
        SqlType::BigInt(_) => "Int64",
        SqlType::Date => "Date32",
        other => {
            return Err(format!(
                "Partition column '{name}' is declared {other}. A partition column reads as \
                 VARCHAR, INT, BIGINT or DATE"
            ))
        }
    };
    Ok(arrow.to_string())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::PathBuf;
    use std::{env, process};

    use strata_model::{ConnectionDef, Provider, S3Auth, S3Store};

    use crate::engine::{Engine, RunOutcome, RunTag, StatementReport, WsId};
    use crate::project::{save_defs, ProjectDefs};
    use crate::register::register_project;

    use super::*;

    /// Run one statement and take its report.
    async fn statement(eng: &Engine, sql: &str) -> Result<StatementReport, String> {
        match eng.run(WsId(1), RunTag(1), sql.into(), 10).await? {
            RunOutcome::Statement(report) => Ok(report),
            RunOutcome::Rows(..) => panic!("{sql} ran as a query"),
        }
    }

    /// The values a query returns, as text.
    async fn read(eng: &Engine, sql: &str) -> Vec<Vec<String>> {
        let RunOutcome::Rows(output, _) = eng
            .run(WsId(2), RunTag(2), sql.into(), 100)
            .await
            .expect("query")
        else {
            panic!("{sql} did not return rows");
        };
        output
            .rows
            .into_iter()
            .map(|row| row.into_iter().map(|cell| cell.text).collect())
            .collect()
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("strata_external_{}_{tag}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        save_defs(&dir, &ProjectDefs::default()).unwrap();
        dir
    }

    /// A project folder with one CSV in it, and an engine pointed at it.
    fn project(tag: &str, file: &str, body: &str) -> (PathBuf, Engine) {
        let root = scratch(tag);
        fs::write(root.join(file), body).unwrap();
        let eng = Engine::new(BTreeMap::new());
        eng.set_data_dir(&root);
        (root, eng)
    }

    fn def_of(report: &StatementReport) -> &TableDef {
        match report.effect.as_ref() {
            Some(StoreEffect::TableUpserted { def, .. }) => def,
            other => panic!("{other:?}"),
        }
    }

    /// **The whole acceptance in one statement**: the def the typed form lands is the def Table
    /// Config would have written for the same choices — every `CsvRead` field, a project-relative
    /// source, and the partition columns typed as declared — and the table is queryable through
    /// the name straight afterwards.
    ///
    /// Asserted as a whole-value equality rather than field by field, because the claim is that
    /// the two gestures produce *one* def: a field this statement forgot to read would be a
    /// difference here rather than a silently defaulted option.
    #[tokio::test]
    async fn a_typed_csv_registration_lands_the_def_table_config_would_have_written() {
        let (root, eng) = project("csv_def", "events.csv", "id;name\n1;a\n2;b\n");

        let report = statement(
            &eng,
            &format!(
                "CREATE EXTERNAL TABLE events STORED AS CSV LOCATION '{}' OPTIONS (\
                 'format.has_header' 'true', 'format.delimiter' ';', 'format.quote' '''', \
                 'format.newlines_in_values' 'false', 'format.truncated_rows' 'true', \
                 'format.schema_infer_max_rec' '500', 'format.compression' 'uncompressed')",
                root.join("events.csv").display()
            ),
        )
        .await
        .expect("registered");

        assert_eq!(report.message, "Table 'events' created, 2 columns");
        assert_eq!(report.count, None, "a registration moves no rows");
        assert_eq!(
            def_of(&report),
            &TableDef {
                name: "events".into(),
                format: SourceFormat::Csv(CsvRead {
                    header: true,
                    delimiter: ';',
                    quote: '\'',
                    escape: None,
                    comment: None,
                    newlines_in_values: false,
                    truncated_rows: true,
                    infer_rows: Some(500),
                    compression: FileCompression::None,
                }),
                connection: None,
                sources: vec!["events.csv".into()],
                partition_cols: Vec::new(),
                origin: TableOrigin::External,
            }
        );
        assert_eq!(
            read(&eng, "SELECT name FROM events ORDER BY id")
                .await
                .len(),
            2
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// A def written by the statement replays through the **ordinary** registration pass — no
    /// code of its own, which is the point of arriving at a `TableDef` rather than at a provider.
    #[tokio::test]
    async fn a_typed_def_replays_on_the_next_open() {
        let (root, eng) = project("replay", "t.csv", "id\n1\n2\n3\n");
        let report = statement(
            &eng,
            &format!(
                "CREATE EXTERNAL TABLE t STORED AS CSV LOCATION '{}'",
                root.join("t.csv").display()
            ),
        )
        .await
        .expect("registered");

        let defs = ProjectDefs {
            tables: vec![def_of(&report).clone()],
            ..Default::default()
        };
        let cold = Engine::new(BTreeMap::new());
        let mut outcomes = Vec::new();
        register_project(&cold, &root, &defs, |o| outcomes.push(o)).await;
        assert_eq!(outcomes.len(), 1);
        assert_eq!(read(&cold, "SELECT count(*) FROM t").await, [["3"]]);
        let _ = fs::remove_dir_all(&root);
    }

    /// **A format with no reader fails by name** (P4-11) rather than falling through onto
    /// parquet or minting a `SourceFormat::Unknown`, which exists to keep a legacy *def* loading
    /// and is not something a statement may write.
    #[tokio::test]
    async fn a_format_strata_cannot_read_is_refused_by_name() {
        let (root, eng) = project("avro", "t.csv", "id\n1\n");
        let err = statement(
            &eng,
            "CREATE EXTERNAL TABLE t STORED AS AVRO LOCATION 'data/'",
        )
        .await
        .expect_err("refused");
        assert_eq!(
            err,
            "STORED AS AVRO is not a format Strata reads. Use PARQUET, CSV, JSON or ARROW"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// Every option refusal, each by name — an option the def has no field for, an option that
    /// belongs to another format, and one on a format that takes none at all. A silently dropped
    /// option is a def that lies about how the table reads.
    #[tokio::test]
    async fn an_option_with_no_field_on_the_def_is_refused_by_name() {
        let (root, eng) = project("opts", "t.csv", "id\n1\n");
        for (sql, expected) in [
            (
                "CREATE EXTERNAL TABLE t STORED AS CSV LOCATION 'd/' \
                 OPTIONS ('format.null_regex' 'NA')",
                "'format.null_regex' is not a read option for a csv table. \
                 Table 't' is STORED AS CSV",
            ),
            (
                "CREATE EXTERNAL TABLE t STORED AS PARQUET LOCATION 'd/' \
                 OPTIONS ('format.delimiter' ';')",
                "Table 't' is STORED AS PARQUET, which takes no read options",
            ),
            (
                "CREATE EXTERNAL TABLE t STORED AS JSON LOCATION 'd/' \
                 OPTIONS ('format.has_header' 'true')",
                "'format.has_header' is not a read option for a json table. \
                 Table 't' is STORED AS JSON",
            ),
        ] {
            assert_eq!(statement(&eng, sql).await.expect_err("refused"), expected);
        }
        let _ = fs::remove_dir_all(&root);
    }

    /// **The collision this statement family has with connections**, both halves.
    ///
    /// `OPTIONS` is where `datafusion-cli` writes an object store's credentials, its region and
    /// its endpoint — Strata keeps every one of those on a `ConnectionDef` instead, so they are
    /// refused toward the surface that owns them. And the refusal **never carries the value**:
    /// the arm answers off the key alone, because the sentence it produces is one the user then
    /// reads, copies and pastes.
    #[tokio::test]
    async fn an_object_store_option_is_refused_toward_connections_without_its_value() {
        let (root, eng) = project("store_opts", "t.csv", "id\n1\n");
        let surface = "A bucket, its region and where its credentials come from belong to a \
                       connection. Add one in Connections";
        for key in [
            "aws.access_key_id",
            "aws.secret_access_key",
            "aws.region",
            "aws.endpoint",
            "google.service_account",
            "timeout",
            "allow_invalid_certificates",
        ] {
            let err = statement(
                &eng,
                &format!(
                    "CREATE EXTERNAL TABLE t STORED AS CSV LOCATION 'd/' \
                     OPTIONS ('{key}' 'AKIAsecretvalue')"
                ),
            )
            .await
            .expect_err("refused");
            assert_eq!(
                err,
                format!("'{key}' is an object store setting, not a table read option. {surface}")
            );
            assert!(
                !err.contains("AKIAsecretvalue"),
                "a refusal never echoes an option's value: {err}"
            );
        }
        let _ = fs::remove_dir_all(&root);
    }

    /// A `LOCATION` naming a bucket is a **reference to a connection**, and one the project does
    /// not have is refused here rather than at registration — where it would arrive as
    /// DataFusion's "No suitable object store found", the message the connections-first phase
    /// exists to keep off a table row.
    #[tokio::test]
    async fn a_location_over_a_bucket_with_no_connection_is_refused_naming_it() {
        let (root, eng) = project("no_conn", "t.csv", "id\n1\n");
        let err = statement(
            &eng,
            "CREATE EXTERNAL TABLE events STORED AS PARQUET LOCATION 's3://acme-lake/events/'",
        )
        .await
        .expect_err("refused");
        assert_eq!(
            err,
            "'s3://acme-lake' is not a connection in this project. Add it in Connections"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// And one the project **does** have splits into the pair `resolve_source` composes: the
    /// connection's URL, and a source relative to its bucket. Asserted through `source_of`
    /// directly, because everything past it is a listing of a bucket that has to be real — the
    /// end-to-end version is `tests/object_store_minio.rs`.
    #[test]
    fn a_location_over_a_connection_splits_into_the_url_and_a_bucket_relative_source() {
        let connections = Connections::default();
        connections.note("s3://acme-lake");
        let root = Path::new("/proj");

        assert_eq!(
            source_of(
                root,
                "s3://acme-lake/events/2024/**/*.parquet",
                &connections
            ),
            Ok((
                Some("s3://acme-lake".to_string()),
                "events/2024/**/*.parquet".to_string()
            ))
        );
        assert_eq!(
            source_of(root, "s3://acme-lake", &connections),
            Err(
                "LOCATION 's3://acme-lake' names the bucket. Add the path inside it that holds \
                 the files"
                    .into()
            )
        );
        assert_eq!(
            source_of(root, "file:///data/events/", &connections),
            Err("LOCATION takes a path, not a file:// URL".into())
        );
        assert_eq!(
            source_of(root, "/elsewhere/events/", &connections),
            Ok((None, "/elsewhere/events/".to_string()))
        );
        assert_eq!(
            source_of(root, "/proj/events/", &connections),
            Ok((None, "events".to_string()))
        );
        assert_eq!(
            source_of(root, "S3://ACME-LAKE/events/", &connections),
            Ok((Some("s3://acme-lake".to_string()), "events/".to_string()))
        );
    }

    /// **A connection this project has is a connection whether or not it connected.** The set is
    /// membership, not liveness: a def whose region is wrong or whose SSO session has expired is
    /// still one the user can point a table at, and the fix happens afterwards.
    #[tokio::test]
    async fn a_connection_that_failed_to_connect_is_still_one_a_statement_may_name() {
        let eng = Engine::new(BTreeMap::new());
        let conn = ConnectionDef {
            address: "acme-lake".into(),
            provider: Provider::S3(S3Store {
                auth: S3Auth::Anonymous,
                ..Default::default()
            }),
            client_config: BTreeMap::new(),
        };
        assert!(
            eng.connect(conn).await.is_err(),
            "the def cannot describe a store"
        );
        assert_eq!(
            eng.connections.resolve("s3://acme-lake").as_deref(),
            Some("s3://acme-lake")
        );
        eng.disconnect("s3://acme-lake");
        assert_eq!(eng.connections.resolve("s3://acme-lake"), None);
    }

    /// The schema is inferred, so a **data** column list is refused — while a list that is
    /// entirely partition column definitions is how a partition column states its type.
    #[tokio::test]
    async fn a_column_list_is_partition_types_or_it_is_refused() {
        let (root, eng) = project("cols", "t.csv", "id\n1\n");
        let err = statement(
            &eng,
            "CREATE EXTERNAL TABLE t (id INT, name VARCHAR) STORED AS CSV LOCATION 'd/'",
        )
        .await
        .expect_err("refused");
        assert_eq!(
            err,
            "Schemas are inferred. Remove the column list, or move 'id' into PARTITIONED BY"
        );

        let lake = root.join("lake/year=2024/month=03");
        fs::create_dir_all(&lake).unwrap();
        fs::write(lake.join("part.csv"), "id\n1\n").unwrap();
        let report = statement(
            &eng,
            &format!(
                "CREATE EXTERNAL TABLE hits STORED AS CSV LOCATION '{}' \
                 PARTITIONED BY (year INT, month VARCHAR)",
                root.join("lake/").display()
            ),
        )
        .await
        .expect("registered");
        assert_eq!(
            def_of(&report).partition_cols,
            [
                ("year".to_string(), "Int32".to_string()),
                ("month".to_string(), "Utf8".to_string())
            ]
        );
        assert_eq!(
            read(&eng, "SELECT year FROM hits WHERE year = 2024").await,
            [["2024"]]
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// A bare partition list is text, which is what DataFusion infers on its own and what the
    /// Configure window defaults to — and a type it cannot store is refused by name rather than
    /// quietly becoming text (`catalog::parse_dtype`'s fallback, which a *def* still needs).
    #[tokio::test]
    async fn a_partition_column_is_text_unless_it_says_otherwise() {
        let root = scratch("part_types");
        let lake = root.join("lake/year=2024");
        fs::create_dir_all(&lake).unwrap();
        fs::write(lake.join("part.csv"), "id\n1\n").unwrap();
        let eng = Engine::new(BTreeMap::new());
        eng.set_data_dir(&root);

        let report = statement(
            &eng,
            &format!(
                "CREATE EXTERNAL TABLE hits STORED AS CSV LOCATION '{}' PARTITIONED BY (year)",
                root.join("lake/").display()
            ),
        )
        .await
        .expect("registered");
        assert_eq!(
            def_of(&report).partition_cols,
            [("year".to_string(), "Utf8".to_string())]
        );

        let err = statement(
            &eng,
            &format!(
                "CREATE EXTERNAL TABLE other STORED AS CSV LOCATION '{}' \
                 PARTITIONED BY (year FLOAT)",
                root.join("lake/").display()
            ),
        )
        .await
        .expect_err("refused");
        assert_eq!(
            err,
            "Partition column 'year' is declared FLOAT. A partition column reads as VARCHAR, \
             INT, BIGINT or DATE"
        );

        let err = statement(
            &eng,
            &format!(
                "CREATE EXTERNAL TABLE twice STORED AS CSV LOCATION '{}' \
                 PARTITIONED BY (year, year)",
                root.join("lake/").display()
            ),
        )
        .await
        .expect_err("refused");
        assert_eq!(err, "Partition column 'year' is listed twice");
        let _ = fs::remove_dir_all(&root);
    }

    /// **Compression is DataFusion's own vocabulary**, and every codec maps onto the def's own —
    /// asserted over the whole set, because a transposed arm compiles: the def would then persist
    /// the wrong codec, `SourceFormat::extension` would filter the listing on the wrong suffix,
    /// and the table would register as "No files matched" with the files sitting right there.
    #[tokio::test]
    async fn every_compression_spelling_lands_on_its_own_codec() {
        let (root, eng) = project("codecs", "t.csv", "id\n1\n");
        for (spelling, codec) in [
            ("uncompressed", FileCompression::None),
            ("gzip", FileCompression::Gzip),
            ("bzip2", FileCompression::Bzip2),
            ("xz", FileCompression::Xz),
            ("zstd", FileCompression::Zstd),
        ] {
            let report = statement(
                &eng,
                &format!(
                    "CREATE OR REPLACE EXTERNAL TABLE t STORED AS CSV LOCATION '{}' \
                     OPTIONS ('format.compression' '{spelling}')",
                    root.join("t.csv").display()
                ),
            )
            .await;
            match report {
                Ok(report) => assert_eq!(
                    def_of(&report).format,
                    SourceFormat::Csv(CsvRead {
                        compression: codec,
                        ..Default::default()
                    }),
                    "{spelling}"
                ),
                Err(e) => assert!(
                    e.contains(&format!(".csv{}", codec.extension())),
                    "{spelling}: the listing filtered on the codec's own suffix: {e}"
                ),
            }
        }
        let _ = fs::remove_dir_all(&root);
    }

    /// The clauses a `TableDef` cannot carry, each refused **by name** — the destructure has no
    /// `..`, so a clause sqlparser learns later is a compile error rather than one silently
    /// dropped.
    #[tokio::test]
    async fn the_clauses_a_def_cannot_carry_are_refused_by_name() {
        let (root, eng) = project("clauses", "t.csv", "id\n1\n");
        for (sql, expected) in [
            (
                "CREATE EXTERNAL TABLE t STORED AS CSV LOCATION 'd/' WITH ORDER (id)",
                "CREATE EXTERNAL TABLE does not support WITH ORDER",
            ),
            (
                "CREATE UNBOUNDED EXTERNAL TABLE t STORED AS CSV LOCATION 'd/'",
                "CREATE EXTERNAL TABLE does not support UNBOUNDED",
            ),
            (
                "CREATE EXTERNAL TABLE t (id INT, PRIMARY KEY (id)) STORED AS CSV LOCATION 'd/'",
                "Table constraints are not supported",
            ),
            (
                "CREATE EXTERNAL TEMPORARY TABLE t STORED AS CSV LOCATION 'd/'",
                "CREATE EXTERNAL TABLE does not support TEMPORARY",
            ),
        ] {
            assert_eq!(statement(&eng, sql).await.expect_err("refused"), expected);
        }
        let _ = fs::remove_dir_all(&root);
    }

    /// The one namespace, resolved before anything registers: `IF NOT EXISTS` reports a no-op
    /// with nothing to fold, a plain create over a taken name errors, `OR REPLACE` rewrites the
    /// def — and an **internal** table's name is fenced off, because pointing it at the user's
    /// own directory would leave `.strata/tables/<slug>/` with no def naming it and nothing left
    /// that could ever delete it.
    #[tokio::test]
    async fn the_name_resolves_against_the_one_namespace() {
        let (root, eng) = project("names", "t.csv", "id\n1\n");
        let create = format!(
            "CREATE EXTERNAL TABLE t STORED AS CSV LOCATION '{}'",
            root.join("t.csv").display()
        );
        statement(&eng, &create).await.expect("registered");

        assert_eq!(
            statement(&eng, &create).await.expect_err("taken"),
            "Table 't' already exists"
        );
        let skipped = statement(
            &eng,
            &create.replacen("TABLE t", "TABLE IF NOT EXISTS t", 1),
        )
        .await
        .expect("no-op");
        assert_eq!(skipped.message, "Table 't' already exists");
        assert_eq!(skipped.effect, None, "a no-op folds nothing");

        let replaced = statement(&eng, &create.replacen("CREATE ", "CREATE OR REPLACE ", 1))
            .await
            .expect("replaced");
        assert_eq!(replaced.message, "Table 't' replaced, 1 column");

        statement(&eng, "CREATE TABLE owned AS SELECT 1 AS n")
            .await
            .expect("internal table");
        let owned = create.replacen("TABLE t", "TABLE owned", 1);
        assert_eq!(
            statement(&eng, &owned.replacen("CREATE ", "CREATE OR REPLACE ", 1))
                .await
                .expect_err("fenced"),
            "'owned' is a table Strata stores in this project. Drop it first"
        );
        assert_eq!(
            statement(&eng, &owned).await.expect_err("taken"),
            "Table 'owned' already exists"
        );
        let skipped = statement(
            &eng,
            &owned.replacen("TABLE owned", "TABLE IF NOT EXISTS owned", 1),
        )
        .await
        .expect("no-op");
        assert_eq!(skipped.message, "Table 'owned' already exists");
        assert_eq!(skipped.effect, None, "a no-op folds nothing");

        statement(&eng, "CREATE OR REPLACE VIEW v AS SELECT 1 AS n")
            .await
            .expect("view");
        assert_eq!(
            statement(&eng, &create.replacen("TABLE t", "TABLE v", 1))
                .await
                .expect_err("a view"),
            "'v' is a view"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// `\t` is a tab in a typed `OPTIONS` exactly as it is in a delimiter box — one rule
    /// (`util::one_char`), three surfaces, so the typed statement lands on the def Configure
    /// would have written for the same file.
    #[tokio::test]
    async fn a_delimiter_reads_the_way_the_configure_box_reads_it() {
        let (root, eng) = project("tabs", "t.csv", "id\tname\n1\ta\n");
        let report = statement(
            &eng,
            &format!(
                "CREATE EXTERNAL TABLE t STORED AS CSV LOCATION '{}' \
                 OPTIONS ('format.delimiter' '\\t')",
                root.join("t.csv").display()
            ),
        )
        .await
        .expect("registered");
        assert_eq!(
            def_of(&report).format,
            SourceFormat::Csv(CsvRead {
                delimiter: '\t',
                ..Default::default()
            })
        );
        assert_eq!(read(&eng, "SELECT name FROM t").await, [["a"]]);
        let _ = fs::remove_dir_all(&root);
    }

    /// `STORED AS NDJSON` **is** a shape, so the option that would contradict it is refused —
    /// two statements of one fact that can disagree. `STORED AS JSON` is where the shape is set.
    #[tokio::test]
    async fn ndjson_states_the_shape_it_cannot_then_be_told() {
        let root = scratch("json_shape");
        fs::write(root.join("a.json"), "[{\"id\":1},{\"id\":2}]").unwrap();
        let eng = Engine::new(BTreeMap::new());
        eng.set_data_dir(&root);

        assert_eq!(
            statement(
                &eng,
                "CREATE EXTERNAL TABLE t STORED AS NDJSON LOCATION 'a.json' \
                 OPTIONS ('format.newline_delimited' 'false')"
            )
            .await
            .expect_err("refused"),
            "STORED AS NDJSON is newline-delimited JSON. Use STORED AS JSON to set \
             'format.newline_delimited'"
        );

        let report = statement(
            &eng,
            "CREATE EXTERNAL TABLE t STORED AS JSON LOCATION 'a.json' \
             OPTIONS ('format.newline_delimited' 'false', 'format.schema_infer_max_rec' '0')",
        )
        .await
        .expect("registered");
        assert_eq!(
            def_of(&report).format,
            SourceFormat::Json(JsonRead {
                shape: JsonShape::Array,
                infer_rows: None,
                compression: FileCompression::None,
            })
        );
        assert_eq!(read(&eng, "SELECT count(*) FROM t").await, [["2"]]);
        let _ = fs::remove_dir_all(&root);
    }

    /// **The per-format key projection and each key's declared kind agree with the arms.**
    /// For every format word, every key [`option_keys_for`] offers must land through [`apply`]
    /// with a value of its declared kind — which catches both a projection drift (a key
    /// offered that the format's arm refuses) and a kind/coercion drift (a `Bool` row whose
    /// setter actually wants a character). The NDJSON drop is asserted against
    /// [`read_format`]'s own refusal, and the no-options formats answer empty.
    #[test]
    fn option_keys_for_agrees_with_apply() {
        let plausible = |kind: OptionKind| match kind {
            OptionKind::Bool => "true",
            OptionKind::Char => "x",
            OptionKind::Int => "10",
            OptionKind::Enum(words) => words[0],
        };
        for format_word in STORED_AS_FORMATS {
            let mut format = read_format(format_word, "t", &[]).expect("a format Strata reads");
            for (key, kind, _) in option_keys_for(format_word) {
                let value = Value::SingleQuotedString(plausible(kind).into());
                apply(&mut format, "t", key, &value).unwrap_or_else(|e| {
                    panic!("{format_word} offers '{key}' but apply refuses: {e}")
                });
            }
        }
        assert!(
            option_keys_for("NDJSON")
                .iter()
                .all(|(k, ..)| *k != "format.newline_delimited"),
            "NDJSON must not offer the shape key read_format refuses there"
        );
        assert!(
            read_format(
                "NDJSON",
                "t",
                &[(
                    "format.newline_delimited".into(),
                    Value::SingleQuotedString("true".into())
                )]
            )
            .is_err(),
            "the premise: read_format refuses the shape key on NDJSON"
        );
        assert!(option_keys_for("PARQUET").is_empty());
        assert!(option_keys_for("ARROW").is_empty());
        assert!(
            option_keys_for("AVRO").is_empty(),
            "an unknown word offers nothing"
        );
    }

    /// **The completion vocabulary is this module's own arms.** Every entry of
    /// [`STORED_AS_FORMATS`] parses through [`read_format`] and a non-member does not, so the
    /// offer at `STORED AS |` can never name a format the arm then refuses; and every word an
    /// `Enum` option offers parses through its own coercion.
    #[test]
    fn stored_as_formats_parse_through_read_format() {
        for format in STORED_AS_FORMATS {
            assert!(read_format(format, "t", &[]).is_ok(), "{format}");
        }
        assert!(
            read_format("AVRO", "t", &[]).is_err(),
            "a format with no reader is not offered"
        );
        for word in COMPRESSION_WORDS {
            assert!(
                compression(
                    "format.compression",
                    &Value::SingleQuotedString(word.to_string())
                )
                .is_ok(),
                "{word}"
            );
        }
    }
}
