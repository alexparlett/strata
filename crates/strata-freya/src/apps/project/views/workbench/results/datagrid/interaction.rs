//! Grid interaction tests (P2-11): the a11y-focus-routed edit chords (⌘A dead while the
//! grid is unfocused, cell press focuses, ⌘A then selects all) and the right-click copy
//! menu (retarget outside the selection, keep inside it, menu opens into the viewer).

use std::sync::Arc;

use freya_testing::prelude::{KeyboardEventName, MouseEventName, PlatformEvent};
use freya_testing::TestingRunner;
use strata_core::engine::{RecordBatch, Schema};
use strata_model::{Cell as CellData, ColumnInfo, Kind};

use super::super::find::FindState;
use super::super::sort::SortState;
use super::*;
use crate::apps::project::state::{use_init_session, Chan, SessionState};

/// A 2×2 page (scalar columns, empty batch — ⌘A is pure selection, no serialization).
fn page() -> Rc<GridData> {
    let col = |name: &str, dtype: &str, kind: Kind| ColumnInfo {
        name: name.into(),
        dtype: dtype.into(),
        kind,
        nullable: true,
        children: Vec::new(),
        stats: Vec::new(),
    };
    let cell = |text: &str| CellData { text: text.into(), null: false };
    Rc::new(GridData {
        columns: vec![col("id", "Int64", Kind::Num), col("name", "Utf8", Kind::Str)],
        rows: vec![vec![cell("1"), cell("a")], vec![cell("2"), cell("b")]],
        batch: RecordBatch::new_empty(Arc::new(Schema::empty())),
    })
}

/// The grid stood up like the results pane does: session radio (for the toolbar), its
/// own find/sort state, the page as `PageRead::Ready`, the window's context-menu host
/// (the right-click copy menu opens into it). Settings + the shared selection come in
/// as root contexts from the runner.
fn app() -> impl IntoElement {
    use_init_theme(|| crate::theme::strata_theme(&strata_core::theme::load("midnight")));
    let _station = use_init_session();
    let session = freya::radio::use_radio::<SessionState, Chan>(Chan::Tabs);
    let tab = session.read().active.expect("open tab");
    let find = FindState::use_new();
    let page_no = use_state(|| 1usize);
    let sel = use_consume::<State<Selection>>();
    let sort = SortState::use_new(page_no, sel);
    let data = page();
    rect()
        .expanded()
        .child(ContextMenuViewer::new())
        .child(DataGrid::new(data.clone(), PageRead::Ready(data), 0, tab, find, sort).total(2))
}

fn primary_a() -> PlatformEvent {
    PlatformEvent::Keyboard {
        name: KeyboardEventName::KeyDown,
        key: Key::Character("a".into()),
        code: Code::KeyA,
        modifiers: Modifiers::META,
    }
}

/// The focused edit-chord routing (P2-11 acceptance): ⌘A does nothing while the grid is
/// unfocused; a cell press focuses the grid (and starts a rectangle); ⌘A then selects
/// every cell.
#[test]
fn cell_press_focuses_the_grid_and_cmd_a_selects_all() {
    let (mut runner, sel) = TestingRunner::new(
        app,
        (900., 700.).into(),
        |r| {
            r.provide_root_context(|| State::create(strata_core::config::Settings::default()));
            r.provide_root_context(|| State::create(Selection::None))
        },
        1.,
    );
    // Two passes: the virtual scroller builds its visible rows off the first layout.
    runner.sync_and_update();
    runner.sync_and_update();

    // Unfocused grid: the chord routes by a11y focus, so nothing happens.
    runner.send_event(primary_a());
    runner.sync_and_update();
    assert_eq!(*sel.peek(), Selection::None, "⌘A must not reach an unfocused grid");

    // Press the first body cell (toolbar 38 + header 46, first data column past the
    // 52px gutter): a single-cell rectangle, and the grid takes a11y focus.
    runner.move_cursor((100., 100.));
    runner.click_cursor((100., 100.));
    runner.sync_and_update();
    assert_eq!(*sel.peek(), Selection::Cell { ar: 0, ac: 0, fr: 0, fc: 0 });

    // Focused grid: ⌘A selects the whole page.
    runner.send_event(primary_a());
    runner.sync_and_update();
    assert_eq!(*sel.peek(), Selection::Cell { ar: 0, ac: 0, fr: 1, fc: 1 });
}

/// Right-click retargets a selection that doesn't contain the pressed cell (Excel
/// semantics) and opens the copy menu into the mounted `ContextMenuViewer` — a menu
/// row ("Copy as TSV") is findable afterwards. A right-click *inside* the selection
/// keeps it.
#[test]
fn right_click_retargets_outside_the_selection_and_opens_the_menu() {
    let (mut runner, sel) = TestingRunner::new(
        app,
        (900., 700.).into(),
        |r| {
            r.provide_root_context(|| State::create(strata_core::config::Settings::default()));
            r.provide_root_context(|| State::create(Selection::None))
        },
        1.,
    );
    runner.sync_and_update();
    runner.sync_and_update();

    let right_down = |cursor: (f64, f64)| PlatformEvent::Mouse {
        name: MouseEventName::MouseDown,
        cursor: cursor.into(),
        button: Some(MouseButton::Right),
    };

    // Select cell (0, 0), then right-click row 1 in the second column: outside the
    // selection → it retargets to that single cell.
    runner.click_cursor((100., 100.));
    runner.sync_and_update();
    runner.send_event(right_down((260., 130.)));
    runner.sync_and_update();
    assert_eq!(*sel.peek(), Selection::Cell { ar: 1, ac: 1, fr: 1, fc: 1 });

    // The copy menu is open: its TSV row exists in the tree.
    runner
        .find(|node, element| {
            Label::try_downcast(element).filter(|l| l.text == "Copy as TSV").map(|_| node)
        })
        .expect("the copy menu is open with its TSV row");

    // Right-click *inside* the selection keeps it.
    runner.send_event(right_down((260., 130.)));
    runner.sync_and_update();
    assert_eq!(*sel.peek(), Selection::Cell { ar: 1, ac: 1, fr: 1, fc: 1 });
}
