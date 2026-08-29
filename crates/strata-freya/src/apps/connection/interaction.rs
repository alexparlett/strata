//! The connection editor driven the way a user drives it: which controls a provider has, and
//! what Save actually writes.
//!
//! **Every form here is driven through a source that exists nowhere in the app.** `TestSource` is
//! declared in this file, registered on this test's own engine, and the editor draws its whole
//! form from that registration — which is the deliverable: a `DataSource` an embedder writes gets
//! a working editor with no code named after it. Nothing in `apps/connection` mentions a kind, so
//! there is no shipped source to drive these tests with that would prove anything weaker.
//!
//! The window **root** is not mounted — it needs the app-globals, a menubar scope and an owner
//! window id, none of which say anything about the editor. What is mounted is the pair that does:
//! the fields and the footer, over the same contexts the real window provides them.
//!
//! Asserted through rendered text and the store, because that is the deliverable: a form whose
//! controls are the ones its source declared, and a Save that writes one def, deregisters the one
//! it moved off, and then waits rather than claiming success.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use freya::prelude::*;
use freya::radio::RadioStation;
use freya_testing::TestingRunner;
use strata_core::project::ProjectDefs;
use strata_core::theme::load;
use strata_engine::secrets::SecretProvider;
use strata_engine::{
    DataSource, Engine, Field, SourceInfo, SourceKind, SourceMode, SourceSetting, Sourced, When,
};
use strata_model::SourceDef;

use super::views::{ConnectionBody, Footer};
use super::{ConnectionCtx, ConnectionDraft, ConnectionTarget, SecretProbe, Status};
use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::{CatalogState, Log, PersistFaults, ProjChan, ProjectState, ScanRequest};
use crate::theme::strata_theme;

/// A source with no server behind it, declaring one key of every shape the form draws. Registered
/// on this test's engine and on nothing else, so every row it produces is produced by the
/// declaration alone.
#[derive(Debug)]
struct TestSource;

impl SourceKind for TestSource {
    const NAME: &'static str = "test-source";
    const LABEL: &'static str = "Test source";
    const BADGE: &'static str = "TST";
    const MODE: SourceMode = SourceMode::Catalog;
    const WRITABLE: bool = true;
}

const TEST_SETTINGS: &[SourceSetting] = &[
    SourceSetting {
        key: "address",
        label: "ADDRESS",
        field: Field::Text,
        group: Some("CONNECTION"),
        required: true,
        default: None,
        when: None,
        hint: Some("The server and the database on it"),
        placeholder: None,
    },
    SourceSetting {
        key: "user",
        label: "USER",
        field: Field::Text,
        group: Some("CONNECTION"),
        required: true,
        default: None,
        when: None,
        hint: Some("The role to log in as"),
        placeholder: Some("reader"),
    },
    SourceSetting {
        key: "password",
        label: "PASSWORD",
        field: Field::Secret,
        group: Some("CONNECTION"),
        required: false,
        default: None,
        when: None,
        hint: None,
        placeholder: None,
    },
    SourceSetting {
        key: "mode",
        label: "MODE",
        field: Field::Choice(&["off", "on"]),
        group: Some("SECURITY"),
        required: false,
        default: Some("off"),
        when: None,
        hint: None,
        placeholder: None,
    },
    SourceSetting {
        key: "certificate",
        label: "ROOT CERTIFICATE",
        field: Field::Path,
        group: Some("SECURITY"),
        required: true,
        default: None,
        when: Some(When {
            key: "mode",
            values: &["on"],
        }),
        hint: None,
        placeholder: None,
    },
];

#[async_trait]
impl DataSource for TestSource {
    async fn connect(
        &self,
        _def: &SourceDef,
        _secrets: Arc<dyn SecretProvider>,
    ) -> Result<Sourced, String> {
        Err("This test source has no server.".into())
    }

    /// A naming rule of its own, so the editor's address refusal can be shown to be **the kind's**
    /// rather than one written in the form.
    fn check_address(&self, address: &str) -> Result<(), String> {
        match address.contains('/') {
            true => Ok(()),
            false => Err("A test address is 'server/database'.".into()),
        }
    }

    fn settings(&self) -> &'static [SourceSetting] {
        TEST_SETTINGS
    }
}

/// An engine serving [`TestSource`] and nothing else this crate ships.
fn engine() -> EngineCtx {
    EngineCtx::of(Engine::builder().with_source(TestSource).build())
}

/// The registration the draft is edited against — what the picker would have adopted.
fn registrant() -> SourceInfo {
    SourceInfo {
        kind: TestSource::NAME,
        label: TestSource::LABEL,
        badge: TestSource::BADGE,
        mode: TestSource::MODE,
        settings: TEST_SETTINGS,
        writable: TestSource::WRITABLE,
        unique: &[],
        scheme: TestSource::SCHEME,
    }
}

/// A scratch project folder for one test — `env::temp_dir()` + pid, the convention every test
/// that really writes `.strata/project.json` follows, because the OS temp dir is machine-shared
/// and a hardcoded path collides between parallel test binaries.
fn temp_root(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("strata-connection-{tag}-{}", std::process::id()))
}

/// One connection already in the project, so a name clash has something to clash with.
///
/// An **object store**, deliberately: it is a def this editor has no form for, and it still has
/// to be listed, keyed and clashed against exactly as before — nothing about withholding its
/// *form* touches the store.
fn project(root: &Path) -> ProjectState {
    let defs = ProjectDefs {
        name: "test".into(),
        connections: vec![SourceDef {
            kind: "s3".into(),
            name: "old_lake".into(),
            config: [("region", "eu-west-2")]
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            ..Default::default()
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
/// No keystore is opened: the window's secret probes are a root `use_hook` and the root is not
/// mounted, so `secret_probes` starts empty, which reads as the answer that read would have
/// parked for a key nothing is stored for.
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
            r.provide_root_context(engine);
            r.provide_root_context(|| State::create(CatalogState::Cold));
            let rescan = r.provide_root_context(|| State::create(ScanRequest::default()));
            r.provide_root_context(|| State::create(Log::default()));
            r.provide_root_context(|| State::create(PersistFaults::default()));
            let project = r.provide_root_context(|| {
                RadioStation::<ProjectState, ProjChan>::create(project(&root))
            });
            let ctx = r.provide_root_context(|| ConnectionCtx {
                secret_expected: State::create(draft.secrets.clone()),
                draft: State::create(draft.clone()),
                target: State::create(target.clone()),
                status: State::create(Status::Idle),
                secret_values: State::create(BTreeMap::new()),
                secret_removed: State::create(BTreeSet::new()),
                secret_probes: State::create(BTreeMap::new()),
            });
            (ctx, project, rescan)
        },
        1.,
    )
}

/// Settle the tree — several passes, because the fields mount buffers that report on their own
/// first effect and the address box echoes itself once.
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

/// A draft on [`TestSource`], as the picker would have left it.
fn source_draft() -> ConnectionDraft {
    ConnectionDraft {
        kind: TestSource::NAME.into(),
        settings: TEST_SETTINGS,
        name: "warehouse".into(),
        config: [
            ("address".to_string(), "db.internal/analytics".into()),
            ("user".to_string(), "reader".to_string()),
        ]
        .into_iter()
        .collect(),
        ..Default::default()
    }
}

/// **Save writes the def, asks for the pass, and then waits.** It does not claim success: the
/// window settles on the row the pass answers, so the status here is `Connecting` and the store's
/// row is back to `Loading`.
#[test]
fn saving_writes_the_def_and_waits_for_the_pass() {
    let (mut runner, (ctx, project, rescan)) =
        runner("save", ConnectionTarget::New, source_draft());
    settle(&mut runner);

    click_lowest(&mut runner, "Save");

    let names: Vec<String> = project
        .peek()
        .connections
        .iter()
        .map(|c| c.def.named())
        .collect();
    assert_eq!(names, ["old_lake", "warehouse"], "sorted by name");
    assert_eq!(
        *ctx.status.peek(),
        Status::Connecting("warehouse".into()),
        "the window is waiting on its own row, not claiming it connected"
    );
    assert_eq!(rescan.peek().seq, 1, "one pass asked for");
    assert_eq!(
        *ctx.target.peek(),
        ConnectionTarget::Edit("warehouse".into())
    );
}

/// **An edit that renames a connection leaves no row behind** — otherwise the project keeps a def
/// under the old name and the pass registers both.
#[test]
fn an_edit_that_renames_leaves_no_row_behind() {
    let mut draft = source_draft();
    draft.name = "depot".into();
    let (mut runner, (_, mut project, _)) =
        runner("moved", ConnectionTarget::Edit("warehouse".into()), draft);
    {
        let mut p = project.write_channel(ProjChan::Connections);
        p.upsert_connection(SourceDef {
            config: [("address".to_string(), "db.internal/analytics".into())]
                .into_iter()
                .collect(),
            name: "warehouse".into(),
            kind: TestSource::NAME.into(),
            ..Default::default()
        });
    }
    settle(&mut runner);

    click_lowest(&mut runner, "Save");

    let names: Vec<String> = project
        .peek()
        .connections
        .iter()
        .map(|c| c.def.named())
        .collect();
    assert_eq!(names, ["old_lake", "depot"], "the old name's row is gone");
}

/// **A registered source's rows are the ones it declared, and there are no others.**
///
/// This is the deliverable in one test: `TestSource` exists only in this file, nothing in
/// `apps/connection` names it, and the form is a name, an address, a box per declared key in the
/// dress its `Field` asks for, and the read-only switch its `MODE` earns. There is no
/// object-store vocabulary to be absent, because there is no object-store dress.
#[test]
fn a_registered_sources_rows_are_the_ones_it_declared() {
    let (mut runner, (ctx, ..)) = runner(
        "declared-rows",
        ConnectionTarget::New,
        ConnectionDraft::new(&[registrant()]),
    );
    settle(&mut runner);

    assert_eq!(
        ctx.draft.peek().kind,
        TestSource::NAME,
        "a new draft opens on the first registrant"
    );
    assert_eq!(
        ctx.draft.peek().settings,
        TEST_SETTINGS,
        "carrying its declaration, not just its name"
    );

    for row in [
        "PROVIDER",
        "NAME",
        "ADDRESS",
        "USER",
        "PASSWORD",
        "MODE",
        "READ ONLY",
    ] {
        assert!(shows(&runner, row), "{row}: {:?}", texts(&runner));
    }
    assert!(
        !shows(&runner, "ROOT CERTIFICATE"),
        "'off' does not read a certificate, so there is no control for one"
    );
    for heading in ["CONNECTION", "SECURITY"] {
        assert!(
            shows(&runner, heading),
            "{heading}: the sections the kind grouped its keys into, {:?}",
            texts(&runner)
        );
    }
    assert!(
        !shows(&runner, "READ ONLY") || registrant().writable,
        "the read-only switch is offered because the kind says it can be written to"
    );
    assert!(
        shows(&runner, TestSource::BADGE),
        "and the picker offers it by its own badge"
    );
}

/// **A row appears and disappears on another row's answer, and the kind is what says so.**
///
/// `TestSource` declares its certificate `shown` by `mode: on` — the shape `Pg` uses for
/// `sslmode`'s two verifying modes. The editor holds no condition of its own; it asks the draft,
/// which asks the declaration.
#[test]
fn a_declared_condition_is_what_puts_a_row_on_screen() {
    let (mut runner, (ctx, ..)) = runner("conditional", ConnectionTarget::New, source_draft());
    settle(&mut runner);
    assert!(!shows(&runner, "ROOT CERTIFICATE"));

    ctx.edit(|draft| draft.set("mode", "on".into()));
    settle(&mut runner);
    assert!(shows(&runner, "ROOT CERTIFICATE"), "{:?}", texts(&runner));

    ctx.edit(|draft| draft.set("mode", "off".into()));
    settle(&mut runner);
    assert!(!shows(&runner, "ROOT CERTIFICATE"), "and back again");
}

/// **A source's form has no standing note.** What each secret box does with what is typed into it
/// is the row's own sentence, which is specific because it reports this machine; a paragraph about
/// the kind is prose only the kind could write.
#[test]
fn a_sources_form_ends_with_its_last_row() {
    let (mut runner, ..) = runner("no-note", ConnectionTarget::New, source_draft());
    settle(&mut runner);

    let said = texts(&runner);
    assert!(
        !said.iter().any(|t| t.contains("The project file keeps")),
        "no standing note: {said:?}"
    );
    assert!(
        said.iter()
            .any(|t| t.contains("signs in without a password")),
        "but the secret row still reports this machine: {said:?}"
    );
}

/// **A section heading is printed once, when the group changes** — and a group whose every key is
/// hidden prints no heading at all, because the heading rides the first row that survives the
/// declaration's own conditions.
#[test]
fn a_group_heading_rides_the_rows_that_survive() {
    let (mut runner, (ctx, ..)) = runner("groups", ConnectionTarget::New, source_draft());
    settle(&mut runner);

    let headings = |runner: &TestingRunner| {
        texts(runner)
            .into_iter()
            .filter(|t| t == "CONNECTION" || t == "SECURITY")
            .collect::<Vec<_>>()
    };
    assert_eq!(
        headings(&runner),
        ["CONNECTION", "SECURITY"],
        "one each, in declaration order — MODE keeps SECURITY on screen"
    );

    ctx.edit(|draft| draft.set("mode", "on".into()));
    settle(&mut runner);
    assert_eq!(
        headings(&runner),
        ["CONNECTION", "SECURITY"],
        "and revealing a second key under it does not print it twice"
    );
    assert!(shows(&runner, "ROOT CERTIFICATE"));
}

/// **The address is refused by the kind's own rule**, reached through
/// `sources().check_address` — so the sentence under the button is the one `connect` would have
/// given, and this form holds no copy of it to drift.
#[test]
fn an_address_is_refused_in_the_kinds_own_words() {
    let mut draft = source_draft();
    draft.set("address", "db.internal".into());
    let (mut runner, (ctx, _, rescan)) = runner("address", ConnectionTarget::New, draft);
    settle(&mut runner);

    assert!(
        shows(&runner, "A test address is 'server/database'."),
        "{:?}",
        texts(&runner)
    );
    click_lowest(&mut runner, "Save");
    assert_eq!(rescan.peek().seq, 0, "and nothing was asked for");

    ctx.edit(|draft| draft.set("address", "db.internal/analytics".into()));
    settle(&mut runner);
    assert!(!shows(&runner, "A test address is 'server/database'."));
}

/// **A required declared key that is empty blocks the save, and the form says which one.** That
/// is the only thing a generic form can be wrong about — what a value may *be* is the kind's
/// rule, and `connect` is where it is asked.
#[test]
fn a_required_declared_key_blocks_the_save() {
    let mut draft = source_draft();
    draft.set("user", String::new());
    let (mut runner, (ctx, project, rescan)) = runner("required", ConnectionTarget::New, draft);
    settle(&mut runner);

    assert!(
        shows(&runner, "This connection has no user."),
        "{:?}",
        texts(&runner)
    );
    click_lowest(&mut runner, "Save");
    assert_eq!(project.peek().connections.len(), 1, "nothing was written");
    assert_eq!(rescan.peek().seq, 0);

    ctx.edit(|draft| draft.set("user", "reader".into()));
    settle(&mut runner);
    click_lowest(&mut runner, "Save");
    assert_eq!(
        rescan.peek().seq,
        1,
        "and it saves once the box is answered"
    );
}

/// **A name another connection already holds is refused, and a blank box is not blank.** The
/// address mints the handle, so a source with no name typed still saves under a name every
/// surface can address it by.
#[test]
fn a_name_clash_is_explained_beside_the_button() {
    let mut draft = source_draft();
    draft.name = String::new();
    let (mut runner, (ctx, mut project, rescan)) = runner("name", ConnectionTarget::New, draft);
    settle(&mut runner);

    assert_eq!(
        ctx.draft.peek().named(),
        "",
        "a blank name is blank — nothing mints one from the address"
    );

    {
        let mut p = project.write_channel(ProjChan::Connections);
        p.upsert_connection(SourceDef {
            config: [("address".to_string(), "other/sales".into())]
                .into_iter()
                .collect(),
            name: "warehouse".into(),
            kind: TestSource::NAME.into(),
            ..Default::default()
        });
    }
    ctx.edit(|draft| draft.name = "WAREHOUSE".into());
    settle(&mut runner);

    let said = texts(&runner);
    assert!(
        said.iter()
            .any(|t| t.contains("is already the name of another source")),
        "a folded clash against another source: {said:?}"
    );

    ctx.edit(|draft| draft.name = "depot".into());
    settle(&mut runner);
    click_lowest(&mut runner, "Save");
    assert_eq!(rescan.peek().seq, 1, "and it saves once the name is free");
}

/// **Editing a source connection's settings does not make it clash with itself.**
///
/// `check_catalog_name` skips the candidate by comparing identities, so a change to a declared
/// key must leave the row this window opened on out of the clash set — otherwise the footer
/// quotes that connection's own name back and Save never re-enables.
#[test]
fn editing_a_source_connection_does_not_clash_with_the_row_it_replaces() {
    let (mut runner, (ctx, mut project, _)) = runner(
        "self-clash",
        ConnectionTarget::Edit("warehouse".into()),
        source_draft(),
    );
    {
        let mut p = project.write_channel(ProjChan::Connections);
        p.upsert_connection(SourceDef {
            name: "warehouse".into(),
            kind: TestSource::NAME.into(),
            config: [("user".to_string(), "reader".to_string())]
                .into_iter()
                .collect(),
            ..Default::default()
        });
    }
    settle(&mut runner);

    // The footer holds the station rather than subscribing to it, so this edit is what re-renders
    // it — and it has to land with the row already stored, or the clash never gets asked about.
    ctx.edit(|draft| draft.set("user", "writer".into()));
    settle(&mut runner);

    let said = texts(&runner);
    assert!(
        !said
            .iter()
            .any(|t| t.contains("is already the catalog name")),
        "the row this window opened on is not a peer to clash against: {said:?}"
    );
    assert_eq!(ctx.draft.peek().blocker(), None);
}

/// **A secret typed into a new connection reaches the def as an expectation and nothing more.**
/// The value is bound for this machine's keystore, which no test opens; what is asserted is the
/// half that is the def's.
#[test]
fn a_declared_secret_is_an_expectation_in_the_def_and_a_value_on_this_machine() {
    let (mut runner, (ctx, ..)) = runner("secret", ConnectionTarget::New, source_draft());
    settle(&mut runner);

    assert!(ctx.draft.peek().secrets.is_empty());
    assert!(
        shows(&runner, "This connection signs in without a password."),
        "{:?}",
        texts(&runner)
    );

    ctx.set_secret("password", "hunter2".into());
    settle(&mut runner);
    assert!(
        ctx.draft.peek().secrets.contains("password"),
        "typing one is what makes the def expect one"
    );
    assert!(shows(
        &runner,
        "This password goes into this machine's keystore when you save."
    ));
    let def = ctx.draft.peek().def();
    assert!(
        !def.config.contains_key("password"),
        "and no secret value reaches the def: {:?}",
        def.config
    );

    ctx.set_secret("password", String::new());
    settle(&mut runner);
    assert!(
        !ctx.draft.peek().secrets.contains("password"),
        "and clearing the box puts back what the def said, rather than leaving a committed \
         expectation nothing holds"
    );
}

/// **The two clearing gestures are two presses, and only one of them edits the def.** Made
/// casually on a machine with no entry, "this connection uses no password" breaks the colleague
/// who has one, so it is never what a removal does. The press names the key, off its declared
/// label, so a source with two credentials offers two distinguishable presses.
#[test]
fn removing_a_secret_from_this_machine_is_not_declaring_the_connection_has_none() {
    let mut draft = source_draft();
    draft.secrets.insert("password".into());
    let (mut runner, (ctx, ..)) = runner(
        "secret-forget",
        ConnectionTarget::Edit("warehouse".into()),
        draft,
    );
    {
        let mut probes = ctx.secret_probes;
        probes.set(
            [("password".to_string(), SecretProbe::Stored)]
                .into_iter()
                .collect(),
        );
    }
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
        ctx.draft.peek().secrets.contains("password"),
        "the connection still expects one, so other machines keep theirs"
    );
    assert!(ctx.secret_removed.peek().contains("password"));

    click_lowest(&mut runner, "This connection uses no password");
    assert!(
        !ctx.draft.peek().secrets.contains("password"),
        "this one is a def edit, and it is the only one that is"
    );
    assert!(
        ctx.secret_removed.peek().contains("password"),
        "and it drops this machine's entry too, which nothing would name again"
    );
}

/// **This machine's answer is not the def's**: a def that expects a secret says one is expected,
/// not that this machine holds one.
#[test]
fn a_secret_row_says_what_this_machine_holds() {
    let mut draft = source_draft();
    draft.secrets.insert("password".into());
    let (mut runner, (ctx, ..)) = runner("secret-probe", ConnectionTarget::New, draft);
    let mut probes = ctx.secret_probes;

    for (answer, said) in [
        (SecretProbe::Asking, "Checking this machine's keystore…"),
        (
            SecretProbe::Stored,
            "A password is stored on this machine. Type a new one to replace it.",
        ),
        (
            SecretProbe::Absent,
            "This connection expects a password and none is stored on this machine. Enter it \
             here.",
        ),
    ] {
        probes.set([("password".to_string(), answer)].into_iter().collect());
        settle(&mut runner);
        assert!(shows(&runner, said), "{said:?}: {:?}", texts(&runner));
    }
}

/// **A declared choice is a `Select` over the words the kind gave**, so a value it would refuse is
/// unreachable — and a key nothing has touched is written as the key declares it.
#[test]
fn a_declared_choice_writes_the_kinds_own_word() {
    let (mut runner, (ctx, ..)) = runner("choice", ConnectionTarget::New, source_draft());
    settle(&mut runner);

    let def = ctx.draft.peek().def();
    assert_eq!(
        def.config.get("mode").map(String::as_str),
        Some("off"),
        "the declared default"
    );

    ctx.edit(|draft| draft.set("mode", "on".into()));
    settle(&mut runner);
    let def = ctx.draft.peek().def();
    assert_eq!(def.config.get("mode").map(String::as_str), Some("on"));
}

/// **A source's def is byte-equivalent through the form**: opened on a stored def and saved with
/// nothing touched, what reaches the store is the def that was there.
#[test]
fn a_stored_source_def_survives_the_form_untouched() {
    let stored = SourceDef {
        name: "warehouse".into(),
        kind: TestSource::NAME.into(),
        config: [
            ("user", "reader"),
            ("mode", "on"),
            ("certificate", "/c.pem"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect(),
        secrets: BTreeSet::from(["password".to_string()]),
        schemas: vec!["public".into()],
        read_only: false,
    };
    let draft = ConnectionDraft::of(&stored, &[registrant()]);
    let (mut runner, (ctx, ..)) = runner(
        "round-trip",
        ConnectionTarget::Edit("warehouse".into()),
        draft,
    );
    settle(&mut runner);

    assert_eq!(
        ctx.draft.peek().def(),
        stored,
        "every box mounted and reported, and nothing moved"
    );
}

/// **A name another connection already holds is refused**, because `upsert_connection` replaces
/// on it — so without this the save would silently take that connection's def out from under it.
///
/// The connection it clashes with is an **object store**, which this editor has no form for: what
/// a def is keyed by has nothing to do with whether this window can draw it.
#[test]
fn a_name_another_connection_holds_blocks_the_save() {
    let mut draft = source_draft();
    draft.name = "old_lake".into();
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
