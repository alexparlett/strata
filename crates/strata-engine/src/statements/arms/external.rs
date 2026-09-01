//! **Typed `CREATE EXTERNAL TABLE`** (ED-10) — the second gesture into the funnel Table Config
//! already uses. `docs/STATEMENTS_SPEC.md` §6.7.
//!
//! The statement becomes a [`TableDef`] and goes through
//! [`register_external`](crate::catalog::register_external), so the store, the persist
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
//! [`SourceDef`](strata_model::SourceDef), which holds a reference to credentials and never
//! a credential. So the split is by namespace:
//!
//! - a `format.` key the def has a field for is **read** onto it;
//! - a client option or store namespace is **refused toward Sources** on the key alone —
//!   [`store_key`] never looks at the value, because that value may be a secret and a refusal is a
//!   sentence the user then copies and pastes;
//! - anything else is refused **by name**, which keeps the mechanism total rather than a list of
//!   the keys we thought of.
//!
//! A refused statement is not recorded, so a pasted key does not outlive its buffer.

use std::collections::BTreeMap;
use std::path::Path;

use datafusion::logical_expr::TableType;
use datafusion::prelude::SessionContext;
use datafusion::sql::parser::{CreateExternalTable, Statement as DFStatement};
use datafusion::sql::sqlparser::ast::{ColumnDef, DataType as SqlType, Value};
use datafusion::sql::TableReference;

use crate::catalog::register_external;
use crate::export::partition_columns_are_bare_words;
use crate::formats::Formats;
use crate::policy::Principal;
use crate::register::table_spec;
use crate::statements::ctx::StmtCtx;
use crate::statements::pipeline::Qualified;
use crate::statements::report::{StatementOutcome, StoreEffect};
use crate::statements::target::{elsewhere, resolve_target};
use crate::statements::StmtKind;
use crate::SourceDefs;
use strata_arrow::client::client_key;
use strata_core::project::{relativize, split_remote};
use strata_core::util::plural;
use strata_model::{SourceFormat, TableDef, TableOrigin};

use super::existing;

/// What [`elsewhere`] calls the objects this statement creates.
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
    cx: &StmtCtx,
    who: &Principal,
    stmt: &Qualified,
) -> Result<StatementOutcome, String> {
    let ctx = &cx.ctx;
    let DFStatement::CreateExternalTable(create) = (**stmt).clone() else {
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

    let Some(root) = cx.root.as_deref() else {
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
    let target = resolve_target(ctx, &TableReference::parse_str(&name.to_string()));
    cx.require_target(who, StmtKind::CreateExternalTable, &target)
        .await?;
    let name = target.workspace(WHAT)?;
    let format = read_format(&cx.formats, &file_type, &name, &options)?;
    let (data_source, path) = source_of(root, &location, &cx.sources, &cx.registrants)?;
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
        Some(_) if cx.internal.contains(&name) => {
            return Err(format!(
                "'{name}' is a table Strata stores in this project. Drop it first"
            ))
        }
        taken => taken.is_some(),
    };

    let def = TableDef {
        name: name.clone(),
        format,
        source: data_source,
        paths: vec![path],
        partition_cols: partitions,
        origin: TableOrigin::External,
    };
    let meta = register_external(
        ctx,
        &cx.formats,
        cx.tables.as_ref(),
        &table_spec(root, &def, &cx.sources, &cx.registrants),
    )
    .await?;

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

/// The `format.*` options a statement wrote, as the format's own reader takes them.
///
/// The three things this decides are true of every format, which is why they are decided here
/// rather than by each reader: a key written twice, a key that belongs to the object store, and
/// the shape of a value the parser produced. What each key then *means* is the registered
/// format's own ([`Formats::read`]).
fn read_format(
    formats: &Formats,
    file_type: &str,
    name: &str,
    options: &[(String, Value)],
) -> Result<SourceFormat, String> {
    let mut read = BTreeMap::new();
    for (key, value) in options {
        let key = key.to_ascii_lowercase();
        if let Some(surface) = store_key(&key) {
            return Err(format!(
                "'{key}' is an object store setting, not a table read option. {surface}"
            ));
        }
        let value = text(&key, value)?;
        if read.insert(key.clone(), value).is_some() {
            return Err(format!("The option '{key}' is set twice"));
        }
    }
    formats.read(file_type, name, &read)
}

/// Which surface owns `key`, when it is one the object store takes rather than the reader — and
/// `None` for every other key, so the caller's refusal-by-name stays the total answer.
///
/// The namespaces are `object_store`'s own provider prefixes plus the client options Strata
/// already publishes ([`strata_arrow::client::CLIENT_KEYS`], shared rather than re-listed). It is
/// an enumeration used **only to choose a better sentence** — every key it does not recognise is
/// still refused by the caller — so it is not a gate that can let something through by omission.
fn store_key(key: &str) -> Option<&'static str> {
    const STORE_NAMESPACES: [&str; 5] = ["aws.", "s3.", "gcp.", "google.", "azure."];
    if STORE_NAMESPACES.iter().any(|ns| key.starts_with(ns)) || client_key(key).is_some() {
        return Some("A bucket, its region and where its credentials come from belong to a data source. Add one in Sources");
    }
    None
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

/// The def's `(data source, source)` for a `LOCATION`, which is the pair
/// [`resolve_source`](strata_core::project::resolve_source) composes, arrived at from the composed
/// string.
///
/// A location with a scheme names an object store, and that store has to be a **data source this
/// project has**: a data source carries a provider, a region and where its credentials come from,
/// none of which a `CREATE EXTERNAL TABLE` says and one of which it must never carry. So the
/// statement *references* one, exactly as `TableDef::data source` does, and a bucket with no
/// data source is refused on the terms Configure's Save is blocked on rather than left to fail at
/// registration with "No suitable object store found" — the message the data sources-first phase
/// exists to keep off a table row.
///
/// A location with no scheme is a path, and takes the local rule: stored project-relative where
/// it sits inside `root` (portable, which is what `project.json` promises), absolute otherwise.
fn source_of(
    root: &Path,
    location: &str,
    sources: &SourceDefs,
    registrants: &crate::sources::source::Registrants,
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
    let Some(url) = sources.by_prefix(registrants, &url) else {
        return Err(format!(
            "'{url}' is not a data source in this project. Add it in Sources"
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
    use std::sync::Arc;
    use std::{env, process};

    use strata_model::{CsvRead, FileCompression, JsonRead, JsonShape, SourceDef};

    use crate::formats::fake::TestFormat;
    use crate::sql::complete;
    use crate::sql::symbols::Symbols;
    use crate::{Engine, EngineBuilder, RunOutcome, RunRows, RunTag, StatementReport, WsId};
    use strata_core::project::{save_defs, ProjectDefs};

    use super::*;

    /// Run one statement and take its report.
    async fn statement(eng: &Engine, sql: &str) -> Result<StatementReport, String> {
        match eng
            .ws(WsId(1))
            .run(RunTag(1), sql.into(), 10)
            .await
            .map_err(|e| e.to_string())?
        {
            RunOutcome::Statement(report) => Ok(report),
            RunOutcome::Rows(..) => panic!("{sql} ran as a query"),
        }
    }

    /// The values a query returns, as text.
    async fn read(eng: &Engine, sql: &str) -> Vec<Vec<String>> {
        let RunOutcome::Rows(RunRows { output, .. }) = eng
            .ws(WsId(2))
            .run(RunTag(2), sql.into(), 100)
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
    fn project(tag: &str, file: &str, body: &str) -> (PathBuf, Arc<Engine>) {
        project_with(tag, file, body, |builder| builder)
    }

    /// The same, for a test whose engine is built with something extra on it.
    fn project_with(
        tag: &str,
        file: &str,
        body: &str,
        with: impl FnOnce(EngineBuilder) -> EngineBuilder,
    ) -> (PathBuf, Arc<Engine>) {
        let root = scratch(tag);
        fs::write(root.join(file), body).unwrap();
        let eng = with(Engine::builder()).build();
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
                source: None,
                paths: vec!["events.csv".into()],
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
        let cold = Engine::builder().build();
        let mut outcomes = Vec::new();
        cold.catalog()
            .sync(cold.catalog().spec(&root, &defs), |a| outcomes.push(a.outcome))
            .await;
        assert_eq!(outcomes.len(), 1);
        assert_eq!(read(&cold, "SELECT count(*) FROM t").await, [["3"]]);
        let _ = fs::remove_dir_all(&root);
    }

    /// **An embedder's format, end to end through the app's own funnels.** Registered with one
    /// builder call, named by a typed statement with options of its own, landed on a def that
    /// carries them verbatim, registered through `register_external` like any other table, and
    /// read back through its name — no arm, no store path and no surface knows the format exists.
    #[tokio::test]
    async fn a_registered_format_is_named_by_a_statement_and_read_back_through_it() {
        let (root, eng) = project_with(
            "extension",
            "places.testfmt",
            "id,name\n1,here\n2,there\n",
            |builder| builder.with_format(TestFormat),
        );
        let report = statement(
            &eng,
            "CREATE EXTERNAL TABLE places STORED AS testfmt LOCATION 'places.testfmt' \
             OPTIONS ('format.crs' 'EPSG:4326')",
        )
        .await
        .expect("the registered format is named");

        assert_eq!(
            def_of(&report).format,
            SourceFormat::Extension {
                format: "testfmt".into(),
                options: BTreeMap::from([("format.crs".into(), "EPSG:4326".into())]),
            },
            "the def carries the format's own options verbatim"
        );
        assert_eq!(
            read(&eng, "SELECT name FROM places ORDER BY id").await,
            [["here"], ["there"]]
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// **The `STORED AS` offer is the registry**, so a format appears in completion exactly when
    /// it is registered — the offer-mirrors-arm rule, now with nothing to keep in step by hand.
    #[test]
    fn a_registered_format_appears_in_the_stored_as_offer() {
        let eng = Engine::builder().with_format(TestFormat).build();
        let catalog = Symbols::build([], [], eng.lang().bundle(), String::new());
        let offered: Vec<String> =
            complete(&catalog, "CREATE EXTERNAL TABLE t STORED AS ", 34, true)
                .into_iter()
                .map(|c| c.label)
                .collect();
        assert!(offered.contains(&"TESTFMT".to_string()), "{offered:?}");
        assert!(offered.contains(&"PARQUET".to_string()), "{offered:?}");
    }

    /// **A format nothing is registered for fails by name** (P4-11) rather than falling through
    /// onto parquet or minting an extension def a registration would then have to refuse. The
    /// words it offers instead are the registry's own, so an embedder's format appears there the
    /// moment it is registered.
    #[tokio::test]
    async fn a_format_with_no_registrant_is_refused_by_name() {
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

    /// **The collision this statement family has with data sources**, both halves.
    ///
    /// `OPTIONS` is where `datafusion-cli` writes an object store's credentials, its region and
    /// its endpoint — Strata keeps every one of those on a `SourceDef` instead, so they are
    /// refused toward the surface that owns them. And the refusal **never carries the value**:
    /// the arm answers off the key alone, because the sentence it produces is one the user then
    /// reads, copies and pastes.
    #[tokio::test]
    async fn an_object_store_option_is_refused_toward_sources_without_its_value() {
        let (root, eng) = project("store_opts", "t.csv", "id\n1\n");
        let surface = "A bucket, its region and where its credentials come from belong to a \
                       data source. Add one in Sources";
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

    /// A `LOCATION` naming a bucket is a **reference to a data source**, and one the project does
    /// not have is refused here rather than at registration — where it would arrive as
    /// DataFusion's "No suitable object store found", the message the data sources-first phase
    /// exists to keep off a table row.
    #[tokio::test]
    async fn a_location_over_a_bucket_with_no_source_is_refused_naming_it() {
        let (root, eng) = project("no_conn", "t.csv", "id\n1\n");
        let err = statement(
            &eng,
            "CREATE EXTERNAL TABLE events STORED AS PARQUET LOCATION 's3://acme-lake/events/'",
        )
        .await
        .expect_err("refused");
        assert_eq!(
            err,
            "'s3://acme-lake' is not a data source in this project. Add it in Sources"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// And one the project **does** have splits into the pair `resolve_source` composes: the
    /// data source's URL, and a source relative to its bucket. Asserted through `source_of`
    /// directly, because everything past it is a listing of a bucket that has to be real — the
    /// end-to-end version is `tests/object_store_minio.rs`.
    #[test]
    fn a_location_over_a_source_splits_into_the_address_and_a_relative_path() {
        let registrants = Engine::builder().build();
        let sources = SourceDefs::of(&[SourceDef {
            config: [("address".to_string(), "acme-lake".into())]
                .into_iter()
                .collect(),
            name: "acme_lake".into(),
            kind: "s3".into(),
            ..Default::default()
        }]);
        let root = Path::new("/proj");

        assert_eq!(
            source_of(
                root,
                "s3://acme-lake/events/2024/**/*.parquet",
                &sources,
                registrants.registry()
            ),
            Ok((
                Some("acme_lake".to_string()),
                "events/2024/**/*.parquet".to_string()
            ))
        );
        assert_eq!(
            source_of(root, "s3://acme-lake", &sources, registrants.registry()),
            Err(
                "LOCATION 's3://acme-lake' names the bucket. Add the path inside it that holds \
                 the files"
                    .into()
            )
        );
        assert_eq!(
            source_of(
                root,
                "file:///data/events/",
                &sources,
                registrants.registry()
            ),
            Err("LOCATION takes a path, not a file:// URL".into())
        );
        assert_eq!(
            source_of(root, "/elsewhere/events/", &sources, registrants.registry()),
            Ok((None, "/elsewhere/events/".to_string()))
        );
        assert_eq!(
            source_of(root, "/proj/events/", &sources, registrants.registry()),
            Ok((None, "events".to_string()))
        );
        assert_eq!(
            source_of(
                root,
                "S3://ACME-LAKE/events/",
                &sources,
                registrants.registry()
            ),
            Ok((Some("acme_lake".to_string()), "events/".to_string()))
        );
    }

    /// **A data source this project has is a data source whether or not it connected.** The set is
    /// membership, not liveness: a def whose region is wrong or whose SSO session has expired is
    /// still one the user can point a table at, and the fix happens afterwards.
    #[tokio::test]
    async fn a_source_that_failed_to_connect_is_still_one_a_statement_may_name() {
        let eng = Engine::builder().build();
        let conn = SourceDef {
            kind: "s3".into(),
            name: "acme_lake".into(),
            config: [("address", "acme-lake"), ("auth", "anonymous")]
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            ..Default::default()
        };
        assert!(
            eng.sources().connect(conn).await.is_err(),
            "the def cannot describe a store"
        );
        assert_eq!(
            eng.source_defs.resolve("acme_lake").as_deref(),
            Some("acme_lake")
        );
        let _ = eng.sources().disconnect("acme_lake");
        assert_eq!(eng.source_defs.resolve("acme_lake"), None);
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
        let eng = Engine::builder().build();
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

    /// **The shape is an option, not a format.** DataFusion has one JSON format whose
    /// `newline_delimited` defaults to true, so `NDJSON` is refused like any other word nothing
    /// is registered for, and `STORED AS JSON` is where the layout is chosen.
    #[tokio::test]
    async fn the_json_shape_is_an_option_and_ndjson_is_not_a_format() {
        let root = scratch("json_shape");
        fs::write(root.join("a.json"), "[{\"id\":1},{\"id\":2}]").unwrap();
        let eng = Engine::builder().build();
        eng.set_data_dir(&root);

        assert_eq!(
            statement(
                &eng,
                "CREATE EXTERNAL TABLE t STORED AS NDJSON LOCATION 'a.json' \
                 OPTIONS ('format.newline_delimited' 'false')"
            )
            .await
            .expect_err("refused"),
            "STORED AS NDJSON is not a format Strata reads. Use PARQUET, CSV, JSON or ARROW"
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
}
