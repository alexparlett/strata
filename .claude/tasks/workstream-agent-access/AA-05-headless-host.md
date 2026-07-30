# AA-05 · Headless host: `strata mcp <project>` over stdio

**Workstream:** Agent access · **Status:** ⬜ · **DEV_TASKS:** — · **Depends on:** AA-01, AA-02

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
  against a real project with the GUI closed.
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
