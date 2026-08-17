# Architecture — the system as built

How Strata is put together, end to end: the workspace, the engine, the query round trip, the
statement pipeline, where state lives, and how windows relate. This is the guided tour; each section
links the document that owns its detail. If you are changing code rather than reading about it,
[AGENTS.md](../AGENTS.md) holds the conventions and principles.

---

## The workspace

A virtual Cargo workspace, eight member crates plus a vendored fork:

| Crate | Role |
|---|---|
| `strata-freya` | The app — Freya (Skia, native) frontend. One module per OS window under `apps/`: launcher, project, settings, export, configure, connection. The default build target. |
| `strata-core` | App services, DataFusion-free: config, keymap, themes, `.strata/` project persistence, the OS-keystore secret store, the model-listings satellite, the updater mechanism, shared `util`. |
| `strata-arrow` | The Arrow-level vocabulary, DataFusion-free: the `ColumnInfo` row an Arrow field becomes, the value tree, the Copy/record-view serializers, the EXPLAIN plan model, and the two written-down catalogues (DataFusion's config keys, `object_store`'s client options). Sits on `strata-core`. |
| `strata-engine` | The only place DataFusion is touched: query execution, the statement pipeline, snapshots, export, profiling, the SQL language service, the registration pass. Sits on `strata-arrow`. |
| `strata-model` | The leaf data vocabulary — schema, results, catalog, session, history, connections. Serde only, no logic, so every other crate can speak it without dragging dependencies. |
| `strata-code-editor` | The vendored Skia code editor (Rope buffer, tree-sitter highlighting, completion popup, diagnostic squiggles) the SQL surface is built on. |
| `strata-agent` | Agent access: the read-only MCP tool vocabulary, the HTTP server, and the headless stdio host. Deliberately Freya-free — one implementation serves the in-app server and `strata mcp` alike. |
| `strata-command-macro` | One proc macro: `#[command_router]` / `#[command]`, the command palette's registration mechanism. Knows nothing of Strata's types. |
| the Freya fork | [github.com/alexparlett/freya](https://github.com/alexparlett/freya) — an ordinary git dependency pinned by `Cargo.lock`; every build compiles against it. |

The dependency direction is strict: `strata-freya` sits on top; `strata-engine` sits on
`strata-arrow`, `strata-core` and `strata-model` alike; `strata-arrow` sits on the last two;
`strata-core` and `strata-agent` never depend on UI; `strata-model` depends on nothing of ours.
`strata-arrow` is a layer **below** the engine rather than one in front of `strata-core` — the
engine still reads core's services directly, and what the crate buys is the other direction: a
consumer can take the Arrow vocabulary without the DataFusion boundary above it. That is why
`strata-freya` and `strata-agent` name **both**: the engine for what only a planner can answer,
`strata-arrow` for everything else. The engine re-exports none of it — a `strata_engine` path to
an Arrow-vocabulary item fails the use-direction check
(`crates/strata-engine/src/boundaries.rs`), which reads the frontend's own `use` declarations
because a re-export added back here would silently undo the split. The same check holds the
engine's one internal boundary, the peer modules `sources` and `sql`. When a Freya limitation
shows up, the fix goes **into the fork**, not around it in app code.

**Arrow is pinned once, at the workspace.** `strata-arrow` names `arrow` directly while
`strata-engine` reaches the same crate through `datafusion::arrow`, and the two must resolve to
one arrow or a `RecordBatch` built on either side is a different type to the other. DataFusion 54
is on arrow 58, so the root manifest's `arrow` line and the engine's `datafusion` line move
together or not at all. This is what lets a surface that formats a cell, expands a nested value or
offers a config key depend on `strata-arrow` alone and compile no query planner to do it —
a claim `crates/strata-arrow/Cargo.toml` keeps rather than care does.

**Two reqwest majors, on purpose.** `strata-core` pins reqwest **0.12** because that is what
`strata-engine`'s `object_store` resolves, so the updater's HTTP client is the crate already
compiled into the graph rather than a second stack; it takes `default-features = false` with
exactly object_store's features, since reqwest's defaults would pull `native-tls` in beside the
rustls this graph resolves and a second TLS backend is a second root store to keep straight.
`strata-agent` pins **0.13** because `genai` does. The two coexist — nothing passes a reqwest
type across that seam, so there is no unification to force and no version to reconcile. Neither
pin is free to move on its own: each follows the dependency that chose it.

## The engine: a direct-call async facade

`strata_engine::Engine` owns a private multi-thread Tokio runtime (DataFusion's operators
need a Tokio context, and query CPU must never run on the render thread), spawns each call onto
it, and the caller awaits the `JoinHandle`. That await is executor-agnostic, so Freya's non-Tokio
UI executor awaits engine methods like any async fn. There are no channels, no request ids, no
event stream, no worker loop — a caller gets its own call's return value, and errors arrive as
that call's `Err`, not through a side channel.

In the app the handle is `EngineCtx` — an `Arc<Engine>` with `Deref`, held in context. One engine
per project window; the headless host builds the same engine over the same project without any of
the app around it.

## The query round trip: snapshots

Raw SQL is never a cache key — the same SQL over the same tables can read different files a
second later. So a **Run executes exactly once** and spools the full result to an immutable
on-disk **Arrow IPC** snapshot (LZ4-compressed; Arrow rather than parquet so a result's type
always survives — parquet cannot write a union at all). Every later read — page, sort, chart,
export — is a bounded read of that snapshot. Immutability is what makes the page cache sound and
paging stable.

```mermaid
sequenceDiagram
    participant U as Editor (Run ⌘↵)
    participant Q as freya-query<br/>(per-press cache entry)
    participant E as Engine<br/>(private Tokio runtime)
    participant S as Snapshot store<br/>(Arrow IPC on disk)

    U->>Q: QuerySpec { run: fresh nonce, sql, … }
    Q->>E: engine.run(ws, tag, sql)
    Note over E: classify → Query
    E->>S: execute once, spool __snap_{id}<br/>(+ __strata_ord ordinal column)
    E-->>Q: QueryOutput { snapshot, columns, total } + page 1
    Note over Q: settled — cached under the press's nonce

    U->>Q: page / sort
    Q->>E: fetch_page(snapshot, page, sort)
    E->>S: bounded read, ORDER BY __strata_ord
    E-->>Q: page rows (cached per key, forever sound)
```

The load-bearing rules, each held by construction rather than by care:

- **A Run is keyed by a per-press nonce**, so pressing Run is the only thing that executes, and
  revisiting a tab re-reads the cache rather than re-running the SQL.
- **Reads have no order of their own** — above DataFusion's file-split threshold an unordered
  `LIMIT/OFFSET` is measured-nondeterministic — so the spool writes a `__strata_ord` ordinal
  column and every reader orders by it (and projects it away; an export never writes it).
- **Retire-on-dispatch**: a new Run retires the tab's previous snapshot when it starts. A reader
  that outlives one press — the export window — **pins** the snapshot it reads (RAII); a retire
  arriving while pinned is deferred, never skipped.
- **DDL and catalog changes never retire a snapshot.** A result is point-in-time, Athena-style;
  result freshness is the Run button.

The full read model — identity, the lock-file sweep, pins, the ordinal measurements — is
[SNAPSHOT_SPEC.md](SNAPSHOT_SPEC.md).

## The statement pipeline

One pipeline sits in front of dispatch — `statements::accept`, three typed stages
(`Parsed → Qualified → Admitted`) whose order the types enforce — and `Engine::run` spends the
answer.

```mermaid
flowchart LR
    RUN["Engine::run<br/>(one statement per press)"] --> C{"accept<br/>parse → qualify → classify"}
    C -->|Query| Q["query()<br/>SELECT · EXPLAIN · SHOW · DESCRIBE<br/>→ snapshot pipeline"]
    C -->|Statement| I["ddl::execute<br/>CREATE EXTERNAL TABLE · CREATE TABLE / CTAS ·<br/>INSERT · DROP TABLE · CREATE / DROP VIEW ·<br/>COPY · SET / RESET · PREPARE / DEALLOCATE ·<br/>CREATE / DROP FUNCTION"]
    C -->|Refusal| R["the engine's own message,<br/>before DataFusion can plan<br/>(same string as the squiggle)"]
```

- **Grammar and policy are two questions.** `classify_stmt` answers what a statement *is*, purely
  from the parsed AST; an injected `PolicyProvider` answers whether the caller may perform it,
  in **codes** rather than prose so the engine mints every sentence from one table. The shipped
  provider is data — a `Capability` of grants over a local/remote axis — and its two presets are
  the app's editor (`full()`) and its agent (`read_only()`). A caller's capability narrows the
  engine's and never widens it, which is what lets one engine serve both.
- An interception lands in an app funnel that already exists: `CREATE TABLE` / CTAS spools into
  `.strata/tables/<slug>/` as Arrow IPC and registers through the ordinary external-table path —
  the def it produces is a plain `TableDef` flagged `origin: Internal`, so persist, replay and the
  headless host need no new code. A typed `CREATE EXTERNAL TABLE` is that same funnel with the def
  read off the statement instead, which is what makes it and Table Config two gestures at one
  registration — and is why its `LOCATION` names a **connection** the project already has rather
  than describing a bucket of its own.
- A statement's outcome is a value the app folds — a `StatementReport` carrying a `StoreEffect` —
  never something read back out of DataFusion. Strata owns the catalog list, catalog and schema
  providers for identity and visibility only; lifecycle is intercepted in front of `ctx.sql`,
  because a sync `register_table` with no caller identity can neither spool a CTAS nor authorize a
  `DROP`. The **workspace** catalog is one catalog with one flat, bare-name schema; a database
  connection registers a sibling catalog beside it, with as many schemas as the server has. A name
  qualified into one of those is read like any other and **managed by nothing** — v1 is read-only
  against a database — and one predicate (`providers::in_workspace`) draws that line for both the
  statement gate and the `__snap_` reserved namespace.

The statement surface and its policy tables are [STATEMENTS_SPEC.md](STATEMENTS_SPEC.md).

## Where state lives

Each project window is one **Session**, and the design splits two concerns that are easy to
tangle:

1. **Tab management** — a Radio store (`SessionState`) of stateful tabs. Each `QueryTab` owns its
   editor buffer (rope, cursor, undo), its run request, its results-view choice and its chart
   config, under granular per-concern channels — a keystroke wakes that tab's editor subscribers
   and nothing else.
2. **Query execution** — freya-query, keyed as above. **The store holds specs, never results**;
   there is no runs-by-id store, and cache-entry lifetime is subscriber presence (an invisible
   per-tab keeper holds a background tab's press alive).

Around those, satellites with one job each: the project store (the catalog — a store, **not** a
query against DataFusion; a def whose registration failed is exactly the row it must keep
showing), the event log, the agents satellite (bookkeeping only — no surface shows it), query
history (a `.jsonl` file, not a store
field), the assistant's model listings (`strata_core::models` — what each provider last reported
serving, beside the config file rather than in it, because a fetched list is a cache of a remote
fact and not something the user edited), the update status (`state::updates` over
`strata_core::update` — what the newest release is and where a verified download is staged; not
persisted, because a check result is a fact about a request made minutes ago), and one app-global
config store whose single write path also persists.

The updater is the one thing that outlives the event loop. It never mutates the running bundle:
the press records the swap and calls the ordinary `quit()`, so every close confirm keeps its say,
and `main` performs it after `launch` has returned and no window is left. What makes a download
installable is its signature rather than where it came from — strict `codesign`, the team id, the
bundle id, failing closed — so the network is untrusted by construction. What it *offers*, and
what a press means, is one pure answer (`updater::Affordance`) that the launcher rail's version
line, App ▸ Check for Updates… and the update dialog all read, so a dev build
offers nothing, a bundle that cannot be replaced degrades to the release page, and a staged
update is a restart. The status is app-global; the question the dialog asks is per window — the
restart, or the report the menubar item raises, which answers the check where it was asked and
shows the release's own Markdown when there is something to install.

Config never holds a secret. The one class of secret the app must keep — third-party provider keys
for the assistant — lives in the OS keystore (`strata_core::secret`, opened once in `main`), and
config carries only a minted `SecretRef` to it. That is enforced by the types rather than by care:
the in-memory `Secret` derives no `Serialize`, so there is no path from a pasted key to
`config.json` at all.

The full design — the channel vocabulary, persistence, the menu seam, the diagnostics driver —
is [FREYA_STATE_ARCHITECTURE.md](FREYA_STATE_ARCHITECTURE.md).

## Windows

Every OS window is its own Freya tree with its own state; nothing reactive is shared across
windows except the app-global config store and the theme registry.

- The **project window** is the workspace: two 48px rails, a sidebar (the data-sources
  tree — the project's own catalog and its connections in one tree), the tabbed workbench, a right pane (column inspector *or* the assistant's chat — the
  right rail picks one) and the drawer. One project per window; opening a project that is already
  windowed focuses it — that decision lives in one pure function.
- The **launcher** shows when no project is open and closes when one does.
- **Settings** is app-wide, one instance, pinned above the window that asked for it. Its edits
  are a draft; Apply commits a per-field diff against the seed.
- **Export**, **Configure** and **Connection** are child windows owned by a project window —
  closing the owner closes them, and their lifetime is tied to the project subtree they were
  opened from. Export pins the snapshot it was opened on.

Anything that must survive a project re-root lives on the window; the project subtree is keyed on
the project folder, and there is no reopen-in-place path.

## Agent access

`strata-agent` packages the same read-only questions the app answers — list tables, describe,
validate, run, page — as MCP tools over a `Host` seam, with two deployments today: the in-app
HTTP server (loopback, bearer token, off by default) and the headless stdio host
(`strata mcp <project>`). Agent runs are real engine runs on query sessions of the agent's own,
so they share the snapshot machinery and none of the user's tabs, history or settings.
Connecting a client is [MCP_CLIENTS.md](MCP_CLIENTS.md).

## Data in and out

- **Registration** — a table def names its sources (files, directories, globs; local or
  bucket-relative through a named connection) and its per-format read options; the registration
  pass connects connections first, then tables, then views to a fixed point. Failures land on
  the def's row, visible with their reason. [CONNECTIONS_SPEC.md](CONNECTIONS_SPEC.md),
  [IMPORT_OPTIONS.md](IMPORT_OPTIONS.md).
- **Databases** — a PostgreSQL connection registers a DataFusion **catalog** instead of an object
  store, so the whole database is queryable as `pg.public.orders` with no per-table declaration,
  and a same-source subplan is pushed back to the server as one statement
  (`datafusion-federation`). Discovery gets catalogs; declaration gets defs.
  [CONNECTIONS_SPEC.md](CONNECTIONS_SPEC.md).
- **Export** — the export window renders an `ExportSpec` into one `COPY … TO` over the pinned
  snapshot, with per-format options and Hive partitioning. [EXPORT_OPTIONS.md](EXPORT_OPTIONS.md).

## Reading on

| Question | Document |
|---|---|
| What runs, and what a result is | [SNAPSHOT_SPEC.md](SNAPSHOT_SPEC.md) |
| What the editor accepts, intercepts, refuses | [STATEMENTS_SPEC.md](STATEMENTS_SPEC.md) |
| How completion works | [COMPLETION_SPEC.md](COMPLETION_SPEC.md) |
| The EXPLAIN plan view | [EXPLAIN_PLAN_SPEC.md](EXPLAIN_PLAN_SPEC.md) |
| The chart view | [CHART_SPEC.md](CHART_SPEC.md) |
| Remote data | [CONNECTIONS_SPEC.md](CONNECTIONS_SPEC.md) |
| Per-window state | [FREYA_STATE_ARCHITECTURE.md](FREYA_STATE_ARCHITECTURE.md) |
| Connecting MCP clients | [MCP_CLIENTS.md](MCP_CLIENTS.md) |
| Themes | [FREYA_THEME_SPEC.md](FREYA_THEME_SPEC.md) |
| Shipping a build | [RELEASING.md](RELEASING.md) |
