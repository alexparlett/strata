//! The **project registration pass** (AA-01) — one implementation of "make the engine
//! match the defs": register each table, then create each view, and report what the
//! engine answered per def. Extracted from the Freya app's project-open hook so a
//! headless host (AA-05) can run the same sequence with no store to fold into; the
//! app's hook consumes [`register_pass`] and keeps only what is genuinely the store's
//! (`Reg<T>` rows, epochs, log entries).
//!
//! Loading the defs stays the caller's ([`load_defs`](crate::project::load_defs)), and
//! so does acting on the outcomes — the catalog-is-the-store rule is untouched: the
//! pass returns outcomes, it never introspects DataFusion, and nothing refetches.

use std::path::Path;

use strata_model::TableDef;

use crate::engine::{Engine, TableMeta, TableSpec, ViewMeta};
use crate::project::{resolve_source, ProjectDefs};

/// What the engine answered for one def — the pass's per-entry product. A failed entry
/// does not abort the pass; its outcome is the row.
#[derive(Clone, Debug, PartialEq)]
pub enum RegOutcome {
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
    }
}

/// Register `tables` then create `views` on `engine`, returning what it answered for
/// each. **Ordering is the contract**: tables first (a view's SQL reads tables), each in
/// the order given; then views by fixed-point rounds — DataFusion requires a view's
/// dependencies to exist when its `CREATE VIEW` plans, so rather than parse SQL to
/// topo-sort, each round creates what it can and a view whose dependency landed last
/// round succeeds this round. A round without progress means the remainder are genuinely
/// broken (bad SQL or a missing table) and their errors are their outcomes.
///
/// `settled` is called with each outcome as the engine answers it — the app folds
/// catalog rows and log entries per answer rather than after the whole pass. A caller
/// that only wants the collected result passes `|_| {}`. A view retried across rounds
/// settles **once**, on its final answer — never once per attempt, which would report
/// failures that never happened.
pub async fn register_pass(
    engine: &Engine,
    tables: Vec<TableSpec>,
    views: Vec<(String, String)>,
    mut settled: impl FnMut(&RegOutcome),
) -> Vec<RegOutcome> {
    let mut out = Vec::new();

    for spec in tables {
        let name = spec.name.clone();
        let result = engine.register(spec).await;
        let outcome = RegOutcome::Table { name, result };
        settled(&outcome);
        out.push(outcome);
    }

    let mut pending = views;
    while !pending.is_empty() {
        let before = pending.len();
        let mut failed = Vec::new();
        for (name, sql) in pending {
            match engine.create_view(name.clone(), sql.clone()).await {
                Ok(meta) => {
                    let outcome = RegOutcome::View {
                        name,
                        result: Ok(meta),
                    };
                    settled(&outcome);
                    out.push(outcome);
                }
                // Not settled yet: a view whose dependency lands later succeeds on a
                // following round.
                Err(e) => failed.push((name, sql, e)),
            }
        }
        if failed.len() == before {
            // A full round without progress — the rest are genuinely broken.
            for (name, _, e) in failed {
                let outcome = RegOutcome::View {
                    name,
                    result: Err(e),
                };
                settled(&outcome);
                out.push(outcome);
            }
            break;
        }
        pending = failed.into_iter().map(|(n, s, _)| (n, s)).collect();
    }
    out
}

/// The whole-project pass: every table and view in `defs`, sources resolved against
/// `root`. What a host that just loaded a project runs (AA-05). The app's catalog
/// passes call [`register_pass`] directly instead, because their work list is not
/// always the whole project — a row's Refresh is the same pass, one table wide.
pub async fn register_project(
    engine: &Engine,
    root: &Path,
    defs: &ProjectDefs,
    settled: impl FnMut(&RegOutcome),
) -> Vec<RegOutcome> {
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
    register_pass(engine, tables, views, settled).await
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::PathBuf;
    use std::{env, process};

    use strata_model::{SourceFormat, ViewDef};

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
        }
    }

    fn view(name: &str, sql: &str) -> ViewDef {
        ViewDef {
            name: name.into(),
            sql: sql.into(),
        }
    }

    /// The happy path: the table lands first, then the view — and the sink sees exactly
    /// the sequence the collected result holds.
    #[tokio::test]
    async fn tables_then_views_register_in_order() {
        let root = scratch("happy");
        fs::write(root.join("t.csv"), "id,name\n1,a\n2,b\n").unwrap();
        let defs = ProjectDefs {
            tables: vec![table("t", "t.csv")],
            views: vec![view("v", "SELECT id FROM t")],
            ..Default::default()
        };
        let engine = Engine::new(BTreeMap::new());

        let mut seen = Vec::new();
        let out = register_project(&engine, &root, &defs, |o| seen.push(o.clone())).await;

        assert_eq!(out, seen, "the sink sees exactly the collected sequence");
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
        let engine = Engine::new(BTreeMap::new());

        let out = register_project(&engine, &root, &defs, |_| {}).await;

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
        let engine = Engine::new(BTreeMap::new());

        let out = register_project(&engine, &root, &defs, |_| {}).await;

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

    /// Views arrive in whatever order the defs hold; a view over a view given first
    /// succeeds on the round after its dependency lands — the fixed-point retry.
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
        let engine = Engine::new(BTreeMap::new());

        let out = register_project(&engine, &root, &defs, |_| {}).await;

        let settled: Vec<(&str, bool)> = out
            .iter()
            .map(|o| match o {
                RegOutcome::Table { name, result } => (name.as_str(), result.is_ok()),
                RegOutcome::View { name, result } => (name.as_str(), result.is_ok()),
            })
            .collect();
        assert_eq!(
            settled,
            vec![("t", true), ("base_v", true), ("top_v", true)],
            "{out:?}"
        );
    }
}
