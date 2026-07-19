# A9 — DataFusion 43 → 54 upgrade: migration plan

## Why this is a plan, not a finished diff

A 43 → 54 jump is **11 major versions** of breaking API churn (arrow 53 → 58, object_store 0.11 → 0.13, sqlparser 0.51 →
0.62, plus DataFusion's own module and signature changes). The only reliable way to do it is a **compile-fix loop**:
bump the version, run `cargo check`, fix what breaks, repeat.

That loop needs a compiler, and the Cowork sandbox cannot build this project:

- **No Rust toolchain**, and installing one is not enough because…
- …this is a Dioxus **desktop** app: `wry`/`tao` hard-require `webkit2gtk-4.1`,
  `gtk+-3.0`, `libsoup-3.0` at build time — all **missing**, and there is **no root**
  to `apt-get install` them.
- ~4 GB free disk and 3.8 GB RAM would also not survive a full DataFusion build.

So A9 must be driven from the Mac (where the normal build loop runs). This document is the map to make that loop fast:
the exact bump, every DataFusion call site we touch, the changes to expect, and the reading order. Confidence is marked
per item — **[confirmed]** from docs I verified, **[verify]** = likely-changed, confirm against the version guides.

## 1. Dependency bump (the only Cargo.toml change)

`datafusion` is our **only** DataFusion-family pin — arrow, parquet, sqlparser and object_store are all reached through
its re-exports (`datafusion::arrow`,
`datafusion::parquet`, `datafusion::sql::sqlparser`). So:

```toml
# Cargo.toml
- datafusion = "43"
+ datafusion = "54"
```

Nothing else in `[dependencies]` changes for A9. (S26 may add a direct
`sqlparser = "0.62"` matching DataFusion's, or use the `datafusion::sql::sqlparser`
re-export; S21 will add a direct `object_store` pin matching DataFusion's 0.13.)

After the bump, `cargo update -p datafusion` then `cargo check` and work the errors.

## 2. Our DataFusion surface (from `engine.rs` imports + call sites)

Everything DataFusion-facing is in `src/engine.rs`. The imports enumerate the surface:

```
datafusion::arrow::array::Array
datafusion::arrow::datatypes::{DataType, Field}
datafusion::arrow::record_batch::RecordBatch
datafusion::arrow::util::display::{ArrayFormatter, FormatOptions}
datafusion::logical_expr::LogicalPlan
datafusion::parquet::arrow::ArrowWriter
datafusion::physical_plan::display::DisplayableExecutionPlan
datafusion::physical_plan::{collect, displayable, ExecutionPlan}
datafusion::prelude::*                 // SessionContext, ParquetReadOptions, DataFrame, col, lit, …
```

Call sites and what to expect:

| Area (engine.rs)                         | Calls                                                                                                                                           | Expectation                                                                                                                                                                                                                                         |
|------------------------------------------|-------------------------------------------------------------------------------------------------------------------------------------------------|-----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| Context                                  | `SessionContext::new()`, `ctx.sql(&str).await`, `ctx.table(name).await`, `ctx.deregister_table(name)`, `ctx.state()`, `ctx.task_ctx()`          | **[verify]** core surface is stable across 43→54, but `SessionContext` construction gained builder paths; `ctx.sql` / `ctx.table` return types unchanged. Low risk.                                                                                 |
| Register (snapshot)                      | `ctx.register_parquet(name, path, ParquetReadOptions::default())`                                                                               | **[verify]** `register_parquet` + `ParquetReadOptions` are stable in shape; confirm no added required args.                                                                                                                                         |
| Register (external, `register_external`) | `ListingTableConfig` / `ListingOptions` / `ListingTable::try_new` + `.infer_schema(&ctx.state())` + `ctx.register_table(name, Arc::new(table))` | **[verify — highest risk]** the Listing* + object_store registration path saw the most change (this is also the S21 path). Expect `ListingOptions`/`ListingTableConfig` builder tweaks and `infer_schema` signature to shift. Work this file first. |
| Execute + page                           | `ctx.sql(...).await?` → `DataFrame`; `collect(physical, task_ctx)`                                                                              | **[verify]** `collect` signature stable; `DataFrame` collect/limit APIs stable.                                                                                                                                                                     |
| Explain (`run_explain`)                  | `state.create_physical_plan(&logical).await`, `DisplayableExecutionPlan`, `displayable(plan)`, `ExecutionPlan` metrics                          | **[verify]** physical-plan + `ExecutionPlan` trait changed across versions (metrics/children accessors, `Arc<dyn ExecutionPlan>`); the plan-walk in `run_explain` / `plan.rs` is the second-riskiest area.                                          |
| Export (`run_export`)                    | `COPY … TO` via `ctx.sql`, `ArrowWriter` for arrow/parquet snapshots                                                                            | **[verify]** SQL-level `COPY` stable; `ArrowWriter::try_new` API stable-ish (parquet 58).                                                                                                                                                           |
| Arrow display                            | `ArrayFormatter::try_new(array, &FormatOptions)` for cell text                                                                                  | **[verify]** arrow 58 keeps `ArrayFormatter`; confirm `FormatOptions` builder unchanged.                                                                                                                                                            |
| Types (`util::Kind`, ColumnInfo)         | `DataType`, `Field`, nested `DataType::{List,Struct,Map}` matching in `nested_children`                                                         | **[verify]** arrow `DataType` variants are stable; the `Field`/`Fields` (Arc) shape settled pre-53, low risk.                                                                                                                                       |

## 3. Function-registry push (F5) — do it during A9 while in the engine

This is **additive** and **[confirmed]** against the 54 API (`FunctionRegistry` on
`SessionState`). Add after `let ctx = SessionContext::new();`, emit once on startup:

```rust
// engine.rs — after context construction, before the command loop
use datafusion::execution::registry::FunctionRegistry; // trait for udfs/udafs/udwfs
let functions = {
    let s = ctx.state();
    let mut scalar: Vec<String>    = s.udfs().into_iter().collect();
    let mut aggregate: Vec<String> = s.udafs().into_iter().collect();
    let mut window: Vec<String>    = s.udwfs().into_iter().collect();
    scalar.sort(); aggregate.sort(); window.sort();
    (scalar, aggregate, window)
};
let _ = evt_tx.send(Event::Functions {
    scalar: functions.0, aggregate: functions.1, window: functions.2,
});
```

Then add `Event::Functions { scalar, aggregate, window: Vec<String> }` to the `Event`
enum, a UI-side `FunctionCatalog` (per-window, like the schema) fed by the reducer, and fold it into
`crate::sql::Catalog` (S26). `higher_order_function_names()` also exists if we want those. (`udafs`/`udwfs` are
54-only — the reason A9 gates S26/S7/S25.)

## 4. Reading order (version guides)

Skim each version's **Upgrade Guide** for our surface (Listing tables, physical plan /
`ExecutionPlan`, arrow re-export, `register_*`). Guides exist 44 → 54 at
`datafusion.apache.org/library-user-guide/upgrading/<v>.0.0.html`. Highest-signal for us: whichever versions reworked
**ListingTable/ListingOptions** and the **physical-plan/ExecutionPlan** trait — grep each guide for `ListingTable`,
`ExecutionPlan`, `register_`, `arrow`. (I couldn't pre-digest all nine here; the 54 guide alone is ~50 KB.)

## 5. Compile-loop process (on the Mac)

1. Bump Cargo.toml → `cargo update -p datafusion` → `cargo check`.
2. Fix in this order (riskiest first): `register_external` (Listing*) → `run_explain`
    + `plan.rs` (physical plan) → `register_parquet`/snapshot → arrow display → the rest. Then add F5 (§3).
3. `cargo check` green → run a real query, an EXPLAIN, a register, and an export to confirm behaviour (the API can
   compile but behave differently — esp. COPY options and plan metrics).
4. Commit as its own change **before** starting S26 — never stack the language service on an unverified engine.

## 6. Then, and only then: S26 → S7 → S25

Per `docs/SQL_LANGUAGE_SPEC.md`, F0 (this) unblocks: the `sqlparser` 0.62 tokenizer +
`DFParser` reuse (F3), the function push (F5, seeded here), and planner-diagnostic spans (semantic validation). S7/S25
are built on S26 + the **vendored editor** (also unverifiable here — it needs the same compile loop, plus the dioxus-0.7
`MountedData`
selection check). Sequence: **A9 (verified) → S26 → S7/S25**, each through the Mac loop.
