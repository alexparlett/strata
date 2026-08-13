//! The PREVIEW pane's text — what the chosen options will actually produce.
//!
//! **Only real facts** (the P3-08 rule). Every row shown is a row the grid already fetched
//! (`ExportTarget::sample`), every type is the run's own schema, and every count is read from
//! the run. Nothing here estimates: the canvas's `estSize()` invented compression factors to
//! quote "≈ 1.2 MB", and a made-up byte figure next to real ones is exactly what the inspector
//! rejected. The footer quotes rows instead.
//!
//! The partitioned preview shows the tree *shape*, built from values that genuinely appear in
//! the page in hand — never a fabricated distinct count. It is honest about being partial: the
//! trailing `…` says there is more than one page's worth.

use strata_model::{Cell, Kind};

use super::model::{ExportDraft, ExportTarget, FormatId, NullChoice};

/// How many rows the preview shows. Enough to see the shape of a row, few enough to stay in
/// the pane without scrolling for a typical schema.
const PREVIEW_ROWS: usize = 5;

/// How many values of the first partition level to draw before summarising.
const TREE_BRANCHES: usize = 3;

/// Build the preview for the current draft.
pub fn build(draft: &ExportDraft, target: &ExportTarget) -> String {
    if draft.partition.is_active() {
        return partition_tree(draft, target);
    }
    match draft.format {
        FormatId::Csv => csv(draft, target),
        FormatId::Json => json(target),
        FormatId::Parquet => parquet(draft, target),
        FormatId::Arrow => arrow(target),
    }
}

/// The rows the preview draws — the page in hand, capped.
fn rows(target: &ExportTarget) -> &[Vec<Cell>] {
    let n = target.sample.len().min(PREVIEW_ROWS);
    &target.sample[..n]
}

/// A CSV rendering that mirrors the writer: the chosen delimiter and null text, and a field
/// quoted exactly when it contains the delimiter, the quote, or a newline.
fn csv(draft: &ExportDraft, target: &ExportTarget) -> String {
    let delimiter = resolve(&draft.csv_delimiter).unwrap_or(',');
    let quote = resolve(&draft.csv_quote).unwrap_or('"');
    let escape = resolve(&draft.csv_escape);
    let null_text = match draft.csv_null {
        NullChoice::Empty => "",
        NullChoice::Null => "NULL",
        NullChoice::NaN => "NaN",
        NullChoice::Custom => &draft.csv_null_custom,
    };

    let field = |raw: &str| -> String {
        if raw.contains(delimiter) || raw.contains(quote) || raw.contains('\n') {
            let inner = match (draft.csv_double_quote, escape) {
                (true, _) => raw.replace(quote, &format!("{quote}{quote}")),
                (false, Some(esc)) => raw.replace(quote, &format!("{esc}{quote}")),
                (false, None) => raw.to_string(),
            };
            format!("{quote}{inner}{quote}")
        } else {
            raw.to_string()
        }
    };

    let mut lines = Vec::new();
    if draft.csv_header {
        lines.push(
            target
                .columns
                .iter()
                .map(|c| field(&c.name))
                .collect::<Vec<_>>()
                .join(&delimiter.to_string()),
        );
    }
    for row in rows(target) {
        lines.push(
            row.iter()
                .map(|cell| {
                    if cell.null {
                        null_text.to_string()
                    } else {
                        field(&cell.text)
                    }
                })
                .collect::<Vec<_>>()
                .join(&delimiter.to_string()),
        );
    }
    if lines.is_empty() {
        return "(no rows to preview)".into();
    }
    lines.join("\n")
}

/// NDJSON — one object per line, which is the only shape DataFusion's JSON writer produces.
fn json(target: &ExportTarget) -> String {
    let rows = rows(target);
    if rows.is_empty() {
        return "(no rows to preview)".into();
    }
    rows.iter()
        .map(|row| {
            let fields: Vec<String> = target
                .columns
                .iter()
                .zip(row)
                .map(|(col, cell)| {
                    let value = if cell.null {
                        "null".to_string()
                    } else if matches!(col.kind, Kind::Num | Kind::Bool) {
                        cell.text.clone()
                    } else {
                        format!("\"{}\"", cell.text.replace('"', "\\\""))
                    };
                    format!("\"{}\":{value}", col.name)
                })
                .collect();
            format!("{{{}}}", fields.join(","))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A Parquet schema summary plus the settings that will be written — no rows, because a
/// parquet file has none to show as text.
fn parquet(draft: &ExportDraft, target: &ExportTarget) -> String {
    let body: Vec<String> = target
        .columns
        .iter()
        .map(|c| {
            let repetition = if c.nullable { "optional" } else { "required" };
            format!("  {repetition} {} {};", parquet_type(c.kind), c.name)
        })
        .collect();
    let level = match draft.pq_codec.levels() {
        Some(_) => format!("({})", draft.pq_level),
        None => String::new(),
    };
    let codec = format!("{:?}", draft.pq_codec).to_lowercase();
    let statistics = format!("{:?}", draft.pq_statistics).to_lowercase();
    let writer = match draft.pq_writer_version {
        strata_core::engine::export::WriterVersion::V1 => "1.0",
        strata_core::engine::export::WriterVersion::V2 => "2.0",
    };
    let dictionary = if draft.pq_dictionary { "on" } else { "off" };
    format!(
        "message result {{\n{}\n}}\n\ncompression : {codec}{level}\nstatistics  : {statistics}\n\
         row group   : {} rows\nwriter      : v{writer} · dictionary {dictionary}",
        body.join("\n"),
        super::model::thousands(draft.pq_row_group),
    )
}

/// The Arrow schema — the only thing there is to say, since the format takes no options.
fn arrow(target: &ExportTarget) -> String {
    let body: Vec<String> = target
        .columns
        .iter()
        .map(|c| format!("  {}: {}", c.name, arrow_type(c.kind)))
        .collect();
    format!(
        "Arrow IPC file\nschema {{\n{}\n}}\n\n(no write options)",
        body.join("\n")
    )
}

/// The Hive tree a partitioned export writes, drawn from values that actually appear in the
/// page in hand. Deliberately partial and says so — the page is not the snapshot.
fn partition_tree(draft: &ExportDraft, target: &ExportTarget) -> String {
    let columns = draft.partition.effective();
    let extension = draft.format.extension();
    let index = |name: &str| target.columns.iter().position(|c| c.name == name);

    let mut lines = vec![format!("{}/", draft.suggested_name(target))];

    match columns
        .first()
        .and_then(|name| index(name).map(|i| (name, i)))
    {
        Some((first, col)) => {
            let mut values: Vec<String> = target
                .sample
                .iter()
                .filter_map(|row| row.get(col))
                .map(|cell| cell.text.clone())
                .collect();
            values.sort();
            values.dedup();

            for value in values.iter().take(TREE_BRANCHES) {
                lines.push(format!("  {first}={value}/"));
                match columns.get(1) {
                    Some(second) => {
                        lines.push(format!("    {second}=…/"));
                        lines.push(format!("      part-0.{extension}"));
                    }
                    None => lines.push(format!("    part-0.{extension}")),
                }
            }
            if values.len() > TREE_BRANCHES {
                lines.push("  …".into());
            }
            lines.push(String::new());
            lines.push(format!(
                "shape from the {} loaded; the full export covers {}",
                thousands_rows(target.sample.len()),
                thousands_rows(target.total)
            ));
        }
        None => lines.push("  (choose a column to see the tree)".into()),
    }

    lines.push(format!("levels: {}", columns.join(" / ")));
    lines.push(format!(
        "partition columns in files: {}",
        if draft.partition.keep_columns {
            "kept"
        } else {
            "removed"
        }
    ));
    lines.join("\n")
}

fn thousands_rows(n: usize) -> String {
    let unit = if n == 1 { "row" } else { "rows" };
    format!("{} {unit}", super::model::thousands(n))
}

fn parquet_type(kind: Kind) -> &'static str {
    match kind {
        Kind::Num => "DOUBLE",
        Kind::Str => "BYTE_ARRAY (UTF8)",
        Kind::Bool => "BOOLEAN",
        Kind::Ts => "INT64 (TIMESTAMP_MICROS)",
        Kind::Struct => "group",
        Kind::List => "group (LIST)",
        Kind::Map => "group (MAP)",
    }
}

fn arrow_type(kind: Kind) -> &'static str {
    match kind {
        Kind::Num => "float64",
        Kind::Str => "utf8",
        Kind::Bool => "bool",
        Kind::Ts => "timestamp[us]",
        Kind::Struct => "struct",
        Kind::List => "list",
        Kind::Map => "map",
    }
}

/// The same escape resolution the spec does, so the preview and the file agree.
fn resolve(raw: &str) -> Option<char> {
    match raw {
        "" => None,
        "\\t" => Some('\t'),
        "\\n" => Some('\n'),
        "\\\\" => Some('\\'),
        other => {
            let mut chars = other.chars();
            let first = chars.next()?;
            chars.next().is_none().then_some(first)
        }
    }
}

#[cfg(test)]
mod tests {
    use datafusion::arrow::datatypes::{DataType, Field};
    use strata_core::engine::column_info;

    use super::*;
    use crate::apps::export::model::{CodecChoice, ScopeChoice};
    use strata_model::{ColumnInfo, SnapshotId};

    fn col(name: &str, dtype: DataType) -> ColumnInfo {
        column_info(&Field::new(name, dtype, true))
    }

    fn cell(text: &str) -> Cell {
        Cell {
            text: text.into(),
            null: false,
        }
    }

    fn null_cell() -> Cell {
        Cell {
            text: String::new(),
            null: true,
        }
    }

    fn target() -> ExportTarget {
        ExportTarget {
            snapshot: SnapshotId(1),
            columns: vec![
                col("region", DataType::Utf8),
                col("amount", DataType::Int64),
            ],
            total: 48_213,
            sort: None,
            page: 1,
            page_size: 100,
            label: "join".into(),
            sample: vec![
                vec![cell("emea"), cell("12.50")],
                vec![cell("amer"), null_cell()],
            ],
        }
    }

    #[test]
    fn csv_previews_the_real_rows_with_the_chosen_delimiter() {
        let draft = ExportDraft {
            csv_delimiter: ";".into(),
            ..Default::default()
        };
        let text = build(&draft, &target());
        assert_eq!(
            text, "region;amount\nemea;12.50\namer;",
            "header + the two rows in hand, null as empty"
        );
    }

    #[test]
    fn the_header_row_follows_its_toggle() {
        let draft = ExportDraft {
            csv_header: false,
            ..Default::default()
        };
        assert!(!build(&draft, &target()).contains("region,amount"));
    }

    #[test]
    fn the_null_text_is_whatever_was_chosen() {
        let draft = ExportDraft {
            csv_null: NullChoice::Null,
            ..Default::default()
        };
        assert!(build(&draft, &target()).ends_with("amer,NULL"));
    }

    #[test]
    fn a_field_holding_the_delimiter_is_quoted_the_way_the_writer_will() {
        let mut t = target();
        t.sample = vec![vec![cell("a,b"), cell("1")]];
        let text = build(&ExportDraft::default(), &t);
        assert!(text.contains("\"a,b\",1"), "{text}");
    }

    #[test]
    fn a_quote_inside_a_field_doubles_when_double_quote_is_on() {
        let mut t = target();
        t.sample = vec![vec![cell("say \"hi\""), cell("1")]];
        let text = build(&ExportDraft::default(), &t);
        assert!(text.contains("\"say \"\"hi\"\"\""), "{text}");
    }

    #[test]
    fn a_tab_delimiter_previews_as_a_tab() {
        let draft = ExportDraft {
            csv_delimiter: "\\t".into(),
            ..Default::default()
        };
        assert!(build(&draft, &target()).contains("region\tamount"));
    }

    #[test]
    fn json_previews_ndjson_with_types_from_the_schema() {
        let draft = ExportDraft {
            format: FormatId::Json,
            ..Default::default()
        };
        let text = build(&draft, &target());
        assert_eq!(
            text, "{\"region\":\"emea\",\"amount\":12.50}\n{\"region\":\"amer\",\"amount\":null}",
            "strings quoted, numbers bare, nulls null"
        );
    }

    #[test]
    fn parquet_previews_the_schema_and_the_settings_that_will_be_written() {
        let draft = ExportDraft {
            format: FormatId::Parquet,
            pq_codec: CodecChoice::Zstd,
            pq_level: 9,
            ..Default::default()
        };
        let text = build(&draft, &target());
        assert!(
            text.contains("optional BYTE_ARRAY (UTF8) region;"),
            "{text}"
        );
        assert!(text.contains("compression : zstd(9)"), "{text}");
        assert!(text.contains("row group   : 1,048,576 rows"), "{text}");
    }

    #[test]
    fn a_levelless_codec_previews_without_a_level() {
        let draft = ExportDraft {
            format: FormatId::Parquet,
            pq_codec: CodecChoice::Snappy,
            ..Default::default()
        };
        assert!(build(&draft, &target()).contains("compression : snappy\n"));
    }

    #[test]
    fn arrow_previews_the_schema_and_says_it_takes_no_options() {
        let draft = ExportDraft {
            format: FormatId::Arrow,
            ..Default::default()
        };
        let text = build(&draft, &target());
        assert!(text.contains("region: utf8"), "{text}");
        assert!(text.contains("(no write options)"), "{text}");
    }

    #[test]
    fn a_partitioned_preview_draws_the_tree_from_values_really_present() {
        let mut draft = ExportDraft::default();
        draft.partition.enabled = true;
        draft.partition.columns = vec!["region".into()];
        let text = build(&draft, &target());
        assert!(text.contains("region=amer/"), "{text}");
        assert!(text.contains("region=emea/"), "{text}");
        assert!(text.contains("part-0.csv"), "{text}");
    }

    #[test]
    fn the_partitioned_preview_admits_it_only_saw_one_page() {
        let mut draft = ExportDraft::default();
        draft.partition.enabled = true;
        draft.partition.columns = vec!["region".into()];
        let text = build(&draft, &target());
        assert!(
            text.contains("shape from the 2 rows loaded; the full export covers 48,213 rows"),
            "never claims the tree is complete: {text}"
        );
    }

    #[test]
    fn keeping_partition_columns_is_stated_in_the_preview() {
        let mut draft = ExportDraft::default();
        draft.partition.enabled = true;
        draft.partition.columns = vec!["region".into()];
        assert!(build(&draft, &target()).contains("partition columns in files: removed"));
        draft.partition.keep_columns = true;
        assert!(build(&draft, &target()).contains("partition columns in files: kept"));
    }

    #[test]
    fn a_selection_with_the_toggle_off_previews_the_flat_file() {
        let mut draft = ExportDraft::default();
        draft.partition.columns = vec!["region".into()];
        let text = build(&draft, &target());
        assert!(!text.contains("region=emea/"), "not a tree: {text}");
        assert!(text.starts_with("region,amount"), "{text}");
    }

    #[test]
    fn an_empty_result_says_so_rather_than_previewing_a_bare_header() {
        let mut t = target();
        t.sample = vec![];
        let draft = ExportDraft {
            format: FormatId::Json,
            ..Default::default()
        };
        assert_eq!(build(&draft, &t), "(no rows to preview)");
    }

    #[test]
    fn the_scope_choice_does_not_change_the_preview_shape() {
        let all = build(&ExportDraft::default(), &target());
        let page = build(
            &ExportDraft {
                scope: ScopeChoice::Page,
                ..Default::default()
            },
            &target(),
        );
        assert_eq!(all, page);
    }
}
