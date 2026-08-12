# P5-10 · Role-read re-homing (component-themed surfaces off the direct reads)

**Phase:** 5 · **Status:** ⬜ · **DEV_TASKS:** — · **Depends on:** theme v2 (landed) ·
**Run after P5-09** (coupled — P5-09 grows `window`'s field set and re-homes the
connection/configure text reads itself; this task sweeps what remains).

## Goal
Enforce FREYA_UI's "a surface with its own component theme reads colours from that theme, not
also from the roles": every non-semantic direct `use_roles().get(Role::…)` read inside a surface
that owns a `define_theme!` group becomes a field on that theme (a new mapping-table row), or
reuses a field the theme already has.

## Current state (verified 2026-08-12)
~28 violating reads across **15** owning groups:

| Owning group | Direct reads |
|---|---|
| `settings` (`apps/settings/mod.rs:71`) | `settings/mod.rs:687`; `views/keymap/table.rs:115,181,260,350,373`; `views/engine/table.rs:115,192`; `views/engine/mod.rs:75`; `views/ai/configure.rs:72` — **10** |
| `launcher` (`apps/launcher/mod.rs:34`) | `launcher/mod.rs:135`; `views/row.rs:45`; `views/projects.rs:36` — **3** |
| `export` (`apps/export/mod.rs:50`) | `export/mod.rs:304`; `views/partition.rs:179`; `views/formats.rs:74` — **3** |
| `chat` (`views/chat/mod.rs:72`) — **new scope**, landed with AS-04 | `chat/mod.rs:123`; `card.rs:53`; `composer.rs:76` — **3** |
| `command_palette` (`palette/mod.rs:68`) | `palette/row.rs:76` |
| `header_bar` (`views/header/mod.rs:46`) | `header/project_menu.rs:85` |
| `tab_bar` (`tab_bar/bar.rs:27`) | `bar.rs:101`; `controls.rs:86` |
| `tab` (`tab_bar/tab.rs:26`) | `tab.rs:373` |
| `status_bar` (`results/status_bar.rs:29`) | `status_bar.rs:173` |
| `cell_view` (`results/cell_view.rs:33`) | `cell_view.rs:196` |
| `record_view` (`results/record_view.rs:43`) | `record_view.rs:168` |
| `explain_plan` (`results/explain_plan/mod.rs:35`) | `mod.rs:96` |
| `running` (`results/running.rs:21`) | `running.rs:61` |
| `toggle_button` (`components/toggle_button.rs:27`) | `toggle_button.rs:121` |
| `segmented_toggle` (`components/segmented_toggle.rs:28`) | `segmented_toggle.rs:238` |

**Corrections to the task as originally filed:**
- The Agents pane is **dead scope** — removed in `6927f15`; what survives (`state/agents.rs`,
  `agent/`) reads no roles.
- The confirm dialogs (`views/dialogs/*.rs` — 6 files reading roles) are **not** violations as
  filed: there is no `dialog` theme group to re-home onto, and `components/dialog.rs` itself
  reads roles at 7+ sites (:191,207,230-236,253,294). Re-homing them means **creating a `dialog`
  component theme group first**, covering the shell's own reads and the per-dialog ones — do it
  (recommended: the dialogs are exactly the kind of surface the rule is for), but as an explicit
  step, sized as such.
- `views/sidebar/mod.rs:87-92` reads five roles legitimately — the sidebar's groups live one
  level down (`sidebar/catalog/mod.rs:53`, `sidebar/connections/mod.rs:81`).
- The connection/configure windows' text reads are **P5-09's** to move (it adds the `window`
  text fields); do not re-home them here first.

Semantic reads through `tones()` are correct and stay. `components/divider.rs` is the one
sanctioned dual-read (hooks run unconditionally; the role picks after) — leave it.

## Build
- Per surface: add the missing field(s) to its `define_theme!`, add the mapping-table row
  (`theme/components.rs`) targeting the same role the direct read used, and swap the call site
  to the theme field. Name fields for the role they play, not the first consumer (FREYA_UI).
- Where the surface's theme already has a field resolving to the same role, reuse it instead of
  adding one.
- Create the `dialog` group (shell + confirm dialogs) as its own step.

## Acceptance
- [ ] `grep -rn "use_roles()" crates/strata-freya/src` hits only: un-themed surfaces,
      `components/tones.rs`, `components/divider.rs`, `views/sidebar/mod.rs`, and `theme/`.
- [ ] `cargo test -p strata-freya` green; `schema_in_sync` untouched (roles didn't change).

## Freya / references
- `docs/reference/FREYA_UI.md` ("reads colours from that theme"), `theme/components.rs`,
  the table above.
