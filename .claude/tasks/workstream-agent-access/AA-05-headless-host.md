# AA-05 · Headless host: `strata mcp <project>` over stdio

**Workstream:** Agent access · **Status:** ✅ · **DEV_TASKS:** — · **Depends on:** AA-01, AA-02

## As built

- **The CLI branch** is `main.rs`'s `cli(args) -> Cli` — a pure parser over an iterator, taken
  **first in `main`**, ahead of the theme registry, app config, the windows registry, the menubar
  and the fonts. `Cli::{Gui(Option<String>), Mcp(String), Usage}`; `startup` now takes the folder
  as an argument rather than reading `env::args` itself, which is what makes both testable.
  `strata mcp` with no folder — or with two — is a usage error naming the form, because a client
  that spawned this is waiting on stdout for MCP and a GUI would leave it waiting forever.
- **Logging is a parameter, not a constant**: `init_logging(Log::Stdout | Log::Stderr)`. The
  headless branch takes `Stderr` before anything can log, stdout being the transport's.
- **The host** is `strata_agent::headless::HeadlessHost` — in the vocabulary's own crate, beside
  the mock and the trait, so it is testable with no GUI and no renderer. `HeadlessHost::open(root)`
  loads the defs, builds `Engine::new(BTreeMap::new())` and replays `register_project`; the pass's
  outcomes are folded **once** into the catalog listing and the `describe` listing, which is the
  "registration outcomes *are* the catalog" rule as data rather than as a lookup.
- **The pass completes before anything is served**, which is why this host needs no equivalent of
  the app's scan claim: `register` deregisters before re-inferring, and there is no second pass
  here to race a query.
- **A folder with no project is refused, not scaffolded** (`exists_at` first, with its own
  message). The GUI open path scaffolds; a server the user cannot see must not create the files
  the app owns.
- **`default_page_size` is `Settings::default().row_limit`** — the shipped default reached without
  opening app config. Same reasoning for the engine: no `datafusion.*` overrides in v1, stated in
  the module doc.
- **Query sessions are engine workspaces and nothing else**, with AA-03c's close-vs-dispatch
  tombstone kept: a close aborts the engine immediately and, if a run is still in flight, leaves
  the row for the last settle to sweep (`dispatched_back`). Without it a close landing between the
  ownership check and `engine.query` would leave a workspace nothing holds.
- **The `project` argument is not consulted** anywhere in the impl: `host::resolve` only ever
  hands back a project this host listed, and it listed one. A lookup would be a check that can
  only pass, and its error arm a taxonomy entry nothing can reach.
- **`Described::name()`** moved onto the type (was a private helper in `mock.rs`), so the two
  hosts match a `describe_table` name the same way and a new variant is forgettable in one place
  rather than two.
- **Tests**: `cli` unit tests in `main.rs` (three forms); `headless.rs` unit tests over a real
  project on disk (catalog states, describe per kind, a real run in a session, cross-agent
  scoping, close/disconnect teardown, the refusal, the page size); and
  `tests/mcp_over_stdio.rs` — rmcp's own client against `StrataTools` over a `tokio::io::duplex`
  pair, which is the *same* `AsyncRead + AsyncWrite` adapter `stdio()` feeds. It asserts the
  vocabulary end to end, the policy refusal, and that `.strata/` still holds only what it
  arrived with (no `session.json`, no `history.jsonl`).

## Goal
The same vocabulary with the app closed: `strata mcp <project>` serves MCP over **stdio** (the
client owns the process — Claude Code spawns it) against a plain `Engine` built from the
project's defs. Spec §10.

## Current state
AA-02's `Host` trait has one impl (in-app). AA-01 extracted the registration pass
(`register_project(engine, defs)`). `main()` handles `argv[1]` as a project path
(`crates/strata-freya/src/main.rs::startup`) and nothing else.

## What to build

### The CLI branch
In `main()`, **before** logging-to-stderr-only is even a question and before any GUI or
app-global work: if `argv[1] == "mcp"`, resolve `argv[2]` through the same
`platform::resolve_project_folder` normalisation the GUI open path uses (naming `.strata` opens
the project), then run the headless host and exit. It must not touch the theme registry, app
config, the windows registry, or fonts — none of that exists headless.

Two cautions:
- **Logging goes to stderr** (stdio transport owns stdout). The tracing subscriber setup must
  respect that in this branch.
- A bare folder as `argv[1]` still means "open the GUI on it" — the subcommand is the exact
  string `mcp`, and `strata mcp` with no path is a usage error naming the form.

### The host impl
`Host` over `Engine::new(BTreeMap::new())` + `load_defs` + AA-01's `register_project`:

- Registration outcomes **are** the catalog: `list_tables` reports `failed` rows from them,
  the same shape the in-app host projects from the store.
- Tab handles are plain workspaces (`WsId` nonces) — no UI, but one vocabulary everywhere; the
  supersede/retire semantics come from the engine as always.
- Engine config: run with defaults in v1 (the app's `datafusion.*` overrides live in *app*
  config, which this branch deliberately does not read — note it in the doc comment; a
  `--config` flag can come later if wanted).
- Shared-state honesty (already guaranteed, assert in tests where cheap): no app-config
  read/write, no `session.json`, no history writes; snapshot dirs are lock-claimed per engine
  (`claim_snapshot_dir`), so running beside the live app is safe.

### The transport
rmcp's stdio server transport over the same tool router. No port, no token — process ownership
is the auth.

## Acceptance
- `claude mcp add strata-headless -- strata mcp /path/to/project` works: list/describe/run/page
  against a real project with the GUI closed. (The bundled binary is
  `/Applications/Strata.app/Contents/MacOS/Strata` — `CFBundleExecutable`, not the cargo bin
  name. README's Agent access section carries the client configuration.)
- Runs fine while the app is open on the same project (side-by-side engines; distinct snapshot
  dirs).
- A project with a failing table def serves it as a `failed` catalog row; queries against the
  others work.
- Policy gate verified over stdio too (blocked DDL refused with the editor's message).
- Unit tests for the arg parsing (subcommand vs project path vs usage error) beside `startup`'s
  logic; an integration test driving the host impl directly (no GUI).

## Notes
- Do not reuse or reach for the in-app service directory — this host has exactly one project by
  construction; `list_projects` returns it.
- If `--connect` (stdio↔HTTP proxy to a running app, the JetBrains shape / Claude Desktop
  pairing) gets built later it is a separate mode with its own task — spec §11 parks it.
