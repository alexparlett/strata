use async_trait::async_trait;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;
use strata_engine::{
    Admit, DenyCode, Engine, GrantFamily, PolicyProvider, Principal, RunTag, TargetFacts, WsId,
};
use tokio::sync::Notify;

struct DenyAll;

#[async_trait]
impl PolicyProvider for DenyAll {
    async fn admit(&self, _: &Principal, _: GrantFamily) -> Result<Admit, String> {
        Ok(Admit::Deny(DenyCode::NotGranted))
    }
    async fn permit(
        &self,
        _: &Principal,
        _: GrantFamily,
        _: &TargetFacts,
    ) -> Result<Admit, String> {
        Ok(Admit::Deny(DenyCode::NotGranted))
    }
}

#[tokio::test]
async fn an_unobserved_statement_commits_its_definition() {
    use strata_core::project::{load_defs, ProjectDefs, ProjectStore};
    let root = std::env::temp_dir().join(format!("strata-unobserved-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let store = ProjectStore::new(root.clone(), ProjectDefs::default());
    let engine = Engine::builder().with_project_store(store.clone()).build();
    let mut running = Box::pin(engine.ws(WsId(1)).run(
        RunTag(1),
        "CREATE TABLE t AS SELECT 1 AS n".into(),
        10,
    ));
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if !store.snapshot().tables.is_empty() {
                break;
            }
            if futures::poll!(running.as_mut()).is_ready() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    drop(running);
    assert_eq!(load_defs(&root).unwrap().tables[0].name, "t");
    engine
        .ws(WsId(1))
        .run(RunTag(2), "DROP TABLE t".into(), 10)
        .await
        .unwrap();
    assert!(load_defs(&root).unwrap().tables.is_empty());
    drop(engine);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn a_definition_write_failure_is_reported_and_can_be_retried() {
    use strata_core::project::{load_defs, ProjectDefs, ProjectStore};
    use strata_engine::{Persistence, RunOutcome};
    let root = std::env::temp_dir().join(format!("strata-commit-failed-{}", std::process::id()));
    let obstruction = root.join(".strata/project.json");
    std::fs::create_dir_all(&obstruction).unwrap();
    let store = ProjectStore::new(root.clone(), ProjectDefs::default());
    let engine = Engine::builder().with_project_store(store.clone()).build();
    let RunOutcome::Statement(report) = engine
        .ws(WsId(1))
        .run(RunTag(1), "CREATE VIEW v AS SELECT 1".into(), 10)
        .await
        .unwrap()
    else {
        panic!("statement");
    };
    assert!(matches!(report.persistence, Persistence::Failed(_)));
    assert_eq!(store.snapshot().views[0].name, "v");
    std::fs::remove_dir(&obstruction).unwrap();
    store.update(|_| {}).unwrap();
    assert_eq!(load_defs(&root).unwrap().views[0].name, "v");
    drop(engine);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn every_read_entry_enforces_the_engine_policy() {
    let engine = Engine::builder().with_policy(DenyAll).build();
    assert!(engine
        .ws(WsId(1))
        .run(RunTag(1), "SELECT 1".into(), 10)
        .await
        .is_err());
    assert!(engine
        .ws(WsId(1))
        .query(RunTag(2), "SELECT 1".into(), 10)
        .await
        .is_err());
    assert!(engine
        .ws(WsId(1))
        .explain(RunTag(3), "EXPLAIN ANALYZE SELECT 1".into())
        .await
        .is_err());
}

#[tokio::test]
async fn colliding_storage_names_are_refused_without_changing_the_owner() {
    let root = std::env::temp_dir().join(format!("strata-audit-collision-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let engine = Engine::builder().with_data_dir(&root).build();
    engine
        .ws(WsId(1))
        .run(
            RunTag(1),
            "CREATE TABLE \"sales eu\" AS SELECT 1 AS n".into(),
            10,
        )
        .await
        .unwrap();
    assert!(engine
        .ws(WsId(1))
        .run(
            RunTag(2),
            "CREATE TABLE \"sales_eu-2dc32f1b\" AS SELECT 2 AS n".into(),
            10
        )
        .await
        .is_err());
    let rows = engine
        .ws(WsId(1))
        .query(RunTag(3), "SELECT n FROM \"sales eu\"".into(), 10)
        .await
        .unwrap();
    assert_eq!(rows.output.rows[0][0].text, "1");
    assert!(engine
        .ws(WsId(1))
        .run(RunTag(4), "DROP TABLE \"sales_eu-2dc32f1b\"".into(), 10)
        .await
        .is_err());
    assert!(root.join(".strata/tables/sales_eu-2dc32f1b").exists());
    drop(engine);
    std::fs::remove_dir_all(root).unwrap();
}

struct SlowFirst {
    calls: AtomicUsize,
    entered: Arc<Notify>,
    release: Arc<Notify>,
}

#[async_trait]
impl PolicyProvider for SlowFirst {
    async fn admit(&self, _: &Principal, _: GrantFamily) -> Result<Admit, String> {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            self.entered.notify_one();
            self.release.notified().await;
        }
        Ok(Admit::Allow)
    }
    async fn permit(
        &self,
        _: &Principal,
        _: GrantFamily,
        _: &TargetFacts,
    ) -> Result<Admit, String> {
        Ok(Admit::Allow)
    }
}

#[tokio::test]
async fn a_superseded_classification_cannot_dispatch_after_cleanup() {
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let engine = Engine::builder()
        .with_policy(SlowFirst {
            calls: AtomicUsize::new(0),
            entered: entered.clone(),
            release: release.clone(),
        })
        .build();
    let first_engine = engine.clone();
    let first = tokio::spawn(async move {
        first_engine
            .ws(WsId(1))
            .run(RunTag(1), "SELECT 1".into(), 10)
            .await
    });
    tokio::time::timeout(Duration::from_secs(10), entered.notified())
        .await
        .unwrap();
    engine
        .ws(WsId(1))
        .run(RunTag(2), "SELECT 2".into(), 10)
        .await
        .unwrap();
    assert!(!engine.ws(WsId(1)).is_running());
    engine.ws(WsId(1)).cleanup();
    release.notify_one();
    assert!(tokio::time::timeout(Duration::from_secs(10), first)
        .await
        .unwrap()
        .unwrap()
        .is_err());
}
