//! Catalog sidebar interaction tests (P3-02) — the rendered pane, driven the way the user drives
//! it. The **filter** carries most of the weight: it is the one behaviour that spans all three
//! sections at once, and the one whose edge cases (case folding, an empty section vs a filtered-out
//! one, the live counts) are invisible to a unit test of the matcher.
//!
//! The column-flattening maths are unit-tested next door in [`super::columns`]; these are about
//! what actually reaches the tree.

use std::path::PathBuf;
use std::time::Duration;

use datafusion::arrow::datatypes::{DataType, Field, TimeUnit};
use freya::radio::RadioStation;
use freya_testing::prelude::{MouseEventName, PlatformEvent};
use freya_testing::TestingRunner;
use strata_core::engine::{column_info, TableMeta, ViewMeta};
use strata_core::project::ProjectDefs;
use strata_core::theme::load;
use strata_model::{
    ColRef, ColumnInfo, Origin, SavedQuery, SourceFormat, TableDef, TableOrigin, ViewDef,
};
use uuid::Uuid;

use crate::apps::configure::ConfigureTarget;
use crate::apps::project::state::{CatalogState, Log, PersistFaults};

use super::entry::{folds_badge, ACTIONS_SIZE};
use super::*;
use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::state::{Chan, Reg, ScanRequest, ScanScope, SessionState};
use crate::apps::project::views::{DropTarget, ProfileTarget};
use crate::components::PROGRESS_HOLD;
use crate::state::ConfigStation;
use crate::theme::strata_theme;
use strata_core::config::AppConfig;

/// A leaf column, through the engine's own `column_info` — so the fixture's type spelling,
/// display kind and chart role are all derived from one Arrow type rather than stated three
/// times and able to disagree.
fn col(name: &str, dtype: DataType) -> ColumnInfo {
    column_info(&Field::new(name, dtype, true))
}

/// A leaf column's Arrow field, for nesting inside a struct.
fn field(name: &str, dtype: DataType) -> Field {
    Field::new(name, dtype, true)
}

/// A struct column over its children — the fixture builds the Arrow type and `column_info`
/// derives the whole row from it, nested children included.
fn nested(name: &str, children: Vec<Field>) -> ColumnInfo {
    column_info(&Field::new(name, DataType::Struct(children.into()), true))
}

fn table(name: &str, partition_cols: Vec<(String, String)>) -> TableDef {
    TableDef {
        name: name.into(),
        format: SourceFormat::Parquet,
        sources: vec![format!("{name}.parquet")],
        partition_cols,
        origin: TableOrigin::External,
    }
}

/// An **internal** table def — one a `CREATE TABLE` wrote into the project (ED-04). Kept out of
/// [`defs`] deliberately: the pane's counts, filters and spinner assertions are about the fixture
/// as a whole, and the two things an origin changes (the row's marker, the row's menu) are
/// answered better by a project of their own than by a fourth row every other test has to allow
/// for.
fn internal(name: &str) -> TableDef {
    TableDef {
        name: name.into(),
        format: SourceFormat::Arrow,
        sources: vec![format!(".strata/tables/{name}/")],
        partition_cols: Vec::new(),
        origin: TableOrigin::Internal,
    }
}

/// A project whose names deliberately overlap across sections: `orders` (table), `orders_daily`
/// (view) and `orders by region` (saved query) all contain "order", so one filter can be shown to
/// reach all three; `users`, `regions`, `events` and `archive_totals` never match it.
///
/// The last two carry the **invalid** states (P3-04): `events` is a table the engine refuses, and
/// `archive_totals` is a view whose base table `archive` isn't in the catalog at all.
fn defs() -> ProjectDefs {
    ProjectDefs {
        name: "test".into(),
        tables: vec![
            table("events", vec![]),
            table("orders", vec![("year".into(), "Int32".into())]),
            table("users", vec![]),
        ],
        views: vec![
            ViewDef {
                name: "archive_totals".into(),
                sql: "SELECT 1".into(),
            },
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
        ..Default::default()
    }
}

/// A store whose registrations have already landed — `orders` carries a nested `address` struct
/// and the `year` partition column, so expansion and the PART chip are exercisable. `users` is
/// deliberately left `Loading` to cover the unanswered state, `events` is **refused** by the
/// engine, and `archive_totals` registered cleanly over a base table that isn't in the catalog.
fn project() -> ProjectState {
    let mut p = ProjectState::from_defs(defs(), PathBuf::from("/tmp/strata-catalog-test"));
    p.table_failed("events", "No such file or directory (os error 2)".into());
    p.table_registered(
        "orders",
        TableMeta {
            columns: vec![
                col("id", DataType::Int64),
                nested(
                    "address",
                    vec![field("city", DataType::Utf8), field("zip", DataType::Utf8)],
                ),
                col("year", DataType::Int32),
            ],
            rows: Some(10),
        },
    );
    // `users` stays `Reg::Loading` — the first-paint state every row passes through.
    p.view_registered(
        "archive_totals",
        ViewMeta {
            columns: vec![col("total", DataType::Int64)],
            tables: vec!["archive".into()],
            aliases: Vec::new(),
        },
    );
    p.view_registered(
        "orders_daily",
        ViewMeta {
            columns: vec![col("day", DataType::Date32)],
            tables: vec!["orders".into()],
            aliases: Vec::new(),
        },
    );
    p.view_registered(
        "regions",
        ViewMeta {
            columns: vec![col("region", DataType::Utf8)],
            tables: Vec::new(),
            aliases: Vec::new(),
        },
    );
    p
}

/// A project holding **one table of each origin**, both registered — the pair the row marker and
/// the row menu have to tell apart.
fn mixed_origins() -> ProjectState {
    let defs = ProjectDefs {
        name: "test".into(),
        tables: vec![table("orders", vec![]), internal("daily_totals")],
        ..Default::default()
    };
    let mut p = ProjectState::from_defs(defs, PathBuf::from("/tmp/strata-catalog-origins"));
    for name in ["orders", "daily_totals"] {
        p.table_registered(
            name,
            TableMeta {
                columns: vec![col("n", DataType::Int64)],
                rows: Some(3),
            },
        );
    }
    p
}

/// The pane over the stores the runner provides. Both the project and the session store come from
/// the runner as **root contexts**, so a test can write to the catalog (dropping a table, landing a
/// registration) and read the layout back.
///
/// The [`ContextMenuViewer`] is the window root's in the real app; the row menus need it in an
/// ancestor scope, and it is also what *renders* an open menu — so it is what makes the menu
/// items assertable as text.
fn app() -> impl IntoElement {
    use_init_theme(|| strata_theme(&load("midnight")));
    let filter = use_consume::<State<String>>();
    rect()
        .expanded()
        .child(ContextMenuViewer::new())
        .child(Catalog::new(filter))
}

/// What the test holds onto: the filter slot to type into, the inspected-column slot, the
/// session + project stores to assert against (and, for validity, to mutate), and the
/// drop-confirm / profile-confirm slots the destructive and scanning items are supposed to set.
type Handles = (
    State<String>,
    State<Option<ColRef>>,
    RadioStation<SessionState, Chan>,
    RadioStation<ProjectState, ProjChan>,
    State<Option<DropTarget>>,
    State<ScanRequest>,
    State<Option<ProfileTarget>>,
);

/// A tall window so every row lays out (the pane's `ScrollView` keeps off-screen children in the
/// tree, but height removes all doubt). The session starts with the inspector **closed**, so a
/// selection opening it is observable rather than a no-op against the default.
fn runner() -> (TestingRunner, Handles) {
    runner_over(project)
}

/// [`runner`] over a project of the caller's choosing — a `fn` pointer rather than a value,
/// because `TestingRunner`'s initializer is a plain closure and a `fn` is `Copy` in one.
fn runner_over(project: fn() -> ProjectState) -> (TestingRunner, Handles) {
    runner_sized(project, 300.)
}

/// [`runner_over`] at a chosen pane width — what the badge's fold is measured against.
fn runner_sized(project: fn() -> ProjectState, width: f32) -> (TestingRunner, Handles) {
    TestingRunner::new(
        app,
        (width, 1400.).into(),
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
            let store = r.provide_root_context(move || {
                RadioStation::<ProjectState, ProjChan>::create(project())
            });
            // The row menus' remaining handles: the engine (never asked anything here — no test
            // presses Refresh), the scan flag, the app config behind "View table"'s LIMIT, and
            // the drop-confirm slot the Drop items set.
            r.provide_root_context(EngineCtx::default);
            r.provide_root_context(|| State::create(CatalogState::Settled(0)));
            let rescan = r.provide_root_context(|| State::create(ScanRequest::default()));
            r.provide_root_context(|| ConfigStation::create(AppConfig::default()));
            let drop_target = r.provide_root_context(|| State::create(None::<DropTarget>));
            // The Configure-window request slot (P4-11). The row menus only ever *set* it —
            // the window is opened by the project root's launcher, which is not mounted here.
            r.provide_root_context(|| State::create(None::<ConfigureTarget>));
            let profile_target = r.provide_root_context(|| State::create(None::<ProfileTarget>));
            // Where the one action in these menus that writes `project.json` itself — the
            // saved-query rename — reports a failed write (P4-15): the event log, and the
            // write-fault satellite that holds the condition after it.
            r.provide_root_context(|| State::create(Log::default()));
            r.provide_root_context(|| State::create(PersistFaults::default()));
            (
                filter,
                selection,
                session,
                store,
                drop_target,
                rescan,
                profile_target,
            )
        },
        1.,
    )
}

/// Settle the tree: render, then let the effects those renders scheduled actually run.
///
/// **Several passes, not two.** Freya polls tasks only once *no scope is dirty*
/// (`Runner::handle_events_immediately`), and `use_side_effect` is a task — so a pass that leaves
/// anything dirty defers every effect in the tree to the next one. Every catalog row now mounts a
/// ⋮ `Button`, which costs one such pass on first paint. Under-settling fails silently and
/// confusingly: the render is correct, but effect-derived state (the status slot's held verdict)
/// is simply never computed.
fn settle(runner: &mut TestingRunner) {
    for _ in 0..4 {
        runner.sync_and_update();
    }
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
    settle(runner);
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

/// The laid-out box of the first text run equal to `text`.
fn text_area(runner: &TestingRunner, text: &str) -> Area {
    runner
        .find(|node, element| {
            Label::try_downcast(element)
                .filter(|l| l.text == text)
                .map(|_| node.layout().area)
        })
        .unwrap_or_else(|| panic!("no text run {text:?} in the tree"))
}

/// Right-click the row named `text` — how a user opens its context menu. The cursor is moved
/// first because that is what the [`ContextMenuViewer`] tracks to place the card.
fn right_click_row(runner: &mut TestingRunner, text: &str) {
    let area = text_area(runner, text);
    let point = (
        (area.min_x() + area.width() / 2.) as f64,
        (area.min_y() + area.height() / 2.) as f64,
    );
    runner.move_cursor(point);
    runner.send_event(PlatformEvent::Mouse {
        name: MouseEventName::MouseDown,
        cursor: point.into(),
        button: Some(MouseButton::Right),
    });
    settle(runner);
}

/// Press the ⋮ button of the row named `text` — the menu's *other* trigger. It sits at the row's
/// trailing edge, so it is found by the tallest 22×22 box on that row's line.
fn press_row_actions(runner: &mut TestingRunner, text: &str) {
    let row = text_area(runner, text);
    let mid_y = row.min_y() + row.height() / 2.;
    let button = runner
        .find_many(|node, _| {
            let a = node.layout().area;
            let square = (a.width() - 22.).abs() < 0.5 && (a.height() - 22.).abs() < 0.5;
            (square && a.min_y() <= mid_y && a.max_y() >= mid_y).then_some(a)
        })
        .into_iter()
        .max_by(|a, b| a.min_x().total_cmp(&b.min_x()))
        .unwrap_or_else(|| panic!("no ⋮ button on the {text:?} row"));
    let point = (
        (button.min_x() + button.width() / 2.) as f64,
        (button.min_y() + button.height() / 2.) as f64,
    );
    runner.move_cursor(point);
    runner.click_cursor(point);
    settle(runner);
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
    settle(runner);
}

// ---- filtering ----------------------------------------------------------------------------

/// The headline behaviour: one filter narrows tables *and* views *and* saved queries at once,
/// keeping only the matches in each — not just the section that happens to be first.
#[test]
fn filter_narrows_all_three_sections_at_once() {
    let (mut runner, (mut filter, ..)) = runner();
    settle(&mut runner);

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
    settle(&mut runner);

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
    settle(&mut runner);

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
    settle(&mut runner);

    assert!(shows(&runner, "TABLES · 3"));
    assert!(shows(&runner, "VIEWS · 3"));
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
    settle(&mut runner);

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
    settle(&mut runner);

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
    settle(&mut runner);

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
    settle(&mut runner);

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
    settle(&mut runner);

    click_text(&mut runner, "orders_daily");
    assert!(shows(&runner, "day"));
}

/// Selecting a column publishes the full [`ColRef`] — kind, owner and **path** — and reveals the
/// inspector, which is how it reopens once collapsed.
#[test]
fn selecting_a_column_publishes_its_ref_and_opens_the_inspector() {
    let (mut runner, (_, selection, session, ..)) = runner();
    settle(&mut runner);

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
    settle(&mut runner);

    click_text(&mut runner, "TABLES · 3");
    assert!(!shows(&runner, "orders"), "the table rows are put away");
    assert!(!shows(&runner, "users"));
    assert!(
        shows(&runner, "TABLES · 3"),
        "the header (and its count) stays"
    );
    assert!(
        shows(&runner, "orders_daily") && shows(&runner, "signup funnel"),
        "the other sections are untouched"
    );

    click_text(&mut runner, "TABLES · 3");
    assert!(shows(&runner, "orders"), "pressing again restores them");
}

/// A nested field selects by its **whole path**, so the inspector can tell `orders.address.city`
/// from a top-level `city` — the identity bug `ColRef`'s `Vec<String>` path exists to prevent.
#[test]
fn a_nested_field_selects_by_its_full_path() {
    let (mut runner, (_, selection, ..)) = runner();
    settle(&mut runner);

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

// ---- the status slot: unanswered · invalid (P3-04) --------------------------------------------

/// Every status glyph's message, from its **a11y label** — the spinner's "Loading…" and the
/// validity triangle's reason. Nothing else in the pane declares a label (Freya's own rows, icons
/// and loaders don't), so this *is* the list of unsettled rows and what each is saying: read the
/// way a screen reader would rather than the way a 500ms hover would, and settled rows contribute
/// nothing, which is what makes a whole-list `assert_eq` meaningful.
fn status_labels(runner: &TestingRunner) -> Vec<String> {
    let mut labels: Vec<String> = runner.find_many(|_, element| {
        element
            .accessibility()
            .builder
            .label()
            .map(|label| label.to_string())
    });
    labels.sort();
    labels
}

/// Run the tree past the spinner's hold-back, so a row that is genuinely still waiting gets to
/// spin. Real time, because the delay is a real timer (`Timer::after(SPINNER_DELAY)`).
///
/// **Deliberately generous, and expressed as a multiple of the app's own constant.** This was a
/// hand-tuned `550ms` against a 400ms hold, and 150ms of slack is not enough on a shared runner:
/// the first CI run of this suite failed right here, with zero spinners, because the wait expired
/// before the timer the row had armed. The margin has to cover however long the runner takes to
/// get from the update that arms the timer to an update after it fires — so it is stated as
/// `PROGRESS_HOLD * 3`, which tracks the constant instead of drifting from it, and `settle`s
/// afterwards because `poll` ends on a tick it never renders.
fn wait_out_the_spinner_delay(runner: &mut TestingRunner) {
    runner.poll(Duration::from_millis(20), PROGRESS_HOLD * 3);
    settle(runner);
}

/// The two halves of the slot, and the asymmetry between them. A **failure is a settled answer**,
/// so both triangles are there on the first paint, each with its own reason. **Waiting is
/// transient**, so `users` shows nothing yet — only once the wait outlasts the hold-back does it
/// join. Every settled row stays silent throughout, and none of it is text in the row any more,
/// which is the point of the slot.
#[test]
fn failures_flag_at_once_but_a_wait_has_to_last_before_it_spins() {
    let (mut runner, ..) = runner();
    settle(&mut runner);

    assert_eq!(
        status_labels(&runner),
        vec![
            "No such file or directory (os error 2)".to_string(),
            "Reads archive, which is no longer in the catalog.".to_string(),
        ],
        "the broken rows flag immediately; the waiting one holds its peace"
    );
    // The words live on the glyph, not in the row: the name gets the whole width back.
    for gone in ["loading…", "failed"] {
        assert!(!shows(&runner, gone), "{gone:?} is no longer a text run");
    }

    wait_out_the_spinner_delay(&mut runner);

    assert_eq!(
        status_labels(&runner),
        vec![
            "Loading…".to_string(),
            "No such file or directory (os error 2)".to_string(),
            "Reads archive, which is no longer in the catalog.".to_string(),
        ],
        "a wait this long is worth reporting"
    );
}

/// The derived half is *live*: dropping a table flags every view that reads it, even though the
/// view's own row never changed and the drop raises no view event of its own. This is why the row
/// subscribes to TABLES as well as VIEWS.
#[test]
fn dropping_a_table_flags_the_views_that_read_it() {
    let (mut runner, (.., mut store, _, _, _)) = runner();
    settle(&mut runner);

    assert!(
        !status_labels(&runner).iter().any(|w| w.contains("orders")),
        "`orders_daily` starts healthy"
    );

    store.write_channel(ProjChan::Tables).remove_table("orders");
    settle(&mut runner);

    assert!(
        status_labels(&runner)
            .iter()
            .any(|w| w == "Reads orders, which is no longer in the catalog."),
        "the view over the dropped table is flagged"
    );
}

/// Nothing is stored, so nothing has to be invalidated: land the registration the row was waiting
/// for and its triangle is simply gone on the next render — the row goes silent rather than
/// swapping one status for another.
#[test]
fn a_triangle_clears_when_the_row_registers() {
    let (mut runner, (.., mut store, _, _, _)) = runner();
    settle(&mut runner);

    assert!(status_labels(&runner)
        .iter()
        .any(|w| w == "No such file or directory (os error 2)"));

    store.write_channel(ProjChan::Tables).table_registered(
        "events",
        TableMeta {
            columns: vec![col("at", DataType::Timestamp(TimeUnit::Millisecond, None))],
            rows: Some(4),
        },
    );
    settle(&mut runner);

    assert_eq!(
        status_labels(&runner),
        vec!["Reads archive, which is no longer in the catalog.".to_string()],
        "the flag follows the catalog — there is nothing to clear"
    );
}

/// The point of the hold-back: a row whose answer lands quickly — which is nearly all of them —
/// never spins at all. Waiting out the delay afterwards is what makes this an assertion rather
/// than a coincidence of when the test looked.
#[test]
fn a_row_answered_inside_the_delay_never_spins() {
    let (mut runner, (.., mut store, _, _, _)) = runner();
    settle(&mut runner);

    assert!(shows(&runner, "users"), "the def renders regardless");

    store.write_channel(ProjChan::Tables).table_registered(
        "users",
        TableMeta {
            columns: vec![col("id", DataType::Int64)],
            rows: Some(2),
        },
    );
    settle(&mut runner);

    wait_out_the_spinner_delay(&mut runner);

    assert!(
        !status_labels(&runner).iter().any(|l| l == "Loading…"),
        "the armed spinner was cancelled by the answer, not merely hidden"
    );
}

/// How many rows are currently spinning.
fn spinners(runner: &TestingRunner) -> usize {
    status_labels(runner)
        .iter()
        .filter(|l| *l == "Loading…")
        .count()
}

/// The whole point of the hold, and the case that sent us looking: **↻ on a broken row must not
/// blink its triangle**. A re-scan resets every table row to `Loading`, at which point the store
/// honestly has no verdict — but the slot keeps showing the last one, so a row that was broken
/// before and is broken after never visibly changes. Nor does the pane fill with spinners the
/// instant ↻ is pressed.
#[test]
fn a_rescan_does_not_blink_the_triangle_of_a_row_that_stays_broken() {
    let (mut runner, (.., mut store, _, _, _)) = runner();
    settle(&mut runner);
    let broken = "No such file or directory (os error 2)";
    assert!(status_labels(&runner).iter().any(|l| l == broken));

    // ↻ — every table row unanswered again, `events` included.
    store.write_channel(ProjChan::Tables).reload_tables();
    settle(&mut runner);

    assert!(
        status_labels(&runner).iter().any(|l| l == broken),
        "the verdict is held through the gap rather than un-said: {:?}",
        status_labels(&runner)
    );
    assert_eq!(spinners(&runner), 0, "and no row spins on the spot");

    // The retry lands, still broken: the triangle was there before, during and after.
    store
        .write_channel(ProjChan::Tables)
        .table_failed("events", broken.into());
    settle(&mut runner);

    assert!(status_labels(&runner).iter().any(|l| l == broken));
}

/// The exception that keeps the hold honest — a **settled** answer applies at once. A re-scan that
/// fixes a row clears its triangle the moment the registration lands, with no wait to sit through:
/// holding a verdict we now know to be wrong would be worse than the blink.
#[test]
fn a_rescan_that_fixes_a_row_clears_its_triangle_at_once() {
    let (mut runner, (.., mut store, _, _, _)) = runner();
    settle(&mut runner);

    store.write_channel(ProjChan::Tables).reload_tables();
    settle(&mut runner);

    store.write_channel(ProjChan::Tables).table_registered(
        "events",
        TableMeta {
            columns: vec![col("at", DataType::Timestamp(TimeUnit::Millisecond, None))],
            rows: Some(4),
        },
    );
    settle(&mut runner);

    assert!(
        !status_labels(&runner)
            .iter()
            .any(|l| l == "No such file or directory (os error 2)"),
        "a good answer supersedes the held verdict immediately"
    );
}

/// Past the hold, a wait is news in its own right: a row still unanswered after the delay gives its
/// slot over to the spinner, held verdict or not. The row that was *already* waiting keeps spinning
/// throughout — its wait never stopped, so re-arming it would blink the spinner instead.
#[test]
fn a_slow_rescan_gives_every_waiting_row_over_to_the_spinner() {
    let (mut runner, (.., mut store, _, _, _)) = runner();
    runner.sync_and_update();
    wait_out_the_spinner_delay(&mut runner);
    assert_eq!(
        spinners(&runner),
        1,
        "only `users` has been waiting long enough to spin"
    );

    store.write_channel(ProjChan::Tables).reload_tables();
    settle(&mut runner);
    assert_eq!(
        spinners(&runner),
        1,
        "the rows the re-scan reset serve the hold; the one already waiting keeps its spinner"
    );

    wait_out_the_spinner_delay(&mut runner);

    assert_eq!(
        spinners(&runner),
        3,
        "once every table row's wait is real, every one of them spins"
    );
    assert!(
        !status_labels(&runner)
            .iter()
            .any(|l| l == "No such file or directory (os error 2)"),
        "and the spinner supersedes the held triangle rather than doubling up on it"
    );
}

// ---- the row menus (P3-06) --------------------------------------------------------------------

/// **The trailing run is one column, whatever the row is doing.** The badge, the validity
/// triangle and the profiling spinner all used to be separate children, so a row that had ever
/// been profiled kept a mounted, idle slot in the run and everything left of it sat 20px further
/// in than on a row that had not — which is what made the `INTERNAL` badge look misaligned
/// against a row carrying a warning triangle.
#[test]
fn the_trailing_marks_line_up_whatever_each_row_is_doing() {
    let (runner, ..) = settled_over(|| {
        let mut p = mixed_origins();
        p.table_failed("orders", "boom".into());
        // The state that broke it: a scan asked for on the internal row.
        p.request_profile(CatalogKind::Table, "daily_totals");
        p
    });

    // The ⋮ is the row's rightmost item and is unconditional, so it is the column to measure
    // everything else against.
    let right_of = |name: &str| {
        let row = text_area(&runner, name);
        let mid = row.min_y() + row.height() / 2.;
        runner
            .find_many(|node, _| {
                let a = node.layout().area;
                ((a.width() - ACTIONS_SIZE).abs() < 0.5
                    && (a.height() - ACTIONS_SIZE).abs() < 0.5
                    && a.min_y() <= mid
                    && a.max_y() >= mid)
                    .then_some(a.max_x())
            })
            .into_iter()
            .fold(f32::MIN, f32::max)
    };

    assert_eq!(
        right_of("orders"),
        right_of("daily_totals"),
        "a failed row and a profiled internal row end in the same place"
    );
}

/// **The badge folds only when folding saves the name.** Three cases, and the middle one is the
/// only fold — the first version of this got the third wrong, dropping the badge on any narrow
/// pane, so a long-named internal table lost the marker *and* still truncated.
#[test]
fn the_internal_badge_folds_before_the_name_truncates() {
    // A short name at a wide pane: room for both, so the marker stays.
    let (runner, ..) = settled_over(mixed_origins);
    assert!(shows(&runner, "INTERNAL"), "a wide pane keeps the marker");

    // Narrow enough that the badge is what pushes the name over — the fold. Not *too* narrow:
    // below about 190 the name (`daily_totals`, 12 mono characters) no longer fits even without
    // the badge, and the rule correctly keeps it there.
    let (mut runner, _) = runner_sized(mixed_origins, 240.);
    settle(&mut runner);
    assert!(
        !shows(&runner, "INTERNAL"),
        "the badge goes so the name can stay whole: {:?}",
        texts(&runner)
    );
    assert!(shows(&runner, "daily_totals"), "and the name is intact");
}

/// The rule on its own — `components::toolbar`'s order, applied to this row: the foldable item
/// goes while the leading run is still whole. Both earlier versions are pinned here as the cases
/// they got wrong: a flat floor let a long name ellipsize with the badge still up, and a "cannot
/// fit either way" case kept the badge beside an empty name.
#[test]
fn the_badge_never_costs_the_name_a_character() {
    // Room for both.
    assert!(!folds_badge(400., 100.));
    // The badge is what tips the name into an ellipsis.
    assert!(folds_badge(260., 100.));
    // Tight enough that the name is in trouble regardless — the badge still goes, because that
    // is when its 71px is worth the most.
    assert!(folds_badge(180., 300.));
}

/// And the name goes on collapsing **after** the badge has gone. A leading run ellipsizes all the
/// way down rather than setting a floor and making the row spill (AGENTS.md §3), so the order the
/// user sees is: badge disappears, then the name shortens.
#[test]
fn the_name_goes_on_collapsing_once_the_badge_has_folded() {
    let (mut runner, _) = runner_sized(mixed_origins, 150.);
    settle(&mut runner);

    assert!(
        !shows(&runner, "INTERNAL"),
        "the badge folded first: {:?}",
        texts(&runner)
    );
    // The row still owns its own width — nothing spilled out of the pane to keep the name whole.
    let name = text_area(&runner, "daily_totals");
    assert!(
        name.max_x() <= 150.,
        "the name shrank inside the pane rather than spilling: {name:?}"
    );
}

/// **The icon says it too**, in a colour of its own — the mark that survives the fold above, and
/// the reason dropping the badge costs nothing at width. Reinforcement only: the badge and its
/// a11y label are what a colour-blind or screen-reader user gets, which is why the fold drops the
/// badge *last* rather than first.
///
/// Asserted on the resolved role rather than on painted pixels: what this is about is that the
/// theme actually distinguishes the two origins, and `entity.table.internal` falls back to
/// `entity.table`, so a theme that forgot to author it would silently draw no distinction at all.
#[test]
fn both_built_in_themes_tell_the_two_table_origins_apart() {
    for name in ["midnight", "daylight"] {
        let roles = load(name).roles;
        let internal = roles.get("entity.table.internal");
        assert!(
            internal.is_some(),
            "{name} must author entity.table.internal — it falls back to entity.table, so \
             omitting it draws no distinction at all and says nothing about having done so"
        );
        assert_ne!(
            internal,
            roles.get("entity.table"),
            "{name} authors it as a colour of its own"
        );
    }
}

/// The menu items currently in the tree — what an open menu card is actually offering.
///
/// Taken as the text runs that are *not* catalog rows: the pane's own runs are all present
/// before the menu opens, so a set difference is both simpler and stricter than trying to
/// identify the card by geometry.
fn menu_items(runner: &TestingRunner, before: &[String]) -> Vec<String> {
    let mut rest = before.to_vec();
    texts(runner)
        .into_iter()
        .filter(|t| match rest.iter().position(|b| b == t) {
            Some(at) => {
                rest.remove(at);
                false
            }
            None => true,
        })
        .collect()
}

/// Open the menu for the row named `name` and return its items.
fn open_menu(runner: &mut TestingRunner, name: &str) -> Vec<String> {
    let before = texts(runner);
    right_click_row(runner, name);
    menu_items(runner, &before)
}

/// A runner whose first paint has settled — a menu test's starting point, and a separate
/// function so a test can take several without shadowing the constructor.
fn settled() -> (TestingRunner, Handles) {
    settled_over(project)
}

/// [`settled`] over a project of the caller's choosing.
fn settled_over(project: fn() -> ProjectState) -> (TestingRunner, Handles) {
    let (mut runner, handles) = runner_over(project);
    settle(&mut runner);
    (runner, handles)
}

/// The three menus, by row kind — the item lists themselves, in order, because the *order* is
/// the design (the destructive action last, behind a rule) as much as the wording is.
#[test]
fn each_row_kind_offers_its_own_menu() {
    let (mut runner, ..) = settled();

    assert_eq!(
        open_menu(&mut runner, "orders"),
        vec![
            "View table",
            "Profile table",
            "Refresh table",
            "Configure",
            "Drop table"
        ],
        "the table menu"
    );

    let (mut runner, ..) = settled();
    assert_eq!(
        open_menu(&mut runner, "orders_daily"),
        vec!["View view", "Profile view", "Edit query", "Drop view"],
        "a view has no files to re-infer, so no Refresh — and it can be edited, which a table \
         cannot"
    );

    let (mut runner, ..) = settled();
    assert_eq!(
        open_menu(&mut runner, "signup funnel"),
        vec!["Open in new tab", "Rename", "Delete query"],
        "a saved query is a stored string: nothing to profile, configure or refresh"
    );

    // An **internal** table (ED-04) is a fourth row kind as far as this list is concerned. Its
    // omission is pinned here, by the same test that pins every other kind's, rather than being
    // incidental to whoever edits the menu next.
    let (mut runner, ..) = settled_over(mixed_origins);
    assert_eq!(
        open_menu(&mut runner, "daily_totals"),
        vec!["View table", "Profile table", "Refresh table", "Drop table"],
        "Configure edits the sources, format and partitions of a def that points at the user's \
         own files, and an internal table has none of those to edit — ever, which is why the \
         item is absent rather than disabled. Refresh stays: re-inference is how its row count \
         moves"
    );
    assert!(
        open_menu(&mut runner, "orders").contains(&"Configure".to_string()),
        "and the external table beside it still has it"
    );
}

/// **The row says which origin it is**, because that is what stands between the user and a drop
/// that means two different things. Off the def, so it does not depend on registration having
/// answered.
#[test]
fn an_internal_table_row_is_marked_and_an_external_one_is_not() {
    let (runner, ..) = settled_over(mixed_origins);

    let runs = texts(&runner);
    assert_eq!(
        runs.iter().filter(|t| *t == "INTERNAL").count(),
        1,
        "exactly the one table Strata owns carries the marker: {runs:?}"
    );
    // Beside the row it belongs to, not floating somewhere in the pane.
    let badge = text_area(&runner, "INTERNAL");
    let row = text_area(&runner, "daily_totals");
    assert!(
        badge.min_y() >= row.min_y() && badge.max_y() <= row.max_y(),
        "the marker sits on its own row"
    );
}

/// The ⋮ button opens the **same** menu right-click does — one item list, two triggers, which is
/// the whole reason the builders live in one module.
#[test]
fn the_actions_button_opens_the_same_menu_as_a_right_click() {
    let (mut runner, ..) = settled();
    let by_right_click = open_menu(&mut runner, "orders");

    let (mut runner, ..) = settled();
    let before = texts(&runner);
    press_row_actions(&mut runner, "orders");

    assert_eq!(menu_items(&runner, &before), by_right_click);
}

/// **View table** puts `SELECT *` in a tab, ready to run but not run — a full scan of a big
/// table must not start itself. The `LIMIT` is the row-limit setting.
#[test]
fn view_table_opens_a_select_star_tab_without_running_it() {
    let (mut runner, (_, _, session, ..)) = settled();
    right_click_row(&mut runner, "orders");

    click_text(&mut runner, "View table");

    let s = session.peek();
    let tab = s
        .active
        .and_then(|id| s.tabs.get(&id))
        .expect("the new tab is focused");
    assert_eq!(tab.name, "orders");
    assert_eq!(tab.text(), "SELECT *\nFROM orders\nLIMIT 100;");
    assert!(tab.request.is_none(), "opened, not run");
}

/// **Edit query** opens the view's own SQL bound to it, so ⌘S redefines *that view* rather than
/// saving a new query — the DEV_TASKS "⌘S on a view saves a saved-query" bug, from the other end.
#[test]
fn edit_query_opens_the_views_sql_bound_to_the_view() {
    let (mut runner, (_, _, session, ..)) = settled();
    right_click_row(&mut runner, "orders_daily");

    click_text(&mut runner, "Edit query");

    let s = session.peek();
    let tab = s
        .active
        .and_then(|id| s.tabs.get(&id))
        .expect("the view opened in a tab");
    assert_eq!(tab.text(), "SELECT 1", "the view's own SQL, not a SELECT *");
    assert!(
        matches!(&tab.origin, Origin::View(v) if v == "orders_daily"),
        "bound to the view: {:?}",
        tab.origin
    );
}

/// Pressing a saved-query row opens it — the canvas's own row `title` — bound by **id**, which
/// is what makes the rename below free.
#[test]
fn pressing_a_saved_query_row_opens_it_bound_by_id() {
    let (mut runner, (_, _, session, ..)) = settled();

    click_text(&mut runner, "signup funnel");

    let s = session.peek();
    let tab = s
        .active
        .and_then(|id| s.tabs.get(&id))
        .expect("the query opened in a tab");
    assert_eq!(tab.name, "signup funnel");
    assert_eq!(tab.text(), "SELECT 4");
    assert!(
        matches!(&tab.origin, Origin::SavedQuery(q) if *q == Uuid::from_u128(2)),
        "bound by id, not by label: {:?}",
        tab.origin
    );
}

/// Every **Drop** item opens P3-05's confirm rather than dropping: it sets the target slot the
/// dialog watches, and the catalog is untouched until that dialog is confirmed. There is
/// deliberately no second drop path — this is the assertion that pins it.
#[test]
fn drop_asks_the_confirm_and_leaves_the_catalog_alone() {
    let (mut runner, (_, _, _, store, drop_target, _, _)) = settled();
    right_click_row(&mut runner, "orders");

    click_text(&mut runner, "Drop table");

    assert!(
        matches!(drop_target.peek().as_ref(), Some(DropTarget::Table(n)) if n == "orders"),
        "the confirm was asked about `orders`: {:?}",
        drop_target.peek()
    );
    assert!(
        store.peek().tables.iter().any(|t| t.def.name == "orders"),
        "and nothing has been dropped yet"
    );
}

/// **Profile** asks the cost confirm (P3-10) rather than scanning, and the row is left carrying
/// no request until that dialog says so. Same shape as Drop, and for the same reason: one entry
/// point, shared with the inspector's scan card, so a full read of the user's data can never be
/// started by a stray press.
#[test]
fn profile_asks_the_cost_confirm_rather_than_scanning() {
    let (mut runner, (_, _, _, store, _, _, profile_target)) = settled();
    right_click_row(&mut runner, "orders");

    click_text(&mut runner, "Profile table");

    assert_eq!(
        profile_target.peek().as_ref().map(|t| t.name.clone()),
        Some("orders".to_string()),
        "the confirm was asked about `orders`: {:?}",
        profile_target.peek()
    );
    assert_eq!(
        store.peek().profile_scan(CatalogKind::Table, "orders"),
        None,
        "and nothing is scanning yet"
    );

    // A **view's** item asks about the view — the other section, and the kind that decides which
    // channel the request lands on.
    let (mut runner, (.., profile_target)) = settled();
    right_click_row(&mut runner, "orders_daily");
    click_text(&mut runner, "Profile view");
    assert_eq!(
        profile_target
            .peek()
            .as_ref()
            .map(|t| (t.kind, t.name.clone())),
        Some((CatalogKind::View, "orders_daily".to_string()))
    );
}

/// A row the engine **refused** is not offered a scan. There is nothing behind it to read, so the
/// scan could only fail — and it would fail out of sight, since the inspector shows a failed row's
/// reason rather than any column a scan could report on. Asserted through the press, not the
/// disabled dress: what matters is that nothing is asked for.
#[test]
fn a_refused_row_is_not_offered_a_scan() {
    let (mut runner, (_, _, _, store, _, _, profile_target)) = settled();
    assert_eq!(
        store.peek().tables[0].def.name,
        "events",
        "the refused row (`table_failed` in the fixture)"
    );
    right_click_row(&mut runner, "events");

    click_text(&mut runner, "Profile table");

    assert!(
        profile_target.peek().is_none(),
        "no confirm was raised: {:?}",
        profile_target.peek()
    );
    assert_eq!(
        store.peek().profile_scan(CatalogKind::Table, "events"),
        None
    );
}

/// Once a row carries a request it **spins**, and the spinner is its own — the registration
/// spinner beside it means something else entirely, and the two must be tellable apart.
///
/// A scan of a table that was never registered fails almost at once, which is exactly what makes
/// this assertable: the glyph is up while the scan is in flight and gone the moment it settles,
/// with no delay hold of its own.
#[test]
fn a_row_being_profiled_says_so_in_its_own_words() {
    let (mut runner, (_, _, _, mut store, ..)) = settled();
    let profiling = |runner: &TestingRunner| {
        status_labels(runner)
            .iter()
            .filter(|l| *l == "Profiling…")
            .count()
    };
    assert_eq!(profiling(&runner), 0);

    store
        .write_channel(ProjChan::Tables)
        .request_profile(CatalogKind::Table, "orders");
    // Two passes: one mounts the subscription, one renders what it says. The scan itself has not
    // settled — its label is up because the query is in flight, not because time has passed.
    runner.sync_and_update();
    runner.sync_and_update();

    assert_eq!(
        profiling(&runner),
        1,
        "the row that was asked about, and only that row: {:?}",
        status_labels(&runner)
    );
    // Deliberately no claim about `Loading…` here: `users` is left unanswered by the fixture, so
    // whether *it* is spinning depends on whether 400ms of wall clock has passed — nothing to do
    // with profiling. That the two spinners are distinguishable at all is the point, and it is
    // `a_row_wearing_every_status_glyph_still_opens_its_own_menu` that pins it.
}

/// The row's trailing run changes shape under it — the status column can hold a spinner, a
/// triangle or nothing, and the badge folds in and out — and Freya assigns scopes by *position*.
/// So the most populated case has to keep working: `events` is a refused table being profiled,
/// and its ⋮ must still open its own menu rather than a scope some other element left behind.
///
/// **The two glyphs are one column now, and the spinner wins while it runs.** They were separate
/// children until the trailing run was collapsed (ED-04): a row that had *ever* been profiled
/// kept a mounted, idle profile slot, and because a zero-width child still costs a full row
/// `spacing`, everything left of it sat further in than on a row that had not — which is what
/// made the `INTERNAL` badge look misaligned against a row carrying a triangle. Nothing is lost
/// by the collapse: "a scan is running" is the newer fact about the same row, and the triangle
/// returns the moment it settles.
#[test]
fn a_row_wearing_every_status_glyph_still_opens_its_own_menu() {
    let (mut runner, (_, _, _, mut store, ..)) = settled();
    wait_out_the_spinner_delay(&mut runner);
    store
        .write_channel(ProjChan::Tables)
        .request_profile(CatalogKind::Table, "events");
    runner.sync_and_update();
    runner.sync_and_update();
    let labels = status_labels(&runner);
    assert!(
        labels.iter().any(|l| l == "Profiling…"),
        "the scan outranks the settled verdict while it runs: {labels:?}"
    );
    assert!(
        !labels
            .iter()
            .any(|l| l == "No such file or directory (os error 2)"),
        "and the column holds one glyph, not two: {labels:?}"
    );

    let before = texts(&runner);
    press_row_actions(&mut runner, "events");

    assert_eq!(
        menu_items(&runner, &before),
        vec![
            "View table",
            "Profile table",
            "Refresh table",
            "Configure",
            "Drop table"
        ],
        "the ⋮ still belongs to `events`"
    );
}

/// A view's Drop and a saved query's Delete go through the *same* slot, so the one dialog covers
/// all three row kinds — a saved query by `id`, because that is its identity.
#[test]
fn dropping_a_view_and_deleting_a_query_use_the_same_confirm_slot() {
    let (mut runner, (.., drop_target, _, _)) = settled();
    right_click_row(&mut runner, "orders_daily");
    click_text(&mut runner, "Drop view");
    assert!(
        matches!(drop_target.peek().as_ref(), Some(DropTarget::View(n)) if n == "orders_daily")
    );

    let (mut runner, (.., drop_target, _, _)) = settled();
    right_click_row(&mut runner, "signup funnel");
    click_text(&mut runner, "Delete query");
    assert!(
        matches!(
            drop_target.peek().as_ref(),
            Some(DropTarget::Query { id, name }) if *id == Uuid::from_u128(2) && name == "signup funnel"
        ),
        "addressed by id, with the label only for the dialog to show: {:?}",
        drop_target.peek()
    );
}

/// **Rename** is inline, in the row: the menu item only flips the row into rename mode — the
/// input, and the commit, are the row's own, so they outlive the menu that started them.
#[test]
fn renaming_a_saved_query_commits_from_the_row_and_persists_by_id() {
    let (mut runner, (_, _, _, store, _, _, _)) = settled();
    right_click_row(&mut runner, "signup funnel");

    click_text(&mut runner, "Rename");

    assert!(
        !shows(&runner, "signup funnel"),
        "the label gave way to the rename input"
    );
    // The seeded name arrives **selected** (`Input::select_all_on_init`), so this replaces it
    // rather than landing in front of it. That is the whole behaviour: a rename opens over the
    // old label.
    runner.write_text("funnel v2");
    runner.sync_and_update();
    runner.press_key(Key::Named(NamedKey::Enter));
    settle(&mut runner);

    let p = store.peek();
    let q = p
        .saved_queries
        .iter()
        .find(|q| q.id == Uuid::from_u128(2))
        .expect("the query is still there, under its new label");
    assert_eq!(
        q.name, "funnel v2",
        "the typing replaced the name, it did not prepend to it"
    );
    assert_eq!(
        q.sql, "SELECT 4",
        "a rename touches the label and nothing else"
    );
    assert!(shows(&runner, "funnel v2"), "and the row shows it");
    assert_eq!(
        p.saved_queries[0].name, "funnel v2",
        "the section is still in name order — the relabelled row sorts first now"
    );
}

/// Escape abandons a rename outright — the row comes back wearing the name it had.
#[test]
fn escape_abandons_a_rename() {
    let (mut runner, (_, _, _, store, _, _, _)) = settled();
    right_click_row(&mut runner, "signup funnel");
    click_text(&mut runner, "Rename");

    runner.write_text("funnel v2");
    runner.sync_and_update();
    runner.press_key(Key::Named(NamedKey::Escape));
    settle(&mut runner);

    assert!(
        shows(&runner, "signup funnel"),
        "the row is back, unrenamed"
    );
    assert_eq!(
        store.peek().saved_queries[1].name,
        "signup funnel",
        "and nothing was written"
    );
}

/// **Refresh table asks the window root for a pass over that table.**
///
/// The regression this pins shipped and had to be found by hand: the pass was `spawn`ed from the
/// menu item's own handler, so it belonged to a `MenuButton` scope the very same press tore down.
/// Freya drops a scope's tasks before polling them, so the rows were reset to `Loading` and
/// nothing ever came back — the table *and* the view over it spun forever.
///
/// The fix is structural: the item raises a [`ScanRequest`] and the driver at the window root
/// runs it, exactly as the ↻ does (`state/catalog.rs`). So this asserts the request — its
/// **scope** is the new part, and it is what tells the driver to re-answer one row instead of
/// re-scanning the project. There is no driver in this pane-only harness, which is the same
/// reason the sidebar's own ↻ test asserts a request rather than a scan.
#[test]
fn refresh_table_asks_for_a_pass_scoped_to_that_row() {
    let (mut runner, (.., store, _, rescan, _)) = settled();
    assert_eq!(
        *rescan.peek(),
        ScanRequest::default(),
        "nothing asked for yet"
    );
    right_click_row(&mut runner, "orders");

    click_text(&mut runner, "Refresh table");

    assert_eq!(
        *rescan.peek(),
        ScanRequest {
            seq: 1,
            scope: ScanScope::Table("orders".into())
        },
        "the row it was pressed on, not the whole catalog"
    );
    // And the item itself touches nothing: resetting rows is the driver's half of the pass, so a
    // request that never gets served can't strand a row in `Loading`.
    assert!(
        matches!(store.peek().tables[1].reg, Reg::Ready(_)),
        "`orders` still wears the answer it had"
    );
}
