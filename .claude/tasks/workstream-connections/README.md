# Workstream — Connections + remote object stores (W7)

A cross-cutting feature the phases don't own: **project-scoped connections** (S3 / GCS / HTTP) with
**no app-managed secrets**, plus the config-table **LOCATION** toggle to register tables over them.
Touches the activity rail (U2), the sidebar (U3 pane), and the config modal (U14). Spec:
`docs/CONNECTIONS_SPEC.md`.

## State of play
**The model and the connection half of the engine are built (01); no surface is, and remote
*tables* are not wired.** A project can persist connections in its `project.json`, and the
registration pass connects each bucket's object store before any table registers. What is still
missing on the data path is task 04's: `register::table_spec` resolves every source through
`project::resolve_source`, which joins a non-absolute path onto the project folder — so a
bucket-relative source would become a local path before it reached the engine. Composing a
source against `ConnectionDef::url()` is 04's, together with the LOCATION control that produces
one. 02–04 are the surfaces, and they span phases 2–4, which is why this lives here rather than
in one phase. Secrets are **by reference** (paths / env), never read into or stored by Strata
(per the canvas).

## Tasks

| # | Task | Status | DEV_TASKS | Depends on |
|---|---|---|---|---|
| 01 | Connections model + spec (project-scoped, no stored secrets) | ✅ | W7 | — |
| 02 | Activity-rail button + sidebar connections pane | ⬜ | W7 (U2/U3) | 01, P3-01 |
| 03 | Connection editor forms (S3 / GCS / HTTP) | ⬜ | W7 | 01 |
| 04 | Config LOCATION toggle + object-store branch | ⬜ | W7 (U14) | 01, P4-11 |

**01 raised the workspace's effective MSRV to rustc 1.94.1** (`aws-config` and the `aws-smithy-*`
tree). CI installs `stable`, so it is only a local-toolchain concern.

## Legend
✅ done · 🟢 UI only · 🟡 partial · ⬜ todo · `[core ✓]` logic in `strata-core`.
