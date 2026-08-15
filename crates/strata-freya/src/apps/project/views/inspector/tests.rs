//! Column-inspector tests (P3-08) — the rendered panel, driven by moving the selection the way
//! the catalog sidebar moves it.
//!
//! The derivations are unit-tested next door in [`super::model`]; these are about what actually
//! reaches the tree, and they lean on the same three-tier store the model tests use: a Parquet
//! table that carries footer statistics, a CSV table that carries none, and a view whose columns
//! are derived. **The honesty rules are the deliverable**, so each has a test that would fail if
//! the panel started inventing a row: no facts a CSV never reported, no completeness bar without
//! a real null count, and the null count never shown twice.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use datafusion::arrow::datatypes::{DataType, Field, TimeUnit};
use freya::radio::RadioStation;
use freya_testing::TestingRunner;
use futures::executor::block_on;
use strata_core::engine::{column_info, TableMeta, TableSpec, ViewMeta};
use strata_core::project::ProjectDefs;
use strata_core::theme::load;
use strata_model::{
    ColRef, ColumnInfo, RemoteRef, SourceFormat, Stat, StatKey, TableDef, TableOrigin, ViewDef,
};

use super::*;
use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::query::{ProfileTarget, ScanId};
use crate::apps::project::state::CatalogState;
use crate::components::metrics::{ACTION_HEIGHT, PANE_BODY_MIN_W};
use crate::theme::strata_theme;

/// The panel's own width, from the design canvas (`inspectorW: 292`).
const PANEL_WIDTH: f32 = 292.;

/// A leaf column carrying the facts a source reported for free. Built through the engine's
/// own `column_info`, so the fixture's dtype spelling, display kind and chart role all come
/// from one Arrow type instead of being stated separately and drifting.
fn col(name: &str, dtype: DataType, stats: Vec<Stat>) -> ColumnInfo {
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

/// The one table in these tests the engine can really scan — the `regions.csv` fixture, two `Utf8`
/// columns and five rows. Its def's `sources` is what `SCAN_TABLE` registers, so the store row and
/// the engine describe the same file.
const SCAN_TABLE: &str = "regions";
const SCAN_FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../strata-core/tests/fixtures/loadfix/regions.csv"
);

/// `events` is Parquet with footer facts and a nested struct; `uploads` is CSV, which reports
/// nothing at all; `daily` is a view, whose columns are derived; `regions` is the CSV the engine
/// actually holds, for the one test that runs a real scan.
fn project() -> ProjectState {
    let defs = ProjectDefs {
        name: "test".into(),
        tables: vec![
            table("events", "parquet"),
            table(SCAN_TABLE, "csv"),
            table("uploads", "csv"),
        ],
        views: vec![ViewDef {
            name: "daily".into(),
            sql: "SELECT 1".into(),
        }],
        saved_queries: Vec::new(),
        ..Default::default()
    };
    let mut p = ProjectState::from_defs(defs, PathBuf::from("/tmp/strata-inspector-view-test"));
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
    p.table_registered(
        SCAN_TABLE,
        TableMeta {
            columns: vec![
                col("country", DataType::Utf8, Vec::new()),
                col("region", DataType::Utf8, Vec::new()),
            ],
            rows: None,
        },
    );
    p.view_registered(
        "daily",
        ViewMeta {
            columns: vec![col("day", DataType::Date32, Vec::new())],
            tables: vec!["events".into()],
            remote: Vec::new(),
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
    State<Option<ProfileTarget>>,
);

fn runner() -> (TestingRunner, Handles) {
    runner_at(PANEL_WIDTH)
}

/// The panel at an arbitrary width, so P5-06's squeeze can be laid out and measured. Every other
/// test wants the canvas width, which is what plain [`runner`] gives.
fn runner_at(width: f32) -> (TestingRunner, Handles) {
    TestingRunner::new(
        app,
        (width, 900.).into(),
        |r| {
            let selection = r.provide_root_context(|| State::create(None::<ColRef>));
            let project = r
                .provide_root_context(|| RadioStation::<ProjectState, ProjChan>::create(project()));
            let session = r.provide_root_context(|| {
                RadioStation::<SessionState, Chan>::create(SessionState::default())
            });
            r.provide_root_context(|| {
                let engine = EngineCtx::default();
                block_on(engine.register(TableSpec {
                    name: SCAN_TABLE.into(),
                    paths: vec![SCAN_FIXTURE.into()],
                    format: SourceFormat::from_name("csv"),
                    partitions: Vec::new(),
                    internal: false,
                }))
                .expect("the fixture registers");
                engine
            });
            let profile_target = r.provide_root_context(|| State::create(None::<ProfileTarget>));
            r.provide_root_context(|| State::create(CatalogState::Settled(1)));
            r.provide_root_context(|| State::create(BTreeMap::<RemoteRef, ScanId>::new()));
            (selection, project, session, profile_target)
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
    ColRef::entry(kind, owner, path.iter().map(|s| (*s).to_string()).collect())
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
    let (mut runner, (mut sel, mut project, ..)) = runner();
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
    let (mut runner, (mut sel, mut project, ..)) = runner();
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
                DataType::Float64,
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

/// The scan card offers the scan and **asks before running it** (P3-09 → P3-10): the press fills
/// the confirm slot rather than starting a full read of the user's data. It is a committing
/// action, so it wears the design system's 34px — asserted because Freya's button layout hugs its
/// label (≈28px) unless told otherwise, which reads as squashed.
#[test]
fn the_scan_card_asks_the_cost_confirm_rather_than_scanning() {
    let (mut runner, (mut sel, project, _, target)) = runner();
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

    click_text(&mut runner, "Profile table");

    assert_eq!(
        target.peek().as_ref().map(ProfileTarget::label),
        Some("events".to_string()),
        "the press asks the question; confirming it is what records the request"
    );
    assert_eq!(
        project.peek().profile_scan(CatalogKind::Table, "events"),
        None,
        "nothing is scanned until the confirm says so"
    );
}

/// **The whole round trip, against a real engine.** A CSV reports nothing at all, so every number
/// here was computed by the scan: the distinct count a footer can never carry, a row count the
/// source never gave, and — off that same pass — the completeness bar. The zone also grows its
/// header controls once a scan exists, because the card has nothing left to offer.
///
/// The only test that scans, and the only one that renders `ScannedStatistics`' settled arm.
#[test]
fn a_settled_scan_shows_what_it_computed_and_when() {
    let (mut runner, (mut sel, mut project, ..)) = runner();
    settle(&mut runner);
    select(
        &mut runner,
        &mut sel,
        column(CatalogKind::Table, SCAN_TABLE, &["region"]),
    );
    assert!(shows(&runner, "Profile table"), "the offer, before the ask");

    project
        .write_channel(ProjChan::Tables)
        .request_profile(CatalogKind::Table, SCAN_TABLE);
    runner.poll(Duration::from_millis(10), Duration::from_millis(2_000));

    assert!(
        shows(&runner, "DISTINCT") && shows(&runner, "2"),
        "the fact a CSV can never report for free: {:?}",
        texts(&runner)
    );
    assert!(
        shows(&runner, "ROWS") && shows(&runner, "5"),
        "…and the row count it never gave either"
    );
    assert!(
        shows(&runner, "Completeness") && shows(&runner, "100%"),
        "the bar, off a counted null count over a counted row count: {:?}",
        texts(&runner)
    );
    assert!(shows(&runner, "Full scan · 5 rows"), "what it read");
    assert!(shows(&runner, "scanned just now"), "and how old that is");
    assert!(
        !shows(&runner, "Profile table"),
        "the card is gone: its controls moved to the zone header"
    );
    let past = past_the_edge(&runner);
    assert!(
        past.is_empty(),
        "the scanned zone lays out past the {PANEL_WIDTH}px panel edge: {past:?}"
    );
}

/// A **view's** card offers to scan the view, and says what that costs there — its whole query,
/// not a file read. A view is also where a scan is worth most: it reports nothing for free.
#[test]
fn a_view_column_offers_to_scan_the_view() {
    let (mut runner, (mut sel, _, _, target)) = runner();
    settle(&mut runner);

    select(
        &mut runner,
        &mut sel,
        column(CatalogKind::View, "daily", &["day"]),
    );

    assert!(shows(&runner, "Profile view"));
    assert!(
        texts(&runner)
            .iter()
            .any(|t| t.contains("Running the view's query in full")),
        "{:?}",
        texts(&runner)
    );

    click_text(&mut runner, "Profile view");
    assert_eq!(
        target.peek().as_ref().map(|t| (t.kind(), t.label())),
        Some((CatalogKind::View, "daily".to_string())),
        "asked about the view, on the views channel"
    );
}

/// **Nothing overflows the panel.** It is 292px wide and everything in it is long-form: a
/// truncated string bound as a Max, a struct field with a wordy dtype, a column name from
/// somebody's file. Each run has to take the slack and truncate; a run laid out past the edge
/// is invisible however correct the element tree is (the same regression the sidebar header
/// shipped with).
#[test]
fn nothing_in_the_panel_is_laid_out_past_its_edge() {
    let (mut runner, (mut sel, mut project, ..)) = runner();
    settle(&mut runner);
    project.write_channel(ProjChan::Tables).table_registered(
        "events",
        TableMeta {
            columns: vec![col(
                "customer_shipping_address_line_one",
                DataType::Timestamp(TimeUnit::Millisecond, None),
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

    let past = past_the_edge(&runner);
    assert!(
        past.is_empty(),
        "laid out past the {PANEL_WIDTH}px panel edge: {past:?}"
    );
}

/// Every box laid out past the panel's right edge, as `(min_x, max_x)` — empty is the only
/// acceptable answer. A run off the edge is invisible however correct the element tree is.
fn past_the_edge(runner: &TestingRunner) -> Vec<(f32, f32)> {
    past_the_edge_at(runner, PANEL_WIDTH)
}

/// As [`past_the_edge`], for a panel laid out at some other width.
fn past_the_edge_at(runner: &TestingRunner, width: f32) -> Vec<(f32, f32)> {
    runner.find_many(|node, _| {
        let a = node.layout().area;
        (a.width() > 0. && a.max_x() > width + 0.5).then(|| (a.min_x(), a.max_x()))
    })
}

/// P5-06: squeezed to the shell's stub width, the panel lays everything out **rightward from its
/// own origin** and never wider than the stated body floor.
///
/// Two separate faults this pins. The header was `main_align(SpaceBetween)` over `Content::Normal`
/// with no clip, and `Overflow` defaults to painting *outside* the bounds, so a narrow panel drew
/// "COLUMN INSPECTOR" straight through the collapse ×. And the body had no floor at all, so a run
/// with no break opportunity (`customer_shipping_address_line_one`) was wider than the panel
/// whatever the layout did — and centred rows around it started at a **negative x**, painting off
/// the left edge into the workbench.
///
/// The bound is `PANE_BODY_MIN_W`, not the panel: below that the body deliberately holds its floor
/// and the panel clips it, which is the whole point of having one. What must never happen is
/// content to the left of zero, or content wider than the floor it was promised.
#[test]
fn the_panel_lays_out_within_its_body_floor_at_stub_width() {
    const STUB: f32 = 84.;

    for width in [STUB, 120., 180., PANEL_WIDTH] {
        let (mut runner, (mut sel, mut project, ..)) = runner_at(width);
        settle(&mut runner);
        project.write_channel(ProjChan::Tables).table_registered(
            "events",
            TableMeta {
                columns: vec![col(
                    "customer_shipping_address_line_one",
                    DataType::Timestamp(TimeUnit::Millisecond, None),
                    vec![
                        stat(StatKey::Nulls, "1"),
                        stat(StatKey::Min, "Aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
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

        let bound = width.max(PANE_BODY_MIN_W);
        let past: Vec<_> = runner.find_many(|node, _| {
            let a = node.layout().area;
            (a.width() > 0. && (a.max_x() > bound + 0.5 || a.min_x() < -0.5))
                .then(|| (a.min_x(), a.max_x()))
        });
        assert!(
            past.is_empty(),
            "at a {width}px panel, laid out outside 0..={bound}: {past:?}"
        );
    }
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

/// The same, for the STATISTICS zone's **scanned** state — the age / view-as-query / ↻ header over
/// facts a real scan computed. Ignored like its neighbour:
/// `cargo test -p strata-freya inspector_scanned_preview -- --ignored`.
#[test]
#[ignore = "writes target/inspector-scanned.png for eyeballing; run explicitly"]
fn inspector_scanned_preview() {
    let (mut runner, (mut sel, mut project, ..)) = runner();
    settle(&mut runner);
    select(
        &mut runner,
        &mut sel,
        column(CatalogKind::Table, SCAN_TABLE, &["region"]),
    );
    project
        .write_channel(ProjChan::Tables)
        .request_profile(CatalogKind::Table, SCAN_TABLE);
    runner.poll(Duration::from_millis(10), Duration::from_millis(2_000));
    runner.render_to_file(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../target/inspector-scanned.png"
    ));
}
