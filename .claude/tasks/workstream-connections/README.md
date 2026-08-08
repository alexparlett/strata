# Workstream — Connections + remote object stores (W7)

A cross-cutting feature the phases don't own: **project-scoped connections** (S3 / GCS / HTTP) with
**no app-managed secrets**, plus the config-table **LOCATION** toggle to register tables over them.
Touches the activity rail (U2), the sidebar (U3 pane), and the config modal (U14). Spec:
`docs/CONNECTIONS_SPEC.md`.

## State of play
**Done (01–04): connections are managed, and tables read through them.** The pane lists
connections with their registration outcome, its `+` / CTA / Edit open the editor window
(`apps/connection/`), and Forget removes one (store + persist + `Engine::disconnect`) — now naming
the tables and views it leaves invalid. A project persists its connections in `project.json`, the
registration pass connects each bucket's object store before any table registers, and a table def
can **name one**: `TableDef::connection` carries the connection's `url()` and its sources are then
bucket-relative, composed by `project::resolve_source` (which takes the connection, so the local
"join onto the project folder" rule cannot be reached for by mistake). The Configure window's
LOCATION toggle is what produces one. 02–04 are the surfaces, and they span phases 2–4, which is
why this lives here rather than in one phase. Secrets are **by reference** (paths / env), never
read into or stored by Strata (per the canvas).

## Tasks

| # | Task | Status | DEV_TASKS | Depends on |
|---|---|---|---|---|
| 01 | Connections model + spec (project-scoped, no stored secrets) | ✅ | W7 | — |
| 02 | Activity-rail button + sidebar connections pane | ✅ | W7 (U2/U3) | 01, P3-01 |
| 03 | Connection editor forms (S3 / GCS / HTTP) | ✅ | W7 | 01 |
| 04 | Config LOCATION toggle + object-store branch | ✅ | W7 (U14) | 01, P4-11 |

**01 raised the workspace's effective MSRV to rustc 1.94.1** (`aws-config` and the `aws-smithy-*`
tree). CI installs `stable`, so it is only a local-toolchain concern.

## Legend
✅ done · 🟢 UI only · 🟡 partial · ⬜ todo · `[core ✓]` logic in `strata-core`.
