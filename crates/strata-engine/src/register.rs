//! The project registration pass: connect each data source, register each table, then create each
//! view, and report what the engine answered for each def.
//!
//! [`sync`] is the contract. Hand it the catalog you want and it makes the engine match: what the
//! spec does not name is deregistered and reported as [`RegOutcome::Removed`], and what it names
//! is registered. It takes the whole catalog, never a work list.
//!
//! Narrower gestures are the facade's own — [`Catalog::register`](crate::Catalog::register) for
//! one table, [`Catalog::create_view`](crate::Catalog::create_view) for one view, and
//! [`Catalog::create_views`](crate::Catalog::create_views) for a set of them.
//!
//! Loading the defs ([`load_defs`](strata_core::project::load_defs)) and acting on the outcomes
//! stay the caller's. The pass reports outcomes, never introspects DataFusion, and nothing
//! refetches.
//!
//! A pass is safe to run against an engine that is being read: each new provider is built aside
//! and swapped in under the schema map's own lock, so a query landing mid-pass sees the old
//! provider or the new one and never a transient "not found".

use std::collections::BTreeSet;
use std::fmt;
use std::path::Path;

use futures::stream::{self, StreamExt};
use strata_model::{SourceDef, TableDef, ViewDef};

use crate::catalog::registered;
use crate::{fold_ident, CatalogGen, Engine, SourceDefs, TableMeta, TableSpec, ViewMeta};
use strata_core::project::{resolve_source, ProjectDefs};

/// How many tables register at once ([`sync`]'s table phase).
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
    /// A data source's object store or database catalog went in, or the data source could not
    /// describe one ([`Sources::connect`](crate::Sources::connect)). Nothing the *store* learns is reported here — an
    /// object store is registered, not inferred, and a database's enumeration is read back
    /// through [`Sources::listing`](crate::Sources::listing) rather than folded onto a row — so the payload is the
    /// answer itself.
    Source {
        /// The data source's own **name**, **not** its address. An address alone is not unique —
        /// two data sources may read one bucket, or reach one server as two roles — so a caller
        /// folding these answers onto rows by address would land both on whichever it found
        /// first and leave the other unanswered forever. It is also what every consumer addresses
        /// a row by, so anything else here settles nothing and the row waits for good.
        name: String,
        /// Whether the data source connected.
        result: Result<(), String>,
    },
    /// A table the pass registered.
    Table {
        /// The table's name.
        name: String,
        /// What registration learned about it, or why it was refused.
        result: Result<TableMeta, String>,
    },
    /// A view the pass created.
    View {
        /// The view's name.
        name: String,
        /// What creating it learned, or why it was refused.
        result: Result<ViewMeta, String>,
    },
    /// [`sync`] took an entry out because the desired catalog no longer names it.
    ///
    /// No `Result`: a removal is a map write against a registry this engine owns. The kind is
    /// carried because the def a host would have looked the name up by is the thing that is gone.
    Removed {
        /// The name that was taken out.
        name: String,
        /// Which registry it left.
        kind: RegKind,
    },
}

/// Which of the three registries an entry belongs to — see [`RegOutcome::Removed`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegKind {
    /// A data source.
    Source,
    /// A table.
    Table,
    /// A view.
    View,
}

impl fmt::Display for RegKind {
    /// Lower case, so it reads inside a sentence rather than as a label.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            RegKind::Source => "data source",
            RegKind::Table => "table",
            RegKind::View => "view",
        })
    }
}

/// The catalog a host wants the engine to hold — [`sync`]'s one argument.
///
/// **The whole of it, never a work list**: [`sync`] takes out what this does not name. A caller
/// with a narrower gesture registers that entry directly
/// ([`Catalog::register`](crate::Catalog::register)).
///
/// The fields are the three phases in the order they run. `views` is taken **in order**, which
/// [`view_order`] sorts so a view is re-created after everything it reads.
#[derive(Clone, Debug, Default)]
pub struct CatalogSpec {
    /// The data sources to connect, run first.
    pub sources: Vec<SourceDef>,
    /// The tables to register, run second.
    pub tables: Vec<TableSpec>,
    /// The views to create, run last and in order.
    pub views: Vec<ViewDef>,
}

impl CatalogSpec {
    /// The catalog a project's defs describe, with sources resolved against `root`.
    ///
    /// Views come out in defs order, which suits an engine that holds none of them: [`sync`]'s
    /// retry finds the dependency order by creating what it can. Replaying onto an engine that
    /// already holds them, sort with [`view_order`] first, or an outer view inlines the
    /// definition the pass is replacing.
    pub fn of_project(
        root: &Path,
        defs: &ProjectDefs,
        registrants: &crate::sources::source::Registrants,
    ) -> CatalogSpec {
        let sources = defs.sources.clone();
        let known = SourceDefs::of(&sources);
        CatalogSpec {
            tables: defs
                .tables
                .iter()
                .map(|def| table_spec(root, def, &known, registrants))
                .collect(),
            views: defs.views.clone(),
            sources,
        }
    }
}

/// What one [`sync`] settled at.
///
/// Per-def facts are not here: they arrive through `settled` as the engine answers them, so a
/// host's row flips when its own answer is known rather than when the pass ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PassReport {
    /// The generation the pass left the catalog at.
    pub generation: CatalogGen,
}

/// The engine-facing projection of one table def: sources resolved through [`resolve_source`] —
/// composed onto the store its data source registered under where it names one, and otherwise
/// joined onto the project folder — with everything else carried as stored. One copy of the
/// mapping, shared by the app's catalog passes and [`CatalogSpec::of_project`].
///
/// **A table names its source, and only a registry can say what that data source is.** The def
/// carries a name, the store is registered under `scheme://address`, and nothing about the first
/// yields the second — so this takes the registry rather than a string, which is what stops a
/// caller handing over the name and getting a path with no scheme in it. `None` for a name the
/// project has no data source for: the sources then compose as written and registration fails with
/// DataFusion's "No suitable object store", which is the honest answer.
///
/// A remote path needs **nothing** of the engine beyond this: the data source's object store is
/// already registered under that same URL by the time any table registers (data sources are
/// the pass's first phase), so `s3://acme-lake/events/` is a `ListingTableUrl` the
/// session can already resolve.
pub fn table_spec(
    root: &Path,
    def: &TableDef,
    sources: &SourceDefs,
    registrants: &crate::sources::source::Registrants,
) -> TableSpec {
    let prefix = def
        .source
        .as_deref()
        .and_then(|named| sources.prefix(registrants, named));
    TableSpec {
        name: def.name.clone(),
        paths: def
            .paths
            .iter()
            .map(|s| resolve_source(root, prefix.as_deref(), s))
            .collect(),
        format: def.format.clone(),
        partitions: def.partition_cols.clone(),
        source: def.source.clone(),
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
/// a view's known view-dependencies — the landed [`ViewMeta::views`], from a store row or
/// from the previous pass's own answer. Names compare case-insensitively (the engine folds
/// unquoted identifiers). A view with no known deps sorts wherever it falls — from cold that is
/// every view, which is why [`Catalog::create_views`](crate::Catalog::create_views) keeps its
/// fixed-point retry as well. A
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

/// Connect `data sources`, register `tables`, then create `views` on `engine`, handing
/// `settled` what it answered for each — [`sync`]'s additive half, and reachable only through
/// it: registering without reconciling leaves rows the defs no longer name.
///
/// **Ordering is the contract**: data sources first
/// (a table's source path cannot resolve to an object store that isn't registered, and a
/// view over `pg.public.orders` cannot plan before that catalog exists — see
/// [`Sources::connect`](crate::Sources::connect)); then tables (a view's SQL reads tables), **concurrently and in no
/// particular order** ([`TABLE_CONCURRENCY`]); then views, through [`create_views`].
///
/// Sources need no ordering among themselves and are not retried: each registers one bucket or
/// one catalog and reads nothing the pass provides. What a failure *costs* differs by kind — an
/// object store takes the tables over its bucket with it, each saying so on its own row, while a
/// **database** has no def rows at all and leaves nothing failed but its own row and whatever views
/// read across it.
///
/// `settled` is called with each outcome as the engine answers it, so the app folds catalog rows
/// and log entries per answer rather than after the whole pass. A failed entry never aborts the
/// pass.
async fn register_pass(
    engine: &Engine,
    sources: Vec<SourceDef>,
    tables: Vec<TableSpec>,
    views: Vec<ViewDef>,
    mut settled: impl FnMut(RegOutcome),
) {
    for conn in sources {
        let name = conn.named();
        let result = engine.sources().connect(conn).await;
        settled(RegOutcome::Source {
            name,
            result: result.map_err(|e| e.to_string()),
        });
    }

    let mut registrations = stream::iter(tables)
        .map(|spec| {
            let name = spec.name.clone();
            async move {
                let result = engine.catalog().register(spec).await;
                RegOutcome::Table {
                    name,
                    result: result.map_err(|e| e.to_string()),
                }
            }
        })
        .buffer_unordered(TABLE_CONCURRENCY);
    while let Some(outcome) = registrations.next().await {
        settled(outcome);
    }

    create_views(engine, views, settled).await;
}

/// Create `views` on `engine` in the order given, retrying until a round makes no progress, and
/// hand `settled` each view's final answer — once, on whatever it last produced.
///
/// The fixed point is what makes a chain work from cold: DataFusion requires a view's
/// dependencies to exist when its `CREATE VIEW` plans, so each round creates what it can and a
/// view whose dependency landed last round succeeds this round. A round without progress means
/// the remainder are broken (bad SQL, or a missing table) and their errors are their outcomes.
///
/// The retry cannot order views the engine **already holds** — every `CREATE OR REPLACE` succeeds
/// on round one — so those must arrive in dependency order ([`view_order`]) or an outer view
/// inlines the definition being replaced.
pub(crate) async fn create_views(
    engine: &Engine,
    views: Vec<ViewDef>,
    mut settled: impl FnMut(RegOutcome),
) {
    let mut pending = views;
    while !pending.is_empty() {
        let before = pending.len();
        let mut failed = Vec::new();
        for def in pending {
            match engine
                .catalog()
                .register_view(def.name.clone(), def.sql.clone())
                .await
            {
                Ok(meta) => settled(RegOutcome::View {
                    name: def.name,
                    result: Ok(meta),
                }),
                Err(e) => failed.push((def, e)),
            }
        }
        if failed.len() == before {
            for (def, e) in failed {
                settled(RegOutcome::View {
                    name: def.name,
                    result: Err(e.to_string()),
                });
            }
            break;
        }
        pending = failed.into_iter().map(|(def, _)| def).collect();
    }
}

/// Makes `engine` hold exactly the catalog `desired` describes, handing `settled` what it
/// answered for each entry.
///
/// Two phases. The **difference comes out first**: every table, view and data source the engine
/// holds that `desired` does not name is deregistered and reported as [`RegOutcome::Removed`].
/// Then the additive half registers what `desired` does name. That order is load-bearing — a
/// data source whose URL moved (below) is deregistered by name, so registering first would take
/// back the registration the pass had just made.
///
/// **Removal is deregistration.** An internal table's data stays on disk; destroying a table's
/// files is [`Catalog::drop_table`](crate::Catalog::drop_table)'s.
///
/// The diff is against the engine's own registries rather than a list the caller kept: the
/// workspace catalog for tables and views, and [`Sources`](crate::Sources) for data sources —
/// **membership,
/// not liveness**, so a def the engine refused is still removed and still reported, which is
/// what a host holding a row for it needs. A live result snapshot is out of reach, being absent
/// from what the catalog enumerates and nameable by no def.
///
/// **A data source is diffed by `(name, url)`, and only a dropped name is reported.** A name
/// `desired` keeps whose identity moved — the bucket edited, the provider changed — is
/// deregistered silently, since its store went in under the old URL; the pass re-connects it and
/// its [`Source`](RegOutcome::Source) outcome answers its row.
pub async fn sync(
    engine: &Engine,
    desired: CatalogSpec,
    mut settled: impl FnMut(RegOutcome),
) -> PassReport {
    remove_absent(engine, &desired, &mut settled).await;
    let (workspace, sources) = named(&desired);
    engine.dependencies.retain(&workspace);
    engine.ledger.retain(&workspace, &sources);
    register_pass(
        engine,
        desired.sources,
        desired.tables,
        desired.views,
        &mut settled,
    )
    .await;
    PassReport {
        generation: engine.catalog().generation(),
    }
}

/// What `desired` names, folded and split by namespace: the workspace's tables and views, then
/// its data sources — the sets [`Dependencies`](crate::Dependencies) and [`Ledger`](crate::Ledger)
/// are pruned to.
///
/// [`remove_absent`] cannot do this job: it diffs against what is **registered**, and a def whose
/// registration failed is not — so its entry is reported by no removal and would outlive the def
/// that put it there, which for the ledger is precisely the entry a host most needs retired.
/// Reconciling against the spec instead is the same rule the pass itself follows, and it is total.
fn named(desired: &CatalogSpec) -> (BTreeSet<String>, BTreeSet<String>) {
    let workspace = desired
        .tables
        .iter()
        .map(|t| fold_ident(&t.name))
        .chain(desired.views.iter().map(|v| fold_ident(&v.name)))
        .collect();
    let sources = desired
        .sources
        .iter()
        .map(|def| fold_ident(&def.named()))
        .collect();
    (workspace, sources)
}

/// Takes out everything the engine holds that `desired` does not name — [`sync`]'s first phase,
/// which carries the reasoning.
///
/// Views, then tables, then data sources: the reverse of the order they register in, so a view is
/// gone before the table it reads and a table before the store it reads through.
async fn remove_absent(
    engine: &Engine,
    desired: &CatalogSpec,
    settled: &mut impl FnMut(RegOutcome),
) {
    let wanted_views: BTreeSet<String> =
        desired.views.iter().map(|v| fold_ident(&v.name)).collect();
    let wanted_tables: BTreeSet<String> =
        desired.tables.iter().map(|t| fold_ident(&t.name)).collect();
    let held = registered(&engine.ctx).await;
    let absent: Vec<(String, RegKind)> = held
        .into_iter()
        .filter_map(|(name, is_view)| {
            let (kind, wanted) = match is_view {
                true => (RegKind::View, &wanted_views),
                false => (RegKind::Table, &wanted_tables),
            };
            (!wanted.contains(&fold_ident(&name))).then_some((name, kind))
        })
        .collect();
    for (name, kind) in absent
        .iter()
        .filter(|(_, kind)| *kind == RegKind::View)
        .chain(absent.iter().filter(|(_, kind)| *kind == RegKind::Table))
    {
        engine.catalog().deregister(name);
        settled(RegOutcome::Removed {
            name: name.clone(),
            kind: *kind,
        });
    }
    for (name, _) in engine.source_defs.held() {
        match desired
            .sources
            .iter()
            .find(|def| def.named().eq_ignore_ascii_case(&name))
        {
            None => {
                engine.sources().disconnect(&name);
                settled(RegOutcome::Removed {
                    name,
                    kind: RegKind::Source,
                });
            }
            Some(def) if engine.source_defs.moved(def) => engine.sources().disconnect(&name),
            Some(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::{env, process};

    use strata_model::{SourceFormat, TableOrigin, ViewDef};

    use super::*;

    /// A scratch project folder of our own, per test.
    fn scratch(tag: &str) -> PathBuf {
        let d = env::temp_dir().join(format!("strata_register_{}_{tag}", process::id()));
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
            source: None,
            paths: vec![source.into()],
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

    /// One pass over `defs` on a fresh engine, through the call a host makes. Nothing is
    /// registered yet, so the reconciliation has nothing to take out and what these tests read is
    /// the additive half.
    async fn run(root: &Path, defs: &ProjectDefs) -> Vec<RegOutcome> {
        let engine = Engine::builder().build();
        let mut out = Vec::new();
        engine
            .catalog()
            .sync(engine.catalog().spec(root, defs), |o| out.push(o))
            .await;
        out
    }

    /// Each outcome as `(name, did it settle Ok)`, in the order the pass answered — a removal
    /// has nothing that could have failed and reads as `false`, which the tests that use this
    /// helper never see (they assert on removals by kind instead).
    fn names(out: &[RegOutcome]) -> Vec<(&str, bool)> {
        out.iter()
            .map(|o| match o {
                RegOutcome::Source { name, result } => (name.as_str(), result.is_ok()),
                RegOutcome::Table { name, result } => (name.as_str(), result.is_ok()),
                RegOutcome::View { name, result } => (name.as_str(), result.is_ok()),
                RegOutcome::Removed { name, .. } => (name.as_str(), false),
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

    /// **Sources come first, before any table** — and each is answered under its own
    /// **URL**, not its bucket.
    ///
    /// Both halves are load-bearing. A source path under a bucket resolves through the object
    /// store registered for it, so a table that registers before its data source fails on a def
    /// that is perfectly correct — an ordering bug that would look exactly like a broken table.
    /// And an address is not unique across providers: the two `lake` defs below are two
    /// data sources over one bucket, so an outcome carrying only the address would be
    /// indistinguishable between them, and a caller folding by it would answer one row twice and
    /// leave the other waiting forever. What tells them apart is the **name**, which is also what
    /// every consumer looks a row up by — so that is what an outcome carries.
    ///
    /// **Every data source here is one that is refused locally**, and that is deliberate.
    /// `Sources::connect` now asks the bucket whether it answers (`store::reachable`), so a def
    /// that is merely *well-formed* is no longer one this test can settle `Ok` — it would send
    /// this suite to `s3.eu-west-2.amazonaws.com` and `storage.googleapis.com` on every run, for
    /// buckets nobody owns, and fail on a plane. Each of the three is refused before any socket
    /// opens (a blank region twice, a blank service-account path once), which costs the test
    /// nothing it was actually asserting: the subject is *order* and *identity*, and an outcome
    /// carries its name whether it succeeded or not. `("local", true)` is still what proves the
    /// pass carried on to the table phase after three refusals.
    #[tokio::test]
    async fn sources_settle_first_and_each_under_its_own_name() {
        let root = scratch("data sources");
        fs::write(root.join("local.csv"), "id\n1\n").unwrap();
        let defs = ProjectDefs {
            sources: vec![
                SourceDef {
                    name: "lake_s3".into(),
                    kind: "s3".into(),
                    config: [("auth".to_string(), "anonymous".to_string())]
                        .into_iter()
                        .collect(),
                    ..Default::default()
                },
                SourceDef {
                    name: "lake_gcs".into(),
                    kind: "gcs".into(),
                    config: [("auth".to_string(), "service-account".to_string())]
                        .into_iter()
                        .collect(),
                    ..Default::default()
                },
                SourceDef {
                    name: "elsewhere".into(),
                    kind: "s3".into(),
                    config: [("auth".to_string(), "anonymous".to_string())]
                        .into_iter()
                        .collect(),
                    ..Default::default()
                },
            ],
            tables: vec![table("local", "local.csv")],
            ..Default::default()
        };

        let out = run(&root, &defs).await;

        assert_eq!(
            names(&out),
            vec![
                ("lake_s3", false),
                ("lake_gcs", false),
                ("elsewhere", false),
                ("local", true)
            ],
            "{out:?}"
        );
    }

    /// **A def that names a data source is composed onto that data source's store, never onto the
    /// project folder.** The engine half needs nothing further: the store went in under that same
    /// URL in the pass's first phase, so what reaches `register` is an address the session can
    /// already resolve.
    ///
    /// The def carries the data source's **name**, so the lookup is the point — driven here
    /// through a real registry rather than by handing the composition a pre-made identity, which
    /// is exactly how this went green while a real bucket read `//acme-lake/events/`. A name the
    /// registry does not hold composes nothing, which is what makes registration fail loudly.
    ///
    /// The failure the local half pins is silent rather than loud: a bucket-relative source under
    /// the local rule becomes `<project>/events/2024/`, a missing folder on the user's own disk
    /// that says nothing about a bucket.
    #[test]
    fn a_table_over_a_source_resolves_against_its_bucket() {
        let registrants = Engine::builder().build();
        let known = SourceDefs::of(&[SourceDef {
            config: [("address".to_string(), "acme-lake".into())]
                .into_iter()
                .collect(),
            kind: "s3".into(),
            name: "acme_lake".into(),
            ..Default::default()
        }]);
        let def = TableDef {
            name: "events".into(),
            format: SourceFormat::from_name("parquet"),
            source: Some("acme_lake".into()),
            paths: vec!["events/2024/**/*.parquet".into()],
            partition_cols: Vec::new(),
            origin: TableOrigin::External,
        };
        assert_eq!(
            registrants
                .catalog()
                .table_spec(Path::new("/proj"), &def, &known)
                .paths,
            ["s3://acme-lake/events/2024/**/*.parquet"]
        );
        let stranded = TableDef {
            source: Some("gone".into()),
            ..def.clone()
        };
        assert_eq!(
            registrants
                .catalog()
                .table_spec(Path::new("/proj"), &stranded, &known)
                .paths,
            ["/proj/events/2024/**/*.parquet"],
            "a name the project has no data source for composes nothing remote"
        );
        let local = TableDef {
            source: None,
            ..def
        };
        assert_eq!(
            registrants
                .catalog()
                .table_spec(Path::new("/proj"), &local, &SourceDefs::default())
                .paths,
            ["/proj/events/2024/**/*.parquet"]
        );
    }

    /// A [`CatalogSpec`] over a scratch root — the named tables all read the same CSV.
    fn desired(root: &Path, tables: &[&str], views: &[(&str, &str)]) -> CatalogSpec {
        CatalogSpec {
            sources: Vec::new(),
            tables: tables
                .iter()
                .map(|name| {
                    Engine::builder().build().catalog().table_spec(
                        root,
                        &table(name, "t.csv"),
                        &SourceDefs::default(),
                    )
                })
                .collect(),
            views: views
                .iter()
                .map(|(name, sql)| ViewDef {
                    name: (*name).into(),
                    sql: (*sql).into(),
                })
                .collect(),
        }
    }

    /// Whether `name` resolves in `engine`'s workspace catalog right now.
    async fn resolves(engine: &Engine, name: &str) -> bool {
        engine.ctx.table(name).await.is_ok()
    }

    /// What the spec does not name comes out, what it names is registered, and each removal is
    /// reported.
    ///
    /// Both kinds and both directions in one test because they are one rule: a host folding
    /// outcomes learns of a removal the same way it learns of everything else.
    #[tokio::test]
    async fn sync_takes_out_what_the_spec_no_longer_names() {
        let root = scratch("sync_removals");
        fs::write(root.join("t.csv"), "id,name\n1,a\n").unwrap();
        let engine = Engine::builder().build();

        let mut first = Vec::new();
        engine
            .catalog()
            .sync(
                desired(
                    &root,
                    &["kept", "dropped"],
                    &[
                        ("v_kept", "SELECT id FROM kept"),
                        ("v_gone", "SELECT 1 AS n"),
                    ],
                ),
                |o| first.push(o),
            )
            .await;
        let mut settled = names(&first);
        settled.sort_unstable();
        assert_eq!(
            settled,
            vec![
                ("dropped", true),
                ("kept", true),
                ("v_gone", true),
                ("v_kept", true)
            ],
            "the first pass is an ordinary registration: {first:?}"
        );

        let mut second = Vec::new();
        engine
            .catalog()
            .sync(
                desired(&root, &["kept"], &[("v_kept", "SELECT id FROM kept")]),
                |o| second.push(o),
            )
            .await;

        let removed: Vec<(&str, RegKind)> = second
            .iter()
            .filter_map(|o| match o {
                RegOutcome::Removed { name, kind } => Some((name.as_str(), *kind)),
                _ => None,
            })
            .collect();
        assert_eq!(
            removed,
            vec![("v_gone", RegKind::View), ("dropped", RegKind::Table)],
            "the view goes before the table, and each says which listing it came out of: {second:?}"
        );
        assert!(!resolves(&engine, "dropped").await);
        assert!(!resolves(&engine, "v_gone").await);
        assert!(
            resolves(&engine, "kept").await && resolves(&engine, "v_kept").await,
            "and what the spec still names is registered, not swept and rebuilt"
        );

        let _ = fs::remove_dir_all(&root);
    }

    /// A data source the spec has dropped is taken out and reported; one whose identity moved
    /// under a name it kept is taken out silently and re-connected by the pass.
    ///
    /// Every def here is refused before a socket opens (a blank region), for
    /// `data sources_settle_first_and_each_under_its_own_name`'s reason: the subject is the diff,
    /// and a data source carries its name into an outcome whether it reached anything or not.
    #[tokio::test]
    async fn a_source_is_diffed_by_name_and_address() {
        let engine = Engine::builder().build();
        let at = |name: &str, address: &str| SourceDef {
            kind: "s3".into(),
            name: name.into(),
            config: [("address", address), ("auth", "anonymous")]
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            ..Default::default()
        };

        engine
            .catalog()
            .sync(
                CatalogSpec {
                    sources: vec![at("lake", "first"), at("spare", "second")],
                    ..Default::default()
                },
                |_| {},
            )
            .await;
        assert_eq!(engine.source_defs.held().len(), 2);

        let mut out = Vec::new();
        engine
            .catalog()
            .sync(
                CatalogSpec {
                    sources: vec![at("lake", "moved")],
                    ..Default::default()
                },
                |o| out.push(o),
            )
            .await;

        let removed: Vec<(&str, RegKind)> = out
            .iter()
            .filter_map(|o| match o {
                RegOutcome::Removed { name, kind } => Some((name.as_str(), *kind)),
                _ => None,
            })
            .collect();
        assert_eq!(
            removed,
            vec![("spare", RegKind::Source)],
            "only the name the spec dropped is a removal: {out:?}"
        );
        assert_eq!(
            engine.source_defs.held(),
            vec![("lake".to_string(), "s3:moved".to_string())],
            "and the one that moved is held at its new identity, not both"
        );
    }

    /// **An unchanged data source has not moved, so a sync leaves it alone** — the half the diff
    /// above never asked about.
    ///
    /// `sync` is not only the project open: `CREATE TABLE`, `CREATE VIEW`, a drop and the sidebar
    /// refresh all reach it. A diff that answered "moved" for an unmoved source would tear down
    /// every live one on each of those — dropping a database's pool and its cached listing, and
    /// widening the window where its catalog resolves to nothing — before the pass reconnected
    /// them anyway, which is what a comparison of a *name* against an *address* produced: the
    /// answer was "moved" for every source not named after its own address.
    ///
    /// Asked of `SourceDefs::moved` rather than through `sync`, deliberately: the additive phase
    /// reconnects every desired source on every pass, and a failed connect takes back whatever it
    /// had registered, so the two behaviours **converge** in everything observable afterwards.
    /// What the bug cost was the teardown in between, and the predicate is where it is decided.
    #[tokio::test]
    async fn an_unchanged_source_has_not_moved() {
        let engine = Engine::builder().build();
        let at = |kind: &str, name: &str, address: &str| SourceDef {
            kind: kind.into(),
            name: name.into(),
            config: [("address", address), ("auth", "anonymous")]
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            ..Default::default()
        };

        let def = at("s3", "lake_s3", "acme-lake");
        engine
            .catalog()
            .sync(
                CatalogSpec {
                    sources: vec![def.clone()],
                    ..Default::default()
                },
                |_| {},
            )
            .await;

        assert!(
            !engine.source_defs.moved(&def),
            "the def the engine was told about is where it is held"
        );
        assert!(
            engine
                .source_defs
                .moved(&at("s3", "lake_s3", "other-bucket")),
            "a moved address is a move"
        );
        assert!(
            engine.source_defs.moved(&at("gcs", "lake_s3", "acme-lake")),
            "and so is a moved kind, which keeps the address and still changes the URL the \
             object store registered under"
        );
        assert!(
            !engine
                .source_defs
                .moved(&at("s3", "unheard_of", "acme-lake")),
            "a name this engine holds nothing for has nothing to take back"
        );
    }

    /// **A name is a SQL identifier, so the diff folds case on both sides.**
    ///
    /// `check_catalog_name` lets a source be renamed to a name differing only in case — the
    /// candidate is excluded from its own uniqueness check — and `remove_absent` matches the
    /// desired def to a held name with `eq_ignore_ascii_case`. So the move test has to fold too,
    /// or an edit that changed the case *and* the address reads as unmoved: the old registration
    /// is never taken back and the engine ends up holding both spellings, one of them a phantom
    /// nothing will ever match away.
    #[tokio::test]
    async fn the_move_test_folds_case_like_every_other_lookup() {
        let engine = Engine::builder().build();
        let at = |name: &str, address: &str| SourceDef {
            kind: "s3".into(),
            name: name.into(),
            config: [("address", address), ("auth", "anonymous")]
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            ..Default::default()
        };
        let spec = |def: SourceDef| CatalogSpec {
            sources: vec![def],
            ..Default::default()
        };

        engine
            .catalog()
            .sync(spec(at("Lake", "acme-lake")), |_| {})
            .await;

        assert!(
            engine.source_defs.moved(&at("lake", "other-bucket")),
            "a re-cased name whose address moved is a move, and the old registration owes a \
             take-back"
        );
        assert!(
            !engine.source_defs.moved(&at("lake", "acme-lake")),
            "and a re-cased name that moved nothing else has not moved"
        );

        engine
            .catalog()
            .sync(spec(at("lake", "acme-lake")), |_| {})
            .await;
        assert_eq!(
            engine.source_defs.held(),
            vec![("lake".to_string(), "s3:acme-lake".to_string())],
            "one entry per source, under the spelling it was last given"
        );
    }

    /// A live result snapshot is out of a reconciliation's reach: no def can name one, and a
    /// sweep that took it would retire whatever another tab is paging through.
    #[tokio::test]
    async fn sync_never_sweeps_a_result_snapshot() {
        let engine = Engine::builder().build();
        engine
            .ws(crate::WsId(1))
            .query(crate::RunTag(1), "SELECT 1 AS n".into(), 10)
            .await
            .expect("run");
        let snapshot = crate::snapshots::snapshot_name(strata_model::SnapshotId(1));

        engine.catalog().sync(CatalogSpec::default(), |_| {}).await;

        assert!(
            resolves(&engine, snapshot.as_str()).await,
            "the pass swept a snapshot the spec could not possibly have named"
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
