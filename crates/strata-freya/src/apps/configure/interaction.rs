//! The Configure window's **LOCATION** arm driven the way a user drives it (W7 · 04): the toggle,
//! the connection picker, and what Save then writes.
//!
//! The window **root** is not mounted — it needs the app-globals, a menubar scope and an owner
//! window id, none of which say anything about the form. What is mounted is the pair that does:
//! the body and the footer, over the same contexts the real window provides them. The connection
//! editor's own interaction test is shaped this way for the same reason.
//!
//! Asserted through rendered text and the store, because that is the deliverable: a form that
//! says which bucket a path is written against, a Save that is refused while it cannot say, and a
//! def that carries the connection rather than a path composed into it.
//!
//! **What is not driven here is a `Select`'s own menu.** Freya's `Select` closes itself when the
//! focused accessibility node is not its own, and a press in the testing runner never lands that
//! focus — so the list is opened and shut inside one update and its items are never in the tree.
//! That is the harness, not the picker (nothing in this window differs from the format picker
//! beside it), so what a menu item does — choosing a connection, and *New connection…* raising
//! the project window's request — is left to the model tests and to the one-line handlers, rather
//! than asserted through a control that cannot be opened from here.

use std::path::{Path, PathBuf};

use freya::prelude::*;
use freya::radio::RadioStation;
use freya_testing::TestingRunner;
use strata_core::project::ProjectDefs;
use strata_core::theme::load;
use strata_model::{ConnectionDef, Provider, ProviderId, S3Store};

use super::views::{ConfigureBody, Footer};
use super::{ConfigureCtx, ConfigureDraft, ConfigureTarget, Status};
use crate::apps::connection::ConnectionTarget;
use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::{
    CatalogState, ConnectionRequest, Log, PersistFaults, ProjChan, ProjectState, ScanRequest,
};
use crate::theme::strata_theme;

/// A scratch project folder for one test — `env::temp_dir()` + pid, the convention every test
/// that really writes `.strata/project.json` follows.
fn temp_root(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("strata-configure-{tag}-{}", std::process::id()))
}

/// A project with one S3 connection, or none at all — the two states the picker has to tell
/// apart, since only one of them has anything to offer.
fn project(root: &Path, connected: bool) -> ProjectState {
    let defs = ProjectDefs {
        name: "test".into(),
        connections: match connected {
            false => Vec::new(),
            true => vec![ConnectionDef {
                address: "acme-lake".into(),
                provider: Provider::S3(S3Store {
                    region: "eu-west-2".into(),
                    ..Default::default()
                }),
                client_config: Default::default(),
            }],
        },
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
        .child(ConfigureBody)
        .child(Footer)
}

type Handles = (
    ConfigureCtx,
    RadioStation<ProjectState, ProjChan>,
    ConnectionRequest,
);

/// The window's body and footer over `draft`, against its own scratch project.
fn runner(tag: &'static str, connected: bool, draft: ConfigureDraft) -> (TestingRunner, Handles) {
    let root = temp_root(tag);
    TestingRunner::new(
        app,
        (620., 900.).into(),
        move |r| {
            // A real engine, asked nothing: there is no scan *driver* here — that lives at the
            // project window's root — so Save raises a request and the row stays `Loading`.
            r.provide_root_context(EngineCtx::default);
            r.provide_root_context(|| State::create(CatalogState::Settled(0)));
            r.provide_root_context(|| State::create(ScanRequest::default()));
            r.provide_root_context(|| State::create(Log::default()));
            r.provide_root_context(|| State::create(PersistFaults::default()));
            let project = r.provide_root_context(|| {
                RadioStation::<ProjectState, ProjChan>::create(project(&root, connected))
            });
            // The project window's connection-editor slot: this window sets it and stops.
            let connections: ConnectionRequest =
                r.provide_root_context(|| State::create(None::<ConnectionTarget>));
            let ctx = r.provide_root_context(|| ConfigureCtx {
                draft: State::create(draft.clone()),
                target: State::create(ConfigureTarget::New),
                status: State::create(Status::Idle),
                selected_path: State::create(0),
            });
            (ctx, project, connections)
        },
        1.,
    )
}

/// Settle the tree — several passes, because the fields mount buffers that report on their own
/// first effect.
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

fn draft(source: &str) -> ConfigureDraft {
    ConfigureDraft {
        name: "events".into(),
        sources: vec![source.into()],
        ..Default::default()
    }
}

/// **A project with no connections for the chosen provider says so, and Save is refused** — the
/// two halves of the same fact, in the two places the user is looking: under the picker, and in
/// the footer beside the button it disabled.
#[test]
fn remote_with_no_connection_explains_itself_and_blocks_save() {
    let (mut runner, (ctx, ..)) = runner("empty", false, draft("events/"));
    settle(&mut runner);

    click_lowest(&mut runner, "Remote");

    assert!(ctx.draft.peek().remote);
    assert_eq!(ctx.draft.peek().connection, None, "there is none to pick");
    assert!(
        shows(&runner, "No S3 connections yet. Add one to continue."),
        "the picker says why it is empty: {:?}",
        texts(&runner)
    );
    assert!(
        shows(
            &runner,
            "A remote table needs a connection to read through."
        ),
        "and the footer says why Save is off: {:?}",
        texts(&runner)
    );

    // The other providers are reachable from here, and each answers for itself.
    click_lowest(&mut runner, "GCS");
    assert!(shows(
        &runner,
        "No GCS connections yet. Add one to continue."
    ));
}

/// **The headline**: flipping to the object store picks the project's connection, the path box
/// wears that bucket, and Save writes a def naming the connection with the path left
/// bucket-relative — which is what `register::table_spec` composes back into an address.
#[test]
fn a_table_saved_over_a_connection_carries_the_url_and_a_relative_path() {
    let (mut runner, (ctx, project, _)) = runner("save", true, draft("events/2024/"));
    settle(&mut runner);

    click_lowest(&mut runner, "Remote");

    assert_eq!(
        ctx.draft.peek().connection.as_deref(),
        Some("s3://acme-lake"),
        "the provider's first connection is picked for you"
    );
    assert!(
        shows(&runner, "SOURCE PATH"),
        "one path, said in the singular: {:?}",
        texts(&runner)
    );
    assert!(
        shows(&runner, "s3://acme-lake/"),
        "the box wears the bucket its path is written against: {:?}",
        texts(&runner)
    );

    click_lowest(&mut runner, "Save");

    let store = project.peek();
    let def = &store
        .tables
        .iter()
        .find(|t| t.def.name == "events")
        .expect("the table was written")
        .def;
    assert_eq!(def.connection.as_deref(), Some("s3://acme-lake"));
    assert_eq!(
        def.sources,
        ["events/2024/"],
        "stored as typed — relativizing it against the project folder would mangle it"
    );
}

/// **And back again.** The flip to Local is the direction that *shortens* the form and returns
/// the source list to its multi-path shape, so it is the one that exercises a section coming down
/// rather than going up — and the one nothing else drives.
///
/// What it asserts is that the whole surface returns: the TYPE / CONNECTION pair is gone, the
/// label is plural again, the bucket prefix is off the box, and the connection is *remembered*
/// rather than discarded, which is what makes a flip back and forth free.
#[test]
fn flipping_back_to_local_returns_the_multi_path_list_and_keeps_the_choice() {
    let (mut runner, (ctx, ..)) = runner("back", true, draft("events/2024/"));
    settle(&mut runner);

    click_lowest(&mut runner, "Remote");
    assert!(shows(&runner, "CONNECTION"), "{:?}", texts(&runner));

    click_lowest(&mut runner, "Local");

    assert!(!ctx.draft.peek().remote);
    assert_eq!(
        ctx.draft.peek().connection.as_deref(),
        Some("s3://acme-lake"),
        "the choice is remembered, so coming back does not ask again"
    );
    assert_eq!(
        ctx.draft.peek().store(),
        None,
        "but it is not this table's location any more"
    );
    let texts = texts(&runner);
    assert!(
        !texts.iter().any(|t| t == "CONNECTION" || t == "TYPE"),
        "the store row is gone: {texts:?}"
    );
    assert!(
        texts.iter().any(|t| t == "SOURCE PATHS"),
        "and the label is plural again: {texts:?}"
    );
    assert!(
        !texts.iter().any(|t| t == "s3://acme-lake/"),
        "with no bucket in front of the box: {texts:?}"
    );
}

/// A def over a connection **this project no longer has** keeps naming it, and the footer says
/// so rather than letting the reference be saved again — the treatment a format with no reader
/// gets, for the same reason.
#[test]
fn a_forgotten_connection_is_named_and_blocks_save() {
    let mut draft = draft("events/");
    draft.remote = true;
    draft.provider = ProviderId::S3;
    draft.connection = Some("s3://gone".into());
    let (mut runner, _) = runner("forgotten", true, draft);
    settle(&mut runner);

    assert!(
        shows(
            &runner,
            "'s3://gone' is not a connection in this project. Choose one, or add it back."
        ),
        "{:?}",
        texts(&runner)
    );
}
