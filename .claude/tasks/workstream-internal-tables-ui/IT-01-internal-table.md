# IT-01 · Creating an internal table: Configure's LOCATION ▸ **Internal**

**Workstream:** Internal tables in the UI · **Status:** ✅ **built 2026-08-13** · **DEV_TASKS:** — · **Depends on:** —

## What landed

**A third LOCATION in the Configure window**, not a panel of its own —
`Where::{Local, Remote, Internal}`, using the word the catalog row's chip and
`TableOrigin::Internal` already use. On `Internal` the FORMAT picker, SOURCE PATHS, the import
options and HIVE draw nothing, and `views/columns.rs` takes their place: the paths list's own
`Table`, `+`/`−` `ToolButton` toolbar and two-way-synced bare fields, with a third cell per row
for the planner's verdict. Save branches in `views/footer.rs`, composing the statement and
folding it through `state::settle`. `ToggleSegment` grew an `enabled` for the create-only segment.

Engine side (unchanged by the rework): `ddl::tables::column_type` behind `Engine::column_type`,
plus `unenforced_clause` and `duplicate_column` factored out of `create` so both callers reach
them, `fold_ident` made `pub`, and `ProjectState::name_taken` shared with the footer's existing
clash check.

### The panel that was built first, and rejected

The first version was a modal panel on the shared `Modal` base, opened from a two-item menu on
the catalog's `+` (*From files…* / *Empty table…*). **Alex rejected it on sight** (2026-08-13),
on two counts, both right:

1. **It did not copy the existing create-table UI**, so it had "completely the wrong theme" — its
   own card, its own row layout, none of it matching the window that already answers this
   question.
2. **The `+` should not have grown a dropdown.** The answer belongs *in* Configure, as a third
   LOCATION option beside Local and Remote.

The panel, its `NewTable` slot, its root mount and its palette command are deleted. Do not
re-propose them. What survived is everything below the surface: the composer, the eager per-row
probe, the engine's shared refusals, and the fold.

### Corrections to the plan below, each because the code said otherwise

1. **The row's detail is `short_type`, not the full Arrow `Debug`.** §2's example line shows
   `Timestamp(Nanosecond, "Europe/London")`; `ColumnInfo::dtype` — which the same paragraph names
   as the promise — is `catalog::short_type`, which renders that as `Timestamp`. The promise is
   the load-bearing half, and `short_type`'s own doc is explicit that it is *the* type spelling
   every surface shows.
2. **The probe requires one column and an `EmptyRelation` input.** `INT, b INT` plans two
   columns; `INT) AS SELECT 1 --` plans as a CTAS with exactly one field and would otherwise read
   as a clean `Int64`.
3. **The probe also runs the create arm's clause refusals.** `INT PRIMARY KEY` plans clean, and
   without this it would be the deferred error at the press that §2 exists to prevent.
4. **`tones()` may not be called inside a match arm** — a theme hook on one branch is a hook
   called conditionally, and it panicked the moment a row acquired a fault (AGENTS §3).

## Goal

A working panel that creates an **empty internal table** — a name and a list of
(column, type) rows — composing `CREATE TABLE "t" (a INT, b VARCHAR)` and sending it through
the statement funnel the editor already uses. The engine side is **done**; this is the surface
and the wiring.

## Current state (verified 2026-08-13)

**The engine already does all of it.** `ddl::tables::create` serves both create kinds from one
body, and the bare column-list form is first-class, in its own words
([`tables.rs:70`](../../../crates/strata-engine/src/ddl/tables.rs)):

> a declared column list with no query becomes an `EmptyRelation` carrying that schema, and the
> spool below then writes it as a schema-carrying, zero-row Arrow file

The router names it apart as `StmtKind::CreateTable` ([`validate.rs:571`](../../../crates/strata-engine/src/sql/validate.rs))
"because the *report* says different things, and because a kind that classifies is a kind some
later task may implement differently". This is that task.

- **Refused by the arm, so the form must not offer them:** table constraints and column
  defaults, both by name (tables.rs:118-123). Nullable columns only.
- **Refused by the planner:** duplicate column names are caught by the arm's own fold
  (tables.rs:127-138, reproduced from DataFusion's rule because "its CTAS never writes a file"
  and an IPC file would store both columns).
- **The name gate exists:** `bare_name`, the `__snap_` fence, and "'x' is a view" against the
  one shared namespace.
- **The trigger's home today:** the catalog TABLES section's `+`
  ([`catalog/mod.rs:183`](../../../crates/strata-freya/src/apps/project/views/sidebar/catalog/mod.rs)),
  currently a direct press to `ConfigureTarget::New` — i.e. registering a table over files you
  already have, which is a different gesture. Its comment already records why TABLES carries it
  and VIEWS/QUERIES do not.
- **Configure is not the home for this.** Configure edits *how files are read*; an internal
  table has no such questions, and the settled rule is that Configure's item is **absent** for
  one, not disabled (`table_menu`, menu.rs:286).
- **The panel precedent is the Shape panel** (`results/shape/`): a modal working panel on the
  shared `Modal` base with **its own card** — "a working panel is not a confirm, so it does not
  wear the 420px confirm card" — built from `components::form`, composing visible SQL.

## Build

### 1. The panel

`apps/project/views/sidebar/catalog/new_table/` (or beside the Shape panel if the trigger has
already moved to the tree — see §5). `components::modal::Modal` + its own card, sized as a
working panel; `components::form`'s `Form` > `Row`.

- **Name row.** Validated against the folded namespace as Configure's name box is.
- **Column rows.** Repeatable (name, type), add/remove. Ids from a counter, **never the name**
  — the free-form-list rule (AGENTS §2), because a row is editable while it is being typed.
- **A composed-SQL preview line**, read-only. With a free-text type field this is what makes
  the panel legible rather than magic, and it is what the "Open in editor" button hands over.

### 2. The type field is free text, validated per row

**Free text, not a picker.** The alternative was investigated first and rejected on evidence:

- DataFusion ships **no Arrow → SQL inverse**. `convert_simple_data_type` (datafusion-sql
  `planner.rs:690`) is many-to-one — `INT | INTEGER | INT4` → `Int32`,
  `CHAR | TEXT | STRING | VARCHAR` → `Utf8` — so "the SQL spelling for this Arrow type" has no
  canonical answer.
- **A static table would be wrong for the project's own config.** `map_string_types_to_utf8view`
  flips `VARCHAR` between `Utf8` and `Utf8View` (planner.rs:713), and `execution.time_zone` is
  what fills the zone on `TIMESTAMP WITH TIME ZONE` (planner.rs:752).
- **Arrow spellings are not accepted anyway.** `SQLDataType::Int64 | Float64 | Int32 | UInt8 | …`
  sit in the `not_impl` arm beside `Custom(_, _)` (planner.rs:836), as do `BINARY`, `VARBINARY`,
  `BLOB`, `UUID` and `JSON` — `BYTEA` is the only route to `Binary`.

So the offer is not authored at all. Instead, **per row, debounced, probe the planner**: plan
`CREATE TABLE __probe (c <what they typed>)` and keep the answer. Planning executes nothing —
"execution lives only in `execute_logical_plan`" (tables.rs module doc) — so this is a cheap,
side-effect-free question, and it is an engine call like any other (nothing blocking on the
render thread, AGENTS §2).

The row then shows the **Arrow type the planner actually produced**, which is the same string
[`ColumnInfo.dtype`](../../../crates/strata-model/src/schema.rs) shows in the inspector once the
table exists — so the form promises exactly what the user will see, derived rather than
declared, with the project's real timezone in it:

```
code     [ VARCHAR                  ]  Utf8
weight   [ DOUBLE                   ]  Float64
ts       [ TIMESTAMP WITH TIME ZONE ]  Timestamp(Nanosecond, "Europe/London")
size     [ FLOAT64                  ]  Unsupported SQL type FLOAT64
```

**Eager per row is the requirement, not a nicety.** Validation deferred to the press means
filling eight rows, getting `Unsupported SQL type FLOAT64`, and hunting for the row it came
from — worse than typing the statement by hand, which defeats the task.

DataFusion's `not_impl` wording reaches the user as written, which is already the app's stated
policy: "those are its clauses, described in its terms" (tables.rs module doc). Do not
paraphrase it.

### 3. Dispatch: `Engine::run` + the existing settle

**Create** composes the statement and dispatches it exactly as a Run does —
`Engine::run(ws, tag, sql, page_size)`, which classifies, intercepts, and returns
`RunOutcome::Statement(report)`. A non-tab caller is fine here: `WsId` is a bare `u128`
(engine/mod.rs:134) and the agent path already dispatches on a minted one rather than a tab's
(AGENTS §2), and `RunTag` is a per-press nonce.

**The load-bearing part is the fold.** The statement's `StoreEffect` is what puts the row in
the store, the def in `project.json`, the epoch bump behind every tab's diagnostics, and the
entry in the log — and today that fold is `use_statement_settle`, driven per `RequestPin` from
`views::keeper` and shaped around a `UseQuery<RunQuery>`
([`state/statement.rs:56`](../../../crates/strata-freya/src/apps/project/state/statement.rs)).
A panel that calls `Engine::run` and stops has created a table the catalog never learns about.

So: expose `settle` (statement.rs:93) as the panel's entry — it already takes a
`StatementReport` and does the whole job, and `use_statement_settle` keeps being the
query-driven wrapper over it. **Do not add a second arm to `apply`, a second persist path, or a
second epoch bump** — the module doc is explicit that one fold serves every effect. There is
also no dedup risk: the `applied` flag guards a *pin's* re-render, and this call has no pin.

**Rejected: composing into a new tab and running it there.** It needs no new seam, and the
Shape panel's artifact really is a tab — but the Shape panel's *output is SQL the user will
edit*, while this one's output is a table. Handing back a tab for a completed intent, when the
feedback the user wants is the catalog row appearing, is the wrong artifact. Recorded here so
it is not re-proposed as a simplification.

### 4. Open in editor

The second button composes the same statement **unrun** into a new tab (the Shape panel's
`actions::open_sql` move). This is the escape hatch for anything the panel cannot express —
and with free-text types that is a short list, which is the point.

### 5. The trigger

The catalog `+` becomes a two-item menu: **From files…** (today's `ConfigureTarget::New`,
unchanged) and **Empty table…** (this panel).

**Seam with DB-05:** that task retires the catalog pane and moves the `+` to the data-sources
tree's header. Build the menu wherever the `+` lives when this is picked up — the menu and the
panel are unaffected by which surface hosts them. Noted in both task files.

## Acceptance

- [x] The catalog `+` offers *From files…* and *Empty table…*; the first behaves exactly as the
      bare `+` does today.
- [x] The panel composes a statement visible in the card and creates a real internal table: the
      row appears in TABLES, `project.json` carries the def, the log carries the report, and it
      survives a restart.
- [x] Each column row validates as it is typed, showing either the Arrow type the planner
      produced or the planner's own refusal — never a deferred error at the press.
- [x] The form offers no constraint and no default, and a duplicate column name is refused with
      the arm's own message.
- [x] *Open in editor* opens the same statement, unrun, in a new tab.
- [x] No second `StoreEffect` fold, persist path or epoch bump — `settle` is reached, not
      reimplemented.
- [x] `cargo clippy --workspace --all-targets --locked -- -D warnings` clean; suite green.

## Freya / references

- `components::modal::Modal` + the Shape panel's own card (`results/shape/mod.rs:1-50`).
- `components::form`'s `Form` / `Row`; the free-form-list rule (AGENTS §2) for the column rows.
- `state/statement.rs` — `settle`, `apply`, and the module doc explaining why the fold is one.
- `engine/ddl/tables.rs` — `create`, the refusals, and the module doc on what stays
  DataFusion's.
- `engine/sql/validate.rs:568` — the `Ctas` / `CreateTable` split.
