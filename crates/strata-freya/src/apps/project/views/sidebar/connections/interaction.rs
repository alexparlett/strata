//! The Connections pane driven the way a user drives it: what a row *says* about a connection,
//! and what its menu does.
//!
//! Asserted through rendered text rather than internals, because the deliverable here is the
//! diagnosis. The pane's whole job is to be the one place a refused bucket says what to fix —
//! `engine::store::connect` throws its probe's answer away precisely so this row can carry it —
//! so a test that checked the row merely *existed* would pass over the feature entirely.

use std::path::PathBuf;
use std::time::Duration;

use freya::prelude::*;
use freya::radio::RadioStation;
use freya_testing::prelude::{MouseEventName, PlatformEvent};
use freya_testing::TestingRunner;
use strata_core::project::ProjectDefs;
use strata_core::theme::load;
use strata_model::{ConnectionDef, GcsAuth, GcsStore, Provider, S3Auth, S3Store};

use super::{Connections, BODY_PAD, EMPTY_PAD};
use crate::apps::connection::ConnectionTarget;
use crate::apps::project::state::{Chan, ProjChan, ProjectState, SessionState};
use crate::apps::project::views::{ConnectionRequest, DropTarget};
use crate::components::metrics::{PANE_BODY_MIN_W, PROGRESS_HOLD};
use crate::theme::strata_theme;

fn s3(bucket: &str, region: &str, auth: S3Auth) -> ConnectionDef {
    ConnectionDef {
        address: bucket.into(),
        provider: Provider::S3(S3Store {
            region: region.into(),
            auth,
            endpoint: String::new(),
            allow_http: false,
        }),
        client_config: Default::default(),
    }
}

/// Three connections whose *providers* differ, because the row's badge and its second line are
/// both provider-shaped: an S3 one that will register, an S3 one the engine will refuse, and a
/// GCS one still mid-pass. `gs://lake` and `s3://lake` deliberately share a bucket — the pair
/// that a bucket-keyed lookup would land on the same row.
fn project() -> ProjectState {
    let defs = ProjectDefs {
        name: "test".into(),
        connections: vec![
            s3("acme-lake", "eu-west-2", S3Auth::Ambient),
            s3(
                "broken",
                "",
                S3Auth::Profile {
                    name: "analytics".into(),
                },
            ),
            ConnectionDef {
                address: "lake".into(),
                provider: Provider::Gcs(GcsStore {
                    auth: GcsAuth::Anonymous,
                }),
                client_config: Default::default(),
            },
        ],
        ..Default::default()
    };
    let mut p = ProjectState::from_defs(defs, PathBuf::from("/tmp/strata-connections-test"));
    p.connection_registered("s3://acme-lake");
    p.connection_failed("s3://broken", "This S3 connection needs a region.".into());
    p
}

/// The [`ContextMenuViewer`] is the window root's in the real app; the row menus need it in an
/// ancestor scope, and it is also what renders an open menu — so it is what makes the items
/// assertable as text.
fn app() -> impl IntoElement {
    use_init_theme(|| strata_theme(&load("midnight")));
    rect()
        .expanded()
        .child(ContextMenuViewer::new())
        .child(Connections::new())
}

type Handles = (
    RadioStation<ProjectState, ProjChan>,
    State<Option<DropTarget>>,
    ConnectionRequest,
);

/// The pane's default test width — a comfortable sidebar.
const PANEL_WIDTH: f32 = 300.;
/// `TooltipContainer`'s own hold-back before it shows (freya-components `tooltip.rs`). Restated
/// rather than imported because it is the fork's private default; if the fork changes it, the
/// popover tests below start failing rather than quietly asserting nothing.
const TOOLTIP_DELAY: Duration = Duration::from_millis(500);

/// A tall, sidebar-width window, so every row lays out at a width the pane really runs at.
fn runner(store: ProjectState) -> (TestingRunner, Handles) {
    runner_at(store, PANEL_WIDTH)
}

/// The same pane at an arbitrary panel width — what a **drag** on the sidebar's edge produces.
fn runner_at(store: ProjectState, width: f32) -> (TestingRunner, Handles) {
    TestingRunner::new(
        app,
        (width, 900.).into(),
        move |r| {
            let project =
                r.provide_root_context(|| RadioStation::<ProjectState, ProjChan>::create(store));
            let drop_target = r.provide_root_context(|| State::create(None::<DropTarget>));
            let editor = r.provide_root_context(|| State::create(None::<ConnectionTarget>));
            r.provide_root_context(|| {
                RadioStation::<SessionState, Chan>::create(SessionState::default())
            });
            (project, drop_target, editor)
        },
        1.,
    )
}

/// Settle the tree — several passes, for `interaction.rs`'s reason: Freya polls tasks only once
/// no scope is dirty, and every row mounts a ⋮ `Button`.
fn settle(runner: &mut TestingRunner) {
    for _ in 0..4 {
        runner.sync_and_update();
    }
}

fn texts(runner: &TestingRunner) -> Vec<String> {
    runner.find_many(|_, element| Label::try_downcast(element).map(|l| l.text.to_string()))
}

fn shows(runner: &TestingRunner, text: &str) -> bool {
    texts(runner).iter().any(|t| t == text)
}

fn text_area(runner: &TestingRunner, text: &str) -> Area {
    runner
        .find(|node, element| {
            Label::try_downcast(element)
                .filter(|l| l.text == text)
                .map(|_| node.layout().area)
        })
        .unwrap_or_else(|| panic!("no text run {text:?} in the tree"))
}

fn centre(area: Area) -> (f64, f64) {
    (
        (area.min_x() + area.width() / 2.) as f64,
        (area.min_y() + area.height() / 2.) as f64,
    )
}

fn click_text(runner: &mut TestingRunner, text: &str) {
    let point = centre(text_area(runner, text));
    runner.move_cursor(point);
    runner.click_cursor(point);
    settle(runner);
}

/// Right-click the row whose bucket is `bucket` — the menu's discoverable-by-habit trigger.
fn right_click_row(runner: &mut TestingRunner, bucket: &str) {
    let point = centre(text_area(runner, bucket));
    runner.move_cursor(point);
    runner.send_event(PlatformEvent::Mouse {
        name: MouseEventName::MouseDown,
        cursor: point.into(),
        button: Some(MouseButton::Right),
    });
    settle(runner);
}

/// Press the ⋮ of the row whose bucket is `bucket` — the menu's *other* trigger, found as the
/// trailing 22×22 box on that row's line.
fn press_row_actions(runner: &mut TestingRunner, bucket: &str) {
    let row = text_area(runner, bucket);
    let mid_y = row.min_y() + row.height() / 2.;
    let button = runner
        .find_many(|node, _| {
            let a = node.layout().area;
            let square = (a.width() - 22.).abs() < 0.5 && (a.height() - 22.).abs() < 0.5;
            (square && a.min_y() <= mid_y && a.max_y() >= mid_y).then_some(a)
        })
        .into_iter()
        .max_by(|a, b| a.min_x().total_cmp(&b.min_x()))
        .unwrap_or_else(|| panic!("no ⋮ button on the {bucket:?} row"));
    let point = centre(button);
    runner.move_cursor(point);
    runner.click_cursor(point);
    settle(runner);
}

/// The **status glyph** of the row whose bucket is `bucket` — the one 12×12 box on its line,
/// which is the warning triangle on a refused row and nothing at all on a clean one.
fn status_glyph(runner: &TestingRunner, bucket: &str) -> Option<Area> {
    let row = text_area(runner, bucket);
    let mid_y = row.min_y() + row.height() / 2.;
    runner
        .find_many(|node, _| {
            let a = node.layout().area;
            let glyph = (a.width() - 12.).abs() < 0.5 && (a.height() - 12.).abs() < 0.5;
            (glyph && a.min_y() <= mid_y && a.max_y() >= mid_y).then_some(a)
        })
        .into_iter()
        .next()
}

/// A **refused** connection wears a triangle, and hovering it points at Problems rather than
/// reciting the engine's reason.
///
/// This assertion is the reverse of the one it replaces, and the reversal is the finding. The
/// reason used to be spelled onto the popover here, on the argument that a sidebar-width row
/// ellipsized it to four useless words while a tooltip could give the whole sentence. A tooltip
/// cannot: it is laid out against the same narrow row, and the message that exposed it was
/// `object_store`'s — "Received redirect without LOCATION, this normally indicates an
/// incorrectly configured region" — clipped at the comma, keeping the symptom and discarding the
/// only clause naming the cause. A diagnosis cut mid-sentence is worse than none, because the
/// half that survives reads like the whole answer.
///
/// So this row says *that* the connection failed and where the words are, and the words are in
/// Problems, which wraps them and has a button that copies them.
#[test]
fn a_refused_connection_points_at_problems_rather_than_reciting_the_reason() {
    let (mut runner, ..) = runner(project());
    settle(&mut runner);

    assert!(
        !shows(&runner, "This S3 connection needs a region."),
        "the reason is not spelled into the row: {:?}",
        texts(&runner)
    );

    let triangle = status_glyph(&runner, "broken").expect("the refused row wears a triangle");
    runner.move_cursor(centre(triangle));
    runner.poll(Duration::from_millis(20), TOOLTIP_DELAY * 3);
    settle(&mut runner);

    assert!(
        shows(&runner, "Connection failed. See Problems for the reason."),
        "hovering the triangle points at the surface that can render it: {:?}",
        texts(&runner)
    );
    assert!(
        !shows(&runner, "This S3 connection needs a region."),
        "and does not carry the engine's sentence into a box that would clip it: {:?}",
        texts(&runner)
    );
}

/// A connection the engine **accepted** is clean — no glyph at all. The absence is the message,
/// the catalog entry row's rule: a mark on every row would be decoration, and the only thing
/// worth spending the slot on is the one row that needs fixing.
#[test]
fn a_settled_connection_wears_no_glyph() {
    let (mut runner, ..) = runner(project());
    settle(&mut runner);

    assert!(shows(&runner, "acme-lake"), "the row itself is listed");
    assert!(
        status_glyph(&runner, "acme-lake").is_none(),
        "a connection that registered has nothing to say"
    );
}

/// Past the hold, a wait is news in its own right: a connection still unanswered gives its slot
/// over to a spinner. Worth its own test here rather than borrowed from the catalog's, because a
/// connection is the *slowest* answer in the pass — a credential chain can reach SSO, ECS or IMDS
/// over the network — so this is the row a user is most likely to sit watching.
#[test]
fn a_connection_still_waiting_past_the_hold_spins() {
    let (mut runner, ..) = runner(project());
    settle(&mut runner);
    assert!(
        status_glyph(&runner, "lake").is_none(),
        "nothing yet, the hold is still running"
    );

    runner.poll(Duration::from_millis(20), PROGRESS_HOLD * 3);
    settle(&mut runner);

    assert!(
        status_glyph(&runner, "lake").is_some(),
        "a wait this long is worth reporting"
    );
    assert!(status_glyph(&runner, "broken").is_some());
    assert!(
        status_glyph(&runner, "acme-lake").is_none(),
        "the row that registered stays clean"
    );
}

/// A connection the pass has **not answered for yet** is clean too, until the wait outlasts
/// `PROGRESS_HOLD`. Mid-pass the store has no verdict, and a row that showed the last one would
/// make the triangle mean two different things — `reload_connections`' rule, from the side that
/// renders it.
#[test]
fn an_unanswered_connection_states_nothing_until_the_hold_expires() {
    let (mut runner, ..) = runner(project());
    settle(&mut runner);

    assert!(shows(&runner, "lake"), "the row itself is listed");
    assert!(
        status_glyph(&runner, "lake").is_none(),
        "an unanswered row makes no claim while the hold is running"
    );
}

/// Every row wears its provider as a **label**, not one shared cloud glyph (spec §1) — and the
/// label is the product's word, not the URL scheme's: GCS, never `gs`.
#[test]
fn each_row_is_badged_with_its_provider() {
    let (mut runner, ..) = runner(project());
    settle(&mut runner);

    let texts = texts(&runner);
    assert_eq!(
        texts.iter().filter(|t| *t == "S3").count(),
        2,
        "both S3 connections: {texts:?}"
    );
    assert!(texts.iter().any(|t| t == "GCS"), "{texts:?}");
    assert!(
        !texts.iter().any(|t| t == "gs"),
        "the badge is the provider's name, not its scheme: {texts:?}"
    );
}

/// **Forget sets the confirm slot and nothing else** — the dialog at the window root owns the
/// removal, the persist and the `Engine::disconnect`. Addressed by `url()`, which is what makes
/// forgetting `s3://lake` and `gs://lake` two different gestures.
#[test]
fn forget_asks_the_confirm_by_the_connections_url() {
    let (mut runner, (project, drop_target, _)) = runner(project());
    settle(&mut runner);

    right_click_row(&mut runner, "acme-lake");
    assert!(shows(&runner, "Forget connection"), "{:?}", texts(&runner));
    click_text(&mut runner, "Forget connection");

    assert_eq!(
        drop_target.peek().clone(),
        Some(DropTarget::Connection("s3://acme-lake".into())),
        "the confirm is asked for by the URL the engine registered under"
    );
    assert_eq!(
        project.peek().connections.len(),
        3,
        "and the menu item removed nothing itself"
    );
}

/// **Edit asks for the editor by the row's URL**, and does nothing else — the window is
/// `ConnectionLauncher`'s at the project root, exactly as the removal is the confirm dialog's.
///
/// The URL and not the bucket, for the reason every other lookup here uses it: `s3://lake` and
/// `gs://lake` are two connections, so a bucket-addressed edit would open one of them on the
/// other's def and its first Save would write over it.
#[test]
fn edit_asks_for_the_editor_by_the_connections_url() {
    let (mut runner, (project, _, editor)) = runner(project());
    settle(&mut runner);

    right_click_row(&mut runner, "lake");
    click_text(&mut runner, "Edit connection");

    assert_eq!(
        editor.peek().clone(),
        Some(ConnectionTarget::Edit("gs://lake".into())),
        "the editor is asked for by the URL the engine registered under"
    );
    assert_eq!(
        project.peek().connections.len(),
        3,
        "and the menu item changed nothing itself"
    );
}

/// The empty state's call to action asks for a **new** connection. It is the only entry point on
/// screen while it is up — the header's `+` is the same request, and both go through the slot.
#[test]
fn the_empty_states_cta_asks_for_a_new_connection() {
    let (mut runner, (_, _, editor)) = runner(empty_project());
    settle(&mut runner);

    click_text(&mut runner, "Add connection");

    assert_eq!(editor.peek().clone(), Some(ConnectionTarget::New));
}

/// The ⋮ opens the **same** menu the right-click does. Two triggers, one item list — the pair the
/// catalog's rows keep in step by building one `Menu`, kept in step here the same way.
#[test]
fn the_actions_button_opens_the_same_menu() {
    let (mut runner, ..) = runner(project());
    settle(&mut runner);

    press_row_actions(&mut runner, "broken");

    assert!(shows(&runner, "Edit connection"), "{:?}", texts(&runner));
    assert!(shows(&runner, "Forget connection"), "{:?}", texts(&runner));
}

/// **A drag that shrinks the sidebar squeezes the rows; it must not spill them** (AGENTS.md §3 —
/// a panel has no usability floor, only a stub floor, and a row gives up space in a stated
/// order).
///
/// The order here is the row's own: the badge and the ⋮ are fixed, and the two-line middle
/// column is the `Size::flex(1.)` run that absorbs every pixel of the squeeze and ellipsizes.
/// The failure this pins is the one a hugging child produces in a `Content::Flex` row — it keeps
/// its size while its flexing sibling shrinks, and the trailing run is pushed past the panel
/// edge, where a control is invisible however correct the element tree is. That is exactly how
/// the catalog header's ↻ shipped unreachable, so it is asserted on geometry rather than on
/// which element is which.
///
/// Measured at [`PANE_BODY_MIN_W`] plus the body's own inset, which is the narrowest the pane
/// lays out at before the floor takes over and the **panel** clips the remainder — below that,
/// content wider than the panel is the stub policy working, not a fault.
///
#[test]
fn a_drag_that_shrinks_the_pane_squeezes_the_rows_rather_than_spilling_them() {
    let width = PANE_BODY_MIN_W + BODY_PAD.left() + BODY_PAD.right();
    let (mut runner, ..) = runner_at(project(), width);
    settle(&mut runner);

    let overflowing: Vec<(f32, f32)> = runner
        .find_many(|node, _| {
            let a = node.layout().area;
            (a.width() > 0. && a.max_x() > width + 0.5).then(|| (a.min_x(), a.max_x()))
        })
        .into_iter()
        .collect();
    assert!(
        overflowing.is_empty(),
        "laid out past the {width}px panel edge: {overflowing:?}"
    );

    let mut lines: Vec<i32> = runner
        .find_many(|node, _| {
            let a = node.layout().area;
            ((a.width() - 22.).abs() < 0.5 && (a.height() - 22.).abs() < 0.5)
                .then(|| a.min_y().round() as i32)
        })
        .into_iter()
        .collect();
    lines.sort_unstable();
    lines.dedup();
    assert_eq!(lines.len(), 3, "one ⋮ per connection, still laid out");
}

/// A project with no connections at all.
fn empty_project() -> ProjectState {
    ProjectState::from_defs(
        ProjectDefs {
            name: "test".into(),
            ..Default::default()
        },
        PathBuf::from("/tmp/strata-connections-empty"),
    )
}

/// No connections is not a fault, so the empty state says what one is *for*.
#[test]
fn an_empty_project_explains_what_a_connection_is_for() {
    let (mut runner, ..) = runner(empty_project());
    settle(&mut runner);

    assert!(
        texts(&runner)
            .iter()
            .any(|t| t.starts_with("No connections yet.")),
        "{:?}",
        texts(&runner)
    );
    assert!(shows(&runner, "Add connection"));
}

/// **The empty state's sentence must not wrap to one letter per line** when the drag takes the
/// sidebar down to its stub (P5-06).
///
/// The pane returns this branch *instead of* its scrolling body, so it carries the body's
/// [`PANE_BODY_MIN_W`] floor itself — miss that and the copy gets the panel's own width less
/// [`EMPTY_PAD`], which at the stub is nothing, and a paragraph becomes a column of single
/// characters. The floor is what makes the panel clip the sentence instead of shredding it,
/// which is the stub policy working: the way out of a squeezed panel is the drag, not the copy.
///
/// Asserted on the text box's laid-out width rather than on a line count, because the width *is*
/// the mechanism — a wrapped run's height is a consequence of it, and one that moves with the
/// type scale.
///
#[test]
fn the_empty_state_keeps_its_floor_when_the_drag_reaches_the_stub() {
    const PANEL_STUB_W: f32 = 48.;
    const WRAP_SLACK: f32 = 12.;
    let floor = PANE_BODY_MIN_W - EMPTY_PAD.left() - EMPTY_PAD.right();

    let (mut runner, ..) = runner_at(empty_project(), PANEL_STUB_W);
    settle(&mut runner);

    let copy = runner
        .find(|node, element| {
            Label::try_downcast(element)
                .filter(|l| l.text.starts_with("No connections yet."))
                .map(|_| node.layout().area)
        })
        .expect("the empty state's copy");

    assert!(
        copy.width() >= floor - WRAP_SLACK,
        "the copy laid out at {}px inside a {PANEL_STUB_W}px panel, under the {floor}px floor — \
         at the stub it wraps to one letter per line",
        copy.width()
    );
}
