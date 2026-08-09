//! What the inspector is describing, derived from the catalog store — **and nothing else**.
//!
//! The rule this module exists to hold (`DEV_TASKS` U9, "only real facts"): every number here
//! was *read* from the source, never computed from whatever rows happen to be on screen. The
//! Dioxus inspector once derived Rows / Nulls / Distinct / Min / Max from the current page of
//! the current tab's query and presented them as column facts; they described one page of one
//! query. They are gone, and the shape below is what replaced them — a fact exists or it
//! doesn't, and an absent fact is an absent row.
//!
//! Two tiers, one list. Free (footer) metadata is [`ColumnInfo::stats`], filled by the engine
//! from DataFusion `Statistics` (one metadata read per file, no data pages), plus
//! `TableMeta.rows`; every format but Parquet/Arrow reports nothing at all, and a view reports
//! nothing ever — which is why the box is a **dynamic list** rather than a grid of blanks. A
//! **scan** (P3-09) lands its facts in that same list through [`with_scan`], matched on
//! [`StatKey`], so no fact can appear twice and no absent fact becomes a blank.

use std::time::SystemTime;

use strata_core::engine::profile::CatalogProfile;
use strata_core::util::{ago, fmt_int};
use strata_model::{CatalogKind, ColRef, ColumnInfo, Kind, Stat, StatKey};

use crate::apps::project::query::ScanId;
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

/// What the inspected column's **source badge** says, and which tone it says it in — the
/// title's second badge.
///
/// Deliberately *not* [`strata_model::SourceFormat`], which is a table's reader and its options.
/// This is a display vocabulary: a closed set plus [`Other`](FormatBadge::Other), because the
/// badge is *coloured* per format and a theme can only name the ones it knows, plus `View` —
/// which is not a file format at all (a view has no files under it, the whole reason it carries
/// no free facts) and so could never be a variant of the reader enum.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum FormatBadge {
    Parquet,
    Csv,
    Json,
    Arrow,
    View,
    /// A format the app has no reader for — shown as written, in the recessive tone.
    Other(String),
}

impl FormatBadge {
    /// The badge for a table def's reader.
    fn of_table(format: &strata_model::SourceFormat) -> Self {
        use strata_model::SourceFormat as F;
        match format {
            F::Parquet => FormatBadge::Parquet,
            F::Csv(_) => FormatBadge::Csv,
            F::Json(_) => FormatBadge::Json,
            F::Arrow => FormatBadge::Arrow,
            F::Unknown(name) => FormatBadge::Other(name.clone()),
        }
    }

    /// The badge's text.
    pub fn label(&self) -> String {
        match self {
            FormatBadge::Parquet => "PARQUET".into(),
            FormatBadge::Csv => "CSV".into(),
            FormatBadge::Json => "JSON".into(),
            FormatBadge::Arrow => "ARROW".into(),
            FormatBadge::View => "VIEW".into(),
            FormatBadge::Other(f) => f.to_uppercase(),
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
    pub format: FormatBadge,
    /// A nested column's fields, at every depth (display only).
    pub children: Vec<ColumnInfo>,
    /// The owner's row count where the source reports one — `None` for CSV/JSON and for every
    /// view.
    pub rows: Option<u64>,
    /// The facts known about this column — the source's free ones, plus whatever a scan added
    /// ([`with_scan`]). Empty for a nested field before a scan (footers describe leaves and we
    /// don't traverse into them), for a view's columns, and for any format without metadata.
    pub stats: Vec<Stat>,
    /// Owned by a view: there are no files under it, so there is no footer tier at all.
    pub derived: bool,
    /// A **nested field** — a struct's child, not a top-level column whose type is a struct.
    /// The scan is keyed by top-level column name, so this is what stops `address.city`
    /// collecting an unrelated top-level `city`'s facts (see [`with_scan`]).
    pub child: bool,
    /// The scan the owner has been asked for, if any — the zone shows its card when there is
    /// none, and subscribes to it when there is (`ProjectState::profile_scan`).
    pub scan: Option<ScanId>,
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
    // Which scan the *owner* has been asked for — a column-level question answered at the entry,
    // because one scan covers every column at once.
    let scan = project.profile_scan(col.kind, &col.owner);
    match col.kind {
        CatalogKind::View => match project.views.iter().find(|v| v.def.name == col.owner) {
            None => Inspected::Gone(gone_owner(&col.owner)),
            Some(row) => match &row.reg {
                Reg::Loading => Inspected::Loading,
                Reg::Failed(e) => Inspected::Failed(e.clone()),
                Reg::Ready(info) => facts(col, &info.columns, FormatBadge::View, None, true, scan),
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
                    FormatBadge::of_table(&row.def.format),
                    meta.rows,
                    false,
                    scan,
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
    format: FormatBadge,
    rows: Option<u64>,
    derived: bool,
    scan: Option<ScanId>,
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
        child: col.is_child(),
        scan,
    }))
}

/// Fold a settled scan's facts for this column into the free ones (P3-09).
///
/// One list, so a fact can never appear twice and the display order stays [`FACT_ORDER`]'s.
/// Three rules, each with a reason:
///
/// - **The scan's row count wins**, and applies to a nested field too (a struct still holds one
///   value per row). Not merely a fallback for the sources that report none — every CSV and every
///   view — because the completeness bar *divides* the null count by it, and a bar built from a
///   footer numerator over a scanned denominator is two reads pretending to be one. The scan
///   counted both, in a single pass.
/// - **So `Nulls` follows `rows`.** It is the one key where free does *not* win a tie: the
///   scan's count is the one that pairs with the scan's row count. And where the scan described
///   the column but reported no null count at all, a free one is dropped rather than divided by a
///   denominator it never belonged to.
/// - **A nested field takes nothing else.** Only top-level columns are profiled and the profile
///   is keyed by their names, so a lookup by leaf name would hand `address.city` the facts of an
///   unrelated top-level `city`. Refused outright rather than guessed at.
/// - **Otherwise free wins a tie, unless the free value is a bound.** The footer's fact cost
///   nothing and is what the source itself said; but a Parquet footer truncates long strings
///   routinely, and showing `~Radia Perl` when the scan computed the whole value is exactly the
///   bound-as-fact this panel exists to avoid — so an *inexact* free stat yields to the computed
///   one.
pub fn with_scan(mut facts: ColumnFacts, profile: &CatalogProfile) -> ColumnFacts {
    facts.rows = Some(profile.rows);
    if facts.child {
        return facts;
    }
    let Some(scanned) = profile.cols.get(&facts.name) else {
        return facts;
    };
    for stat in scanned {
        match facts.stats.iter().position(|s| s.key == stat.key) {
            // `Nulls` and any free *bound* yield to the computed value — see above.
            Some(i) if stat.key == StatKey::Nulls || !facts.stats[i].exact => {
                facts.stats[i] = stat.clone();
            }
            Some(_) => {}
            None => facts.stats.push(stat.clone()),
        }
    }
    if !scanned.iter().any(|s| s.key == StatKey::Nulls) {
        facts.stats.retain(|s| s.key != StatKey::Nulls);
    }
    facts
}

/// What the scan says it covered — the zone's footnote under the facts.
pub fn scan_footnote(profile: &CatalogProfile) -> String {
    format!("Full scan · {} rows", fmt_int(profile.rows))
}

/// How long ago the scan settled, as the zone's header states it. The age itself is
/// [`util::ago`](strata_core::util::ago) — shared with the History drawer's timestamps, so the two
/// surfaces say "3 h ago" the same way. A clock that has gone backwards reads as fresh rather than
/// as a negative age.
pub fn scan_age(at: SystemTime) -> String {
    let secs = at.elapsed().map(|d| d.as_secs()).unwrap_or(0);
    format!("scanned {}", ago(secs))
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

impl ColumnFacts {
    /// Which catalog section owns this column — what a scan request is addressed to.
    ///
    /// `derived` is exactly "owned by a view" ([`inspect`] sets it for `CatalogKind::View` and
    /// nothing else), so there is no second field to keep in step. A saved query has no schema
    /// and can't be inspected at all.
    pub fn owner_kind(&self) -> CatalogKind {
        match self.derived {
            true => CatalogKind::View,
            false => CatalogKind::Table,
        }
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
///
/// A **distinct count is a count**, so it wears the thousands separators every other count in the
/// app wears — including the ROWS row directly above it, which would otherwise read `2,413,118`
/// over a bare `40312`. Min / Max / Mean / Median are *values*: reformatting one would be
/// rewriting the data, and a numeric-looking `Min` is not necessarily a number at all.
fn fact_value(stat: &Stat) -> String {
    let text = match stat.key {
        StatKey::Distinct => stat
            .text
            .parse::<u64>()
            .map(fmt_int)
            .unwrap_or_else(|_| stat.text.clone()),
        _ => stat.text.clone(),
    };
    if stat.exact {
        text
    } else {
        format!("~{text}")
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
    } else if !(10.0..99.5).contains(&pct) {
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
    use std::time::Duration;

    use datafusion::arrow::datatypes::{DataType, Field};
    use strata_core::engine::{column_info, TableMeta, ViewMeta};
    use strata_core::project::ProjectDefs;
    use strata_model::{TableDef, TableOrigin, ViewDef};

    use super::*;
    use strata_model::SourceFormat;

    /// As the inspector's own tests build one — through the engine's `column_info`, so a
    /// fixture's dtype, kind and chart role are one Arrow type's answers rather than three.
    fn col(name: &str, dtype: DataType, stats: Vec<Stat>) -> ColumnInfo {
        // Derive the row, then attach the facts — the order production uses (`free_stats` fills
        // `stats` on an already-derived column), rather than a struct update over a derived value.
        let mut column = column_info(&Field::new(name, dtype, true));
        column.stats = stats;
        column
    }

    /// A leaf column's Arrow field, for nesting inside a struct.
    fn field(name: &str, dtype: DataType) -> Field {
        Field::new(name, dtype, true)
    }

    /// A struct field over its own children, for nesting inside another struct.
    fn nested_field(name: &str, children: Vec<Field>) -> Field {
        Field::new(name, DataType::Struct(children.into()), true)
    }

    /// A struct column — the fixture builds the Arrow type and `column_info` derives the whole
    /// row from it, nested children included.
    fn nested(name: &str, children: Vec<Field>) -> ColumnInfo {
        column_info(&nested_field(name, children))
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
            format: SourceFormat::from_name(format),
            connection: None,
            sources: vec![format!("{name}.{format}")],
            partition_cols: Vec::new(),
            origin: TableOrigin::External,
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
            ..Default::default()
        };
        let mut p = ProjectState::from_defs(defs, PathBuf::from("/tmp/strata-inspector-test"));
        p.table_registered(
            "events",
            TableMeta {
                columns: vec![
                    col(
                        "amount",
                        DataType::Float64,
                        vec![
                            stat(StatKey::Nulls, "147200"),
                            stat(StatKey::Min, "-240.0"),
                            stat(StatKey::Max, "4990.0"),
                        ],
                    ),
                    nested(
                        "address",
                        vec![
                            field("city", DataType::Utf8),
                            nested_field("geo", vec![field("lat", DataType::Float64)]),
                        ],
                    ),
                ],
                rows: Some(2_413_118),
            },
        );
        p.table_registered(
            "uploads",
            TableMeta {
                columns: vec![col("note", DataType::Utf8, Vec::new())],
                rows: None,
            },
        );
        p.view_registered(
            "daily",
            ViewMeta {
                columns: vec![col("day", DataType::Date32, Vec::new())],
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

        assert_eq!(facts.format, FormatBadge::Parquet);
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

        assert_eq!(facts.format, FormatBadge::Csv);
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
        assert_eq!(facts.format, FormatBadge::View);
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
                    nested("address", vec![field("city", DataType::Utf8)]),
                    col(
                        "city",
                        DataType::Int64,
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
                    DataType::Utf8,
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

    // --- the scan tier (P3-09) ---------------------------------------------------------

    /// A settled scan of an entry: `rows` scanned, and facts per **top-level** column name.
    fn scan(rows: u64, cols: &[(&str, Vec<Stat>)]) -> CatalogProfile {
        CatalogProfile {
            at: SystemTime::now(),
            rows,
            sql: "SELECT 1".into(),
            cols: cols
                .iter()
                .map(|(name, stats)| ((*name).to_string(), stats.clone()))
                .collect(),
        }
    }

    /// The headline: a scan fills in exactly what the footer couldn't, in the same list, and
    /// nothing appears twice. The footer's own Min/Max stand — they cost nothing and they are
    /// what the source said — while Distinct / Mean / Median arrive from the scan.
    #[test]
    fn a_scan_fills_the_facts_the_footer_never_carried() {
        let p = project();
        let facts = column(&p, &sel(CatalogKind::Table, "events", &["amount"]));
        let scanned = with_scan(
            facts,
            &scan(
                2_413_118,
                &[(
                    "amount",
                    vec![
                        stat(StatKey::Nulls, "147200"),
                        stat(StatKey::Min, "-240.0"),
                        stat(StatKey::Max, "4990.0"),
                        stat(StatKey::Distinct, "40312"),
                        stat(StatKey::Mean, "812.4"),
                        stat(StatKey::Median, "640.0"),
                    ],
                )],
            ),
        );

        assert_eq!(
            fact_rows(&scanned)
                .into_iter()
                .map(|r| (r.label, r.value))
                .collect::<Vec<_>>(),
            vec![
                ("TYPE", "Float64".to_string()),
                ("ROWS", "2,413,118".to_string()),
                ("DISTINCT", "40,312".to_string()),
                ("MIN", "-240.0".to_string()),
                ("MAX", "4990.0".to_string()),
                ("MEAN", "812.4".to_string()),
                ("MEDIAN", "640.0".to_string()),
            ]
        );
        assert!(
            !fact_rows(&scanned).iter().any(|r| r.label == "NULLS"),
            "the scan's null count is still the bar, not a row as well"
        );
    }

    /// An **inexact** footer value is a bound, so a computed one replaces it. This is the one
    /// case where the free tier does not win: `~Radia Perl` beside a scan that knows the whole
    /// value is a bound shown as a fact.
    #[test]
    fn an_exact_scan_replaces_an_inexact_footer_bound() {
        let mut p = project();
        p.table_registered(
            "events",
            TableMeta {
                columns: vec![col(
                    "name",
                    DataType::Utf8,
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
        let scanned = with_scan(
            facts,
            &scan(9, &[("name", vec![stat(StatKey::Max, "Radia Perlman")])]),
        );

        assert_eq!(
            fact_rows(&scanned)
                .into_iter()
                .find(|r| r.label == "MAX")
                .map(|r| r.value),
            Some("Radia Perlman".to_string()),
            "no ~ — this is the value, not a bound on it"
        );
    }

    /// The case profiling exists for. A CSV reports **nothing**: no row count, no nulls, so no
    /// completeness bar. One scan answers both — and the bar it produces is the same honest
    /// derivation, off numbers that were counted rather than read.
    #[test]
    fn a_scan_answers_what_a_csv_could_never_report() {
        let p = project();
        let facts = column(&p, &sel(CatalogKind::Table, "uploads", &["note"]));
        assert_eq!(completeness(&facts), None, "nothing to divide by yet");

        let scanned = with_scan(
            facts,
            &scan(
                500,
                &[(
                    "note",
                    vec![stat(StatKey::Nulls, "100"), stat(StatKey::Distinct, "312")],
                )],
            ),
        );

        assert_eq!(
            fact_rows(&scanned)
                .into_iter()
                .map(|r| r.label)
                .collect::<Vec<_>>(),
            vec!["TYPE", "ROWS", "DISTINCT"]
        );
        let fill = completeness(&scanned).expect("a counted null count and a counted row count");
        assert_eq!((fill.nulls, fill.rows), (100, 500));
        assert_eq!(fill.label(), "80%");
    }

    /// **The identity bug the scan tier could reintroduce.** The profile is keyed by top-level
    /// column name, so `address.city` must refuse the lookup — by leaf name it would collect the
    /// facts of the unrelated top-level `city` sitting beside it. The entry's row count still
    /// applies: a struct holds one value per row like anything else.
    #[test]
    fn a_nested_field_takes_no_facts_from_a_scan() {
        let p = project();
        let facts = column(&p, &sel(CatalogKind::Table, "events", &["address", "city"]));
        let scanned = with_scan(
            facts,
            &scan(
                2_413_118,
                &[(
                    "city",
                    vec![stat(StatKey::Distinct, "999"), stat(StatKey::Min, "Aachen")],
                )],
            ),
        );

        assert_eq!(
            fact_rows(&scanned)
                .into_iter()
                .map(|r| r.label)
                .collect::<Vec<_>>(),
            vec!["TYPE", "ROWS"],
            "the row count, and not one fact of the top-level `city`"
        );
        assert!(scanned.child, "…because it is a nested field");
    }

    /// A top-level nested column is **not** a nested field, and the scan does describe it — its
    /// null count, which is the one fact a container can honestly report (no element traversal).
    /// So the completeness bar appears for a struct, off a counted number.
    ///
    /// It also pins the pairing rule: the scan's row count is what its null count is divided by,
    /// even though the Parquet footer reported a (stale, larger) one. Mixing the two reads put
    /// this bar at ">99.9%" when a quarter of the column is null. See
    /// [`the_bar_never_divides_one_read_by_another`] for the other half of that rule.
    #[test]
    fn a_top_level_struct_takes_the_null_count_a_scan_computed() {
        let p = project();
        let facts = column(&p, &sel(CatalogKind::Table, "events", &["address"]));
        assert!(!facts.child);
        assert_eq!(facts.rows, Some(2_413_118), "what the footer reported");

        let scanned = with_scan(
            facts,
            &scan(1_000, &[("address", vec![stat(StatKey::Nulls, "250")])]),
        );

        assert_eq!(scanned.rows, Some(1_000), "what the scan counted");
        let fill = completeness(&scanned).expect("counted nulls over counted rows");
        assert_eq!(fill.label(), "75%");
    }

    /// **The bar's two numbers come from one read.** A footer's null count is not divided by a
    /// scanned row count, however exact it is — the footer described the files as they were at
    /// registration, and the scan has just counted them as they are. So `Nulls` is the one key
    /// where the scan wins a tie, and where the scan described the column without counting nulls
    /// there is no bar at all rather than a ratio assembled from two reads.
    #[test]
    fn the_bar_never_divides_one_read_by_another() {
        let p = project();
        let facts = column(&p, &sel(CatalogKind::Table, "events", &["amount"]));
        assert_eq!(facts.rows, Some(2_413_118));
        assert_eq!(completeness(&facts).map(|f| f.nulls), Some(147_200));

        // The files were rewritten under the row: the scan counts a million rows and 100,000
        // nulls, while the footer still remembers 147,200 of 2,413,118.
        let scanned = with_scan(
            facts.clone(),
            &scan(
                1_000_000,
                &[("amount", vec![stat(StatKey::Nulls, "100000")])],
            ),
        );
        let fill = completeness(&scanned).expect("a bar off the scan's own pair");
        assert_eq!(
            (fill.nulls, fill.rows),
            (100_000, 1_000_000),
            "both numbers are the scan's — mixing them read 85%"
        );
        assert_eq!(fill.label(), "90%");

        // And a scan that describes the column without counting its nulls draws nothing: the
        // footer's count belongs to a row count this one has replaced.
        let no_nulls = with_scan(
            facts,
            &scan(1_000_000, &[("amount", vec![stat(StatKey::Distinct, "7")])]),
        );
        assert_eq!(completeness(&no_nulls), None);
        assert!(
            fact_rows(&no_nulls).iter().any(|r| r.label == "DISTINCT"),
            "the facts it did compute still land"
        );
    }

    /// The age is coarse on purpose — minutes or days is the question, and anything finer would
    /// have to tick to stay true.
    #[test]
    fn the_scan_age_reads_coarsely() {
        let ago = |secs: u64| scan_age(SystemTime::now() - Duration::from_secs(secs));
        assert_eq!(ago(0), "scanned just now");
        assert_eq!(ago(59), "scanned just now");
        assert_eq!(ago(60), "scanned 1 min ago");
        assert_eq!(ago(45 * 60), "scanned 45 min ago");
        assert_eq!(ago(3 * 3600), "scanned 3 h ago");
        assert_eq!(ago(50 * 3600), "scanned 2 d ago");
        // A clock that has gone backwards reads as fresh, never as a negative age.
        assert_eq!(
            scan_age(SystemTime::now() + Duration::from_secs(600)),
            "scanned just now"
        );
    }

    /// The footnote states what the scan covered, in the same grouped form every count wears.
    #[test]
    fn the_scan_footnote_states_the_rows_it_read() {
        assert_eq!(
            scan_footnote(&scan(2_413_118, &[])),
            "Full scan · 2,413,118 rows"
        );
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
