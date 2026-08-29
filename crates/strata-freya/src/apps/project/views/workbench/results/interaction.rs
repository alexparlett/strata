//! Results-pane interaction tests — the pane driven the way the user drives it, over a real
//! engine and the real config store.
//!
//! **A display-format change updates the whole visible grid.** The two halves of that hold for
//! different reasons, which is why one test walks both: a page after the first re-keys, because
//! the stamp is in [`PageSpec`], while page 1 is the Run's own rows under a nonce a settings
//! change cannot move. Neither may re-execute the query, so the test also asserts the nonce is
//! the one the tab pressed — cells that refreshed by re-running would be over different data.

use std::sync::Arc;
use std::time::Duration;

use freya::radio::RadioStation;
use freya_testing::prelude::{Key, KeyboardEventName, NamedKey, PlatformEvent};
use freya_testing::TestingRunner;
use strata_core::config::AppConfig;
use strata_core::models::Listings;
use strata_core::theme::load;
use strata_model::Origin;

use super::*;
use crate::agent::create_global_agent;
use crate::apps::project::close::{CloseGuard, CloseTarget};
use crate::apps::project::query::{QueryMode, RunId};
use crate::apps::project::state::{
    use_engine_config, Chats, EngineRestart, Log, PersistFaults, Pick,
};
use crate::menu::create_global_menu;
use crate::platform::{create_global_open, create_global_windows, Subtree};
use crate::state::{
    create_global_theme_preview, create_global_updates, ConfigStation, ModelListings, Probes,
};
use crate::theme::{strata_theme, ThemesCtx};
use crate::updater::create_global_update_request;

/// Three rows of one NULL each, two to a page: `datafusion.format.null` is visible in the grid,
/// and there is a page 2 to walk to.
const SQL: &str =
    "SELECT * FROM (VALUES (1, CAST(NULL AS INT)), (2, CAST(NULL AS INT)), (3, CAST(NULL AS INT))) AS t(i, n)";

const PAGE_SIZE: usize = 2;

/// The pane as the workbench mounts it, under the window root's own engine-config driver.
/// Without that hook the config store would move and the engine would not, which is a different
/// situation from the one under test.
fn app() -> impl IntoElement {
    use_init_theme(|| strata_theme(&load("midnight")));
    let engine = use_consume::<EngineCtx>();
    let confirm = use_state(|| None::<CloseTarget>);
    use_engine_config(&engine, confirm);

    let session = use_radio::<SessionState, Chan>(Chan::Tabs);
    let tab = session.read().active.expect("the fixture opened one");
    let running = use_state(|| None::<RunId>);
    rect()
        .expanded()
        .child(ContextMenuViewer::new())
        .child(Results::new(tab, running))
}

/// A window's worth of root contexts, the fixture's tab already holding a Run press.
fn runner() -> (
    TestingRunner,
    RadioStation<SessionState, Chan>,
    ConfigStation,
) {
    let (mut runner, (session, config)) = TestingRunner::new(
        app,
        (1000., 700.).into(),
        |r| {
            let session = r.provide_root_context(|| {
                RadioStation::<SessionState, Chan>::create({
                    let mut s = SessionState::default();
                    let tab = s.open_named("fixture", SQL.into(), Origin::Scratch);
                    s.set_request(
                        tab,
                        QuerySpec {
                            tab,
                            run: RunId::new(),
                            sql: SQL.into(),
                            mode: QueryMode::Run,
                            page_size: PAGE_SIZE,
                        },
                    );
                    s
                })
            });
            let config = r.provide_root_context(|| ConfigStation::create(AppConfig::default()));
            r.provide_root_context(EngineCtx::default);
            r.provide_root_context(|| Arc::new(CloseGuard::new(false, true)));
            r.provide_root_context(|| EngineRestart(State::create(0)));
            r.provide_root_context(|| Subtree {
                project: "fixture".into(),
                generation: 0,
                restart: EngineRestart(State::create(0)),
            });
            r.provide_root_context(|| State::create(Log::default()));
            r.provide_root_context(|| State::create(PersistFaults::default()));
            r.provide_root_context(|| State::create(Chats::new(Pick::default())));
            r.provide_root_context(|| State::create(None::<ShapeTarget>));
            let listings: ModelListings =
                r.provide_root_context(|| State::create_global(Listings::default()));
            let probes = r.provide_root_context(|| State::create_global(Probes::default()));
            r.provide_root_context(move || AppCtx {
                themes: ThemesCtx::discover(),
                config,
                windows: create_global_windows(),
                preview: create_global_theme_preview(),
                menu: create_global_menu(),
                open: create_global_open(),
                agent: create_global_agent(),
                listings,
                probes,
                assistant: None,
                updates: create_global_updates(),
                update_request: create_global_update_request(),
            });
            (session, config)
        },
        1.,
    );
    settle(&mut runner);
    (runner, session, config)
}

/// Enough passes for a dispatched read to settle and its rows to reach the tree. The engine runs
/// on a runtime of its own, so this has to wait as well as re-render.
fn settle(runner: &mut TestingRunner) {
    runner.poll_n(Duration::from_millis(15), 20);
}

/// A NULL rendering nothing else in the tree can be mistaken for.
const VOID: &str = "(nothing)";

fn texts(runner: &TestingRunner) -> Vec<String> {
    runner.find_many(|_, element| Label::try_downcast(element).map(|l| l.text.to_string()))
}

/// Cells still rendering NULL the built-in way — the one thing the override moves.
fn nulls(runner: &TestingRunner) -> usize {
    texts(runner).iter().filter(|t| *t == "NULL").count()
}

/// Cells rendering it the overridden way.
fn voids(runner: &TestingRunner) -> usize {
    texts(runner).iter().filter(|t| *t == VOID).count()
}

/// Backspaces enough to empty the page box whatever it holds — the fixture's page count is one
/// digit, with room to spare.
const PAGE_BOX_DIGITS: usize = 3;

/// Jump the pager to `page` through its own box, the one page control addressable without
/// measuring an icon: the arrows are icon buttons whose only name is a tooltip, which is not in
/// the tree until it is hovered. The box is the pane's only editable text, so the click finds it
/// as the only `Paragraph` on screen.
///
/// End before clearing: the field is centre-aligned and one glyph wide, so where a click leaves
/// the caret is a question about the text run's box rather than about the number in it.
fn go_to_page(runner: &mut TestingRunner, page: usize) {
    let area = runner
        .find(|node, element| Paragraph::try_downcast(element).map(|_| node.layout().area))
        .expect("the pager's page box is on screen");
    let point = (
        f64::from(area.min_x() + area.width() / 2.),
        f64::from(area.min_y() + area.height() / 2.),
    );
    runner.move_cursor(point);
    runner.click_cursor(point);
    runner.sync_and_update();
    key(runner, NamedKey::End, Code::End);
    for _ in 0..PAGE_BOX_DIGITS {
        key(runner, NamedKey::Backspace, Code::Backspace);
    }
    runner.write_text(page.to_string());
    runner.sync_and_update();
    key(runner, NamedKey::Enter, Code::Enter);
    settle(runner);
}

fn key(runner: &mut TestingRunner, key: NamedKey, code: Code) {
    runner.send_event(PlatformEvent::Keyboard {
        name: KeyboardEventName::KeyDown,
        key: Key::Named(key),
        code,
        modifiers: Modifiers::empty(),
    });
    runner.sync_and_update();
}

/// The press the tab is holding.
fn nonce(session: &RadioStation<SessionState, Chan>) -> RunId {
    session
        .peek()
        .active
        .and_then(|tab| session.peek().request(tab).map(|spec| spec.run))
        .expect("the fixture's press")
}

/// Change `datafusion.format.null` on the app config, which is what Settings ▸ Engine ▸
/// Properties' Apply does.
fn set_null_rendering(runner: &mut TestingRunner, station: &ConfigStation, to: &str) {
    let mut config = *station;
    config
        .write_channel(ConfigChan::Settings)
        .settings
        .engine
        .insert("datafusion.format.null".into(), to.into());
    settle(runner);
}

/// **Both pages follow the format, and neither re-runs.** Page 2 because the stamp is in its
/// read's key; page 1 because the pane stops serving the Run's own rows once the stamp they were
/// rendered under has been left behind.
#[test]
fn a_format_change_re_renders_page_1_and_page_2_without_re_running() {
    let (mut runner, session, station) = runner();
    let pressed = nonce(&session);

    assert_eq!(nulls(&runner), 2, "page 1, two rows, under the default");

    go_to_page(&mut runner, 2);
    assert_eq!(
        nulls(&runner),
        1,
        "page 2, one row, still under the default"
    );

    set_null_rendering(&mut runner, &station, VOID);
    assert_eq!(voids(&runner), 1, "the page read re-keyed on the stamp");
    assert_eq!(nulls(&runner), 0);

    go_to_page(&mut runner, 1);
    assert_eq!(
        voids(&runner),
        2,
        "and page 1 followed, though its Run entry could not have"
    );
    assert_eq!(nulls(&runner), 0, "no cell is left rendering the old way");

    assert_eq!(
        nonce(&session),
        pressed,
        "every re-render was a read of the same settled snapshot"
    );
}
