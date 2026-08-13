//! The connection editor driven the way a user drives it: which controls a provider has, and
//! what Save actually writes.
//!
//! The window **root** is not mounted — it needs the app-globals, a menubar scope and an owner
//! window id, none of which say anything about the editor. What is mounted is the pair that does:
//! the fields and the footer, over the same contexts the real window provides them.
//!
//! Asserted through rendered text and the store, because that is the deliverable: a form whose
//! controls match the provider, and a Save that writes one def, deregisters the one it moved off,
//! and then waits rather than claiming success.

use std::path::{Path, PathBuf};

use freya::prelude::*;
use freya::radio::RadioStation;
use freya_testing::TestingRunner;
use strata_core::project::ProjectDefs;
use strata_core::theme::load;
use strata_model::{ConnectionDef, Provider, ProviderId, S3Store};

use super::views::{ConnectionBody, Footer, OPTION_KEY_WIDTH};
use super::{ConnectionCtx, ConnectionDraft, ConnectionTarget, Status};
use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::{CatalogState, Log, PersistFaults, ProjChan, ProjectState, ScanRequest};
use crate::theme::strata_theme;

/// A scratch project folder for one test — `env::temp_dir()` + pid, the convention every test
/// that really writes `.strata/project.json` follows, because the OS temp dir is machine-shared
/// and a hardcoded path collides between parallel test binaries.
fn temp_root(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("strata-connection-{tag}-{}", std::process::id()))
}

/// One S3 connection to edit, so a moved identity has something to move off.
fn project(root: &Path) -> ProjectState {
    let defs = ProjectDefs {
        name: "test".into(),
        connections: vec![ConnectionDef {
            address: "old-lake".into(),
            provider: Provider::S3(S3Store {
                region: "eu-west-2".into(),
                ..Default::default()
            }),
            client_config: Default::default(),
        }],
        ..Default::default()
    };
    ProjectState::from_defs(defs, root.to_path_buf())
}

fn app() -> impl IntoElement {
    use_init_theme(|| strata_theme(&load("midnight")));
    rect()
        .expanded()
        .vertical()
        .content(Content::Flex)
        .child(ConnectionBody)
        .child(Footer)
}

type Handles = (
    ConnectionCtx,
    RadioStation<ProjectState, ProjChan>,
    State<ScanRequest>,
);

/// The editor over `draft`, opened on `target`, against its own scratch project.
fn runner(
    tag: &'static str,
    target: ConnectionTarget,
    draft: ConnectionDraft,
) -> (TestingRunner, Handles) {
    let root = temp_root(tag);
    TestingRunner::new(
        app,
        (480., 900.).into(),
        move |r| {
            r.provide_root_context(EngineCtx::default);
            r.provide_root_context(|| State::create(CatalogState::Settled(0)));
            let rescan = r.provide_root_context(|| State::create(ScanRequest::default()));
            r.provide_root_context(|| State::create(Log::default()));
            r.provide_root_context(|| State::create(PersistFaults::default()));
            let project = r.provide_root_context(|| {
                RadioStation::<ProjectState, ProjChan>::create(project(&root))
            });
            let ctx = r.provide_root_context(|| ConnectionCtx {
                draft: State::create(draft.clone()),
                target: State::create(target.clone()),
                status: State::create(Status::Idle),
                profiles: State::create(Some(Vec::new())),
                selected_option: State::create(None),
            });
            (ctx, project, rescan)
        },
        1.,
    )
}

/// Settle the tree — several passes, because the fields mount buffers that report on their own
/// first effect and the authority box echoes itself once.
fn settle(runner: &mut TestingRunner) {
    for _ in 0..6 {
        runner.sync_and_update();
    }
}

fn texts(runner: &TestingRunner) -> Vec<String> {
    runner.find_many(|_, element| Label::try_downcast(element).map(|l| l.text.to_string()))
}

fn shows(runner: &TestingRunner, text: &str) -> bool {
    texts(runner).iter().any(|t| t == text)
}

/// The laid-out box of the text run `text` — for the assertions that are about *geometry* rather
/// than about which element is in the tree.
fn text_area(runner: &TestingRunner, text: &str) -> Area {
    runner
        .find(|node, element| {
            Label::try_downcast(element)
                .filter(|l| l.text == text)
                .map(|_| node.layout().area)
        })
        .unwrap_or_else(|| panic!("no text run {text:?} in the tree: {:?}", texts(runner)))
}

/// The laid-out box of the **editable** text `text` — an `Input`'s content is a `paragraph`, not
/// a `label`, so [`text_area`] cannot see the client-option boxes at all.
fn field_area(runner: &TestingRunner, text: &str) -> Area {
    runner
        .find(|node, element| {
            Paragraph::try_downcast(element)
                .filter(|p| p.spans.iter().any(|span| span.text.as_ref() == text))
                .map(|_| node.layout().area)
        })
        .unwrap_or_else(|| panic!("no editable text {text:?} in the tree"))
}

/// The centre of a laid-out box, in the coordinates the runner's cursor takes.
fn centre(area: Area) -> (f64, f64) {
    (
        (area.min_x() + area.width() / 2.) as f64,
        (area.min_y() + area.height() / 2.) as f64,
    )
}

/// Press the **lowest** run of `text` — the footer's buttons sit under everything, and a label in
/// the body can carry the same word.
fn click_lowest(runner: &mut TestingRunner, text: &str) {
    let area = runner
        .find_many(|node, element| {
            Label::try_downcast(element)
                .filter(|l| l.text == text)
                .map(|_| node.layout().area)
        })
        .into_iter()
        .max_by(|a, b| a.min_y().total_cmp(&b.min_y()))
        .unwrap_or_else(|| panic!("no text run {text:?} in the tree: {:?}", texts(runner)));
    let point = (
        (area.min_x() + area.width() / 2.) as f64,
        (area.min_y() + area.height() / 2.) as f64,
    );
    runner.move_cursor(point);
    runner.click_cursor(point);
    settle(runner);
}

fn s3_draft() -> ConnectionDraft {
    ConnectionDraft {
        address: "acme-lake".into(),
        region: "eu-west-2".into(),
        ..Default::default()
    }
}

/// **Which controls exist is the provider's answer, and switching it re-asks.** S3 has a region
/// and an endpoint; HTTP has neither, and no auth either — it is anonymous by construction, so a
/// disabled auth pill would be a control that can never mean anything.
///
/// The scheme chip follows the same rule: shown for HTTP, where `http` and `https` would
/// otherwise be a guess, and hidden for S3 and GCS, where the picker directly above states it.
#[test]
fn a_providers_controls_are_the_ones_that_provider_has() {
    let (mut runner, (ctx, ..)) = runner("providers", ConnectionTarget::New, s3_draft());
    settle(&mut runner);

    assert!(shows(&runner, "PROVIDER"), "{:?}", texts(&runner));
    assert!(shows(&runner, "BUCKET"));
    assert!(shows(&runner, "AUTHENTICATION"));
    assert!(shows(&runner, "REGION"));
    assert!(shows(&runner, "ENDPOINT"));
    assert!(shows(&runner, "BUCKET"), "S3 names a bucket, not a URL");

    click_lowest(&mut runner, "HTTP");

    assert_eq!(ctx.draft.peek().provider, ProviderId::Http);
    assert!(shows(&runner, "URL"), "{:?}", texts(&runner));
    assert!(
        !shows(&runner, "AUTHENTICATION"),
        "HTTP is always anonymous"
    );
    assert!(!shows(&runner, "REGION"));
    assert!(!shows(&runner, "ENDPOINT"));
    assert!(!shows(&runner, "BUCKET"));
    assert!(
        shows(
            &runner,
            "An HTTP connection needs a scheme: write 'https://aserver' or 'http://aserver'."
        ),
        "{:?}",
        texts(&runner)
    );

    click_lowest(&mut runner, "S3");
    assert_eq!(ctx.draft.peek().region, "eu-west-2");
    assert!(shows(&runner, "REGION"));
}

/// **Save is off, and the footer says why, in the same breath.** The two are one value: this used
/// to be the failure mode worth guarding, where a button was disabled by one expression and
/// explained by another.
#[test]
fn a_draft_that_cannot_be_saved_says_so_beside_the_button() {
    let draft = ConnectionDraft {
        region: String::new(),
        ..s3_draft()
    };
    let (mut runner, (ctx, project, rescan)) = runner("blocked", ConnectionTarget::New, draft);
    settle(&mut runner);

    assert!(
        shows(
            &runner,
            "An S3 connection needs a region. It can't be auto-detected."
        ),
        "{:?}",
        texts(&runner)
    );

    click_lowest(&mut runner, "Save");

    assert_eq!(project.peek().connections.len(), 1, "nothing was written");
    assert_eq!(*ctx.status.peek(), Status::Idle);
    assert_eq!(rescan.peek().seq, 0, "and no pass was asked for");
}

/// **Save writes the def, asks for the pass, and then waits.** It does not claim success: the
/// window settles on the row the pass answers, so the status here is `Connecting` and the store's
/// row is back to `Loading`.
#[test]
fn saving_writes_the_def_and_waits_for_the_pass() {
    let (mut runner, (ctx, project, rescan)) = runner("save", ConnectionTarget::New, s3_draft());
    settle(&mut runner);

    click_lowest(&mut runner, "Save");

    let urls: Vec<String> = project
        .peek()
        .connections
        .iter()
        .map(|c| c.def.url())
        .collect();
    assert_eq!(urls, ["acme-lake", "old-lake"].map(|b| format!("s3://{b}")));
    assert_eq!(
        *ctx.status.peek(),
        Status::Connecting("s3://acme-lake".into()),
        "the window is waiting on its own row, not claiming it connected"
    );
    assert_eq!(rescan.peek().seq, 1, "one pass asked for");
    assert_eq!(
        *ctx.target.peek(),
        ConnectionTarget::Edit("s3://acme-lake".into())
    );
}

/// **An edit that moves the bucket moves the connection's identity**, so the row it moved off has
/// to go — otherwise the project keeps a def under the old URL and the pass registers both.
#[test]
fn an_edit_that_moves_the_bucket_leaves_no_row_behind() {
    let draft = ConnectionDraft {
        address: "new-lake".into(),
        region: "eu-west-2".into(),
        ..Default::default()
    };
    let (mut runner, (_, project, _)) = runner(
        "moved",
        ConnectionTarget::Edit("s3://old-lake".into()),
        draft,
    );
    settle(&mut runner);

    click_lowest(&mut runner, "Save");

    let urls: Vec<String> = project
        .peek()
        .connections
        .iter()
        .map(|c| c.def.url())
        .collect();
    assert_eq!(urls, ["s3://new-lake"], "the old URL's row is gone");
}

/// **The client-options header stands at the split it declares, empty or not.**
///
/// This is the bug a first attempt at the test missed, and the miss is worth recording. The
/// empty-state `Table` carried no `column_widths`, so `TableRow` fell back to an equal share per
/// cell — and because `Table` hands its config down through a plain `provide_context` that
/// `use_try_consume` reads **once per render**, the header did not re-read it when rows arrived and
/// changed the Table's props. So the strip laid out 50/50 in the empty state and *stayed* 50/50
/// over rows that were 210px/flex.
///
/// A test comparing the two states therefore passed on the broken code: they agreed, at the wrong
/// number. What has to be asserted is the **declared** split, which is why this reads
/// `OPTION_KEY_WIDTH` rather than the other state.
///
/// The fork no longer goes stale either — `TableConfigContext` is a `Readable`, so a row re-reads a
/// split that changes under it (`freya-components/src/table.rs`). This still declares the same
/// widths in both branches, because the two fixes answer different halves: the fork's makes a
/// *change* propagate, and this one means there is no change to propagate, so the header does not
/// move even for the frame the first row lands in.
///
/// On laid-out geometry, because that *is* the bug — the element tree was right in both states.
#[test]
fn the_client_options_header_stands_at_the_split_it_declares() {
    let (mut runner, (ctx, ..)) = runner("options-header", ConnectionTarget::New, s3_draft());
    settle(&mut runner);

    let split = |runner: &TestingRunner| {
        let option = text_area(runner, "Option");
        let value = text_area(runner, "Value");
        (value.min_x() - option.min_x()).round()
    };

    assert!(
        shows(
            &runner,
            "No client options. The defaults suit most connections."
        ),
        "{:?}",
        texts(&runner)
    );
    assert_eq!(
        split(&runner),
        OPTION_KEY_WIDTH,
        "empty, and already correct"
    );

    ctx.edit(|draft| {
        draft.client_config.add("timeout".into(), "30s".into());
    });
    settle(&mut runner);
    assert!(!shows(
        &runner,
        "No client options. The defaults suit most connections."
    ));

    assert_eq!(
        split(&runner),
        OPTION_KEY_WIDTH,
        "and unmoved once a row exists, at the split it declares"
    );
}

/// **Clicking into either box of a client-option row selects that row**, because the toolbar acts
/// on the selection: a value typed into a row the highlight never moved to is a Remove aimed at
/// the wrong one.
///
/// It has to be the *field's* focus that does it, not the row's press. `Input` stops propagation
/// on its focus press (`on_input_focus_press`), so a click that lands in a box never reaches
/// `TableRow::on_press` — which is exactly how the value box lost its selection when the row's one
/// a11y id moved to the name box for the suggestion panel.
#[test]
fn clicking_into_either_box_of_an_option_row_selects_it() {
    let (mut runner, (ctx, ..)) = runner("options-select", ConnectionTarget::New, s3_draft());
    settle(&mut runner);

    ctx.edit(|draft| {
        draft.client_config.add("timeout".into(), "30s".into());
        draft
            .client_config
            .add("user_agent".into(), "strata".into());
    });
    settle(&mut runner);
    let ids: Vec<u64> = ctx
        .draft
        .peek()
        .client_config
        .rows()
        .iter()
        .map(|row| row.id)
        .collect();
    assert_eq!(ids.len(), 2);

    let mut slot = ctx.selected_option;
    slot.set(None);
    settle(&mut runner);

    let point = centre(field_area(&runner, "strata"));
    runner.move_cursor(point);
    runner.click_cursor(point);
    settle(&mut runner);
    assert_eq!(
        *ctx.selected_option.peek(),
        Some(ids[1]),
        "clicking the value box selected its own row"
    );

    let point = centre(field_area(&runner, "timeout"));
    runner.move_cursor(point);
    runner.click_cursor(point);
    settle(&mut runner);
    assert_eq!(*ctx.selected_option.peek(), Some(ids[0]));
}

/// A URL another connection already holds is refused, because `upsert_connection` replaces on it
/// — so without this the save would silently take that connection's def out from under it. The
/// def's *own* URL never clashes with itself.
#[test]
fn a_url_another_connection_holds_blocks_the_save() {
    let draft = ConnectionDraft {
        address: "old-lake".into(),
        region: "eu-west-2".into(),
        ..Default::default()
    };
    let (mut runner, (_, project, _)) = runner("clash", ConnectionTarget::New, draft);
    settle(&mut runner);

    assert!(
        shows(
            &runner,
            "'s3://old-lake' is already a connection in this project."
        ),
        "{:?}",
        texts(&runner)
    );

    click_lowest(&mut runner, "Save");
    assert_eq!(
        project.peek().connections.len(),
        1,
        "and nothing was written"
    );
}
