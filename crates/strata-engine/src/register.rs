//! The **project registration pass** (AA-01) — one implementation of "register the
//! defs on the engine": connect each connection, register each table, then create each
//! view, and report what the engine answered per def. Extracted from the Freya app's
//! project-open hook so a headless host (AA-05) can run the same sequence with no store
//! to fold into; the app's hook consumes [`register_pass`] and keeps only what is
//! genuinely the store's (`Reg<T>` rows, epochs, log entries).
//!
//! Three things stay the caller's, each named because the headless replayer is the caller this
//! module was cut for:
//!
//! - **Loading the defs** ([`load_defs`](strata_core::project::load_defs)) and acting on the outcomes.
//!   The pass reports outcomes, never introspects DataFusion, and nothing refetches.
//! - **Removal.** The pass is additive: it registers and re-creates, and never deregisters an
//!   engine object whose def is gone. A host replaying a defs file that may have shrunk must diff
//!   the names it registered and deregister the difference first, or a removed table stays
//!   silently queryable. A **connection** is the same case through [`Engine::disconnect`], which
//!   an edit moving a connection's bucket or provider owes too, since that changes the `url()` the
//!   store went in under.
//! - **The registration window.** [`Engine::register`] deregisters before it re-infers, so for the
//!   duration of a pass every table being rebuilt is absent from the catalog. The app gates
//!   validation behind its scan claim; a host serving `validate`, `policy_verdicts` or queries
//!   concurrently must hold them off the same way, or it answers a false, transient "not found"
//!   for a table sitting right there.

use std::path::Path;

use futures::stream::{self, StreamExt};
use strata_model::{ConnectionDef, TableDef};

use crate::store::store_prefix;
use crate::{Connections, Engine, TableMeta, TableSpec, ViewMeta};
use strata_core::project::{resolve_source, ProjectDefs};

/// How many tables register at once ([`register_pass`]'s table phase).
///
/// A ceiling on *this* pass's fan-out, not a parallelism target: a single registration already
/// fetches `datafusion.execution.meta_fetch_concurrency` (32 by default) file footers in
/// parallel, so eight tables is already ~256 requests in flight against whatever store they
/// live on. Enough that a slow remote table cannot hold up a project's worth of local ones,
/// small enough that opening a project is not a thundering herd at one bucket.
const TABLE_CONCURRENCY: usize = 8;

/// What the engine answered for one def — the pass's per-entry product. A failed entry
/// does not abort the pass; its outcome is the row.
#[derive(Clone, Debug, PartialEq)]
pub enum RegOutcome {
    /// A connection's object store or database catalog went in, or the connection could not
    /// describe one ([`Engine::connect`]). Nothing the *store* learns is reported here — an
    /// object store is registered, not inferred, and a database's enumeration is read back
    /// through [`Engine::source_listing`] rather than folded onto a row — so the payload is the
    /// answer itself.
    Connection {
        /// The connection's own **name**, **not** its address. An address alone is not unique —
        /// two connections may read one bucket, or reach one server as two roles — so a caller
        /// folding these answers onto rows by address would land both on whichever it found
        /// first and leave the other unanswered forever.
        url: String,
        result: Result<(), String>,
    },
    Table {
        name: String,
        result: Result<TableMeta, String>,
    },
    View {
        name: String,
        result: Result<ViewMeta, String>,
    },
}

/// The engine-facing projection of one table def: sources resolved through [`resolve_source`] —
/// composed onto the store its connection registered under where it names one, and otherwise
/// joined onto the project folder — with everything else carried as stored. One copy of the
/// mapping, shared by the app's catalog passes and [`register_project`].
///
/// **A table names its connection, and only a registry can say what that connection is.** The def
/// carries a name, the store is registered under `scheme://address`, and nothing about the first
/// yields the second — so this takes the registry rather than a string, which is what stops a
/// caller handing over the name and getting a path with no scheme in it. `None` for a name the
/// project has no connection for: the sources then compose as written and registration fails with
/// DataFusion's "No suitable object store", which is the honest answer.
///
/// A remote path needs **nothing** of the engine beyond this: the connection's object store is
/// already registered under that same URL by the time any table registers (connections are
/// [`register_pass`]'s first phase), so `s3://acme-lake/events/` is a `ListingTableUrl` the
/// session can already resolve.
pub fn table_spec(root: &Path, def: &TableDef, connections: &Connections) -> TableSpec {
    let prefix = def
        .connection
        .as_deref()
        .and_then(|named| connections.identity(named))
        .and_then(|identity| store_prefix(&identity));
    TableSpec {
        name: def.name.clone(),
        paths: def
            .sources
            .iter()
            .map(|s| resolve_source(root, prefix.as_deref(), s))
            .collect(),
        format: def.format.clone(),
        partitions: def.partition_cols.clone(),
        internal: def.origin.is_internal(),
    }
}

/// Order `views` so a view is re-created **after** every view it reads.
///
/// `CREATE OR REPLACE VIEW` inlines the plan of any view it reads *at that moment*, so
/// re-creating an outer view before its inner one inlines the stale inner plan — and
/// with it the very provider the pass is replacing. The sharp part is that nothing
/// errors: the outer `CREATE` succeeds, so no retry fires and no row goes `Failed`.
/// Defs-file order gets this right only by luck.
///
/// Kahn's algorithm over `deps`, restricted to the set being ordered: dependencies
/// *outside* the set are already current, so they can't order anything. `deps` answers
/// a view's known view-dependencies — for the app, the store's landed
/// `ViewInfo::view_deps`; for a replayer, the previous pass's `ViewMeta::aliases`
/// filtered to view names. Names compare case-insensitively (the engine folds unquoted
/// identifiers). A view with no known deps sorts wherever it falls — from cold that is
/// every view, which is why [`register_pass`] keeps its fixed-point retry as well. A
/// cycle is impossible (a view can't read itself, and DataFusion refuses mutual
/// recursion), but a residue is appended rather than dropped: a surprise can cost
/// ordering, never a re-create.
pub fn view_order(views: Vec<String>, deps: impl Fn(&str) -> Vec<String>) -> Vec<String> {
    let mut remaining = views;
    let mut ordered = Vec::with_capacity(remaining.len());
    while !remaining.is_empty() {
        let (ready, blocked): (Vec<String>, Vec<String>) =
            remaining.iter().cloned().partition(|name| {
                !deps(name)
                    .iter()
                    .any(|d| remaining.iter().any(|r| r.eq_ignore_ascii_case(d)))
            });
        if ready.is_empty() {
            ordered.extend(blocked);
            break;
        }
        ordered.extend(ready);
        remaining = blocked;
    }
    ordered
}

/// Connect `connections`, register `tables`, then create `views` on `engine`, handing
/// `settled` what it answered for each. **Ordering is the contract**: connections first
/// (a table's source path cannot resolve to an object store that isn't registered, and a
/// view over `pg.public.orders` cannot plan before that catalog exists — see
/// [`Engine::connect`]); then tables (a view's SQL reads tables), **concurrently and in no
/// particular order** ([`TABLE_CONCURRENCY`]); then views by fixed-point rounds — DataFusion
/// requires a view's dependencies to exist when its `CREATE VIEW` plans, so from cold,
/// each round creates what it can and a view whose dependency landed last round
/// succeeds this round. A round without progress means the remainder are genuinely
/// broken (bad SQL or a missing table) and their errors are their outcomes. Against an
/// engine that **already holds these views**, the retry cannot order anything (every
/// `CREATE OR REPLACE` succeeds round one) — hand `views` in dependency order
/// ([`view_order`]) or an outer view inlines a stale inner plan.
///
/// Connections need no ordering among themselves and are not retried: each registers one bucket or
/// one catalog and reads nothing the pass provides. What a failure *costs* differs by kind — an
/// object store takes the tables over its bucket with it, each saying so on its own row, while a
/// **database** has no def rows at all and leaves nothing failed but its own row and whatever views
/// read across it.
///
/// `settled` is called with each outcome as the engine answers it, so the app folds catalog rows
/// and log entries per answer rather than after the whole pass. A failed entry never aborts the
/// pass, and a view retried across rounds settles **once**, on its final answer.
pub async fn register_pass(
    engine: &Engine,
    connections: Vec<ConnectionDef>,
    tables: Vec<TableSpec>,
    views: Vec<(String, String)>,
    mut settled: impl FnMut(RegOutcome),
) {
    for conn in connections {
        let url = conn.identity();
        let result = engine.connect(conn).await;
        settled(RegOutcome::Connection { url, result });
    }

    let mut registrations = stream::iter(tables)
        .map(|spec| {
            let name = spec.name.clone();
            async move {
                let result = engine.register(spec).await;
                RegOutcome::Table { name, result }
            }
        })
        .buffer_unordered(TABLE_CONCURRENCY);
    while let Some(outcome) = registrations.next().await {
        settled(outcome);
    }

    let mut pending = views;
    while !pending.is_empty() {
        let before = pending.len();
        let mut failed = Vec::new();
        for (name, sql) in pending {
            match engine.create_view(name.clone(), sql.clone()).await {
                Ok(meta) => settled(RegOutcome::View {
                    name,
                    result: Ok(meta),
                }),
                Err(e) => failed.push((name, sql, e)),
            }
        }
        if failed.len() == before {
            for (name, _, e) in failed {
                settled(RegOutcome::View {
                    name,
                    result: Err(e),
                });
            }
            break;
        }
        pending = failed.into_iter().map(|(n, s, _)| (n, s)).collect();
    }
}

/// The whole-project pass **from cold**: every connection, table and view in `defs`,
/// sources resolved against `root`, views in defs order — right for an engine that holds
/// none of them yet, where the fixed-point retry finds the dependency order by creating
/// what it can. It is *not* the re-run: against an engine that already holds these
/// views (the second pass of a long-lived host), defs order silently inlines stale
/// plans — order the views with [`view_order`] over the previous pass's answers and
/// call [`register_pass`], which is exactly what the app does
/// (`ProjectState::refresh_order`).
pub async fn register_project(
    engine: &Engine,
    root: &Path,
    defs: &ProjectDefs,
    settled: impl FnMut(RegOutcome),
) {
    let connections = defs.connections.clone();
    let known = Connections::of(&connections);
    let tables = defs
        .tables
        .iter()
        .map(|def| table_spec(root, def, &known))
        .collect();
    let views = defs
        .views
        .iter()
        .map(|v| (v.name.clone(), v.sql.clone()))
        .collect();
    register_pass(engine, connections, tables, views, settled).await;
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::PathBuf;
    use std::{env, process};

    use strata_model::{
        GcsAuth, GcsStore, Provider, S3Auth, S3Store, SourceFormat, TableOrigin, ViewDef,
    };

    use super::*;

    /// A scratch project folder of our own, per test.
    fn scratch(tag: &str) -> PathBuf {
        let d = env::temp_dir().join(format!("strata_register_pass_{}_{tag}", process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// A CSV table def over a project-relative source — so the pass is exercised
    /// through [`table_spec`]'s resolution, not around it.
    fn table(name: &str, source: &str) -> TableDef {
        TableDef {
            name: name.into(),
            format: SourceFormat::from_name("csv"),
            connection: None,
            sources: vec![source.into()],
            partition_cols: Vec::new(),
            origin: TableOrigin::External,
        }
    }

    fn view(name: &str, sql: &str) -> ViewDef {
        ViewDef {
            name: name.into(),
            sql: sql.into(),
        }
    }

    async fn run(root: &Path, defs: &ProjectDefs) -> Vec<RegOutcome> {
        let engine = Engine::builder().build();
        let mut out = Vec::new();
        register_project(&engine, root, defs, |o| out.push(o)).await;
        out
    }

    /// Each outcome as `(identity, did it settle Ok)`, in the order the pass answered.
    fn names(out: &[RegOutcome]) -> Vec<(&str, bool)> {
        out.iter()
            .map(|o| match o {
                RegOutcome::Connection { url, result } => (url.as_str(), result.is_ok()),
                RegOutcome::Table { name, result } => (name.as_str(), result.is_ok()),
                RegOutcome::View { name, result } => (name.as_str(), result.is_ok()),
            })
            .collect()
    }

    /// The happy path: the table settles first, then the view.
    #[tokio::test]
    async fn tables_then_views_register_in_order() {
        let root = scratch("happy");
        fs::write(root.join("t.csv"), "id,name\n1,a\n2,b\n").unwrap();
        let defs = ProjectDefs {
            tables: vec![table("t", "t.csv")],
            views: vec![view("v", "SELECT id FROM t")],
            ..Default::default()
        };

        let out = run(&root, &defs).await;

        match &out[..] {
            [RegOutcome::Table {
                name: t,
                result: Ok(meta),
            }, RegOutcome::View {
                name: v,
                result: Ok(vmeta),
            }] => {
                assert_eq!(t, "t");
                assert_eq!(meta.columns.len(), 2);
                assert_eq!(v, "v");
                assert_eq!(vmeta.columns.len(), 1);
            }
            other => panic!("{other:?}"),
        }
    }

    /// A failed table is its row, not the pass's: the outcome is `Err` and the rest
    /// proceed.
    ///
    /// Asserted **without regard to order**, deliberately. Tables register concurrently
    /// ([`TABLE_CONCURRENCY`]), so which of two settles first is a race — and it is not a
    /// property worth pinning: `settled` is documented as "called with each outcome as the
    /// engine answers it", and the app folds outcomes onto catalog rows by name. A positional
    /// assertion here would be testing the scheduler.
    #[tokio::test]
    async fn a_failed_table_does_not_abort_the_pass() {
        let root = scratch("bad_table");
        fs::write(root.join("good.csv"), "id\n1\n").unwrap();
        let defs = ProjectDefs {
            tables: vec![table("bad", "missing.csv"), table("good", "good.csv")],
            ..Default::default()
        };

        let out = run(&root, &defs).await;

        let mut settled = names(&out);
        settled.sort_unstable();
        assert_eq!(settled, vec![("bad", false), ("good", true)], "{out:?}");
    }

    /// The table phase settles every def exactly once, in no guaranteed order.
    ///
    /// Concurrency is why the order is unguaranteed: a serial pass cost the sum of its tables, so
    /// one wide remote table (thousands of parquet footers over S3) stalled every local table
    /// behind it — and the app's scan claim, and with it validation, for the same duration.
    ///
    /// **What it does not assert is that the phase is concurrent**, and that is worth stating
    /// rather than leaving as a gap someone fills in badly later.
    ///
    /// The obvious test is completion order against defs order: list the expensive table first,
    /// assert it does not settle first, since the serial loop this replaced settled it first
    /// every time. That version was written, shipped, and **flaked** — a fast table only
    /// overtakes a slow one if the runtime gives it a thread, and on a machine already running a
    /// build it may not until the slow one is done. It is a race the concurrent implementation
    /// usually wins and is not guaranteed to, which makes it a test that fails at random. A
    /// flaky test is worse than an absent one: it trains people to re-run rather than to read.
    ///
    /// There is no seam on `Engine` to inject a controllable delay, so there is nothing here to
    /// assert deterministically. The concurrency is one `buffer_unordered` in a function whose
    /// shape is the assertion; what this test pins is what a test *can* pin — that the phase
    /// still settles every table exactly once, whatever order they arrive in, which is the
    /// property the app's fold depends on.
    #[tokio::test]
    async fn every_table_settles_exactly_once_whatever_the_order() {
        let root = scratch("concurrent_tables");
        let mut wide = String::from("id,name\n");
        for i in 0..50_000 {
            wide.push_str(&format!("{i},row{i}\n"));
        }
        fs::write(root.join("wide.csv"), &wide).unwrap();
        fs::write(root.join("a.csv"), "id\n1\n").unwrap();
        fs::write(root.join("b.csv"), "id\n1\n").unwrap();
        let defs = ProjectDefs {
            tables: vec![
                table("wide", "wide.csv"),
                table("a", "a.csv"),
                table("b", "b.csv"),
            ],
            ..Default::default()
        };

        let out = run(&root, &defs).await;

        let mut settled = names(&out);
        settled.sort_unstable();
        assert_eq!(
            settled,
            vec![("a", true), ("b", true), ("wide", true)],
            "one outcome per table, none dropped and none doubled: {out:?}"
        );
    }

    /// A view over a table that failed to register gets whatever the engine answers for
    /// its CREATE — asserted, not assumed: an error, and one that names the table.
    #[tokio::test]
    async fn a_view_over_a_failed_table_reports_the_engine_answer() {
        let root = scratch("view_over_failed");
        let defs = ProjectDefs {
            tables: vec![table("gone", "missing.csv")],
            views: vec![view("v", "SELECT * FROM gone")],
            ..Default::default()
        };

        let out = run(&root, &defs).await;

        assert_eq!(out.len(), 2, "{out:?}");
        let RegOutcome::View {
            name,
            result: Err(e),
        } = &out[1]
        else {
            panic!("{out:?}");
        };
        assert_eq!(name, "v");
        assert!(e.contains("gone"), "{e}");
    }

    /// From cold, views arrive in whatever order the defs hold; a view over a view
    /// given first succeeds on the round after its dependency lands — the fixed-point
    /// retry.
    #[tokio::test]
    async fn view_dependencies_resolve_across_rounds() {
        let root = scratch("view_rounds");
        fs::write(root.join("t.csv"), "id\n1\n").unwrap();
        let defs = ProjectDefs {
            tables: vec![table("t", "t.csv")],
            views: vec![
                view("top_v", "SELECT id FROM base_v"),
                view("base_v", "SELECT id FROM t"),
            ],
            ..Default::default()
        };

        let out = run(&root, &defs).await;

        assert_eq!(
            names(&out),
            vec![("t", true), ("base_v", true), ("top_v", true)],
            "{out:?}"
        );
    }

    /// **Connections come first, before any table** — and each is answered under its own
    /// **URL**, not its bucket.
    ///
    /// Both halves are load-bearing. A source path under a bucket resolves through the object
    /// store registered for it, so a table that registers before its connection fails on a def
    /// that is perfectly correct — an ordering bug that would look exactly like a broken table.
    /// And a bucket is not unique across providers: the two `lake` defs below are two
    /// connections and two registry keys, so an outcome carrying only `"lake"` would be
    /// indistinguishable between them, and a caller folding by it would answer one row twice
    /// and leave the other waiting forever.
    ///
    /// **Every connection here is one that is refused locally**, and that is deliberate.
    /// `Engine::connect` now asks the bucket whether it answers (`store::reachable`), so a def
    /// that is merely *well-formed* is no longer one this test can settle `Ok` — it would send
    /// this suite to `s3.eu-west-2.amazonaws.com` and `storage.googleapis.com` on every run, for
    /// buckets nobody owns, and fail on a plane. Each of the three is refused before any socket
    /// opens (a blank region twice, a blank service-account path once), which costs the test
    /// nothing it was actually asserting: the subject is *order* and *identity*, and an outcome
    /// carries its URL whether it succeeded or not. `("local", true)` is still what proves the
    /// pass carried on to the table phase after three refusals.
    #[tokio::test]
    async fn connections_settle_first_and_each_under_its_own_url() {
        let root = scratch("connections");
        fs::write(root.join("local.csv"), "id\n1\n").unwrap();
        let defs = ProjectDefs {
            connections: vec![
                ConnectionDef {
                    address: "lake".into(),
                    name: String::new(),
                    provider: Provider::S3(S3Store {
                        auth: S3Auth::Anonymous,
                        ..Default::default()
                    }),
                    client_config: Default::default(),
                },
                ConnectionDef {
                    address: "lake".into(),
                    name: String::new(),
                    provider: Provider::Gcs(GcsStore {
                        auth: GcsAuth::ServiceAccount {
                            path: String::new(),
                        },
                    }),
                    client_config: Default::default(),
                },
                ConnectionDef {
                    address: "no-region".into(),
                    name: String::new(),
                    provider: Provider::S3(S3Store {
                        auth: S3Auth::Anonymous,
                        ..Default::default()
                    }),
                    client_config: Default::default(),
                },
            ],
            tables: vec![table("local", "local.csv")],
            ..Default::default()
        };

        let out = run(&root, &defs).await;

        assert_eq!(
            names(&out),
            vec![
                ("s3:lake", false),
                ("gcs:lake", false),
                ("s3:no-region", false),
                ("local", true)
            ],
            "{out:?}"
        );
    }

    /// **A def that names a connection is composed onto that connection's store, never onto the
    /// project folder.** The engine half needs nothing further: the store went in under that same
    /// URL in the pass's first phase, so what reaches `register` is an address the session can
    /// already resolve.
    ///
    /// The def carries the connection's **name**, so the lookup is the point — driven here
    /// through a real registry rather than by handing the composition a pre-made identity, which
    /// is exactly how this went green while a real bucket read `//acme-lake/events/`. A name the
    /// registry does not hold composes nothing, which is what makes registration fail loudly.
    ///
    /// The failure the local half pins is silent rather than loud: a bucket-relative source under
    /// the local rule becomes `<project>/events/2024/`, a missing folder on the user's own disk
    /// that says nothing about a bucket.
    #[test]
    fn a_table_over_a_connection_resolves_against_its_bucket() {
        let known = Connections::of(&[ConnectionDef {
            address: "acme-lake".into(),
            name: "acme_lake".into(),
            provider: Provider::S3(S3Store::default()),
            client_config: BTreeMap::new(),
        }]);
        let def = TableDef {
            name: "events".into(),
            format: SourceFormat::from_name("parquet"),
            connection: Some("acme_lake".into()),
            sources: vec!["events/2024/**/*.parquet".into()],
            partition_cols: Vec::new(),
            origin: TableOrigin::External,
        };
        assert_eq!(
            table_spec(Path::new("/proj"), &def, &known).paths,
            ["s3://acme-lake/events/2024/**/*.parquet"]
        );
        let stranded = TableDef {
            connection: Some("gone".into()),
            ..def.clone()
        };
        assert_eq!(
            table_spec(Path::new("/proj"), &stranded, &known).paths,
            ["/proj/events/2024/**/*.parquet"],
            "a name the project has no connection for composes nothing remote"
        );
        let local = TableDef {
            connection: None,
            ..def
        };
        assert_eq!(
            table_spec(Path::new("/proj"), &local, &Connections::default()).paths,
            ["/proj/events/2024/**/*.parquet"]
        );
    }

    /// The ordering rule on its own: a chain sorts dependencies-first regardless of
    /// input order, names compare case-insensitively, and a view with no recorded deps
    /// sorts wherever it falls rather than blocking anything.
    #[test]
    fn view_order_puts_a_view_after_what_it_reads() {
        let deps = |name: &str| -> Vec<String> {
            match name {
                "outer" => vec!["Middle".into()],
                "middle" => vec!["base".into()],
                _ => Vec::new(),
            }
        };
        assert_eq!(
            view_order(vec!["outer".into(), "middle".into(), "base".into()], deps),
            vec!["base".to_string(), "middle".into(), "outer".into()],
            "dependencies first, and 'Middle' orders 'middle' despite the case"
        );
        assert_eq!(
            view_order(vec!["outer".into()], deps),
            vec!["outer".to_string()]
        );
    }
}
