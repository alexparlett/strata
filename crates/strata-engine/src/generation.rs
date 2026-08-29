//! The catalog generation: one number per engine, moved by every write to its registries.

use std::sync::atomic::{AtomicU64, Ordering};

/// Which generation of an engine's catalog an answer was derived against.
///
/// Opaque and comparable: keep the value an answer was derived against, and re-derive when
/// [`Catalog::generation`](crate::Catalog::generation) stops matching it. A moved generation
/// means "re-ask", never "entry `x` changed". [`Default`] is the generation of an engine that
/// has registered nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CatalogGen(u64);

/// The counter behind [`CatalogGen`], one per engine.
///
/// `Relaxed` on both operations: no data is published through this number. A bump happens after
/// its registry write has landed behind that registry's own lock, and a read is only ever
/// compared with an earlier one.
#[derive(Debug, Default)]
pub(crate) struct GenClock(AtomicU64);

impl GenClock {
    /// The generation the catalog is at now.
    pub(crate) fn current(&self) -> CatalogGen {
        CatalogGen(self.0.load(Ordering::Relaxed))
    }

    /// Moves to the next generation and returns it.
    pub(crate) fn bump(&self) -> CatalogGen {
        CatalogGen(self.0.fetch_add(1, Ordering::Relaxed) + 1)
    }
}

/// The clock's own properties, and one assertion per gesture that has to move it.
///
/// Kept together rather than beside each facade method: the claim is that nothing moves the
/// catalog without moving the number, and a checklist is only checkable read whole.
#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::{env, fs, process};

    use strata_model::{SourceDef, SourceFormat, ViewDef};

    use super::*;
    use crate::register::CatalogSpec;
    use crate::sources::fake::{fake_def, TestDoc};
    use crate::{Engine, RunTag, TableSpec, WsId};

    /// The two properties every consumer leans on: a fresh clock is the seed, and a bump is
    /// always a value nobody has seen before.
    #[test]
    fn a_bump_is_always_a_new_value_and_the_seed_is_zero() {
        let clock = GenClock::default();
        assert_eq!(clock.current(), CatalogGen::default());

        let first = clock.bump();
        assert_ne!(first, CatalogGen::default());
        assert_eq!(clock.current(), first);

        let second = clock.bump();
        assert_ne!(second, first);
        assert!(second > first, "the clock only ever moves forward");
    }

    /// A scratch project folder per test, holding one two-column CSV.
    fn scratch(tag: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("strata_generation_{}_{tag}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("t.csv"), "id,name\n1,a\n2,b\n").unwrap();
        dir
    }

    fn spec(root: &Path, name: &str) -> TableSpec {
        TableSpec {
            name: name.into(),
            paths: vec![root.join("t.csv").display().to_string()],
            format: SourceFormat::from_name("csv"),
            partitions: Vec::new(),
            source: None,
            internal: false,
        }
    }

    /// A connection refused before any socket opens (S3 with no region), so the data source
    /// gestures can be driven without dialing out.
    fn unreachable(name: &str) -> SourceDef {
        SourceDef {
            config: [("address".to_string(), "no-region".into())]
                .into_iter()
                .collect(),
            kind: "s3".into(),
            name: name.into(),
            ..Default::default()
        }
    }

    /// Every gesture that changes what a name resolves to moves the generation, and the one that
    /// changes only a row's counts does not.
    ///
    /// Driven through the facade, because that is the surface a host has: a test that bumped the
    /// clock directly would pass against an engine that had stopped calling it. Each step
    /// asserts against the number the previous one left, so a gesture that stops minting fails
    /// on its own line.
    #[tokio::test]
    async fn every_catalog_gesture_moves_the_generation() {
        let root = scratch("gestures");
        let engine = Engine::builder()
            .with_data_dir(&root)
            .with_source(TestDoc::holding("fixture", &["orders"]))
            .build();
        let catalog = engine.catalog();
        let mut seen = catalog.generation();
        assert_eq!(seen, CatalogGen::default(), "an engine holding nothing");

        let mut moved = |what: &str, engine: &Engine| {
            let now = engine.catalog().generation();
            assert_ne!(now, seen, "{what} left the generation where it was");
            seen = now;
        };

        catalog.register(spec(&root, "t")).await.expect("register");
        moved("registering a table", &engine);

        catalog
            .register(spec(&root, "gone"))
            .await
            .expect("register");
        moved("registering a second table", &engine);
        catalog.deregister("gone");
        moved("deregistering a table", &engine);

        catalog
            .create_view("v".into(), "SELECT id FROM t".into())
            .await
            .expect("create view");
        moved("creating a view through the save gesture", &engine);

        catalog.drop_view("v".into()).await.expect("drop view");
        moved("dropping a view", &engine);

        engine
            .ws(WsId(1))
            .run(
                RunTag(1),
                "CREATE VIEW typed AS SELECT id FROM t".into(),
                10,
            )
            .await
            .expect("typed view DDL");
        moved("a typed statement that upserts a view", &engine);

        engine
            .ws(WsId(1))
            .run(
                RunTag(2),
                "CREATE FUNCTION add_one(x BIGINT) RETURNS BIGINT RETURN x + 1".into(),
                10,
            )
            .await
            .expect("create function");
        moved(
            "a created function, which changes what a call resolves to",
            &engine,
        );

        catalog
            .register(spec(&root, "spare"))
            .await
            .expect("register");
        moved("registering the table the typed drop is about", &engine);
        engine
            .ws(WsId(1))
            .run(RunTag(3), "DROP TABLE spare".into(), 10)
            .await
            .expect("typed drop");
        moved("a typed statement that removes a table", &engine);

        let _ = engine.sources().connect(unreachable("lake")).await;
        moved("a data source, refused or not", &engine);
        engine
            .sources()
            .connect(fake_def::<TestDoc>("sales", "fixture"))
            .await
            .expect("the fake source connects");
        moved("a source registering its catalog", &engine);
        engine
            .sources()
            .show_schemas("sales", &["public".to_string()]);
        moved("changing which schemas a data source shows", &engine);
        engine.sources().disconnect("lake");
        moved("forgetting a data source", &engine);

        let report = engine.catalog().sync(CatalogSpec::default(), |_| {}).await;
        moved("a pass that took the remaining views out", &engine);
        assert_eq!(
            report.generation,
            engine.catalog().generation(),
            "the report answers with the generation the pass left the catalog at"
        );

        let before = engine.catalog().generation();
        engine
            .ws(WsId(1))
            .query(RunTag(4), "SELECT 1 AS n".into(), 10)
            .await
            .expect("a plain read");
        assert_eq!(
            engine.catalog().generation(),
            before,
            "reading moves nothing — the clock is not a call counter"
        );

        let _ = fs::remove_dir_all(&root);
    }

    /// A pass reconciling a catalog against the same catalog still moves the clock, because
    /// every table in it is re-registered against whatever is on disk now — and it answers with
    /// the generation it left, never an older one.
    #[tokio::test]
    async fn a_pass_answers_with_the_generation_it_left() {
        let root = scratch("pass");
        let engine = Engine::builder().with_data_dir(&root).build();
        let desired = CatalogSpec {
            tables: vec![spec(&root, "t")],
            views: vec![ViewDef {
                name: "v".into(),
                sql: "SELECT id FROM t".into(),
            }],
            ..Default::default()
        };

        let first = engine.catalog().sync(desired.clone(), |_| {}).await;
        assert_ne!(first.generation, CatalogGen::default());

        let second = engine.catalog().sync(desired, |_| {}).await;
        assert!(
            second.generation > first.generation,
            "a re-scan re-registers, so it moves the clock even against an unchanged spec"
        );
        assert_eq!(second.generation, engine.catalog().generation());

        let _ = fs::remove_dir_all(&root);
    }
}
