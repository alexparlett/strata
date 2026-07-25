//! What the inspector is describing, derived from the catalog store — **and nothing else**.
//!
//! The rule this module exists to hold (DEV_TASKS U9, "only real facts"): every number here
//! was *read* from the source, never computed from whatever rows happen to be on screen. The
//! Dioxus inspector once derived Rows / Nulls / Distinct / Min / Max from the current page of
//! the current tab's query and presented them as column facts; they described one page of one
//! query. They are gone, and the shape below is what replaced them — a fact exists or it
//! doesn't, and an absent fact is an absent row.
//!
//! Free (footer) metadata is all P3-08 has: [`ColumnInfo::stats`], filled by the engine from
//! DataFusion `Statistics` (one metadata read per file, no data pages), plus `TableMeta.rows`.
//! Every format but Parquet/Arrow reports nothing at all, and a view reports nothing ever —
//! which is why the box is a **dynamic list** rather than a grid of blanks. P3-09's scan lands
//! its facts in the same list, matched on [`StatKey`], so no fact can appear twice.

use strata_core::util::fmt_int;
use strata_model::{CatalogKind, ColRef, ColumnInfo, Kind, Stat, StatKey};

use crate::apps::project::state::{ProjectState, Reg};

/// Display order for the facts box.
///
/// **`Nulls` is deliberately absent**: it is the completeness bar. A row for it as well would
/// be the same number rendered twice, which is exactly the duplication the single bar replaced.
const FACT_ORDER: [StatKey; 5] = [
    StatKey::Distinct,
    StatKey::Min,
    StatKey::Max,
    StatKey::Mean,
    StatKey::Median,
];

/// The source format behind the inspected column — the title's second badge.
///
/// A closed set plus [`Other`](SourceFormat::Other), because the badge is *coloured* per format
/// and a theme can only name the ones it knows. `View` is not a file format: a view has no files
/// under it at all, which is the whole reason it carries no free facts.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SourceFormat {
    Parquet,
    Csv,
    Json,
    Arrow,
    View,
    /// A format string the app doesn't recognise — shown as written, in the recessive tone.
    Other(String),
}

impl SourceFormat {
    /// The format a table def names. Matched the way the engine matches it
    /// (`register_external`): lower case, and anything unknown is its own thing rather than
    /// silently one of ours.
    fn of_table(format: &str) -> Self {
        match format.to_ascii_lowercase().as_str() {
            "parquet" => SourceFormat::Parquet,
            "csv" => SourceFormat::Csv,
            "json" => SourceFormat::Json,
            "arrow" => SourceFormat::Arrow,
            _ => SourceFormat::Other(format.to_string()),
        }
    }

    /// The badge's text.
    pub fn label(&self) -> String {
        match self {
            SourceFormat::Parquet => "PARQUET".into(),
            SourceFormat::Csv => "CSV".into(),
            SourceFormat::Json => "JSON".into(),
            SourceFormat::Arrow => "ARROW".into(),
            SourceFormat::View => "VIEW".into(),
            SourceFormat::Other(f) => f.to_uppercase(),
        }
    }
}

/// Everything the inspector renders about one resolved column.
#[derive(Clone, PartialEq, Debug)]
pub struct ColumnFacts {
    /// The table or view it belongs to — the title's "from …".
    pub owner: String,
    /// The leaf's own name. The path is how it was found, not what it is called.
    pub name: String,
    pub dtype: String,
    pub kind: Kind,
    pub format: SourceFormat,
    /// A nested column's fields, at every depth (display only).
    pub children: Vec<ColumnInfo>,
    /// The owner's row count where the source reports one — `None` for CSV/JSON and for every
    /// view.
    pub rows: Option<u64>,
    /// The facts the source reported **for free**. Empty for a nested field (footers describe
    /// leaves and we don't traverse into them), for a view's columns, and for any format
    /// without metadata to read.
    pub stats: Vec<Stat>,
    /// Owned by a view: there are no files under it, so there is no footer tier at all.
    pub derived: bool,
}

/// What the inspector has to show, given the current selection.
///
/// The three non-column arms are the states a live selection can find itself in while the
/// catalog moves underneath it — a re-scan in flight, a table the engine refused, a row that
/// was dropped. Each says which, rather than falling back to the "nothing selected" prompt,
/// which would read as if the user had never picked anything.
#[derive(Clone, PartialEq, Debug)]
pub enum Inspected {
    Column(Box<ColumnFacts>),
    /// The owner has no landed registration answer yet (project open, or a re-scan).
    Loading,
    /// The engine refused the owner, so it has no columns to describe.
    Failed(String),
    /// The owner — or this column within it — is no longer there.
    Gone(String),
}

/// Resolve the selection against the catalog store.
///
/// One lookup, not two: [`ColRef::kind`] says which collection owns it. Tables and views share
/// a namespace, so searching both and hoping the name lands in one is how a view's column ends
/// up wearing a table's facts.
pub fn inspect(project: &ProjectState, col: &ColRef) -> Inspected {
    match col.kind {
        CatalogKind::View => match project.views.iter().find(|v| v.def.name == col.owner) {
            None => Inspected::Gone(gone_owner(&col.owner)),
            Some(row) => match &row.reg {
                Reg::Loading => Inspected::Loading,
                Reg::Failed(e) => Inspected::Failed(e.clone()),
                Reg::Ready(info) => facts(col, &info.columns, SourceFormat::View, None, true),
            },
        },
        // A saved query is a stored string, not a schema — nothing can select a column of one,
        // and the catalog never offers to. Treated as a table lookup, which finds nothing and
        // says so, rather than as an unreachable panic.
        _ => match project.tables.iter().find(|t| t.def.name == col.owner) {
            None => Inspected::Gone(gone_owner(&col.owner)),
            Some(row) => match &row.reg {
                Reg::Loading => Inspected::Loading,
                Reg::Failed(e) => Inspected::Failed(e.clone()),
                Reg::Ready(meta) => facts(
                    col,
                    &meta.columns,
                    SourceFormat::of_table(&row.def.format),
                    meta.rows,
                    false,
                ),
            },
        },
    }
}

fn gone_owner(owner: &str) -> String {
    format!("'{owner}' is no longer in the catalog.")
}

/// Walk the path into `columns` and build the facts, or report the column gone.
fn facts(
    col: &ColRef,
    columns: &[ColumnInfo],
    format: SourceFormat,
    rows: Option<u64>,
    derived: bool,
) -> Inspected {
    let Some(info) = resolve(columns, &col.path) else {
        return Inspected::Gone(format!(
            "'{}' is no longer a column of '{}'.",
            col.path.join("."),
            col.owner
        ));
    };
    Inspected::Column(Box::new(ColumnFacts {
        owner: col.owner.clone(),
        name: info.name.clone(),
        dtype: info.dtype.clone(),
        kind: info.kind,
        format,
        children: info.children.clone(),
        rows,
        stats: info.stats.clone(),
        derived,
    }))
}

/// Resolve a column path (`["address", "city"]`) by walking `children`.
///
/// Resolving only the first segment was the old bug: a nested `address.city` was looked up
/// among the top-level columns, so it either found nothing — a blank panel — or, far worse,
/// found an unrelated top-level `city` and showed *its* facts as this column's.
fn resolve<'a>(columns: &'a [ColumnInfo], path: &[String]) -> Option<&'a ColumnInfo> {
    let (first, rest) = path.split_first()?;
    let col = columns.iter().find(|c| &c.name == first)?;
    if rest.is_empty() {
        Some(col)
    } else {
        resolve(&col.children, rest)
    }
}

/// One row of the facts box: an uppercase key and the value beside it.
#[derive(Clone, PartialEq, Debug)]
pub struct FactRow {
    pub label: &'static str,
    pub value: String,
}

/// The facts box's rows, in [`FACT_ORDER`] after the two that are always known.
///
/// **Type is always there; everything else appears only where it exists.** That is the entire
/// point of a dynamic list: a CSV column shows one row, a Parquet column shows four, and
/// neither shows a blank.
///
/// `Rows` is the owner's row count, not the column's — it is the same number for every column
/// of the table, including a nested field, because a struct or a list still holds exactly one
/// value per row. It is shown wherever the source reports it.
pub fn fact_rows(facts: &ColumnFacts) -> Vec<FactRow> {
    let mut rows = vec![FactRow {
        label: "TYPE",
        value: facts.dtype.clone(),
    }];
    if let Some(n) = facts.rows {
        rows.push(FactRow {
            label: "ROWS",
            value: fmt_int(n),
        });
    }
    rows.extend(FACT_ORDER.iter().filter_map(|key| {
        facts.stats.iter().find(|s| s.key == *key).map(|s| FactRow {
            label: fact_label(*key),
            value: fact_value(s),
        })
    }));
    rows
}

fn fact_label(key: StatKey) -> &'static str {
    match key {
        StatKey::Nulls => "NULLS",
        StatKey::Min => "MIN",
        StatKey::Max => "MAX",
        StatKey::Distinct => "DISTINCT",
        StatKey::Mean => "MEAN",
        StatKey::Median => "MEDIAN",
    }
}

/// A fact's value. An **inexact** one is marked `~`: a Parquet footer truncates long strings
/// and binary routinely, so what it stored is a bound rather than the value, and showing it
/// bare would be exactly the fabrication this panel exists to avoid.
fn fact_value(stat: &Stat) -> String {
    if stat.exact {
        stat.text.clone()
    } else {
        format!("~{}", stat.text)
    }
}

/// The completeness bar's numbers — the share of rows that carry a value.
#[derive(Clone, PartialEq, Debug)]
pub struct Completeness {
    /// The filled share, `0.0..=1.0`.
    pub filled: f64,
    pub nulls: u64,
    pub rows: u64,
}

impl Completeness {
    /// The percentage as the bar labels it.
    pub fn label(&self) -> String {
        fill_label(self.filled)
    }

    /// What the bar means, in full — the tooltip, because a bar with no numbers on it is a
    /// shape rather than a fact.
    pub fn detail(&self) -> String {
        format!(
            "{} of {} rows are null. The bar shows the share with a value.",
            fmt_int(self.nulls),
            fmt_int(self.rows)
        )
    }
}

/// The completeness bar, **only** when a real null count exists.
///
/// It needs a null count from somewhere honest and a row count to divide by. Without both
/// there is nothing to draw, so nothing is drawn — it is never computed off the result page,
/// which is what it used to be.
///
/// Three refusals, each for its own reason:
///
/// - **no `Nulls` fact** — the source reported none. Note that the engine already drops a
///   `null_count == num_rows` before it ever gets here: DataFusion reports that both for an
///   all-null column *and* for one it has no statistics for, so the two are indistinguishable
///   and a bar would be a coin flip. P3-09's scan answers it for real with a `COUNT … FILTER`.
/// - **an inexact count** — a bound, not a number. Every free null count is exact today, so
///   this costs nothing and stops a future inexact one from being drawn as if it were precise.
/// - **more nulls than rows** — not a state either tier can produce; refusing beats rendering
///   a bar past its own track.
pub fn completeness(facts: &ColumnFacts) -> Option<Completeness> {
    let rows = facts.rows.filter(|n| *n > 0)?;
    let stat = facts.stats.iter().find(|s| s.key == StatKey::Nulls)?;
    if !stat.exact {
        return None;
    }
    let nulls = stat.text.parse::<u64>().ok().filter(|n| *n <= rows)?;
    Some(Completeness {
        filled: 1.0 - nulls as f64 / rows as f64,
        nulls,
        rows,
    })
}

/// The filled share as a percentage, rounded so it can never claim what it isn't.
///
/// The guards are the whole reason this isn't a plain `{:.0}` format: **"100%" on a column that
/// has nulls is a lie the eye can't check**, and `{:.0}` reaches it from anything at or above
/// 99.5. So the top of the range is handled in two steps — past 99.9% there is no number left to
/// state, and the 99.5..=99.9 band keeps a decimal rather than rounding into the claim. The
/// bottom mirrors it: a column with values never reads "0%". Below 10% the decimal stays because
/// a whole percent there is a tenth of the value.
fn fill_label(filled: f64) -> String {
    let pct = filled * 100.0;
    if pct >= 100.0 {
        "100%".to_string()
    } else if pct > 99.9 {
        ">99.9%".to_string()
    } else if pct <= 0.0 {
        "0%".to_string()
    } else if pct < 0.05 {
        "<0.1%".to_string()
    } else if pct < 10.0 || pct >= 99.5 {
        // Under 10%, and in the band `{:.0}` would round to 100.
        format!("{pct:.1}%")
    } else {
        format!("{pct:.0}%")
    }
}

/// One row of the NESTED FIELDS box, flattened out of the column's type.
#[derive(Clone, PartialEq, Debug)]
pub struct NestedField {
    pub name: String,
    pub dtype: String,
    pub kind: Kind,
    /// Nesting depth below the inspected column, driving the row's indent.
    pub depth: usize,
}

/// A nested column's fields, depth-first and fully expanded.
///
/// Display only — unlike the sidebar's tree there is nothing to collapse here, because the box
/// exists to state the *shape* of the type. Profiling never descends into it either (P3-09).
pub fn nested_fields(children: &[ColumnInfo]) -> Vec<NestedField> {
    let mut out = Vec::new();
    walk(children, 0, &mut out);
    out
}

fn walk(children: &[ColumnInfo], depth: usize, out: &mut Vec<NestedField>) {
    for c in children {
        out.push(NestedField {
            name: c.name.clone(),
            dtype: c.dtype.clone(),
            kind: c.kind,
            depth,
        });
        walk(&c.children, depth + 1, out);
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use strata_core::engine::{TableMeta, ViewMeta};
    use strata_core::project::ProjectDefs;
    use strata_model::{TableDef, ViewDef};

    use super::*;

    fn col(name: &str, dtype: &str, kind: Kind, stats: Vec<Stat>) -> ColumnInfo {
        ColumnInfo {
            name: name.into(),
            dtype: dtype.into(),
            kind,
            nullable: true,
            children: Vec::new(),
            stats,
        }
    }

    fn nested(name: &str, children: Vec<ColumnInfo>) -> ColumnInfo {
        ColumnInfo {
            name: name.into(),
            dtype: "Struct".into(),
            kind: Kind::Struct,
            nullable: true,
            children,
            stats: Vec::new(),
        }
    }

    fn stat(key: StatKey, text: &str) -> Stat {
        Stat {
            key,
            text: text.into(),
            exact: true,
        }
    }

    fn table(name: &str, format: &str) -> TableDef {
        TableDef {
            name: name.into(),
            format: format.into(),
            sources: vec![format!("{name}.{format}")],
            partition_cols: Vec::new(),
        }
    }

    /// A catalog with one Parquet table carrying footer stats and a nested struct, one CSV
    /// table carrying none, and one view. The three tiers of "what a source knows", in one
    /// store.
    fn project() -> ProjectState {
        let defs = ProjectDefs {
            name: "test".into(),
            tables: vec![table("events", "parquet"), table("uploads", "csv")],
            views: vec![ViewDef {
                name: "daily".into(),
                sql: "SELECT 1".into(),
            }],
            saved_queries: Vec::new(),
        };
        let mut p = ProjectState::from_defs(defs, PathBuf::from("/tmp/strata-inspector-test"));
        p.table_registered(
            "events",
            TableMeta {
                columns: vec![
                    col(
                        "amount",
                        "Float64",
                        Kind::Num,
                        vec![
                            stat(StatKey::Nulls, "147200"),
                            stat(StatKey::Min, "-240.0"),
                            stat(StatKey::Max, "4990.0"),
                        ],
                    ),
                    nested(
                        "address",
                        vec![
                            col("city", "Utf8", Kind::Str, Vec::new()),
                            nested("geo", vec![col("lat", "Float64", Kind::Num, Vec::new())]),
                        ],
                    ),
                ],
                rows: Some(2_413_118),
            },
        );
        p.table_registered(
            "uploads",
            TableMeta {
                columns: vec![col("note", "Utf8", Kind::Str, Vec::new())],
                rows: None,
            },
        );
        p.view_registered(
            "daily",
            ViewMeta {
                columns: vec![col("day", "Date32", Kind::Ts, Vec::new())],
                tables: vec!["events".into()],
                aliases: Vec::new(),
            },
        );
        p
    }

    fn sel(kind: CatalogKind, owner: &str, path: &[&str]) -> ColRef {
        ColRef {
            kind,
            owner: owner.into(),
            path: path.iter().map(|s| (*s).to_string()).collect(),
        }
    }

    fn column(project: &ProjectState, col: &ColRef) -> ColumnFacts {
        match inspect(project, col) {
            Inspected::Column(facts) => *facts,
            other => panic!("expected a resolved column, got {other:?}"),
        }
    }

    /// The headline: a Parquet column shows what the footer actually reported, and the value
    /// rows are exactly those facts — nothing invented to fill the box out.
    #[test]
    fn a_parquet_column_shows_the_footer_facts_and_only_those() {
        let p = project();
        let facts = column(&p, &sel(CatalogKind::Table, "events", &["amount"]));

        assert_eq!(facts.format, SourceFormat::Parquet);
        assert_eq!(
            fact_rows(&facts)
                .into_iter()
                .map(|r| (r.label, r.value))
                .collect::<Vec<_>>(),
            vec![
                ("TYPE", "Float64".to_string()),
                ("ROWS", "2,413,118".to_string()),
                ("MIN", "-240.0".to_string()),
                ("MAX", "4990.0".to_string()),
            ],
            "no DISTINCT / MEAN / MEDIAN row: a footer doesn't carry them, and P3-09's scan \
             is what fills them in"
        );
    }

    /// The nulls the footer reported are the **bar**, never a row as well. One number, one
    /// rendering.
    #[test]
    fn nulls_are_the_bar_and_never_a_fact_row() {
        let p = project();
        let facts = column(&p, &sel(CatalogKind::Table, "events", &["amount"]));

        assert!(
            !fact_rows(&facts).iter().any(|r| r.label == "NULLS"),
            "the null count belongs to the completeness bar alone"
        );
        let fill = completeness(&facts).expect("a real null count and a real row count");
        assert_eq!(fill.nulls, 147_200);
        assert_eq!(fill.rows, 2_413_118);
        assert_eq!(fill.label(), "94%");
        assert_eq!(
            fill.detail(),
            "147,200 of 2,413,118 rows are null. The bar shows the share with a value."
        );
    }

    /// A CSV column: the source reports nothing at all, so the box is one row and there is no
    /// bar. This is the case a fixed grid of fields would have rendered as five blanks.
    #[test]
    fn a_source_with_no_metadata_shows_the_type_and_nothing_else() {
        let p = project();
        let facts = column(&p, &sel(CatalogKind::Table, "uploads", &["note"]));

        assert_eq!(facts.format, SourceFormat::Csv);
        assert_eq!(
            fact_rows(&facts)
                .into_iter()
                .map(|r| r.label)
                .collect::<Vec<_>>(),
            vec!["TYPE"],
            "no row count either — the CSV never reported one"
        );
        assert_eq!(completeness(&facts), None);
    }

    /// A view's column is derived: no files underneath it, so no footer tier at all. The
    /// format badge says so rather than borrowing the base table's.
    #[test]
    fn a_view_column_is_derived_and_carries_no_free_facts() {
        let p = project();
        let facts = column(&p, &sel(CatalogKind::View, "daily", &["day"]));

        assert!(facts.derived);
        assert_eq!(facts.format, SourceFormat::View);
        assert_eq!(facts.format.label(), "VIEW");
        assert_eq!(
            fact_rows(&facts)
                .into_iter()
                .map(|r| r.label)
                .collect::<Vec<_>>(),
            vec!["TYPE"]
        );
    }

    /// **The identity bug this shape exists to prevent.** A nested field resolves by its whole
    /// path, so `address.city` is that column — not the top-level `city` that would have been
    /// found by leaf name, wearing facts that belong to something else.
    #[test]
    fn a_nested_field_resolves_by_its_whole_path() {
        let mut p = project();
        p.table_registered(
            "events",
            TableMeta {
                columns: vec![
                    nested("address", vec![col("city", "Utf8", Kind::Str, Vec::new())]),
                    col(
                        "city",
                        "Int64",
                        Kind::Num,
                        vec![stat(StatKey::Min, "1"), stat(StatKey::Max, "9")],
                    ),
                ],
                rows: Some(10),
            },
        );

        let nested_city = column(&p, &sel(CatalogKind::Table, "events", &["address", "city"]));
        assert_eq!(nested_city.dtype, "Utf8", "the field inside the struct");
        assert!(
            nested_city.stats.is_empty(),
            "and it carries none of the top-level column's facts"
        );

        let top_city = column(&p, &sel(CatalogKind::Table, "events", &["city"]));
        assert_eq!(top_city.dtype, "Int64");
    }

    /// The nested box states the whole shape of the type, at every depth.
    #[test]
    fn nested_fields_flatten_depth_first_with_their_depth() {
        let p = project();
        let facts = column(&p, &sel(CatalogKind::Table, "events", &["address"]));

        assert_eq!(
            nested_fields(&facts.children)
                .into_iter()
                .map(|f| (f.name, f.depth))
                .collect::<Vec<_>>(),
            vec![
                ("city".to_string(), 0),
                ("geo".to_string(), 0),
                ("lat".to_string(), 1),
            ]
        );
    }

    /// The states a live selection finds itself in while the catalog moves under it. Each is
    /// its own answer: mid-re-scan is not "gone", and a refused table is not "still loading".
    #[test]
    fn a_selection_reports_what_happened_to_its_row() {
        let mut p = project();
        assert!(matches!(
            inspect(&p, &sel(CatalogKind::Table, "nope", &["x"])),
            Inspected::Gone(m) if m == "'nope' is no longer in the catalog."
        ));
        assert!(
            matches!(
                inspect(&p, &sel(CatalogKind::Table, "events", &["gone"])),
                Inspected::Gone(m) if m == "'gone' is no longer a column of 'events'."
            ),
            "the row is there; the column the schema used to have is not"
        );

        p.reload_tables();
        assert!(matches!(
            inspect(&p, &sel(CatalogKind::Table, "events", &["amount"])),
            Inspected::Loading
        ));

        p.table_failed("events", "No such file or directory (os error 2)".into());
        assert!(matches!(
            inspect(&p, &sel(CatalogKind::Table, "events", &["amount"])),
            Inspected::Failed(e) if e == "No such file or directory (os error 2)"
        ));
    }

    /// An **inexact** fact is a bound, so it is marked. A Parquet footer truncates long strings
    /// routinely, and an unmarked bound reads as the value.
    #[test]
    fn an_inexact_fact_is_marked_as_a_bound() {
        let mut p = project();
        p.table_registered(
            "events",
            TableMeta {
                columns: vec![col(
                    "name",
                    "Utf8",
                    Kind::Str,
                    vec![Stat {
                        key: StatKey::Max,
                        text: "Radia Perl".into(),
                        exact: false,
                    }],
                )],
                rows: Some(9),
            },
        );

        let facts = column(&p, &sel(CatalogKind::Table, "events", &["name"]));
        assert_eq!(
            fact_rows(&facts)
                .into_iter()
                .find(|r| r.label == "MAX")
                .map(|r| r.value),
            Some("~Radia Perl".to_string())
        );
    }

    /// The bar refuses anything it can't state honestly: an inexact count is a bound rather
    /// than a number, and a count past the row count is not a state either tier can produce.
    #[test]
    fn the_bar_refuses_a_count_it_cannot_state() {
        let mut facts = column(&project(), &sel(CatalogKind::Table, "events", &["amount"]));
        assert!(
            completeness(&facts).is_some(),
            "the honest case still draws"
        );

        facts.stats = vec![Stat {
            key: StatKey::Nulls,
            text: "147200".into(),
            exact: false,
        }];
        assert_eq!(completeness(&facts), None, "a bound is not a null count");

        facts.stats = vec![stat(StatKey::Nulls, "3000000")];
        assert_eq!(completeness(&facts), None, "more nulls than rows");

        facts.stats = vec![stat(StatKey::Nulls, "0")];
        facts.rows = None;
        assert_eq!(completeness(&facts), None, "nothing to divide by");
    }

    /// The percentage never rounds into a claim it can't make. One null in millions is not
    /// "100%", and one value in millions is not "0%" — those two roundings are the difference
    /// between a summary and a lie.
    #[test]
    fn the_percentage_never_rounds_into_a_claim() {
        assert_eq!(fill_label(1.0), "100%");
        assert_eq!(fill_label(0.0), "0%");
        assert_eq!(
            fill_label(1.0 - 1.0 / 2_413_118.0),
            ">99.9%",
            "a column with nulls can never read 100%"
        );
        // **The band a plain `{:.0}` rounds straight into "100%".** One null in 500 rows is
        // 99.8% full, and saying "100%" there is the panel's whole honesty rule broken by a
        // format specifier — the `>99.9%` guard alone does not cover it.
        for (nulls, rows, expected) in [
            (1.0, 1000.0, "99.9%"),
            (1.0, 500.0, "99.8%"),
            (1.0, 250.0, "99.6%"),
            (1.0, 200.0, "99.5%"),
            (6.0, 1000.0, "99%"),
        ] {
            assert_eq!(
                fill_label(1.0 - nulls / rows),
                expected,
                "{nulls} null of {rows} rows"
            );
        }
        assert_eq!(
            fill_label(1.0 / 2_413_118.0),
            "<0.1%",
            "a column with values can never read 0%"
        );
        assert_eq!(
            fill_label(0.031),
            "3.1%",
            "under 10% a whole percent is a third of the value, so it keeps a decimal"
        );
        assert_eq!(
            fill_label(0.939),
            "94%",
            "above it, whole percents read better"
        );
        assert_eq!(fill_label(0.5), "50%");
    }
}
