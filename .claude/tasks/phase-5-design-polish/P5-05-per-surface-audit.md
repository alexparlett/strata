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

Sidebar and inspector each owned a private const **named `HEADER_HEIGHT`** with different values.
P5-01 rehomed them without renumbering: `SIDEBAR_HEADER_HEIGHT` 48, `RIGHT_PANE_HEADER_HEIGHT` 40
(the inspector's and the chat pane's, merged — one slot, `Layout::right`, so one row) and
`DRAWER_HEADER_HEIGHT` 36, all in `components::metrics`. **Which values the canvases want is still
this task's call**, and it is now one edit per height. Plausible root cause is unchanged: only the
sidebar header sits on `Toolbar` (P5-06's shared chrome row); inspector, drawer and chat build
their header rows by hand. Consider moving them onto `Toolbar::header()` as the fix rather than
syncing three constants.

**Icon-button sizes — six-plus sizes, now one module:** P5-01 did the rehoming —
`components::metrics` holds `TOOL_SIZE` 28, `COMPACT_BUTTON` 26 (was the title-bar copy in four
windows plus three more sites), `HEADER_CONTROL` 24, `ROW_ACTION` 22 and `STATUS_DOT` 12. What is
left for this audit is **which values the canvases actually want**, and the 30 / 20 / 18 / 16 bare
sites it did not touch (they are glyph sizes, not the button box). Note that
`settings/views/theme.rs`'s 26 was never a title-bar button — it is the preview miniature's rail,
and stays local.

## Build
- Walk each surface against its canvas; list concrete drift and fix the cheap aligns (the two
  false doc comments above are free).
- Fold anything structural back into the owning task (tokens → P5-01, states → P5-02, theme →
  P5-04).

## Acceptance
- [ ] Each surface checked against its canvas; residual drift listed and the quick wins fixed.
- [ ] The header-height and icon-size questions are settled with canvas-sourced values
      (one edit each in `components::metrics` now that P5-01 has rehomed them).

## Freya / references
- The `.dc.html` canvases (`.claude/design-handoff/`, refreshed 2026-08-12), DEV_TASKS Part 1.
