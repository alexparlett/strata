//! Project open against the real engine (P4-13 internals acceptance): load a committed
//! fixture project's defs, register every table (relative sources resolved against the
//! project folder), create every view, and query through one — the same chain the Freya
//! window root drives on launch, with no UI framework involved.
//!
//! This drives a **dedicated test fixture** (`tests/fixtures/loadfix/`), not the app's
//! real `sample/` project: the fixture is free to carry a deliberately-malformed source
//! to exercise the per-table Failed path, without wedging a broken file into the live
//! sample project.

use std::path::Path;

use strata_core::engine::{Engine, RunTag, WsId};
use strata_core::project::load_defs;
use strata_core::register::{register_project, RegOutcome};

/// The dedicated project-load fixture (see the module doc + its `README.md`).
fn fixture_root() -> &'static Path {
    Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/loadfix"
    ))
}

#[tokio::test]
async fn fixture_project_registers_and_queries() {
    let root = fixture_root();
    let defs = load_defs(root).expect("fixture project loads");
    assert_eq!(defs.name, "loadfix");
    assert!(!defs.tables.is_empty());
    assert!(!defs.views.is_empty());
    assert!(!defs.saved_queries.is_empty());

    let eng = Engine::new(Default::default());

    // The registration pass itself — the same `register_project` a headless host
    // replays, and the same `register_pass` the window root drives. A failed
    // registration is a landed per-entry outcome, not an abort (a row flips to
    // `Failed`, the rest of the project lives).
    let mut outcomes = Vec::new();
    register_project(&eng, root, &defs, |o| outcomes.push(o)).await;
    assert_eq!(
        outcomes.len(),
        defs.tables.len() + defs.views.len(),
        "one outcome per def: {outcomes:?}"
    );

    // Tables first (views read them), relative sources resolved against the folder.
    let mut failed = Vec::new();
    for (i, t) in defs.tables.iter().enumerate() {
        match &outcomes[i] {
            RegOutcome::Table { name, result } => {
                assert_eq!(name, &t.name, "tables settle in defs order");
                match result {
                    Ok(meta) => {
                        assert!(!meta.columns.is_empty(), "'{name}' inferred a schema")
                    }
                    Err(_) => failed.push(name.clone()),
                }
            }
            other => panic!("expected every table before any view: {other:?}"),
        }
    }
    // The fixture's one deliberate dud: `signups.json` has a record missing its closing brace,
    // so no reader can take it — a useful Failed-state fixture. (It was pretty-printed JSON until
    // `engine::json_poly` replaced arrow's line-based reader and started reading that correctly;
    // see the fixture README.)
    assert_eq!(failed, ["signups"]);

    // The hive-partitioned table carries its partition columns in the schema.
    let events = defs
        .tables
        .iter()
        .find(|t| t.name == "events")
        .expect("events table");
    assert_eq!(events.partition_cols.len(), 2);

    // Views: created over the registered tables, deps resolved by the planner.
    for outcome in &outcomes[defs.tables.len()..] {
        let RegOutcome::View { name, result } = outcome else {
            panic!("a table settled after a view: {outcome:?}");
        };
        let meta = result
            .as_ref()
            .unwrap_or_else(|e| panic!("create view '{name}': {e}"));
        assert!(!meta.columns.is_empty(), "'{name}' planned columns");
        assert!(!meta.tables.is_empty(), "'{name}' reads base tables");
    }

    // The whole point: a query through a view over the registered catalog answers.
    let (output, _) = eng
        .query(WsId(1), RunTag(1), "SELECT * FROM active_users".into(), 50)
        .await
        .expect("query the view");
    assert!(output.total > 0, "the view yields rows");

    // Dropping a view is idempotent and removes it.
    eng.drop_view("active_users".into()).await.expect("drop");
    eng.drop_view("active_users".into())
        .await
        .expect("drop again (IF EXISTS)");
    assert!(
        eng.query(WsId(1), RunTag(2), "SELECT * FROM active_users".into(), 50)
            .await
            .is_err(),
        "a dropped view no longer resolves"
    );
}
