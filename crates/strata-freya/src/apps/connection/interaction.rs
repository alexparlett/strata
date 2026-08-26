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
use strata_engine::sources::postgres::Pg;
use strata_engine::SourceKind;
use strata_model::{ConnectionDef, Provider, ProviderId, S3Store};

use super::model::PgDraft;

use super::views::{ConnectionBody, Footer, OPTION_KEY_WIDTH};
use super::{ConnectionCtx, ConnectionDraft, ConnectionTarget, PasswordProbe, Status};
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
            name: "old_lake".into(),
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
///
/// No keystore is opened: the window's password probe is a root `use_hook` and the root is not
/// mounted, so `password_probe` is seeded with the answer that read would have parked.
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
            r.provide_root_context(|| State::create(CatalogState::Cold));
            let rescan = r.provide_root_context(|| State::create(ScanRequest::default()));
            r.provide_root_context(|| State::create(Log::default()));
            r.provide_root_context(|| State::create(PersistFaults::default()));
            let project = r.provide_root_context(|| {
                RadioStation::<ProjectState, ProjChan>::create(project(&root))
            });
            let ctx = r.provide_root_context(|| ConnectionCtx {
                password_expected: State::create(draft.pg.password),
                draft: State::create(draft.clone()),
                target: State::create(target.clone()),
                status: State::create(Status::Idle),
                profiles: State::create(Some(Vec::new())),
                selected_option: State::create(None),
                password: State::create(String::new()),
                password_removed: State::create(false),
                password_probe: State::create(PasswordProbe::Absent),
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
        .map(|c| c.def.named())
        .collect();
    assert_eq!(urls, ["acme_lake", "old_lake"]);
    assert_eq!(
        *ctx.status.peek(),
        Status::Connecting("acme_lake".into()),
        "the window is waiting on its own row, not claiming it connected"
    );
    assert_eq!(rescan.peek().seq, 1, "one pass asked for");
    assert_eq!(
        *ctx.target.peek(),
        ConnectionTarget::Edit("acme_lake".into())
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
    let (mut runner, (_, project, _)) =
        runner("moved", ConnectionTarget::Edit("old_lake".into()), draft);
    settle(&mut runner);

    click_lowest(&mut runner, "Save");

    let urls: Vec<String> = project
        .peek()
        .connections
        .iter()
        .map(|c| c.def.named())
        .collect();
    assert_eq!(urls, ["new_lake"], "the old URL's row is gone");
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

/// **The database arm's rows are its own, and none of the object stores' are** — a region, an
/// endpoint, an auth pill and a client-options table cannot mean anything for a database.
///
/// Driven through the picker rather than mounted on a Postgres draft, because reaching this arm
/// at all is half the deliverable: the picker iterated `OBJECT_STORES` while the rows did not
/// exist, and nothing else would have said so.
#[test]
fn a_database_has_the_database_rows_and_none_of_the_object_stores() {
    let (mut runner, (ctx, ..)) = runner("pg-rows", ConnectionTarget::New, s3_draft());
    settle(&mut runner);

    click_lowest(&mut runner, "PG");
    assert_eq!(ctx.draft.peek().provider, ProviderId::Source);

    for row in ["URL", "DATABASE", "CATALOG", "USER", "PASSWORD", "SSL MODE"] {
        assert!(shows(&runner, row), "{row}: {:?}", texts(&runner));
    }
    for row in [
        "BUCKET",
        "REGION",
        "ENDPOINT",
        "AUTHENTICATION",
        "CLIENT OPTIONS",
    ] {
        assert!(!shows(&runner, row), "{row} is object-store vocabulary");
    }
    assert!(
        !shows(&runner, "ROOT CERTIFICATE"),
        "'prefer' does not verify, so there is nothing for a certificate to do"
    );

    ctx.edit(|draft| draft.pg.sslmode = "verify-full".to_string());
    settle(&mut runner);
    assert!(shows(&runner, "ROOT CERTIFICATE"), "{:?}", texts(&runner));

    click_lowest(&mut runner, "S3");
    assert!(shows(&runner, "REGION"));
    assert!(!shows(&runner, "DATABASE"));
    assert!(!shows(&runner, "CATALOG"));
}

/// **A database's blockers block, and the footer is where they are said** — including the one the
/// draft cannot answer on its own, a catalog name another connection in this project holds.
#[test]
fn a_database_draft_is_blocked_and_explained_beside_the_button() {
    let draft = ConnectionDraft {
        provider: ProviderId::Source,
        address: "db.internal:5432/analytics".into(),
        pg: PgDraft {
            user: "reader".into(),
            ..Default::default()
        },
        ..Default::default()
    };
    let (mut runner, (ctx, mut project, rescan)) =
        runner("pg-blocked", ConnectionTarget::New, draft);
    settle(&mut runner);

    assert!(
        shows(&runner, "This connection has no catalog name."),
        "{:?}",
        texts(&runner)
    );
    click_lowest(&mut runner, "Save");
    assert_eq!(project.peek().connections.len(), 1, "nothing was written");
    assert_eq!(rescan.peek().seq, 0);

    {
        let mut p = project.write_channel(ProjChan::Connections);
        p.upsert_connection(ConnectionDef {
            address: "other:5432/sales".into(),
            name: "warehouse".into(),
            provider: Provider::Source(
                PgDraft {
                    kind: Pg::NAME.to_string(),
                    name: "warehouse".into(),
                    user: "reader".into(),
                    ..Default::default()
                }
                .def(),
            ),
            client_config: Default::default(),
        });
    }
    ctx.edit(|draft| draft.pg.name = "WAREHOUSE".into());
    settle(&mut runner);

    let said = texts(&runner);
    assert!(
        said.iter()
            .any(|t| t.contains("is already the catalog name") && t.contains("sales")),
        "a folded clash against another connection: {said:?}"
    );

    ctx.edit(|draft| draft.pg.name = "pg".into());
    settle(&mut runner);
    click_lowest(&mut runner, "Save");
    assert_eq!(rescan.peek().seq, 1, "and it saves once the name is free");
}

/// **Editing a database connection's identity does not make it clash with itself.**
///
/// `check_catalog_name` skips the candidate by comparing URLs, and a database connection's URL
/// carries its user — so changing only the USER moves the identity, the stored row stops matching,
/// and the draft reads as clashing with the very connection it is replacing. The footer quoted
/// that connection's own old URL back and Save stayed disabled short of also renaming the catalog,
/// which is not what the user was doing.
#[test]
fn editing_a_database_connection_does_not_clash_with_the_row_it_replaces() {
    let draft = ConnectionDraft {
        provider: ProviderId::Source,
        address: "db.internal:5432/analytics".into(),
        pg: PgDraft {
            kind: Pg::NAME.to_string(),
            name: "pg".into(),
            user: "reader".into(),
            ..Default::default()
        },
        ..Default::default()
    };
    let stored = "analytics";
    let (mut runner, (ctx, mut project, _)) = runner(
        "pg-self-clash",
        ConnectionTarget::Edit(stored.into()),
        draft,
    );
    {
        let mut p = project.write_channel(ProjChan::Connections);
        p.upsert_connection(ConnectionDef {
            address: "db.internal:5432/analytics".into(),
            name: "analytics".into(),
            provider: Provider::Source(
                PgDraft {
                    kind: Pg::NAME.to_string(),
                    name: "pg".into(),
                    user: "reader".into(),
                    ..Default::default()
                }
                .def(),
            ),
            client_config: Default::default(),
        });
    }
    settle(&mut runner);

    // The footer holds the station rather than subscribing to it, so this edit is what re-renders
    // it — and it has to land with the row already stored, or the clash never gets asked about.
    ctx.edit(|draft| draft.pg.user = "writer".into());
    settle(&mut runner);

    assert_eq!(
        ctx.draft.peek().def().identity(),
        "postgres:db.internal:5432/analytics",
        "the address is unchanged, so the clash is the name's question and not the address's"
    );
    let said = texts(&runner);
    assert!(
        !said
            .iter()
            .any(|t| t.contains("is already the catalog name")),
        "the row this window opened on is not a peer to clash against: {said:?}"
    );
    assert_eq!(ctx.draft.peek().blocker(), None);
}

/// **A password typed into a new connection reaches the def as an expectation and nothing more.**
/// The value is bound for this machine's keystore, which no test opens; what is asserted is the
/// half that is the def's.
#[test]
fn a_database_password_is_an_expectation_in_the_def_and_a_value_on_this_machine() {
    let draft = ConnectionDraft {
        provider: ProviderId::Source,
        address: "db.internal:5432/analytics".into(),
        pg: PgDraft {
            kind: Pg::NAME.to_string(),
            name: "pg".into(),
            user: "reader".into(),
            ..Default::default()
        },
        ..Default::default()
    };
    let (mut runner, (ctx, ..)) = runner("pg-password", ConnectionTarget::New, draft);
    settle(&mut runner);

    assert!(!ctx.draft.peek().pg.password);
    assert!(
        shows(&runner, "This connection signs in without a password."),
        "{:?}",
        texts(&runner)
    );

    let mut typed = ctx.password;
    typed.set("hunter2".into());
    settle(&mut runner);
    assert!(
        ctx.draft.peek().pg.password,
        "typing one is what makes the def expect one"
    );
    assert!(shows(
        &runner,
        "This password goes into this machine's keystore when you save."
    ));

    typed.set(String::new());
    settle(&mut runner);
    assert!(
        !ctx.draft.peek().pg.password,
        "and clearing the box puts back what the def said, rather than leaving a committed \
         expectation nothing holds"
    );
}

/// **The two clearing gestures are two presses, and only one of them edits the def.** Made
/// casually on a machine with no entry, "this connection uses no password" breaks the colleague
/// who has one, so it is never what a removal does.
#[test]
fn removing_a_password_from_this_machine_is_not_declaring_the_connection_has_none() {
    let draft = ConnectionDraft {
        provider: ProviderId::Source,
        address: "db.internal:5432/analytics".into(),
        pg: PgDraft {
            kind: Pg::NAME.to_string(),
            name: "pg".into(),
            user: "reader".into(),
            password: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let (mut runner, (ctx, ..)) = runner(
        "pg-forget",
        ConnectionTarget::Edit("analytics".into()),
        draft,
    );
    let mut probe = ctx.password_probe;
    probe.set(PasswordProbe::Stored);
    settle(&mut runner);

    assert!(
        shows(
            &runner,
            "A password is stored on this machine. Type a new one to replace it."
        ),
        "{:?}",
        texts(&runner)
    );

    click_lowest(&mut runner, "Remove from this machine");
    assert!(
        ctx.draft.peek().pg.password,
        "the connection still expects one, so other machines keep theirs"
    );
    assert!(*ctx.password_removed.peek());

    click_lowest(&mut runner, "This connection uses no password");
    assert!(
        !ctx.draft.peek().pg.password,
        "this one is a def edit, and it is the only one that is"
    );
    assert!(
        *ctx.password_removed.peek(),
        "and it drops this machine's entry too, which nothing would name again"
    );
}

/// **This machine's answer is not the def's**: a def that expects a password says one
/// is expected, not that this machine holds one.
#[test]
fn the_password_row_says_what_this_machine_holds() {
    let draft = ConnectionDraft {
        provider: ProviderId::Source,
        address: "db.internal:5432/analytics".into(),
        pg: PgDraft {
            kind: Pg::NAME.to_string(),
            name: "pg".into(),
            user: "reader".into(),
            password: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let (mut runner, (ctx, ..)) = runner("pg-probe", ConnectionTarget::New, draft);
    let mut probe = ctx.password_probe;

    for (answer, said) in [
        (PasswordProbe::Asking, "Checking this machine's keystore…"),
        (
            PasswordProbe::Stored,
            "A password is stored on this machine. Type a new one to replace it.",
        ),
        (
            PasswordProbe::Absent,
            "This connection expects a password and none is stored on this machine. Enter it \
             here.",
        ),
    ] {
        probe.set(answer);
        settle(&mut runner);
        assert!(shows(&runner, said), "{said:?}: {:?}", texts(&runner));
    }
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
            "'old_lake' is already a connection in this project."
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
