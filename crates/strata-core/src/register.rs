//! The **project registration pass** (AA-01) — one implementation of "register the
//! defs on the engine": connect each object store, register each table, then create each
//! view, and report what the engine answered per def. Extracted from the Freya app's
//! project-open hook so a headless host (AA-05) can run the same sequence with no store
//! to fold into; the app's hook consumes [`register_pass`] and keeps only what is
//! genuinely the store's (`Reg<T>` rows, epochs, log entries).
//!
//! Three things stay the caller's, and each is named because the headless replayer is
//! the caller this module was cut for:
//!
//! - **Loading the defs** ([`load_defs`](crate::project::load_defs)) and acting on the
//!   outcomes — the catalog-is-the-store rule is untouched: the pass reports outcomes,
//!   it never introspects DataFusion, and nothing refetches.
//! - **Removal.** The pass is additive: it registers and re-creates, it never
//!   deregisters an engine object whose def is gone. The app's removals are their own
//!   gestures (the drop confirm, through [`Engine::deregister`] / `Engine::drop_view`);
//!   a host replaying a defs file that may have shrunk since its last pass diffs the
//!   names it registered against the new defs and deregisters the difference first —
//!   or a removed table stays silently queryable, the exact inverse of the
//!   catalog-is-the-store rule above. A **connection** is the same case with a
//!   different call, and it has its owner: [`Engine::disconnect`] is the pane's Forget
//!   (W7 · Connections 02), and an edit that moves a connection's bucket or provider
//!   owes it too, since that changes the `url()` the store went in under. A host
//!   diffing a shrunken defs file must call it for every connection that has gone, on
//!   the same terms as the tables above.
//! - **The registration window.** [`Engine::register`] deregisters before it
//!   re-infers, so for the duration of a pass every table being rebuilt is absent from
//!   the catalog. The app gates validation behind its scan claim
//!   (`CatalogState::Scanning`, the claim that is also the validation gate) so nothing
//!   validates mid-scan; a host serving `Engine::validate`, `Engine::policy_verdicts`
//!   or queries concurrently with a pass must hold them off the same way, or it
//!   answers a false, transient "not found" for a table sitting right there.

use std::path::Path;

use strata_model::{ConnectionDef, TableDef};

use crate::engine::{Engine, TableMeta, TableSpec, ViewMeta};
use crate::project::{resolve_source, ProjectDefs};

/// What the engine answered for one def — the pass's per-entry product. A failed entry
/// does not abort the pass; its outcome is the row.
#[derive(Clone, Debug, PartialEq)]
pub enum RegOutcome {
    /// A connection's object store went in, or the connection could not describe one
    /// ([`Engine::connect`]). Nothing is *learned* by connecting — a store is registered,
    /// not inferred — so the payload is the answer itself.
    Connection {
        /// The connection's identity: [`ConnectionDef::url`], **not** its bucket. The bucket
        /// alone is not unique — `s3://lake` and `gs://lake` are two connections and two
        /// registry keys — so a caller folding these answers onto rows by bucket would land
        /// both on whichever it found first and leave the other unanswered forever.
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

/// The engine-facing projection of one table def: sources resolved against the project
/// folder (`resolve_source` — relative entries join onto `root`), everything else
/// carried as stored. One copy of the mapping, shared by the app's catalog passes and
/// [`register_project`].
pub fn table_spec(root: &Path, def: &TableDef) -> TableSpec {
    TableSpec {
        name: def.name.clone(),
        paths: def
            .sources
            .iter()
            .map(|s| resolve_source(root, s))
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
/// (a table's source path cannot resolve to an object store that isn't registered — see
/// [`Engine::connect`]); then tables (a view's SQL reads tables), each in the order given;
/// then views by fixed-point rounds — DataFusion
/// requires a view's dependencies to exist when its `CREATE VIEW` plans, so from cold,
/// each round creates what it can and a view whose dependency landed last round
/// succeeds this round. A round without progress means the remainder are genuinely
/// broken (bad SQL or a missing table) and their errors are their outcomes. Against an
/// engine that **already holds these views**, the retry cannot order anything (every
/// `CREATE OR REPLACE` succeeds round one) — hand `views` in dependency order
/// ([`view_order`]) or an outer view inlines a stale inner plan.
///
/// Connections need no ordering among themselves and are not retried: each registers one
/// bucket and reads nothing the pass provides, so a failure is final for this pass and its
/// only consequence is that tables over that bucket fail too — which they then report on
/// their own rows, saying no object store was found.
///
/// `settled` is called with each outcome as the engine answers it — the app folds
/// catalog rows and log entries per answer rather than after the whole pass, and a
/// caller that wants the collected list writes `|o| out.push(o)`. A failed entry never
/// aborts the pass, and a view retried across rounds settles **once**, on its final
/// answer — never once per attempt, which would report failures that never happened.
pub async fn register_pass(
    engine: &Engine,
    connections: Vec<ConnectionDef>,
    tables: Vec<TableSpec>,
    views: Vec<(String, String)>,
    mut settled: impl FnMut(RegOutcome),
) {
    for conn in connections {
        let url = conn.url();
        let result = engine.connect(conn).await;
        settled(RegOutcome::Connection { url, result });
    }

    for spec in tables {
        let name = spec.name.clone();
        let result = engine.register(spec).await;
        settled(RegOutcome::Table { name, result });
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
                // Not settled yet: a view whose dependency lands later succeeds on a
                // following round.
                Err(e) => failed.push((name, sql, e)),
            }
        }
        if failed.len() == before {
            // A full round without progress — the rest are genuinely broken.
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
    let tables = defs
        .tables
        .iter()
        .map(|def| table_spec(root, def))
        .collect();
    let views = defs
        .views
        .iter()
        .map(|v| (v.name.clone(), v.sql.clone()))
        .collect();
    register_pass(engine, connections, tables, views, settled).await
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
        let engine = Engine::new(BTreeMap::new());
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
    #[tokio::test]
    async fn a_failed_table_does_not_abort_the_pass() {
        let root = scratch("bad_table");
        fs::write(root.join("good.csv"), "id\n1\n").unwrap();
        let defs = ProjectDefs {
            tables: vec![table("bad", "missing.csv"), table("good", "good.csv")],
            ..Default::default()
        };

        let out = run(&root, &defs).await;

        match &out[..] {
            [RegOutcome::Table {
                name: b,
                result: Err(_),
            }, RegOutcome::Table {
                name: g,
                result: Ok(_),
            }] => {
                assert_eq!(b, "bad");
                assert_eq!(g, "good");
            }
            other => panic!("{other:?}"),
        }
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
    #[tokio::test]
    async fn connections_settle_first_and_each_under_its_own_url() {
        let root = scratch("connections");
        fs::write(root.join("local.csv"), "id\n1\n").unwrap();
        let defs = ProjectDefs {
            connections: vec![
                ConnectionDef {
                    bucket: "lake".into(),
                    provider: Provider::S3(S3Store {
                        region: "eu-west-2".into(),
                        auth: S3Auth::Anonymous,
                        ..Default::default()
                    }),
                },
                // The same authority under another provider: a different connection entirely.
                ConnectionDef {
                    bucket: "lake".into(),
                    provider: Provider::Gcs(GcsStore {
                        auth: GcsAuth::Anonymous,
                    }),
                },
                // A def that cannot describe a store: refused, and the pass carries on.
                ConnectionDef {
                    bucket: "no-region".into(),
                    provider: Provider::S3(S3Store {
                        auth: S3Auth::Anonymous,
                        ..Default::default()
                    }),
                },
            ],
            tables: vec![table("local", "local.csv")],
            ..Default::default()
        };

        let out = run(&root, &defs).await;

        assert_eq!(
            names(&out),
            vec![
                ("s3://lake", true),
                ("gs://lake", true),
                ("s3://no-region", false),
                ("local", true)
            ],
            "{out:?}"
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
        // A dependency outside the set can't order anything: alone, the outer view is
        // simply ready.
        assert_eq!(
            view_order(vec!["outer".into()], deps),
            vec!["outer".to_string()]
        );
    }
}
