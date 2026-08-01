# Strata

A local, **Athena-style parquet query workspace** — a polished native IDE for querying parquet, CSV and JSON with SQL,
with no Glue catalog or schema setup. Built with
[Freya](https://freyaui.dev/) 0.4 (Skia, native — no webview) and
[Apache DataFusion](https://datafusion.apache.org/).

Work is organised into **projects**: a folder with a `.strata/` directory holding its catalog, session and query
history. Open one per window; the app reopens what you had at last quit.

---

## What it does

- **Catalog** of external **tables** (parquet/csv/json over files, directories, or globs — one table over any mix) and
  **views** (saved SQL), in a filterable sidebar with type-coloured columns and Hive `PART` badges.
- **Query workspace** — tabs, a syntax-highlighted SQL editor (DataFusion dialect) with completion and live
  diagnostics, Run (⌘/Ctrl+Enter), Explain and Explain analyze, Format SQL, Save and Save-as-view.
- **Results grid** — virtualized, type-coloured cells with per-column resize and autofit, sort, find-in-results,
  pagination, and cell/row/column selection with copy as TSV / CSV / JSON / Markdown. Double-click a nested cell for
  its JSON, or the row gutter for the whole record. `EXPLAIN` renders as a plan tree.
- **Column inspector** — type, nested-field tree, and **only facts that were actually read**: parquet footer
  statistics and the table row count, plus an opt-in full **scan** (behind a cost confirm) for the numbers the footer
  can't answer.
- **Bottom drawer** — **Problems** (every open tab's SQL diagnostics, grouped by tab), **Events** (what the session
  did), **History** (past runs; press to load, double-press to load and run).
- **Table Config** — multi-path sources with browse, format options, and Hive-partition detection (typed, with the
  string-cast warning).
- **Export window** (via `COPY … TO`) — parquet/csv/json/arrow with per-format options, Hive partitioning, and a
  preview built from the run's real schema and rows.
- **Settings window** — Theme (with sync-with-OS), System, Data display, and **Engine ▸ Properties**, which edits
  DataFusion's own configuration keys directly.
- **Themes** — `Midnight` (dark) and `Daylight` (light) ship built in; user themes are JSON files in the themes
  directory. See [`docs/FREYA_THEME_SPEC.md`](docs/FREYA_THEME_SPEC.md).
- **Agent access** — an opt-in MCP server so an AI agent (Claude Code, Cursor, Copilot…) can list your catalog,
  inspect schemas and run read-only SQL. Its queries are **real runs** on your engine, shown in the sidebar's
  **Agents** pane — a press opens any of them in a new tab. Your tabs stay yours. See
  [Agent access](#agent-access) below.
- **Managed catalog DDL policy** — the editor runs `SELECT`/`EXPLAIN`/`SHOW`/`DESCRIBE` **only**. Everything else is
  blocked with a message naming the surface that owns it: `CREATE TABLE` / `CREATE EXTERNAL TABLE` / `INSERT` → Table
  Config, `CREATE VIEW` → Save as view, `DROP` → the catalog, `COPY TO` → Export, `SET`/`RESET` → Settings.
  `CREATE DATABASE`/`SCHEMA` are refused outright.

---

## Prerequisites

- A Rust toolchain via [rustup](https://rustup.rs).
- **The `crates/freya` submodule.** Freya is our fork, and the build resolves it from the **local checkout path**
  (root `Cargo.toml`, `[workspace.dependencies]`) — without it nothing compiles:

  ```bash
  git submodule update --init --checkout
  ```

  `git submodule status crates/freya` should print no `+` or `-` prefix. (`git worktree add` does **not** update
  submodules — run the command above in each new worktree.)

There is no webview and no system GUI dependency to install; Freya renders with Skia, which compiles from source on
the first build. macOS is the platform Strata ships and is tested on — CI runs on macOS, and the menubar,
traffic-light gutter and child-window pinning are macOS-specific.

---

## Build & run

```bash
cargo run
```

The root's `default-members` is the Freya app, so a bare `cargo run` at the repo root is the app. `cargo run --release`
is the one to use for real work. Either way the first build pulls DataFusion and compiles Skia — give it time.

To try it against real data, open the repo's **`sample/`** folder as a project: it registers parquet, CSV and JSON
tables, a Hive-partitioned `events/` directory, and two saved views.

### Tests

```bash
cargo test --workspace --locked
```

`--workspace` rather than a bare `cargo test`, which `default-members` would narrow to `strata-freya` alone. After any
theme change, regenerate and verify the committed schema:

```bash
UPDATE_SCHEMA=1 cargo test -p strata-freya schema_in_sync
```

---

## Getting a build

For a distributable `.app` and DMG:

```bash
./scripts/bundle-macos.sh
```

Universal binary and DMG land in `target/dist/`; `--arch arm64` is roughly half the build time, `--no-dmg` stops at
the `.app`. The same script runs on a GitHub runner via **Actions → Release**, which can attach the DMG to a run or
publish a release page.

**[`docs/RELEASING.md`](docs/RELEASING.md)** is the full account — the workflow's inputs, cutting a version, what
signing rung the build took, and the `xattr -dr com.apple.quarantine` step testers need while builds are unsigned.

---

## Agent access

Strata can serve its own catalog and query engine to an AI agent over the
[Model Context Protocol](https://modelcontextprotocol.io). The agent lists tables, inspects schemas and runs
**read-only** SQL — the editor's policy exactly. Its queries are real runs against the same engine, materializing the
same immutable snapshots your own do, so anything it finds you can page, sort, export or take over.

The agent works in **query sessions** of its own, not in your tabs: the sidebar's **Agents** pane shows each connected
agent, what it is working on and every query it has run, with the figures each one came back with. Press a query and it
opens in a **new** tab, yours to read, edit and run. Nothing an agent does opens, focuses or closes a tab of yours, and
none of it reaches your query history — a query you promote and run goes in there like any other. Only connected agents
appear: a client that disconnects takes its query sessions with it.

Turn it on in **Settings ▸ Agent access**, which is also where the port and the bearer token live. It is **off by
default**. The token is minted once and persisted, so a client stays configured across restarts — regenerating it
invalidates every client you have set up.

The header bar shows a dot to the left of the search button: **grey** while nothing is paired, **green** once an agent
connects, **amber** if the setting is on but the server could not start — almost always a port already in use.

Everything a client needs is three facts:

| | |
|---|---|
| **URL** | `http://127.0.0.1:<port>/mcp` — `47821` by default |
| **Header** | `Authorization: Bearer <token>` |
| **Transport** | Streamable HTTP (some clients spell it `streamable-http`) |

The server lives *inside* the running app, so there is no command for a client to spawn — Strata has to be open, with
the project you want the agent to see open in a window. A client that only speaks stdio needs a proxy; see Claude
Desktop below.

### Claude Code

```bash
claude mcp add --transport http strata http://127.0.0.1:47821/mcp --header "Authorization: Bearer YOUR_TOKEN"
```

`--scope user` makes it available in every project; `claude mcp list` reports `✔ Connected` when it is working. The
equivalent `.mcp.json` entry — `"type"` is required, since Claude Code reads a typeless entry as a stdio server:

```json
{
  "mcpServers": {
    "strata": {
      "type": "http",
      "url": "http://127.0.0.1:47821/mcp",
      "headers": { "Authorization": "Bearer YOUR_TOKEN" }
    }
  }
}
```

### Claude Desktop

Desktop launches its servers itself and speaks stdio, which an in-app server cannot offer — so it needs a stdio↔HTTP
proxy. In `claude_desktop_config.json` (Settings ▸ Developer ▸ Edit Config), with Node.js installed:

```json
{
  "mcpServers": {
    "strata": {
      "command": "npx",
      "args": ["-y", "mcp-remote", "http://127.0.0.1:47821/mcp",
               "--header", "Authorization: Bearer YOUR_TOKEN"]
    }
  }
}
```

### VS Code (Copilot agent mode)

`.vscode/mcp.json` for one project, or your profile's `mcp.json` for all of them. `${input:…}` prompts for the token
rather than committing it:

```json
{
  "servers": {
    "strata": {
      "type": "http",
      "url": "http://127.0.0.1:47821/mcp",
      "headers": { "Authorization": "Bearer ${input:strata-token}" }
    }
  }
}
```

### Cursor

`.cursor/mcp.json` for one project, `~/.cursor/mcp.json` globally:

```json
{
  "mcpServers": {
    "strata": {
      "url": "http://127.0.0.1:47821/mcp",
      "headers": { "Authorization": "Bearer ${env:STRATA_TOKEN}" }
    }
  }
}
```

### Gemini CLI

`~/.gemini/settings.json`, or `.gemini/settings.json` per project. The field is `httpUrl`, not `url`:

```json
{
  "mcpServers": {
    "strata": {
      "httpUrl": "http://127.0.0.1:47821/mcp",
      "headers": { "Authorization": "Bearer YOUR_TOKEN" }
    }
  }
}
```

### Codex CLI

`~/.codex/config.toml`, or `.codex/config.toml` in a trusted project. Codex takes the **name of an environment
variable**, not the token itself:

```toml
[mcp_servers.strata]
url = "http://127.0.0.1:47821/mcp"
bearer_token_env_var = "STRATA_TOKEN"
```

Older Codex versions only pick up stdio servers; add `[features]` with `experimental_use_rmcp_client = true` above it,
or upgrade.

### Anything else

Point it at the URL as a Streamable HTTP server with that header. The token is checked before a request reaches a
tool, so a missing or wrong one is a plain `401`; the scheme is matched case-insensitively, the secret is not.

### What you are exposing

The server binds loopback only and requires the token on every request, but within those bounds an agent can read
**any data the open project can reach** — every registered table and anything a query can join to it. It cannot write:
there is no table registration, no view creation and no export in the vocabulary, and blocked statements come back
with the same message the editor shows. Turn it off when you are not using it, and treat the token as a credential.

The design — the tool vocabulary, the policy gate, the error taxonomy and the Tokio↔Freya bridge — is
[`docs/AGENT_ACCESS_SPEC.md`](docs/AGENT_ACCESS_SPEC.md).

---

## Architecture

A virtual Cargo workspace (no root package):

```
crates/strata-freya         the Freya (Skia/native) frontend — the app; one module per OS window
                            under apps/ (project, launcher, settings, export, configure)
crates/strata-core          engine logic: the DataFusion boundary (query/plan/profile/serialize),
                            config, theme, SQL language service. The only place DataFusion is touched
crates/strata-model         leaf data vocabulary, serde only (schema, results, catalog, session…)
crates/strata-code-editor   vendored Skia code editor (Rope buffer + tree-sitter highlighting)
crates/freya                our Freya fork (git submodule), resolved by local path — excluded
                            from the workspace, but the build depends on this checkout
```

The engine (`strata_core::engine::Engine`) is a **direct-call async facade**: it owns a private multi-thread Tokio
runtime, spawns each call onto it, and the caller awaits the `JoinHandle` — so query CPU never touches the render
thread and Freya's non-Tokio UI executor awaits engine methods like any async fn. There are no channels, no request
ids and no worker loop; the Dioxus-era `Command`/`Event` protocol was deleted in the port.

The full per-module map is in **[`docs/reference/MODULE_MAP.md`](docs/reference/MODULE_MAP.md)**,
and the state design in [`docs/FREYA_STATE_ARCHITECTURE.md`](docs/FREYA_STATE_ARCHITECTURE.md).

---

## Docs

- [`CLAUDE.md`](CLAUDE.md) — build, workspace layout, docs index, and where to look for the rest.
- [`AGENTS.md`](AGENTS.md) — the engineering bar and every settled convention, one line each.
- [`docs/reference/`](docs/reference) — the detail behind both: the module map, the architecture
  invariants and their reasoning, the Freya UI conventions, the engine model, the fork/git workflow,
  and what each finished task settled.
- [`docs/FREYA_PORT_PLAN.md`](docs/FREYA_PORT_PLAN.md) — why the migration, and the phased plan.
- [`docs/FREYA_STATE_ARCHITECTURE.md`](docs/FREYA_STATE_ARCHITECTURE.md) — the per-window state design.
- [`docs/SNAPSHOT_SPEC.md`](docs/SNAPSHOT_SPEC.md) — the result-snapshot read model.
- [`docs/AGENT_ACCESS_SPEC.md`](docs/AGENT_ACCESS_SPEC.md) — the agent tool vocabulary, its policy gate, and the bridge.
- [`docs/DEV_TASKS.md`](docs/DEV_TASKS.md) — the backlog. Task files live in `.claude/tasks/`.

---

## Status

Under active development. Strata began as a Dioxus (webview) app and was rewritten on Freya for native rendering; the
Dioxus frontend has since been removed, so the Freya app is the only one. The catalog, editor, results grid,
inspector, drawer, export and settings surfaces are in place. Remaining work — the connections and chart workstreams
among it — is tracked in [`docs/DEV_TASKS.md`](docs/DEV_TASKS.md) and `.claude/tasks/`.

## License

MIT.
