# docs/

Documentation for Strata: what the system does and how it is built. Everything here describes the
code **as it is** — no plans, task tracking or acceptance criteria. When a change alters behaviour
a document describes, the document changes in the same commit.

Start with **[ARCHITECTURE.md](ARCHITECTURE.md)** — the system end to end, with pointers into
everything below.

## Feature documentation

| Document | What it covers |
|---|---|
| [SNAPSHOT_SPEC.md](SNAPSHOT_SPEC.md) | The result read model: a Run materializes an immutable snapshot; the `SnapshotStore` contract and the two stores that keep it, identity, lifecycle, pins, the row-order ordinal, and the freya-query layer over it. |
| [STATEMENTS_SPEC.md](STATEMENTS_SPEC.md) | The SQL statement surface: the typed pipeline in front of dispatch (parse → qualify → classify), the grants and policy model, the target/mechanism axis every arm shares, internal tables (the `InternalTableStore` seam) and the writes over them, typed view DDL, typed `COPY`, and the editor and agent policies. |
| [COMPLETION_SPEC.md](COMPLETION_SPEC.md) | SQL completion: the synchronous engine-authoritative provider, the position model, ranking, and the editor integration. |
| [EXPLAIN_PLAN_SPEC.md](EXPLAIN_PLAN_SPEC.md) | The EXPLAIN plan view: the typed `QueryPlan` the engine hands over, self-time attribution, the three metric tiers, and the rendered tree. |
| [CHART_SPEC.md](CHART_SPEC.md) | The chart view: six marks over the result snapshot, encoders, sort, refusals — and why the chart computes nothing SQL can say. |
| [CHART_FUNCTIONS.md](CHART_FUNCTIONS.md) | Shaping a result for the chart in SQL: what the engine's aggregate, window and scalar families buy you. |
| [CONNECTIONS_SPEC.md](CONNECTIONS_SPEC.md) | Remote data: the one `DataSource` trait every kind registers through, the five shipped kinds (S3 / GCS / HTTP / PostgreSQL / MySQL), the credential model (host chains for buckets, an OS-keystore secret whose slot the def records), address rules, the settings a kind declares and the editor renders, tables over buckets, the catalog every source registers, the federated database catalog, and what a pre-EA project file does now. |
| [IMPORT_OPTIONS.md](IMPORT_OPTIONS.md) | Table Config's per-format read options (CSV, JSON, parquet/Arrow) and Hive partition detection. |
| [EXPORT_OPTIONS.md](EXPORT_OPTIONS.md) | The export window: formats, per-format options, partitioning, the preview, and the `COPY … TO` it produces. |
| [MCP_CLIENTS.md](MCP_CLIENTS.md) | Connecting MCP clients to the agent access server: per-client configuration (Claude Code, Claude Desktop, VS Code, Cursor, Gemini CLI, Codex CLI) and the headless stdio server. |
| [FREYA_THEME_SPEC.md](FREYA_THEME_SPEC.md) | The theme format: the role vocabulary, syntax scopes, fonts and typography — for anyone writing a theme. |

## Embedding the engine

The engine trio (`strata-model` · `strata-arrow` · `strata-engine`) is documented for a
crates.io reader in its own rustdoc rather than here, so the prose sits beside the API it
describes and its examples are compiled by CI. `cargo doc -p strata-engine --open` and start at
**`strata_engine::guide`**:

| Guide | What it covers |
|---|---|
| `guide::embedding` | Getting started (builder → sync → run → read), the capability and policy model, the embedder contract, and the session ledger. |
| `guide::json` | The polymorphic JSON reader, the union's journey through snapshot and presentation, the `struct_*` family — and using the reader in a plain DataFusion session. |
| `guide::data_source` | Writing a `DataSource`, and running the two conformance rings against your own backend. |
| `guide::storage` | Writing a `SnapshotStore` or an `InternalTableStore`. |

Runnable versions live in `crates/strata-engine/examples/` (`minimal_embedder`, `no_disk`,
`custom_source`) and `crates/strata-agent/examples/headless_mcp.rs`.

## Architecture and operations

| Document | What it covers |
|---|---|
| [ARCHITECTURE.md](ARCHITECTURE.md) | The guided tour: workspace, the engine's builder and group handles, the query round trip, the statement pipeline, the data-source layer, state, windows. |
| [FREYA_STATE_ARCHITECTURE.md](FREYA_STATE_ARCHITECTURE.md) | Per-window state in full: stores and channels, the stateful tab, persistence, the query layer, satellites, the menu seam. |
| [RELEASING.md](RELEASING.md) | How a build reaches a tester: the bundle script, the Release workflow, versioning, signing and notarization. |

> Note on file names: several feature documents keep a historical `_SPEC` suffix because engine
> code comments cite them by path and section. Their content is documentation, not specification.
