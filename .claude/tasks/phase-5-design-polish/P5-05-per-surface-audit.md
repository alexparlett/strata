# P5-05 · Per-surface design audit (Freya drift pass)

**Phase:** 5 · **Status:** ⬜ · **DEV_TASKS:** Part 1 · **Depends on:** P5-01 (a scale to snap to);
run **last** in the phase — the audit judges the app after it matches the tokens/themes it is
judged against.

## Goal
A final surface-by-surface audit of the Freya app against the `.dc.html` canvases — the DEV_TASKS
Part-1 "align vs build" pass, redone for the Freya build.

## The surface walk-list (current tree, 2026-08-12)
Launcher · header/rail(s) · sidebar (catalog **and connections** panes) · editor/tabs · results
grid/toolbar/status/chart/explain · inspector · **chat pane** · drawer (events/history/problems) ·
dialogs · command palette · settings (**incl. Settings ▸ AI**) · export · configure window ·
**connection editor window**.

Canvas sources: the chat pane and inspector are **sections of `Strata.dc.html`** (there is no
standalone canvas for either); connection editor = `Connections.dc.html`, configure window =
`Configure.dc.html`. Note `chat/header.rs:29` (40px header) and `chat/mod.rs:346` (340px pane)
cite "the canvas" — verify those numbers against the chat section of the refreshed
`Strata.dc.html`, since they were written before this handoff drop.

**Not drift — do not "fix":** the Agents pane and the header's agent-access dot (removed
deliberately, 2026-08-12 — "no surface lists agents") and the Settings ▸ AI eight-provider roster
(settled as one OpenAI-compatible row, `56da328`), both of which the handoff changelog still
describes.

## Known drift going in (verified inventory — confirm value against canvas, then fix or file)

**Header heights — five values coexist, two doc comments are false:**

| Row | Height | Where | Doc claim |
|---|---|---|---|
| Sidebar header | 48 | `sidebar/mod.rs:71` (`HEADER_HEIGHT`) | self-consistent ("the canvas's 48/24") |
| Project title header | 48 | `views/header/mod.rs:127,236` | — |
| Datagrid header | 46 | `results/datagrid/mod.rs:54` | — |
| Inspector header | 40 | `inspector/mod.rs:94` (`HEADER_HEIGHT`) | `:93` claims it matches sidebar + toolbar — **false both ways** |
| Chat header | 40 | `chat/header.rs:30` (`HEADER_H`) | "canvas: 40px" — silent about the other panes |
| Shared `Toolbar` row | 38 | `components/toolbar.rs:273` | — (explain overrides to 37, `explain_plan/mod.rs:189`) |
| Drawer header | 36 | `drawer/mod.rs:99` | `:98` claims it matches sidebar + inspector — **false both ways** |

Sidebar and inspector each own a private const **named `HEADER_HEIGHT`** with different values.
Plausible root cause: only the sidebar header sits on `Toolbar` (P5-06's shared chrome row);
inspector, drawer and chat build their header rows by hand. Consider moving them onto
`Toolbar::header()` as the fix rather than syncing three literals.

**Icon-button sizes — six-plus sizes, one shared const:** `TOOL_SIZE = 28.`
(`tool_button.rs:24`) is the only `pub` size. 30 (project header, menu row, running state), 26
(the title-bar button, **copied in four windows** + 3 more sites), 24 (panel-header controls,
defined twice), 22 (`ACTIONS_SIZE`, defined twice, + 5 bare sites), 20 / 18 / 16 (small glyphs),
`STATUS_SIZE = 12.` defined twice. The **rehoming** is P5-01's job; this audit settles **which
values the canvases actually want**, then snaps.

## Build
- Walk each surface against its canvas; list concrete drift and fix the cheap aligns (the two
  false doc comments above are free).
- Fold anything structural back into the owning task (tokens → P5-01, states → P5-02, theme →
  P5-04).

## Acceptance
- [ ] Each surface checked against its canvas; residual drift listed and the quick wins fixed.
- [ ] The header-height and icon-size questions are settled with canvas-sourced values.

## Freya / references
- The `.dc.html` canvases (`.claude/design-handoff/`, refreshed 2026-08-12), DEV_TASKS Part 1.
