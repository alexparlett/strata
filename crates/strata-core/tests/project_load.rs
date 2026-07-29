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

use strata_core::engine::{Engine, RunTag, TableSpec, WsId};
use strata_core::project::{load_defs, resolve_source};

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

    // Tables first (views read them), relative sources resolved against the folder.
    // A failed registration is a landed per-row answer, not an abort — exactly how the
    // window root treats it (a row flips to `Failed`, the rest of the project lives).
    let mut failed = Vec::new();
    for t in &defs.tables {
        let spec = TableSpec {
            name: t.name.clone(),
            paths: t.sources.iter().map(|s| resolve_source(root, s)).collect(),
            format: t.format.clone(),
            partitions: t.partition_cols.clone(),
        };
        match eng.register(spec).await {
            Ok(meta) => assert!(!meta.columns.is_empty(), "'{}' inferred a schema", t.name),
            Err(_) => failed.push(t.name.clone()),
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
    for v in &defs.views {
        let meta = eng
            .create_view(v.name.clone(), v.sql.clone())
            .await
            .unwrap_or_else(|e| panic!("create view '{}': {e}", v.name));
        assert!(!meta.columns.is_empty(), "'{}' planned columns", v.name);
        assert!(!meta.tables.is_empty(), "'{}' reads base tables", v.name);
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
