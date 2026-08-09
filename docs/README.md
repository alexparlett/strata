# docs/

Documentation for Strata: what the system does and how it is built. Everything here describes the
code **as it is** — plans, task tracking and acceptance criteria live in `.claude/tasks/`, not
here. When a change alters behaviour a document describes, the document changes in the same
commit.

Start with **[ARCHITECTURE.md](ARCHITECTURE.md)** — the system end to end, with pointers into
everything below.

## Feature documentation

| Document | What it covers |
|---|---|
| [SNAPSHOT_SPEC.md](SNAPSHOT_SPEC.md) | The result read model: a Run materializes an immutable Arrow IPC snapshot; identity, lifecycle, pins, the row-order ordinal, and the freya-query layer over it. |
| [STATEMENTS_SPEC.md](STATEMENTS_SPEC.md) | The SQL statement surface: the classify → run/intercept/refuse router, internal tables (`CREATE TABLE` / CTAS), the editor and agent policies, and what is still being lifted. |
| [COMPLETION_SPEC.md](COMPLETION_SPEC.md) | SQL completion: the synchronous engine-authoritative provider, the position model, ranking, and the editor integration. |
| [EXPLAIN_PLAN_SPEC.md](EXPLAIN_PLAN_SPEC.md) | The EXPLAIN plan view: the typed `QueryPlan` the engine hands over, self-time attribution, the three metric tiers, and the rendered tree. |
| [CHART_SPEC.md](CHART_SPEC.md) | The chart view: six marks over the result snapshot, encoders, sort, refusals — and why the chart computes nothing SQL can say. |
| [CHART_FUNCTIONS.md](CHART_FUNCTIONS.md) | Shaping a result for the chart in SQL: what the engine's aggregate, window and scalar families buy you. |
| [CONNECTIONS_SPEC.md](CONNECTIONS_SPEC.md) | Remote data: S3 / GCS / HTTP connections, the no-secrets credential model, address rules, and tables over buckets. |
| [IMPORT_OPTIONS.md](IMPORT_OPTIONS.md) | Table Config's per-format read options (CSV, JSON, parquet/Arrow) and Hive partition detection. |
| [EXPORT_OPTIONS.md](EXPORT_OPTIONS.md) | The export window: formats, per-format options, partitioning, the preview, and the `COPY … TO` it produces. |
| [AGENT_ACCESS_SPEC.md](AGENT_ACCESS_SPEC.md) | Agent access: the MCP tool vocabulary, the in-app server and headless host, identity, the policy gate, and the Agents pane. |
| [FREYA_THEME_SPEC.md](FREYA_THEME_SPEC.md) | The theme format: the role vocabulary, syntax scopes, fonts and typography — for anyone writing a theme. |

## Architecture and operations

| Document | What it covers |
|---|---|
| [ARCHITECTURE.md](ARCHITECTURE.md) | The guided tour: workspace, engine, query round trip, statement routing, state, windows. |
| [FREYA_STATE_ARCHITECTURE.md](FREYA_STATE_ARCHITECTURE.md) | Per-window state in full: stores and channels, the stateful tab, persistence, the query layer, satellites, the menu seam. |
| [RELEASING.md](RELEASING.md) | How a build reaches a tester: the bundle script, the Release workflow, versioning, signing and notarization. |

## Engineering reference — [reference/](reference/)

The detail behind [CLAUDE.md](../CLAUDE.md) (the map) and [AGENTS.md](../AGENTS.md) (the rules),
for anyone changing the code: the annotated module tree, the architecture invariants and the
failures they exist to prevent, the Freya UI conventions, the engine model, the git/fork/CI
workflow, and what each finished task settled. Start at [reference/README.md](reference/README.md).

> Note on file names: several feature documents keep a historical `_SPEC` suffix because engine
> code comments cite them by path and section. Their content is documentation, not specification.
