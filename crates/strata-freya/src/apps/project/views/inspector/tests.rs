//! Column-inspector tests (P3-08) — the rendered panel, driven by moving the selection the way
//! the catalog sidebar moves it.
//!
//! The derivations are unit-tested next door in [`super::model`]; these are about what actually
//! reaches the tree, and they lean on the same three-tier store the model tests use: a Parquet
//! table that carries footer statistics, a CSV table that carries none, and a view whose columns
//! are derived. **The honesty rules are the deliverable**, so each has a test that would fail if
//! the panel started inventing a row: no facts a CSV never reported, no completeness bar without
//! a real null count, and the null count never shown twice.

use std::path::PathBuf;

use freya::radio::RadioStation;
use freya_testing::TestingRunner;
use strata_core::engine::{TableMeta, ViewMeta};
use strata_core::project::ProjectDefs;
use strata_core::theme::load;
use strata_model::{ColRef, ColumnInfo, Kind, Stat, StatKey, TableDef, ViewDef};

use super::*;
use crate::components::ACTION_HEIGHT;
use crate::theme::strata_theme;

/// The panel's own width, from the design canvas (`inspectorW: 292`).
const PANEL_WIDTH: f32 = 292.;

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

/// `events` is Parquet with footer facts and a nested struct; `uploads` is CSV, which reports
/// nothing at all; `daily` is a view, whose columns are derived.
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
    let mut p = ProjectState::from_defs(defs, PathBuf::from("/tmp/strata-inspector-view-test"));
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

fn app() -> impl IntoElement {
    use_init_theme(|| strata_theme(&load("midnight")));
    rect().expanded().child(Inspector::new())
}

type Handles = (
    State<Option<ColRef>>,
    RadioStation<ProjectState, ProjChan>,
    RadioStation<SessionState, Chan>,
);

fn runner() -> (TestingRunner, Handles) {
    TestingRunner::new(
        app,
        (PANEL_WIDTH, 900.).into(),
        |r| {
            let selection = r.provide_root_context(|| State::create(None::<ColRef>));
            let project = r
                .provide_root_context(|| RadioStation::<ProjectState, ProjChan>::create(project()));
            let session = r.provide_root_context(|| {
                RadioStation::<SessionState, Chan>::create(SessionState::default())
            });
            (selection, project, session)
        },
        1.,
    )
}

/// Render, then let the effects those renders scheduled run. Freya polls tasks only once no
/// scope is dirty, so a single pass defers every effect in the tree to the next one.
fn settle(runner: &mut TestingRunner) {
    for _ in 0..4 {
        runner.sync_and_update();
    }
}

/// Select a column and settle — what a press on a catalog column row does.
fn select(runner: &mut TestingRunner, sel: &mut State<Option<ColRef>>, col: ColRef) {
    sel.set(Some(col));
    settle(runner);
}

fn column(kind: CatalogKind, owner: &str, path: &[&str]) -> ColRef {
    ColRef {
        kind,
        owner: owner.into(),
        path: path.iter().map(|s| (*s).to_string()).collect(),
    }
}

/// Every text run currently in the tree.
fn texts(runner: &TestingRunner) -> Vec<String> {
    runner.find_many(|_, element| Label::try_downcast(element).map(|l| l.text.to_string()))
}

fn shows(runner: &TestingRunner, text: &str) -> bool {
    texts(runner).iter().any(|t| t == text)
}

/// Click the centre of the first text run equal to `text`. Coordinates come from the laid-out
/// node, so these tests don't encode pixel offsets that any padding change would break.
fn click_text(runner: &mut TestingRunner, text: &str) {
    let area = runner
        .find(|node, element| {
            Label::try_downcast(element)
                .filter(|l| l.text == text)
                .map(|_| node.layout().area)
        })
        .unwrap_or_else(|| panic!("no text run {text:?} in the tree"));
    let point = (
        (area.min_x() + area.width() / 2.) as f64,
        (area.min_y() + area.height() / 2.) as f64,
    );
    runner.move_cursor(point);
    runner.click_cursor(point);
    settle(runner);
}

/// The **headline**: a Parquet column shows what its footer reported, labelled and in order,
/// with its source format named — and nothing beyond it.
#[test]
fn a_parquet_column_shows_its_footer_facts_and_its_source() {
    let (mut runner, (mut sel, ..)) = runner();
    settle(&mut runner);

    select(
        &mut runner,
        &mut sel,
        column(CatalogKind::Table, "events", &["amount"]),
    );

    for run in [
        "amount",
        "Float64",
        "PARQUET",
        "from events",
        "TYPE",
        "ROWS",
        "2,413,118",
        "MIN",
        "-240.0",
        "MAX",
        "4990.0",
    ] {
        assert!(shows(&runner, run), "{run:?} should be in the panel");
    }
    assert!(
        !shows(&runner, "DISTINCT") && !shows(&runner, "MEAN") && !shows(&runner, "MEDIAN"),
        "a footer carries none of those — P3-09's scan is what computes them: {:?}",
        texts(&runner)
    );
}

/// The null count is the **bar**, and only the bar. Rendering it as a row as well would be the
/// same number twice, which is what the single bar replaced.
#[test]
fn the_null_count_is_the_completeness_bar_and_never_a_row() {
    let (mut runner, (mut sel, ..)) = runner();
    settle(&mut runner);

    select(
        &mut runner,
        &mut sel,
        column(CatalogKind::Table, "events", &["amount"]),
    );

    assert!(shows(&runner, "Completeness"));
    assert!(shows(&runner, "94%"), "{:?}", texts(&runner));
    assert!(
        !shows(&runner, "NULLS") && !shows(&runner, "147,200"),
        "the count belongs to the bar alone: {:?}",
        texts(&runner)
    );
}

/// A source with no metadata gets no facts invented for it: one row, no bar. This is the case a
/// fixed grid of fields rendered as a column of blanks.
#[test]
fn a_csv_column_shows_no_facts_and_no_completeness_bar() {
    let (mut runner, (mut sel, ..)) = runner();
    settle(&mut runner);

    select(
        &mut runner,
        &mut sel,
        column(CatalogKind::Table, "uploads", &["note"]),
    );

    assert!(shows(&runner, "note") && shows(&runner, "CSV"));
    assert!(shows(&runner, "TYPE") && shows(&runner, "Utf8"));
    for absent in ["ROWS", "MIN", "MAX", "Completeness"] {
        assert!(
            !shows(&runner, absent),
            "{absent:?} was never reported: {:?}",
            texts(&runner)
        );
    }
}

/// A view's column says it is derived, and says why there is nothing under the type.
#[test]
fn a_view_column_is_marked_derived() {
    let (mut runner, (mut sel, ..)) = runner();
    settle(&mut runner);

    select(
        &mut runner,
        &mut sel,
        column(CatalogKind::View, "daily", &["day"]),
    );

    assert!(shows(&runner, "day") && shows(&runner, "VIEW") && shows(&runner, "from daily"));
    assert!(
        texts(&runner)
            .iter()
            .any(|t| t.contains("defined by the view's query")),
        "{:?}",
        texts(&runner)
    );
    assert!(
        !shows(&runner, "Completeness"),
        "no null count to draw with"
    );
}

/// A **nested field** is inspected by its whole path, and a nested *column* states its shape.
/// Both halves matter: the field is the gap the sidebar left, and the box is what makes a
/// struct worth selecting at all.
#[test]
fn nested_columns_state_their_shape_and_nested_fields_resolve_by_path() {
    let (mut runner, (mut sel, ..)) = runner();
    settle(&mut runner);

    select(
        &mut runner,
        &mut sel,
        column(CatalogKind::Table, "events", &["address"]),
    );
    assert!(shows(&runner, "NESTED FIELDS"));
    for field in ["city", "geo", "lat"] {
        assert!(shows(&runner, field), "the whole shape, at every depth");
    }

    select(
        &mut runner,
        &mut sel,
        column(CatalogKind::Table, "events", &["address", "city"]),
    );
    assert!(shows(&runner, "city"));
    assert!(
        !shows(&runner, "NESTED FIELDS"),
        "a leaf has no shape to state"
    );
    assert!(
        !shows(&runner, "-240.0"),
        "and it carries none of its sibling's facts: {:?}",
        texts(&runner)
    );
}

/// With nothing selected the panel says so — it is not blank, and it is not showing the last
/// column's facts.
#[test]
fn an_empty_selection_prompts_for_one() {
    let (mut runner, ..) = runner();
    settle(&mut runner);

    assert!(shows(&runner, "COLUMN INSPECTOR"), "the panel is mounted");
    assert!(shows(&runner, "Select a column to inspect."));
    assert!(!shows(&runner, "STATISTICS"));
}

/// The catalog moves under a live selection: dropping the table the panel is describing has to
/// say what happened, not silently keep the facts of a row that is gone.
#[test]
fn a_selection_whose_row_is_dropped_says_so() {
    let (mut runner, (mut sel, mut project, _)) = runner();
    settle(&mut runner);
    select(
        &mut runner,
        &mut sel,
        column(CatalogKind::Table, "events", &["amount"]),
    );
    assert!(shows(&runner, "2,413,118"));

    project
        .write_channel(ProjChan::Tables)
        .remove_table("events");
    settle(&mut runner);

    assert!(shows(&runner, "'events' is no longer in the catalog."));
    assert!(
        !shows(&runner, "2,413,118"),
        "the facts went with the row: {:?}",
        texts(&runner)
    );
}

/// A landing registration reaches the open panel: the facts follow the store, because the
/// panel listens on the section channel its selection belongs to.
#[test]
fn a_landing_registration_refreshes_the_open_panel() {
    let (mut runner, (mut sel, mut project, _)) = runner();
    settle(&mut runner);
    project.write_channel(ProjChan::Tables).reload_tables();
    select(
        &mut runner,
        &mut sel,
        column(CatalogKind::Table, "events", &["amount"]),
    );
    assert!(
        shows(&runner, "Loading…"),
        "no verdict yet: {:?}",
        texts(&runner)
    );

    project.write_channel(ProjChan::Tables).table_registered(
        "events",
        TableMeta {
            columns: vec![col(
                "amount",
                "Float64",
                Kind::Num,
                vec![stat(StatKey::Min, "0.0")],
            )],
            rows: Some(12),
        },
    );
    settle(&mut runner);

    assert!(shows(&runner, "MIN") && shows(&runner, "0.0"));
    assert!(
        shows(&runner, "12"),
        "the new row count: {:?}",
        texts(&runner)
    );
}

/// The scan card is **P3-09's**, so it is offered in full dress but does nothing: the zone keeps
/// the shape the canvas specifies, and the press has no handler behind it until the task that
/// owns profiling adds one. It is a committing action, so it wears the design system's 34px —
/// asserted because Freya's button layout hugs its label (≈28px) unless told otherwise, which
/// reads as squashed.
#[test]
fn the_profile_offer_is_present_and_inert() {
    let (mut runner, (mut sel, project, session)) = runner();
    settle(&mut runner);

    select(
        &mut runner,
        &mut sel,
        column(CatalogKind::Table, "events", &["amount"]),
    );

    assert!(shows(&runner, "Profile table"));
    let heights: Vec<f32> = runner.find_many(|node, element| {
        (element.accessibility().builder.role() == AccessibilityRole::Button)
            .then(|| node.layout().area.height())
    });
    assert!(
        heights.iter().any(|h| (h - ACTION_HEIGHT).abs() < 0.5),
        "the scan action is a {ACTION_HEIGHT}px action button: {heights:?}"
    );

    // Pressing it changes nothing — no tab opened, no catalog mutation, not even a repaint of the
    // panel. That is what "inert" has to mean now the control is no longer disabled: it looks
    // live, so the proof has to be that it *does* nothing rather than that it can't be pressed.
    let before = texts(&runner);
    let tabs_before = session.peek().tabs.len();
    click_text(&mut runner, "Profile table");

    assert_eq!(texts(&runner), before, "the panel is unchanged");
    assert_eq!(session.peek().tabs.len(), tabs_before, "no tab was opened");
    assert!(
        project
            .peek()
            .tables
            .iter()
            .all(|t| t.reg.ready().is_some()),
        "and nothing was asked of the catalog"
    );
}

/// **Nothing overflows the panel.** It is 292px wide and everything in it is long-form: a
/// truncated string bound as a Max, a struct field with a wordy dtype, a column name from
/// somebody's file. Each run has to take the slack and truncate; a run laid out past the edge
/// is invisible however correct the element tree is (the same regression the sidebar header
/// shipped with).
#[test]
fn nothing_in_the_panel_is_laid_out_past_its_edge() {
    let (mut runner, (mut sel, mut project, _)) = runner();
    settle(&mut runner);
    project.write_channel(ProjChan::Tables).table_registered(
        "events",
        TableMeta {
            columns: vec![col(
                "customer_shipping_address_line_one",
                "Timestamp",
                Kind::Str,
                vec![
                    stat(StatKey::Nulls, "1"),
                    stat(StatKey::Min, "Aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
                    stat(StatKey::Max, "Zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"),
                ],
            )],
            rows: Some(9),
        },
    );

    select(
        &mut runner,
        &mut sel,
        column(
            CatalogKind::Table,
            "events",
            &["customer_shipping_address_line_one"],
        ),
    );

    let overflowing: Vec<(f32, f32)> = runner.find_many(|node, _| {
        let a = node.layout().area;
        (a.width() > 0. && a.max_x() > PANEL_WIDTH + 0.5).then(|| (a.min_x(), a.max_x()))
    });
    assert!(
        overflowing.is_empty(),
        "laid out past the {PANEL_WIDTH}px panel edge: {overflowing:?}"
    );
}

/// Headless previews for eyeballing against the canvas's inspector — one per shape the panel
/// takes: a scalar column with footer facts, a nested column stating its shape, and a view's
/// derived column. Ignored by default (they write files and assert nothing):
/// `cargo test -p strata-freya inspector_preview -- --ignored`.
#[test]
#[ignore = "writes target/inspector-*.png for eyeballing; run explicitly"]
fn inspector_preview() {
    let (mut runner, (mut sel, ..)) = runner();
    settle(&mut runner);
    for (col, file) in [
        (
            column(CatalogKind::Table, "events", &["amount"]),
            concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/inspector.png"),
        ),
        (
            column(CatalogKind::Table, "events", &["address"]),
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../target/inspector-nested.png"
            ),
        ),
        (
            column(CatalogKind::View, "daily", &["day"]),
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../target/inspector-view.png"
            ),
        ),
    ] {
        select(&mut runner, &mut sel, col);
        runner.render_to_file(file);
    }
}
