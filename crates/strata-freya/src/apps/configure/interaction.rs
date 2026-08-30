//! The Configure window's **LOCATION** arm driven the way a user drives it (W7 · 04): the toggle,
//! the data source picker, and what Save then writes.
//!
//! The window **root** is not mounted — it needs the app-globals, a menubar scope and an owner
//! window id, none of which say anything about the form. What is mounted is the body and the
//! footer, over the same contexts the real window provides them.
//!
//! Asserted through rendered text and the store, because that is the deliverable: a form that says
//! which bucket a path is written against, a Save refused while it cannot say, and a def that
//! carries the data source rather than a path composed into it.
//!
//! **What is not driven here is a `Select`'s own menu.** Freya's `Select` closes itself when the
//! focused accessibility node is not its own, and a press in the testing runner never lands that
//! focus, so its items are never in the tree. That is the harness rather than the picker, so what a
//! menu item does is left to the model tests and the one-line handlers.

use std::path::{Path, PathBuf};

use freya::prelude::*;
use freya::radio::RadioStation;
use freya_testing::TestingRunner;
use strata_core::project::ProjectDefs;
use strata_core::theme::load;
use strata_model::SourceDef;

use super::model::Where;
use super::views::{ConfigureBody, Footer};
use super::{ConfigureCtx, ConfigureDraft, ConfigureTarget, Probes, Status};
use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::{
    CatalogState, Log, PersistFaults, ProjChan, ProjectState, ScanRequest, SourceRequest,
};
use crate::apps::source::SourceTarget;
use crate::theme::strata_theme;

/// A scratch project folder for one test — `env::temp_dir()` + pid, the convention every test
/// that really writes `.strata/project.json` follows.
fn temp_root(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("strata-configure-{tag}-{}", std::process::id()))
}

/// A project with one S3 data source, or none at all — the two states the picker has to tell
/// apart, since only one of them has anything to offer.
fn project(root: &Path, connected: bool) -> ProjectState {
    let defs = ProjectDefs {
        name: "test".into(),
        sources: match connected {
            false => Vec::new(),
            true => vec![SourceDef {
                kind: "s3".into(),
                name: "acme_lake".into(),
                config: [("region", "eu-west-2")]
                    .into_iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
                ..Default::default()
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
    SourceRequest,
);

/// The window's body and footer over `draft`, against its own scratch project.
fn runner(tag: &'static str, connected: bool, draft: ConfigureDraft) -> (TestingRunner, Handles) {
    let root = temp_root(tag);
    TestingRunner::new(
        app,
        (620., 900.).into(),
        move |r| {
            r.provide_root_context(EngineCtx::default);
            r.provide_root_context(|| State::create(CatalogState::Cold));
            r.provide_root_context(|| State::create(ScanRequest::default()));
            r.provide_root_context(|| State::create(Log::default()));
            r.provide_root_context(|| State::create(PersistFaults::default()));
            let project = r.provide_root_context(|| {
                RadioStation::<ProjectState, ProjChan>::create(project(&root, connected))
            });
            let sources: SourceRequest =
                r.provide_root_context(|| State::create(None::<SourceTarget>));
            let ctx = r.provide_root_context(|| ConfigureCtx {
                draft: State::create(draft.clone()),
                target: State::create(ConfigureTarget::New),
                status: State::create(Status::Idle),
                selected_path: State::create(0),
                selected_column: State::create(0),
                probes: State::create(Probes::new()),
            });
            (ctx, project, sources)
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

/// A table over the **local disk**, with one path in the list.
fn draft(source: &str) -> ConfigureDraft {
    ConfigureDraft {
        name: "events".into(),
        local_sources: vec![source.into()],
        ..Default::default()
    }
}

/// The same table with a **bucket-relative** path typed into the object store's box as well —
/// deliberately a different value, a flip carrying nothing between the two.
fn draft_both(local: &str, remote: &str) -> ConfigureDraft {
    ConfigureDraft {
        remote_source: remote.into(),
        ..draft(local)
    }
}

/// **A project with no data sources for the chosen provider says so, and Save is refused** — the
/// two halves of the same fact, in the two places the user is looking: under the picker, and in
/// the footer beside the button it disabled.
#[test]
fn remote_with_no_source_explains_itself_and_blocks_save() {
    let (mut runner, (ctx, ..)) = runner("empty", false, draft("events/"));
    settle(&mut runner);

    click_lowest(&mut runner, "Remote");

    assert!(ctx.draft.peek().remote());
    assert_eq!(ctx.draft.peek().source, None, "there is none to pick");
    assert!(
        shows(&runner, "No data sources yet. Add one to continue."),
        "the picker says why it is empty: {:?}",
        texts(&runner)
    );
    assert!(
        shows(
            &runner,
            "A remote table needs a data source to read through."
        ),
        "and the footer says why Save is off: {:?}",
        texts(&runner)
    );

    click_lowest(&mut runner, "GCS");
    assert!(shows(
        &runner,
        "No GCS data sources yet. Add one to continue."
    ));
}

/// **The headline**: flipping to the object store picks the project's data source, the path box
/// wears that bucket, and Save writes a def naming the data source with the path left
/// bucket-relative — which is what `register::table_spec` composes back into an address.
///
/// The draft opens with a path on **each** arm, so what is saved also says which of the two the
/// LOCATION chose.
#[test]
fn a_table_saved_over_a_source_carries_the_name_and_a_relative_path() {
    let draft = draft_both("/data/events.parquet", "events/2024/");
    let (mut runner, (ctx, project, _)) = runner("save", true, draft);
    settle(&mut runner);

    click_lowest(&mut runner, "Remote");

    assert_eq!(
        ctx.draft.peek().source.as_deref(),
        Some("acme_lake"),
        "the provider's first data source is picked for you"
    );
    assert!(
        shows(&runner, "SOURCE PATH"),
        "one path, said in the singular: {:?}",
        texts(&runner)
    );
    assert!(
        shows(&runner, "acme_lake"),
        "the row names the data source its path is written against: {:?}",
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
    assert_eq!(def.source.as_deref(), Some("acme_lake"));
    assert_eq!(
        def.paths,
        ["events/2024/"],
        "stored as typed — relativizing it against the project folder would mangle it"
    );
    assert_eq!(
        ctx.draft.peek().local_sources,
        ["/data/events.parquet"],
        "and the disk's own path was neither saved nor forgotten"
    );
}

/// **And back again.** The flip to Local is the direction that *shortens* the form and returns
/// the source list to its multi-path shape, so it is the one that exercises a section coming down
/// rather than going up — and the one nothing else drives.
///
/// What it asserts is that the whole surface returns: the TYPE / CONNECTION pair is gone, the
/// label is plural again, the bucket prefix is off the box, and the data source, the disk's paths
/// and the bucket's path are all *remembered* rather than discarded, which is what makes a flip
/// back and forth free.
#[test]
fn flipping_back_to_local_returns_the_multi_path_list_and_keeps_the_choice() {
    let draft = draft_both("/data/events.parquet", "events/2024/");
    let (mut runner, (ctx, ..)) = runner("back", true, draft);
    settle(&mut runner);

    click_lowest(&mut runner, "Remote");
    assert!(shows(&runner, "CONNECTION"), "{:?}", texts(&runner));
    assert_eq!(
        ctx.draft.peek().nonblank_sources(),
        ["events/2024/"],
        "the bucket's own path is what the one box holds"
    );

    click_lowest(&mut runner, "Local");

    assert!(!ctx.draft.peek().remote());
    assert_eq!(
        ctx.draft.peek().nonblank_sources(),
        ["/data/events.parquet"],
        "and the disk's list comes back whole, without retyping"
    );
    assert_eq!(
        ctx.draft.peek().remote_source,
        "events/2024/",
        "with the bucket's path still there for the next flip"
    );
    assert_eq!(
        ctx.draft.peek().source.as_deref(),
        Some("acme_lake"),
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

/// A def over a data source **this project no longer has** keeps naming it, and the footer says
/// so rather than letting the reference be saved again — the treatment a format with no reader
/// gets, for the same reason.
///
/// The draft is otherwise **whole**, path included, because the footer leads with what the draft
/// itself can answer: an empty box is "a table needs a source path", which is a different
/// sentence about a different thing to fix.
#[test]
fn a_forgotten_source_is_named_and_blocks_save() {
    let mut draft = draft_both("/data/events.parquet", "events/");
    draft.location = Where::Remote;
    draft.kind = "s3".into();
    draft.source = Some("s3://gone".into());
    let (mut runner, _) = runner("forgotten", true, draft);
    settle(&mut runner);

    assert!(
        shows(
            &runner,
            "'s3://gone' is not a data source in this project. Choose one, or add it back."
        ),
        "{:?}",
        texts(&runner)
    );
}

/// An **internal** draft: one column row, ready to be typed into, as `set_location` leaves it.
fn internal_draft() -> ConfigureDraft {
    let mut draft = ConfigureDraft {
        name: "daily".into(),
        ..Default::default()
    };
    draft.set_location(Where::Internal, &[]);
    draft
}

/// **The third LOCATION replaces the file questions with a column list.** The whole point of
/// putting this in Configure rather than a surface of its own: it is the same window answering
/// the same question, so what changes is which sections have anything to ask.
#[test]
fn internal_shows_columns_and_hides_everything_about_files() {
    let (mut runner, _) = runner("internal", false, internal_draft());
    settle(&mut runner);

    assert!(shows(&runner, "Internal"), "the segment is offered");
    assert!(shows(&runner, "COLUMNS"), "{:?}", texts(&runner));
    for absent in ["SOURCE PATHS", "SOURCE PATH", "FORMAT", "HIVE PARTITIONING"] {
        assert!(!shows(&runner, absent), "{absent} is still on screen");
    }
}

/// A table that already has files cannot be turned into one Strata stores — the segment is shown
/// and inert, so the answer is discoverable without being a way to discard a def.
#[test]
fn internal_is_offered_on_a_new_table_and_refused_on_an_edit() {
    let mut draft = ConfigureDraft {
        name: "orders".into(),
        local_sources: vec!["/data/orders.parquet".into()],
        ..Default::default()
    };
    let (mut editing, (ctx, _, _)) = runner("internal-edit", false, draft.clone());
    let mut target = ctx.target;
    target.set(ConfigureTarget::Edit("orders".into()));
    settle(&mut editing);

    click_lowest(&mut editing, "Internal");
    settle(&mut editing);
    assert!(
        !ctx.draft.peek().internal(),
        "an edit may not move a table's data into Strata"
    );
    assert!(
        shows(&editing, "SOURCE PATHS"),
        "…and the form is unchanged"
    );

    draft.name = "fresh".into();
    let (mut fresh, (ctx, _, _)) = runner("internal-new", false, draft);
    settle(&mut fresh);
    click_lowest(&mut fresh, "Internal");
    settle(&mut fresh);
    assert!(ctx.draft.peek().internal());
}

/// **Save is blocked until every column says what it is**, and the note names the first row that
/// does not — the planner's own words where the type is the problem.
#[test]
fn an_internal_table_blocks_save_until_its_columns_are_whole() {
    let mut draft = internal_draft();
    draft.set_column_name(0, "region".into());
    let (mut runner, (ctx, _, _)) = runner("internal-blocked", false, draft);
    settle(&mut runner);

    assert!(
        shows(&runner, "Enter a column type."),
        "{:?}",
        texts(&runner)
    );

    ctx.edit(|draft| draft.set_column_type(0, "FLOAT64".into()));
    for _ in 0..200 {
        runner.sync_and_update();
        if texts(&runner).iter().any(|t| t.contains("FLOAT64")) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(
        texts(&runner)
            .iter()
            .any(|t| t.contains("Unsupported SQL type FLOAT64")),
        "{:?}",
        texts(&runner)
    );
}

/// The Configure window on **Internal**, to a PNG for eyeballing — the same harness the Shape
/// panel's preview uses, and the check that this section wears the window's own dress.
#[test]
#[ignore = "writes target/configure-internal.png for eyeballing; run explicitly"]
fn configure_internal_preview() {
    let mut draft = internal_draft();
    draft.name = "signups".into();
    draft.set_column_name(0, "region".into());
    draft.set_column_type(0, "VARCHAR".into());
    draft.add_column();
    draft.set_column_name(1, "signups".into());
    draft.set_column_type(1, "BIGINT".into());
    draft.add_column();
    draft.set_column_name(2, "amount".into());
    draft.set_column_type(2, "FLOAT64".into());

    let (mut runner, _) = runner("internal-preview", false, draft);
    for _ in 0..200 {
        runner.sync_and_update();
        if texts(&runner).iter().any(|t| t.contains("FLOAT64")) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    runner.render_to_file(format!(
        "{}/../../target/configure-internal.png",
        env!("CARGO_MANIFEST_DIR")
    ));
}

/// **The window will not close while it is running the create itself.**
///
/// The one piece of work in this window that is *this* window's: the fold that writes the def,
/// the catalog row and the log runs after the spawned task's await, and `ddl::tables::create`
/// publishes its spool by rename before its own last await — so a window dismissed mid-create
/// would leave a data directory under `.strata/tables/` that no def points at and no sweep
/// collects. Cancel and Esc both read [`Status::holds_window`], so they cannot disagree.
#[test]
fn only_a_create_in_flight_holds_the_window() {
    assert!(!Status::Idle.holds_window());
    assert!(!Status::Registering("daily".into()).holds_window());
    assert!(!Status::Failed("nope".into()).holds_window());
    assert!(Status::Creating("daily".into()).holds_window());

    assert!(!Status::Idle.busy());
    assert!(Status::Registering("daily".into()).busy());
    assert!(Status::Creating("daily".into()).busy());
}

/// A create in flight freezes the form and says so in its own words — "Creating", not the
/// registration's "Registering", because they are different work.
#[test]
fn a_create_in_flight_freezes_the_form_and_names_itself() {
    let (mut runner, (ctx, _, _)) = runner("internal-hold", false, internal_draft());
    settle(&mut runner);

    let mut status = ctx.status;
    status.set(Status::Creating("daily".into()));
    settle(&mut runner);

    assert!(shows(&runner, "Creating 'daily'…"), "{:?}", texts(&runner));
    ctx.edit(|draft| draft.name = "something else".into());
    assert_eq!(ctx.draft.peek().name, "daily");
}
