# Workstream — Connections + remote object stores (W7)

A cross-cutting feature the phases don't own: **project-scoped connections** (S3 / GCS / HTTP) with
**no app-managed secrets**, plus the config-table **LOCATION** toggle to register tables over them.
Touches the activity rail (U2), the sidebar (U3 pane), and the config modal (U14). Spec:
`docs/CONNECTIONS_SPEC.md`.

## State of play
Not built in Freya. It spans surfaces from phases 2–4, so it lives here rather than in one phase.
Secrets are **by reference** (paths / env), never read into or stored by Strata (per the canvas).

## Tasks

| # | Task | Status | DEV_TASKS | Depends on |
|---|---|---|---|---|
| 01 | Connections model + spec (project-scoped, no stored secrets) | ⬜ | W7 | — |
| 02 | Activity-rail button + sidebar connections pane | ⬜ | W7 (U2/U3) | 01, P3-01 |
| 03 | Connection editor forms (S3 / GCS / HTTP) | ⬜ | W7 | 01 |
| 04 | Config LOCATION toggle + object-store branch | ⬜ | W7 (U14) | 01, P4-11 |

## Legend
✅ done · 🟢 UI only · 🟡 partial · ⬜ todo · `[core ✓]` logic in `strata-core`.
