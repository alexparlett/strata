//! Catalog sidebar interaction tests (P3-02) — the rendered pane, driven the way the user drives
//! it. The **filter** carries most of the weight: it is the one behaviour that spans all three
//! sections at once, and the one whose edge cases (case folding, an empty section vs a filtered-out
//! one, the live counts) are invisible to a unit test of the matcher.
//!
//! The column-flattening maths are unit-tested next door in [`super::columns`]; these are about
//! what actually reaches the tree.

use std::path::PathBuf;

use freya::radio::RadioStation;
use freya_testing::TestingRunner;
use strata_core::engine::{TableMeta, ViewMeta};
use strata_core::project::ProjectDefs;
use strata_core::theme::load;
use strata_model::{ColRef, ColumnInfo, Kind, SavedQuery, TableDef, ViewDef};
use uuid::Uuid;

use super::*;
use crate::apps::project::state::{Chan, SessionState};
use crate::theme::strata_theme;

/// A leaf column.
fn col(name: &str, dtype: &str, kind: Kind) -> ColumnInfo {
    ColumnInfo {
        name: name.into(),
        dtype: dtype.into(),
        kind,
        nullable: true,
        children: Vec::new(),
        stats: Vec::new(),
    }
}

/// A struct column with children.
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

fn table(name: &str, partition_cols: Vec<(String, String)>) -> TableDef {
    TableDef {
        name: name.into(),
        format: "parquet".into(),
        sources: vec![format!("{name}.parquet")],
        partition_cols,
    }
}

/// A project whose names deliberately overlap across sections: `orders` (table), `orders_daily`
/// (view) and `orders by region` (saved query) all contain "order", so one filter can be shown to
/// reach all three; `users` and `regions` never match it.
fn defs() -> ProjectDefs {
    ProjectDefs {
        name: "test".into(),
        tables: vec![
            table("orders", vec![("year".into(), "Int32".into())]),
            table("users", vec![]),
        ],
        views: vec![
            ViewDef {
                name: "orders_daily".into(),
                sql: "SELECT 1".into(),
            },
            ViewDef {
                name: "regions".into(),
                sql: "SELECT 2".into(),
            },
        ],
        saved_queries: vec![
            SavedQuery {
                id: Uuid::from_u128(1),
                name: "orders by region".into(),
                sql: "SELECT 3".into(),
                meta: "—".into(),
            },
            SavedQuery {
                id: Uuid::from_u128(2),
                name: "signup funnel".into(),
                sql: "SELECT 4".into(),
                meta: "—".into(),
            },
        ],
    }
}

/// The pane mounted over a store whose registrations have already landed — `orders` carries a
/// nested `address` struct and the `year` partition column, so expansion and the PART chip are
/// exercisable. One row (`users`) is deliberately left `Loading` to cover the unanswered state.
fn app() -> impl IntoElement {
    use_init_theme(|| strata_theme(&load("midnight")));
    freya::radio::use_init_radio_station::<ProjectState, ProjChan>(|| {
        let mut p = ProjectState::from_defs(defs(), PathBuf::from("/tmp/strata-catalog-test"));
        p.table_registered(
            "orders",
            TableMeta {
                columns: vec![
                    col("id", "Int64", Kind::Num),
                    nested(
                        "address",
                        vec![
                            col("city", "Utf8", Kind::Str),
                            col("zip", "Utf8", Kind::Str),
                        ],
                    ),
                    col("year", "Int32", Kind::Num),
                ],
                rows: Some(10),
            },
        );
        // `users` stays `Reg::Loading` — the first-paint state every row passes through.
        p.view_registered(
            "orders_daily",
            ViewMeta {
                columns: vec![col("day", "Date32", Kind::Ts)],
                tables: vec!["orders".into()],
                aliases: Vec::new(),
            },
        );
        p.view_registered(
            "regions",
            ViewMeta {
                columns: vec![col("region", "Utf8", Kind::Str)],
                tables: Vec::new(),
                aliases: Vec::new(),
            },
        );
        p
    });
    // The session store (which the catalog writes `open_inspector` on) comes from the runner as a
    // root context, so the test can read the layout back.
    let filter = use_consume::<State<String>>();
    rect().expanded().child(Catalog::new(filter))
}

/// What the test holds onto: the filter slot to type into, the inspected-column slot and the
/// session store to assert against.
type Handles = (
    State<String>,
    State<Option<ColRef>>,
    RadioStation<SessionState, Chan>,
);

/// A tall window so every row lays out (the pane's `ScrollView` keeps off-screen children in the
/// tree, but height removes all doubt). The session starts with the inspector **closed**, so a
/// selection opening it is observable rather than a no-op against the default.
fn runner() -> (TestingRunner, Handles) {
    TestingRunner::new(
        app,
        (300., 1400.).into(),
        |r| {
            let filter = r.provide_root_context(|| State::create(String::new()));
            let selection = r.provide_root_context(|| State::create(None::<ColRef>));
            let session = r.provide_root_context(|| {
                RadioStation::<SessionState, Chan>::create({
                    let mut s = SessionState::default();
                    s.close_inspector();
                    s
                })
            });
            (filter, selection, session)
        },
        1.,
    )
}

/// Every text run currently in the tree.
fn texts(runner: &TestingRunner) -> Vec<String> {
    runner.find_many(|_, element| Label::try_downcast(element).map(|l| l.text.to_string()))
}

fn shows(runner: &TestingRunner, text: &str) -> bool {
    texts(runner).iter().any(|t| t == text)
}

/// Type `text` into the filter and settle the tree.
fn type_filter(runner: &mut TestingRunner, filter: &mut State<String>, text: &str) {
    filter.set(text.to_string());
    runner.sync_and_update();
    runner.sync_and_update();
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
    runner.sync_and_update();
    runner.sync_and_update();
}

/// Press the expand chevron of the nested column row named `name` — `back` pixels left of the
/// name run's own left edge (the row's fixed lead-in; see the call site's arithmetic).
fn expand_nested(runner: &mut TestingRunner, name: &str, back: f32) {
    let area = runner
        .find(|node, element| {
            Label::try_downcast(element)
                .filter(|l| l.text == name)
                .map(|_| node.layout().area)
        })
        .unwrap_or_else(|| panic!("no column row {name:?} in the tree"));
    let point = (
        (area.min_x() - back) as f64,
        (area.min_y() + area.height() / 2.) as f64,
    );
    runner.move_cursor(point);
    runner.click_cursor(point);
    runner.sync_and_update();
    runner.sync_and_update();
}

// ---- filtering ----------------------------------------------------------------------------

/// The headline behaviour: one filter narrows tables *and* views *and* saved queries at once,
/// keeping only the matches in each — not just the section that happens to be first.
#[test]
fn filter_narrows_all_three_sections_at_once() {
    let (mut runner, (mut filter, ..)) = runner();
    runner.sync_and_update();
    runner.sync_and_update();

    // Unfiltered: every def in every section.
    for name in [
        "orders",
        "users",
        "orders_daily",
        "regions",
        "orders by region",
        "signup funnel",
    ] {
        assert!(shows(&runner, name), "{name} should show unfiltered");
    }

    type_filter(&mut runner, &mut filter, "order");

    // The three matches survive — one from each section.
    for name in ["orders", "orders_daily", "orders by region"] {
        assert!(shows(&runner, name), "{name} matches 'order'");
    }
    // The non-matches are gone from all three.
    for name in ["users", "regions", "signup funnel"] {
        assert!(!shows(&runner, name), "{name} does not match 'order'");
    }
}

/// User-typed filters fold case — the catalog's names come from files and SQL, and DataFusion
/// folds unquoted identifiers, so a case-sensitive filter would be a trap.
#[test]
fn filter_folds_case_in_both_directions() {
    let (mut runner, (mut filter, ..)) = runner();
    runner.sync_and_update();
    runner.sync_and_update();

    // Upper-case query against lower-case names.
    type_filter(&mut runner, &mut filter, "ORD");
    assert!(shows(&runner, "orders"));
    assert!(shows(&runner, "orders_daily"));
    assert!(!shows(&runner, "users"));

    // Lower-case query against a name with upper-case in it (the saved query "signup funnel"
    // stays out; "Region" must still reach "orders by region" and the `regions` view).
    type_filter(&mut runner, &mut filter, "REGION");
    assert!(shows(&runner, "regions"));
    assert!(shows(&runner, "orders by region"));
    assert!(!shows(&runner, "orders"));
}

/// Clearing the filter restores everything — the filter narrows a view of the store, it never
/// mutates it.
#[test]
fn clearing_the_filter_restores_every_row() {
    let (mut runner, (mut filter, ..)) = runner();
    runner.sync_and_update();
    runner.sync_and_update();

    type_filter(&mut runner, &mut filter, "zzz-nothing-matches");
    for name in ["orders", "users", "orders_daily", "regions"] {
        assert!(!shows(&runner, name), "{name} filtered out");
    }

    type_filter(&mut runner, &mut filter, "");
    for name in [
        "orders",
        "users",
        "orders_daily",
        "regions",
        "orders by region",
        "signup funnel",
    ] {
        assert!(shows(&runner, name), "{name} restored");
    }
}

/// The section counts are the *filtered* counts, so the header can't claim rows the list isn't
/// showing.
#[test]
fn section_counts_follow_the_filter() {
    let (mut runner, (mut filter, ..)) = runner();
    runner.sync_and_update();
    runner.sync_and_update();

    assert!(shows(&runner, "TABLES · 2"));
    assert!(shows(&runner, "VIEWS · 2"));
    assert!(shows(&runner, "QUERIES · 2"));

    type_filter(&mut runner, &mut filter, "order");
    assert!(shows(&runner, "TABLES · 1"));
    assert!(shows(&runner, "VIEWS · 1"));
    assert!(shows(&runner, "QUERIES · 1"));

    type_filter(&mut runner, &mut filter, "zzz");
    assert!(shows(&runner, "TABLES · 0"));
    assert!(shows(&runner, "VIEWS · 0"));
    assert!(shows(&runner, "QUERIES · 0"));
}

/// The filter matches **def names**, not column names — a deliberate scope (the Dioxus sidebar's
/// too). `city` is a column of `orders` and must not surface its table.
#[test]
fn filter_matches_def_names_not_columns() {
    let (mut runner, (mut filter, ..)) = runner();
    runner.sync_and_update();
    runner.sync_and_update();

    type_filter(&mut runner, &mut filter, "city");
    assert!(
        !shows(&runner, "orders"),
        "a column name must not surface its table"
    );
    assert!(shows(&runner, "TABLES · 0"));
}

/// "No saved queries yet" is about the *section*, not the filter: with a filter typed, an empty
/// result is a non-match, and the empty-state copy would be a lie.
#[test]
fn saved_query_empty_note_is_suppressed_while_filtering() {
    let (mut runner, (mut filter, ..)) = runner();
    runner.sync_and_update();
    runner.sync_and_update();

    // Two saved queries exist, so no note either way.
    assert!(!shows(&runner, "No saved queries yet"));

    type_filter(&mut runner, &mut filter, "zzz");
    assert!(
        !shows(&runner, "No saved queries yet"),
        "an empty filter result is a non-match, not an empty section"
    );
}

/// A filtered-out entry's columns go with it — filtering hides the whole subtree, including one
/// that was expanded before the filter was typed.
#[test]
fn filtering_hides_an_expanded_entrys_columns() {
    let (mut runner, (mut filter, ..)) = runner();
    runner.sync_and_update();
    runner.sync_and_update();

    click_text(&mut runner, "orders");
    assert!(shows(&runner, "id"), "expanded before filtering");

    type_filter(&mut runner, &mut filter, "users");
    assert!(!shows(&runner, "orders"));
    assert!(
        !shows(&runner, "id"),
        "a hidden entry must not leave its columns behind"
    );
}

// ---- expansion + selection ----------------------------------------------------------------

/// Pressing an entry reveals its columns; pressing again puts them away. Nested children stay
/// folded until their own chevron is used.
#[test]
fn entry_expands_to_its_columns_and_nests_one_level_at_a_time() {
    let (mut runner, ..) = runner();
    runner.sync_and_update();
    runner.sync_and_update();

    assert!(!shows(&runner, "id"), "columns start folded away");

    click_text(&mut runner, "orders");
    assert!(shows(&runner, "id"));
    assert!(shows(&runner, "address"));
    assert!(
        !shows(&runner, "city"),
        "a struct's children need their own chevron"
    );

    // The PART chip rides the partition column, which is top-level only.
    assert!(shows(&runner, "PART"));

    click_text(&mut runner, "orders");
    assert!(!shows(&runner, "id"), "pressing again folds it back");
}

/// A view's columns are columns — expandable and selectable exactly like a table's. (In the
/// Dioxus sidebar these were a second copy of the list that silently had no click handler.)
#[test]
fn a_view_expands_to_its_columns_too() {
    let (mut runner, ..) = runner();
    runner.sync_and_update();
    runner.sync_and_update();

    click_text(&mut runner, "orders_daily");
    assert!(shows(&runner, "day"));
}

/// Selecting a column publishes the full [`ColRef`] — kind, owner and **path** — and reveals the
/// inspector, which is how it reopens once collapsed.
#[test]
fn selecting_a_column_publishes_its_ref_and_opens_the_inspector() {
    let (mut runner, (_, selection, session)) = runner();
    runner.sync_and_update();
    runner.sync_and_update();

    assert!(selection.peek().is_none());
    assert!(
        !session.peek().layout.inspector_open,
        "the inspector starts collapsed"
    );

    click_text(&mut runner, "orders");
    click_text(&mut runner, "id");

    let selected = selection.peek().clone().expect("a column is selected");
    assert_eq!(selected.owner, "orders");
    assert_eq!(selected.path, vec!["id".to_string()]);
    assert_eq!(selected.kind, CatalogKind::Table);
    assert!(
        session.peek().layout.inspector_open,
        "selecting a column reveals the inspector"
    );
}

/// Pressing a section header collapses it — its rows leave the tree while the other sections and
/// the header itself stay. Pressing again restores them.
#[test]
fn collapsing_a_section_hides_only_its_own_rows() {
    let (mut runner, ..) = runner();
    runner.sync_and_update();
    runner.sync_and_update();

    click_text(&mut runner, "TABLES · 2");
    assert!(!shows(&runner, "orders"), "the table rows are put away");
    assert!(!shows(&runner, "users"));
    assert!(
        shows(&runner, "TABLES · 2"),
        "the header (and its count) stays"
    );
    assert!(
        shows(&runner, "orders_daily") && shows(&runner, "signup funnel"),
        "the other sections are untouched"
    );

    click_text(&mut runner, "TABLES · 2");
    assert!(shows(&runner, "orders"), "pressing again restores them");
}

/// A nested field selects by its **whole path**, so the inspector can tell `orders.address.city`
/// from a top-level `city` — the identity bug `ColRef`'s `Vec<String>` path exists to prevent.
#[test]
fn a_nested_field_selects_by_its_full_path() {
    let (mut runner, (_, selection, ..)) = runner();
    runner.sync_and_update();
    runner.sync_and_update();

    click_text(&mut runner, "orders");
    // The struct's own chevron sits left of its name; pressing the *name* would select the column
    // instead, so the press has to land in the chevron gutter. Its offset back from the name run is
    // fixed by the row's layout: chevron slot (11) + gap (8) + swatch (6) + gap (8) = 33 to the
    // slot's left edge, so its centre is 33 - 11/2 = 27.5 back.
    const CHEVRON_BACK_FROM_NAME: f32 = 27.5;
    expand_nested(&mut runner, "address", CHEVRON_BACK_FROM_NAME);

    assert!(shows(&runner, "city"), "the struct expanded in place");
    click_text(&mut runner, "city");

    let selected = selection
        .peek()
        .clone()
        .expect("a nested field is selected");
    assert_eq!(
        selected.path,
        vec!["address".to_string(), "city".to_string()],
        "the path carries the parent, not just the leaf"
    );
}

// ---- registration state --------------------------------------------------------------------

/// A row whose registration hasn't landed still renders, and says so — the catalog is the
/// project's defs, so a row exists before (and whether or not) the engine ever answers.
#[test]
fn an_unanswered_row_renders_with_its_loading_state() {
    let (runner, ..) = runner();
    let mut runner = runner;
    runner.sync_and_update();
    runner.sync_and_update();

    assert!(shows(&runner, "users"), "the def renders regardless");
    assert!(shows(&runner, "loading…"));
    // A settled row stays clean — no status run for `orders`.
    assert_eq!(
        texts(&runner).iter().filter(|t| *t == "loading…").count(),
        1,
        "only the unanswered row carries a status"
    );
}
