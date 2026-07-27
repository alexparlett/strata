# P4-11 · Config / register-table modal

**Phase:** 4 · **Status:** ⬜ `[core ✓]` · **DEV_TASKS:** U14 / D7 · **Depends on:** —

## Goal
The register/edit-table modal: multi-path sources, format, Hive partitions.

> **This adds a new `project.json` mutation site** — the third def writer after Save and the drop
> confirm. Route its persist through **P4-15**'s funnel (today: P3-13's
> `actions::persisted`, which logs the failure and returns whether the write landed), and gate the
> modal's own success on the answer. Do **not** copy the surrounding
> `if let Err(e) = … { tracing::error!(…) }` idiom — that silence is exactly what P4-15 exists to
> remove, and a registration the project file never heard about reverts on the next open.

## Current state
Not built. Core: `register_external` / `TableSpec`.

## Build (to `Configure.dc.html`, DEV_TASKS U14/D7)
- Multi-path **SOURCE PATHS** with browse + per-path counts; a **REQUIRED badge + resolution tooltip**;
  correct status order (below import-options, above Hive); drop the subtitle.
  > The path-with-a-browse-button row already exists: `components::form::DirectoryField` (P4-06,
  > Settings ▸ System). It owns its buffer and reports per keystroke like `NumberField`, and the
  > picker writes the *box* rather than reaching past it — one buffer, so the two can't disagree.
  > It picks a **folder**; a source path that may be a file or a glob wants that as a mode on the
  > same component, not a second control beside it.
- Format selection; **Hive partition** detection (typed, with the string-cast warning).
- The LOCATION toggle + remote object stores belong to the **Connections workstream** (W7) — leave a hook.

## Acceptance
- [ ] Register a table over one or more paths/globs with format + Hive partitions; REQUIRED badge + tooltip.

## Freya / references
- Design: `Configure.dc.html`. Core `register_external` / `TableSpec`. DEV_TASKS U14/D7. LOCATION → W7.
- **Failure messages come from P3-07**, which maps them inside `register_external` itself. A failed
  Register renders what the engine hands back; do not grow a second set of messages here. The same
  task settled that there is **no pre-flight** file-count or schema-consistency readout (handoff
  `FEATURES.md` §6 over §7) — the Register *is* the check.
