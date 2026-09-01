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

use strata_core::project::load_defs;
use strata_engine::register::RegOutcome;
use strata_engine::{Engine, RunTag, WsId};

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

    let eng = Engine::builder().build();

    let mut outcomes = Vec::new();
    eng.catalog()
        .sync(eng.catalog().spec(root, &defs), |a| outcomes.push(a.outcome))
        .await;
    assert_eq!(
        outcomes.len(),
        defs.sources.len() + defs.tables.len() + defs.views.len(),
        "one outcome per def: {outcomes:?}"
    );

    let after_sources = defs.sources.len();

    let mut failed = Vec::new();
    let mut settled = Vec::new();
    for outcome in &outcomes[after_sources..after_sources + defs.tables.len()] {
        match outcome {
            RegOutcome::Table { name, result } => {
                settled.push(name.clone());
                match result {
                    Ok(meta) => {
                        assert!(!meta.columns.is_empty(), "'{name}' inferred a schema");
                    }
                    Err(_) => failed.push(name.clone()),
                }
            }
            other => panic!("expected every table before any view: {other:?}"),
        }
    }
    settled.sort();
    let mut expected: Vec<String> = defs.tables.iter().map(|t| t.name.clone()).collect();
    expected.sort();
    assert_eq!(settled, expected, "one outcome per table def");
    failed.sort();
    assert_eq!(failed, ["signups"]);

    let events = defs
        .tables
        .iter()
        .find(|t| t.name == "events")
        .expect("events table");
    assert_eq!(events.partition_cols.len(), 2);

    for outcome in &outcomes[after_sources + defs.tables.len()..] {
        let RegOutcome::View { name, result } = outcome else {
            panic!("a table settled after a view: {outcome:?}");
        };
        let meta = result
            .as_ref()
            .unwrap_or_else(|e| panic!("create view '{name}': {e}"));
        assert!(!meta.columns.is_empty(), "'{name}' planned columns");
        assert!(!meta.tables.is_empty(), "'{name}' reads base tables");
    }

    let output = eng
        .ws(WsId(1))
        .query(RunTag(1), "SELECT * FROM active_users".into(), 50)
        .await
        .expect("query the view")
        .output;
    assert!(output.total > 0, "the view yields rows");

    eng.catalog()
        .drop_view("active_users".into())
        .await
        .expect("drop");
    eng.catalog()
        .drop_view("active_users".into())
        .await
        .expect("drop again (IF EXISTS)");
    assert!(
        eng.ws(WsId(1))
            .query(RunTag(2), "SELECT * FROM active_users".into(), 50)
            .await
            .is_err(),
        "a dropped view no longer resolves"
    );
}
