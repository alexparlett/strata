# AA-04 · Settings ▸ Agent access

**Workstream:** Agent access · **Status:** ✅ · **DEV_TASKS:** — · **Depends on:** AA-03

## Goal
The control surface for a capability AA-03 ships dark: enable/disable, the port, and the token
(view / copy / regenerate).

## What was built

A sixth Settings category, **Agent access** — ungrouped, between Keymap and the Engine group,
exactly where the canvas lists it (`Route::AgentAccess`, a `CATEGORIES` entry, breadcrumbs off
`model.rs` as with every other page).

The pane (`apps/settings/views/agent_access.rs`) is an ordinary `Form::preferences` of three
rows, all built through `Anchor::row()` so search reaches them:

- **Enable agent access** — `Switch`, trailing, with the label block as a second press target.
- **Port** — `NumberField` bounded to `AGENT_PORT_MIN..=AGENT_PORT_MAX` (`strata-core::config`,
  named there beside the setting for the reason the column-width and history bounds are: the
  field offers exactly the range its consumer can honour, and below 1024 needs root).
- **Token** — a masked, read-only `ValueField` with reveal · copy · **Regenerate** beside it.

Settings fields needed no work: AA-03 already shipped `Settings::agent_access` (an `AgentAccess`
struct: `enabled` / `port` / `token`), already through `settings_merge!`.

## Descoped

- **Client setup** — a `Note` row carrying a `claude mcp add …` line. Cut: it is *one client's*
  incantation on a surface that has no business favouring a client. Every client's setup belongs
  in the README's Agent access section (spec §6 already says so).
- **Status** — running / not running / why. Cut: the header's status dot (AA-03,
  `agent/status.rs`) already reports listening and paired-client count, and it does it where the
  user is working rather than behind a Settings category.

Both were in this file's original sketch and neither is in the designer's canvas. The canvas won.

## Decisions worth not re-litigating

- **Regenerate is a draft edit, not an immediate write** — a divergence from the canvas's
  subtext ("takes effect at once"). `Settings::merge_onto` diffs whole fields and `agent_access`
  is one field, so a token committed behind the draft's back would be overwritten by the next
  Apply that carried a changed switch. And a credential every client depends on should have an
  undo; Cancel is it, which is also why the action needs no confirm of its own.
- **The reveal sits beside the box, not inside it** — a divergence from the canvas, which draws
  a 24×24 eye inside the field. An icon button in this app is a 28×28 `ToolButton` and a value
  box stands at 30, so the in-box variant would be a hand-rolled lookalike of the one control
  the app already has (AGENTS.md §3). Reveal and copy read as one cluster on the value.
- **Masking is `Input`'s own `InputMode`**, exposed as `ValueField::masked` — the state keeps
  the real token, so revealing is a prop flip rather than a second source of truth, and Freya's
  editable refuses to copy a masked box's contents to the clipboard for free.

## Acceptance

- Toggling enable, editing the port and regenerating all write the draft; Apply commits, and
  `agent::use_agent_server` (mounted by every workspace window) starts / stops / restarts the
  server off `ConfigChan::Settings` with no app restart.
- Out-of-range ports can't be applied — the field's own bounds, not a post-hoc correction.
- Settings search finds all three rows (by name, by the page, and by "mcp" / "agent" / "claude"
  / "bearer" keywords) and reveals them — the P4-09 machinery, driven by the anchors.
- `settings_merge!` already covers `agent_access`; another window's concurrent commit survives
  an Apply here (the standard seed-diff behaviour).
- Tests: `model`'s route/category pins cover the new page; `search`'s table gains one.
