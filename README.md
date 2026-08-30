# Strata

[![CI](https://img.shields.io/github/actions/workflow/status/alexparlett/strata/ci.yml?branch=main&label=CI)](https://github.com/alexparlett/strata/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-MIT-blue)](LICENSE)
![macOS](https://img.shields.io/badge/platform-macOS-lightgrey)

Query your files like a database. Strata is a native macOS SQL workbench for parquet, CSV and
JSON: point it at a folder, get a catalog, write SQL, see results. No server to run, no schemas
to declare, no cloud console.

It exists because the gap between "I have a directory of parquet files" and "I can actually
explore them" is absurdly wide. The usual answers are a terminal one-liner, a Jupyter notebook,
or standing up Athena and a Glue catalog. Strata is the missing middle: a proper IDE, running
on [Apache DataFusion](https://datafusion.apache.org/), that treats your files as a database
from the moment you open the folder.

A few things Strata will always do:

- Run entirely on your machine. Your data never leaves it.
- Work offline, with no account and no telemetry.
- Keep every AI feature off by default, and bring-your-own-key when on.
- Stay free and open source.

Work lives in **projects**: a folder with a `.strata/` directory holding its catalog, history
and session. Open one per window; the app reopens what you had at last quit.

## A tour

### The catalog

Register a table over a file, a directory, a glob, or any mix of them, in any of the formats.
Hive-partitioned directories are detected by listing the actual `key=value` levels, not by
asking you to type them, and partition pruning works. Views are saved SQL and live beside the
tables. When a table's source goes missing it stays in the sidebar as a failed row with the
reason, because the catalog shows what your project says, not just what happened to register.

### Remote data

Data sources read the same formats straight out of S3, GCS, any S3-compatible store (Cloudflare
R2, MinIO, a custom endpoint), or plain HTTPS for a single public file. Strata never stores or
prompts for a bucket secret: a data source records a kind and an auth mode (ambient
credentials, a named `~/.aws` profile, a key file path) and credentials resolve at query time
from your machine's own chains. Running `aws sso login` in another terminal just works.

A **PostgreSQL** data source goes further: the whole database joins your project as a catalog of
its own, so `SELECT … FROM pg.public.orders JOIN local_events USING (id)` federates live server
data against your files, with filters and whole subplans pushed down to the server. Turn **Read
only** off on the data source and it becomes a load path too: `INSERT INTO pg.public.events SELECT
… FROM local_parquet`, and `CREATE TABLE pg.public.report AS SELECT …` to materialize any result —
a cross-source join included — as a real server table. It stays on until you say otherwise.

### The editor

A tabbed SQL editor with syntax highlighting and completion fed by the engine's own vocabulary:
keywords, tables, views, columns, CTE names, and functions with their real signatures.
Diagnostics go well past parse errors. Every statement is dry-planned against the live session,
so unknown columns and type errors get squiggles before you run anything. Run and cancel with
⌘↵, format, explain, save as view. Tabs keep their own buffer, undo history and results, and
all of it is restored when you reopen the project.

### Statements, not just SELECT

The editor runs real DDL and DML. `CREATE TABLE … AS SELECT` spools a durable table into the
project and `INSERT` appends to it. `CREATE VIEW`, `DROP`, `COPY … TO`, `SET`, `PREPARE`, even
`CREATE FUNCTION` for SQL macros: each one does what it says, reports its outcome in the
results pane, and shows up in the catalog immediately. The rule underneath is simple: a write
statement can only ever touch data Strata itself owns. Your files are read, never written, and
anything refused is refused by name with the reason.

### Results

The grid is virtualized and stays smooth on large results, with type-coloured cells,
whole-result sorting, find, pagination, and Excel-style selection you can copy out as TSV, CSV,
JSON or Markdown. The status bar keeps a live aggregate of whatever you have selected.
Double-click a row for a record view; double-click a nested cell for a lazy value tree.

Beyond the grid: flip the results pane to a **chart** (bar, line, area, scatter, histogram,
pie, with a trendline for scatter) that renders exactly the result you ran and computes nothing
SQL can say. Run `EXPLAIN ANALYZE` and get an operator tree with per-operator self-time and a
hotspot badge. Open the **column inspector** for parquet footer statistics and, behind a cost
confirm, a full scan. **Export** to CSV, JSON, parquet or Arrow, Hive-partitioned if you like,
with a preview built from your real rows.

### The assistant

An optional chat pane that knows your project. Bring your own key: Anthropic, OpenAI, Gemini,
DeepSeek, Groq, xAI, a local Ollama, or any OpenAI-compatible endpoint. The assistant works
through the same read-only tools an external agent gets, so it can explore your catalog and run
queries, but it cannot touch your tabs or change your project. When it lands on useful SQL it
offers the statement as a card you can run or open in a tab of your own. Conversations are
saved with the project.

### The app around it

A command palette (⌘K) over actions, tables, views and columns. Query history per project,
deduplicated, double-press to re-run. A Problems drawer with live diagnostics for every open
tab. Every command rebindable in the keymap editor, with conflict detection. DataFusion's own
configuration keys editable in settings, with restart badges on the ones that need it. Two
built-in themes, Midnight and Daylight, and user themes as plain JSON files of named colour
roles. One window per project, geometry remembered. Updates are checked and installed by the
app itself (**App ▸ Check for Updates…**).

## Installing

macOS, from Homebrew:

```bash
brew tap alexparlett/strata
brew trust alexparlett/strata
brew install --cask strata
```

(The first two steps are Homebrew's own trust flow for third-party taps.) Or take the DMG from
the [latest release](https://github.com/alexparlett/strata/releases) and drag Strata to
Applications; the cask installs the same file. Either way the app updates itself, so the cask
is marked `auto_updates` and a plain `brew upgrade` leaves it alone.

## Agent access

Strata can serve its catalog and query engine to an AI agent over the
[Model Context Protocol](https://modelcontextprotocol.io). An agent lists your tables, inspects
schemas and runs read-only SQL; every write-shaped statement is refused with the same wording
the editor would use. Agent queries are real runs on your engine, in query sessions of the
agent's own. Nothing it does opens or closes a tab of yours, and none of it reaches your query
history. The one thing an agent can write is a file: it can export a result it has already run
to a fresh path outside the project, and nothing else.

Turn it on in **Settings ▸ Agent access**. It is off by default, binds loopback only, and
requires a bearer token on every request. A client needs three facts:

| | |
|---|---|
| **URL** | `http://127.0.0.1:<port>/mcp` — `47821` by default |
| **Header** | `Authorization: Bearer <token>` |
| **Transport** | Streamable HTTP |

Copy-paste configuration for Claude Code, Claude Desktop, VS Code, Cursor, Gemini CLI and
Codex CLI is in **[docs/MCP_CLIENTS.md](docs/MCP_CLIENTS.md)**.

The same tools work with the app closed: `strata mcp <project folder>` serves one project over
stdio, no port or token needed, for clients that spawn their servers themselves. And to be
clear about what you are exposing either way: within those bounds an agent can read any data
the project can reach, so turn it off when you are not using it and treat the token as a
credential.

## Building it yourself

You need a Rust toolchain from [rustup](https://rustup.rs). Strata's UI is
[Freya](https://freyaui.dev/), which renders with Skia, so there is no webview and no system
GUI dependency to install; the first build compiles Skia and DataFusion from source, so give
it time.

```bash
git clone https://github.com/alexparlett/strata
cd strata
cargo run
```

Use `cargo run --release` for real work. To try it on real data, open the repo's `sample/`
folder as a project: it registers parquet, CSV and JSON tables, a Hive-partitioned directory,
views and saved queries.

For the federated half, start the sample database — a seeded PostgreSQL in a container, with
no password to enter:

```bash
docker build -t strata-sample-pg sample/postgres && docker run -d --name strata-sample-pg -p 127.0.0.1:55432:5432 -e POSTGRES_USER=strata -e POSTGRES_DB=strata_sample -e POSTGRES_HOST_AUTH_METHOD=trust strata-sample-pg
```

The sample project already carries the data source, so `SELECT * FROM orders` joins the parquet
users straight onto live PostgreSQL. [sample/postgres/README.md](sample/postgres/README.md)
tours what's in it and what to try, including the deliberate name clashes that show which
source a bare name means.

Tests, linting and the rest of the development workflow are in
[CONTRIBUTING.md](CONTRIBUTING.md). Cutting a distributable `.app` and DMG is
[docs/RELEASING.md](docs/RELEASING.md). The architecture, end to end, is
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Use of AI

Strata is developed substantially with AI agents, and the commit log makes no secret of it.
What keeps that honest is a written engineering bar: [AGENTS.md](AGENTS.md) carries the
architecture, principles and checks this codebase is held to, written for humans and agents
alike, and most of it was settled the hard way, after a wrong version was built and rejected
in review. Agent-written code is reviewed like any other code, and CI runs clippy at
`-D warnings` plus a test suite that deliberately fails rather than skips when its
dependencies are missing.

For contributions AI assistance is welcome, careless vibe coding is not. Whoever opens a PR must
understand the change and its consequences for the codebase, whether they typed it or an agent
did. Every PR opens with a short human-written paragraph saying why the work was done and who
it helps; PRs that read entirely machine-generated and don't engage with the codebase's
conventions will be closed.

As for AI in the app itself: everything is opt-in, off by default, and talks only to endpoints
you configure with your own keys. There is no Strata account and no server of ours in the
loop.

## Thanks

Strata stands on excellent work by other people:

- **[Apache DataFusion](https://datafusion.apache.org/)** and
  **[Arrow](https://arrow.apache.org/)** are the engine and the memory model. Most of what
  looks like magic here is DataFusion being genuinely good.
- **[Freya](https://freyaui.dev/)** by **Marc Espín** is the UI toolkit, and his editor
  [valin](https://github.com/marc2332/valin) shaped how the app is structured.
- **[datafusion-table-providers](https://github.com/datafusion-contrib/datafusion-table-providers)**
  and **[datafusion-federation](https://github.com/datafusion-contrib/datafusion-federation)**
  make the PostgreSQL federation possible.
- **[tree-sitter](https://tree-sitter.github.io/)** powers the editor's highlighting, and
  **[object_store](https://crates.io/crates/object_store)** the bucket data sources.

## License

MIT.
