# Strata — project guide

Strata is a local, **Athena-style parquet query workspace**: a polished dark IDE for querying
parquet/csv/json with SQL over **Apache DataFusion**, with no Glue catalog or schema setup. Catalog
of external tables + saved views, a tabbed SQL editor, a virtualized results grid, a column
inspector, table config, export via `COPY … TO`, a command palette, and query history. Product
name **Strata** (uneven sedimentary layers = data strata).

The app is built on **Freya 0.4 (Skia/native)**. It began as a Dioxus (wry/webview) app and was
rewritten clean-slate on Freya; the Dioxus frontend has been **deleted**. The open work is the
remaining workstreams (`.claude/tasks/`): the statement lift (ED-09..11), the assistant's
Settings roster and chat pane (AS-03/AS-04 — the loop under them is built), chart follow-ons,
and design polish.

This file is the **map** — build, layout, and where everything is. @AGENTS.md is the **bar** — the
rules, one line each, imported into every session alongside this file; hold all work to it. Both
are deliberately short: the detail they used to carry lives in `docs/reference/` and is loaded on
demand. **Read the reference file that covers your area before working in it** (routing table
below) — those files hold the reasoning, and most of the rules in them were settled after a wrong
version was built and rejected in review.

---

## Build & run

```bash
cargo run              # root default-member = the Freya app (strata-freya)
cargo run --release    # first build pulls DataFusion + compiles Skia; give it time
```

After **any theme change**, regenerate + verify the schema:
`UPDATE_SCHEMA=1 cargo test -p strata-freya schema_in_sync` (the committed
`themes/theme.schema.json` must match the `Role` vocabulary + the editor's syntax scopes).

**`cargo test` needs a container runtime.** The connections integration test
(`strata-core/tests/object_store_minio.rs`, W7) drives a real MinIO through testcontainers and is
deliberately **not** `#[ignore]`d — an ignored test is one nobody runs, and this is the only thing
that would catch a regression in the S3 credential bridge. Testcontainers Cloud, Docker or colima
all serve — the runtime is found from `~/.testcontainers.properties` (which a Testcontainers Cloud
agent writes) or `DOCKER_HOST`, which is why `testcontainers` carries the **`properties-config`**
feature: without it that file is `#[cfg]`'d out and a perfectly good runtime reads as absent.
Without any runtime the test **fails** rather than skipping, on purpose: "no runtime" must not look
like "the code is fine".

To build something you can hand to a tester — a universal `.app` + DMG in `target/dist/`:

```bash
./scripts/bundle-macos.sh              # universal; --arch arm64 for a quick local check
```

The same script is what the **Release** workflow runs (Actions → Release → Run workflow), which can
also bump the crate version, tag the commit and publish a release page in the same press.
`scripts/version.sh` owns the version number. See **`docs/RELEASING.md`**.

Linting is part of the check, and CI runs it before the tests:

```bash
cargo clippy --workspace --all-targets --locked -- -D warnings
```

The curated lint set is `[workspace.lints]` in the root `Cargo.toml` and the thresholds are
`clippy.toml`; both are annotated, and [AGENTS.md](AGENTS.md) §7 has the rule for adding to them.

Formatting is the **`fmt` skill**, never `cargo fmt --all` (which reformats the fork — see
[AGENTS.md](AGENTS.md) §7). Running the app is the **`run-app` skill**; one Strata window across
every session, enforced by a hook. Reviewing a change *you* just wrote is the
**`adversarial-review` skill** — isolated hostile critics plus a refutation gate, in front of the
build check rather than in place of it.

> **Environment note:** some agent sandboxes can't build this (no crates.io access, no Skia
> toolchain). If you're in one, you can't run `cargo build`/`test` — verify changes against the fork
> source instead and hand off to a Mac build. Claude Code running locally on the Mac has no such
> limit: build and test normally, and treat a clean build + `schema_in_sync` as the check.

---

## Where to look

| Working on | Read first |
|---|---|
| Locating code in `strata-freya`, or placing something new | [docs/reference/MODULE_MAP.md](docs/reference/MODULE_MAP.md) |
| Engine, query, snapshot, catalog, history, window lifetime, config, agent access | [docs/reference/INVARIANTS.md](docs/reference/INVARIANTS.md) |
| Any Freya UI — components, events, layout, theming, state placement | [docs/reference/FREYA_UI.md](docs/reference/FREYA_UI.md) + the `freya:freya` skill |
| The DataFusion boundary, SQL/DDL policy, the function registry | [docs/reference/ENGINE.md](docs/reference/ENGINE.md) |
| Editing `crates/freya`, git, CI, releases, verification | [docs/reference/WORKFLOW.md](docs/reference/WORKFLOW.md) |
| Why an existing surface is shaped the way it is | [docs/reference/SETTLED_TASKS.md](docs/reference/SETTLED_TASKS.md) |
| Picking up a task | `.claude/tasks/README.md`, then the task's own file |

[AGENTS.md](AGENTS.md) carries the one-line form of every rule in `INVARIANTS.md`, `FREYA_UI.md`
and `WORKFLOW.md`, each linking to its full entry. Act on the one-liner; read the entry before
extending, arguing with, or overturning it.

---

## Workspace layout

A virtual workspace (no root package). `cargo run` at the root targets the **Freya** app.

Members:

- **`strata-freya`** — the Freya (Skia/native) frontend. **The app** and the default build.
- **`strata-core`** — engine logic: the DataFusion boundary (query/plan/profile/serialize/value_tree),
  config, theme, the OS-keystore secret store, SQL language service. The only place DataFusion is touched — bar a **dev**-dependency
  in `strata-freya`, so a test can build an Arrow fixture without bending a signature to be testable.
- **`strata-model`** — leaf data vocabulary, serde-only (schema, results, catalog, form, history,
  session, query_error). No logic. (The event log is *not* here: it is ephemeral app state —
  `strata-freya::apps::project::state::log`.)
- **`strata-code-editor`** — vendored Skia code editor (Rope buffer + tree-sitter highlighting) used
  by the Freya SQL editor.
- **`strata-agent`** — agent access (AA-02): the read-only tool vocabulary, the `Host` seam that
  answers it, the error taxonomy, the MCP server, the **headless host** (AA-05 —
  `strata mcp <project>`: a `Host` over a plain `Engine` with AA-01's registration pass replayed
  on it, served over stdio), and the **assistant** (AS-02, `assistant/`): the agentic chat loop
  over a pluggable provider seam (`genai`), its one provider table, and `offer_sql`. A member
  but **not** a default one, and deliberately **Freya-free** — that is what lets one
  `StrataTools` serve HTTP (AA-03), stdio headless (AA-05) and the in-process chat pane
  (AS-04), and lets the loop be tested with no window, no renderer and no vendor account.
  Spec: `docs/AGENT_ACCESS_SPEC.md`.
- **`strata-command-macro`** — the workspace's one proc macro: `#[command_router]` / `#[command]`,
  the command palette's registration mechanism (P6-01). rmcp's `#[tool_router]` declaration shape,
  but it generates an **enum**, so dispatch is total by construction. It knows nothing about
  Strata's types, which keeps it a registration mechanism rather than a second vocabulary.

(The **forms** layer is `strata-freya::components::form`, not a crate.)

Excluded from the workspace (deliberately):

- **`crates/freya`** — our **Freya fork checkout** (below).

**Note on the old frontend:** the Dioxus app (`crates/strata-dioxus`) has been **deleted** — the
Freya app is a clean-slate, Valin-shaped rewrite with its own architecture (Radio `SessionState`,
stateful `QueryTab`s, `EngineCtx` in context). Its patterns (`GlobalStore`, `dispatch`/`action`,
the muda menu, the old keymap/hotkeys registry, the `Command`/`Event` engine protocol) must not
come back; if you find one referenced in older notes, it is history, not a target.

## The Freya fork

`crates/freya` is a **git submodule** pointing at our fork, `github.com:alexparlett/freya`.

- The build resolves Freya from the **local checkout path** (`[workspace.dependencies]` in the root
  `Cargo.toml`), *not* from git — so fork edits are picked up on the next `cargo build`.
- **But** the committed gitlink must be pushed to the fork remote, or a fresh clone / CI can't init
  the submodule. After changing the fork, push it.
- In a fresh worktree, use the **`freya-submodule` skill** before the first build.

Full rules — when to change the fork, the unpushed-gitlink trap, worktree traps:
[docs/reference/WORKFLOW.md](docs/reference/WORKFLOW.md).

## Freya: skill, reference, examples

When writing Freya code, lean on these in roughly this order:

1. **The `freya` skill** (`freya:freya`) — components, hooks, elements, events, state, theming
   (`define_theme!` / `get_theme!`), async, keying, a11y. The fast reference for *how* to structure
   things. Strata's own conventions on top of it: [docs/reference/FREYA_UI.md](docs/reference/FREYA_UI.md).
2. **The fork source** — `crates/freya/`. Ground truth for exact APIs:
   `crates/freya/crates/freya-core/src/events/` (event data + names),
   `crates/freya/crates/freya-components/` (built-ins), `crates/freya/src/_docs/`.
   `crates/freya/AGENTS.md` documents Freya's own dev workflow.
3. **`crates/freya/examples/`** — 150+ runnable examples (`component_*.rs`, `animation_*.rs`, plus
   platform samples). The canonical "how do I wire X" reference.

---

## Engine model (short)

`strata_core::engine::Engine` is a **direct-call async facade**: it owns a private multi-thread
Tokio runtime, spawns each call onto it, and the caller awaits the `JoinHandle` — executor-agnostic,
so Freya's non-Tokio UI executor awaits engine methods like any async fn. No UI-side runtime, no
channels, no request ids. In Freya the handle is `EngineCtx` (`Arc<Engine>` + Deref) held in
context. Snapshots are **Arrow IPC**; lifecycle is the facade's own bookkeeping
(`docs/SNAPSHOT_SPEC.md`). The SQL function set is the **live registry**, not a list we keep.
Statement policy is one router in front of dispatch: `Engine::run` classifies, then runs a query,
intercepts a statement, or refuses it — the editor runs queries, the table statements
(`CREATE TABLE`/CTAS, `INSERT`, `DROP TABLE`), view DDL, `COPY` and the session statements today,
the remaining intercepted statements are being lifted one by one (ED-09..11), and the agent stays
read-only.

Full model — the snapshot format argument, the function registry, the statement router and its
surfaces: [docs/reference/ENGINE.md](docs/reference/ENGINE.md).

---

## Docs index (`docs/`)

Everything in `docs/` is **documentation of the code as built** — plans and tracking live in
`.claude/tasks/`. [docs/README.md](docs/README.md) is the index; keep every document true in the
same change as the code, exactly as with the task files.

- **`ARCHITECTURE.md`** — the system end to end: workspace, engine, query round trip, statement
  routing, state, windows. The place to start (and to keep pointing at the right detail docs).
- **`reference/`** — the agent-facing detail split out of this file and AGENTS.md (routing table
  above).
- **`FREYA_STATE_ARCHITECTURE.md`** — per-window state in full; every API verified against
  Freya 0.4 source.
- **`RELEASING.md`** — how a build reaches a tester: `scripts/bundle-macos.sh`, the **Release**
  workflow, `scripts/version.sh`, signing/notarization, the Gatekeeper bypass.
- Feature docs: `SNAPSHOT_SPEC.md` (the result read model), `STATEMENTS_SPEC.md` (the statement
  router and surface), `COMPLETION_SPEC.md`, `EXPLAIN_PLAN_SPEC.md`, `CHART_SPEC.md`
  (+ `CHART_FUNCTIONS.md`, the chart-side SQL survey), `CONNECTIONS_SPEC.md`,
  `IMPORT_OPTIONS.md`, `EXPORT_OPTIONS.md`, `AGENT_ACCESS_SPEC.md` (run dataflow diagram
  inlined), `FREYA_THEME_SPEC.md`.
  The `_SPEC` suffixes are historical — engine code comments cite these paths, so the names stay.

The **design handoff** lives in **`.claude/design-handoff/`** (gitignored — local only). It's a
Claude Design (claude.ai/design) bundle: the `.dc.html` HTML/CSS prototypes that are the
**pixel-perfect source of truth** for every surface (`Strata`, `Settings`, `Launcher`, `Windows`,
`DrawerProblems`, `StatusBar`, …), plus `strata-windows.js`, reference `screenshots/`, and a
per-bundle README. Read the `.dc.html` source directly; don't render or screenshot unless asked.

---

## Task backlog (`.claude/tasks/`)

The backlog lives in **`.claude/tasks/`** (committed): a top `README.md` index, then **one folder
per phase / workstream**, each with its own `README.md` and **one file per task**. Each task file
is self-contained — current state, what to build, acceptance, Freya components — so a session can
pick up a single task (e.g. in a worktree) without loading the rest. Read the top `README.md`
first (status legend, what remains, known bugs).

The numbered phases are done (their folders removed); what remains is design polish (phase 5) and
the open workstream tasks — the statement lift (ED-09..11), the assistant's Settings roster and
chat pane (AS-03/AS-04) and the chart follow-ons. **What each finished task settled — including
several corrections that must not
be re-litigated** (the catalog is a store and not a query; diagnostics are a reconciliation; a
log is recorded by its observer; only real facts) — is
[docs/reference/SETTLED_TASKS.md](docs/reference/SETTLED_TASKS.md), with the rule form of each in
[AGENTS.md](AGENTS.md) §2.
