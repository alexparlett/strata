# Connections 02 · Activity-rail button + sidebar pane

**Workstream:** Connections (W7) · **Status:** ✅ · **DEV_TASKS:** U2 / U3 · **Depends on:** 01, P3-01

## Goal
The rail entry point + the sidebar pane to manage connections.

## What landed

- **The rail button was already there** (P3-01 built the whole top group, Connections included).
  What this task added is everything the toggle lands on.
- **The pane** — `views/sidebar/connections/`, its own `connections` component theme. One row per
  `ConnRow`, **shaped like a catalog entry row**: an outlined provider badge (`S3` / `GCS` /
  `HTTP` — the product's word, never the URL scheme's), the bucket in mono, one trailing status
  slot, ⋮.
- **The header** — the shell's `CONNECTIONS` label gains the canvas's ⓘ (what a connection *is*),
  and `+` joins the toolbar as a `ToolbarItem::Custom` folding on the catalog ↻'s terms.
- **Forget** — the row menu sets `DropTarget::Connection(url)` and stops. The **shared** remove
  confirm performs it: `ProjectState::remove_connection` (rollback included), `persisted_defs`,
  then `Engine::disconnect`.
- **`Engine::disconnect` / `store::disconnect`** in strata-core — `deregister_object_store` by the
  `ConnectionDef::url()` the store went in under. Synchronous, like a table's `deregister`.
- **A refused connection is a Problems ▸ Project row** (the open call below, decided *for*).

## Decisions this task settled

- **Problems ▸ Project covers connections, and they lead the list.** `registration_faults` is now
  connections → tables → views, which is registration order, so anything broken *by* a connection
  reads below its cause. This is the other half of the probe in `engine::store::connect`: without
  the row, a bucket with no credentials fills the drawer with signing failures on its *tables* and
  says nothing about the one thing that is wrong. `RegistrationFault::kind` is now a
  **`FaultKind`** (`Connection` / `Table` / `View`) rather than a `CatalogKind`: a connection
  registers beside the catalog and fails in the same shape, but it is an object store and has no
  place in the enum `dependent_views` and `name_in_use` dispatch on. The drawer carries it
  through as `ProblemTag::{NotSaved, Refused(FaultKind)}`, so both families stay a type all the
  way to the badge rather than becoming a rendered word early.
- **Forget is the shared confirm's fourth `DropTarget`, not a dialog of its own.** `kind()` is now
  `Option<CatalogKind>`; a connection returns `None`, so no dependency list is asked for. Nothing
  can read an object store *by name*, so there is no consequence line — see the note in 04.
- **The status is one glyph with its words on hover, not a line of text** — the catalog entry
  row's slot and its `tip()` helper. Built first as the canvas's two-line row (green/amber dot
  over a status line) and cut after seeing it run: at sidebar width the engine's reason
  ellipsized to about four words, which is strictly less than the triangle says and costs the
  bucket half the row. A row that registered is now **clean**, which also drops the canvas's
  green dot — the absence is the message, exactly as it is on a table. `Loading` shows nothing
  until the wait outlasts `PROGRESS_HOLD` and then spins, holding the last settled verdict
  across the gap so ↻ does not blink a triangle off and back.
- **Row identity is `url()` everywhere** — the row key, the menu's Forget target, and
  `remove_connection`'s match, which is deliberately case-**sensitive** (a URL, not a folded SQL
  identifier).

## The one fork change

`SideBarItem` set `a11y_focusable(true)` + `AccessibilityRole::Link` unconditionally, so the
connection row — the preset's first **non-pressable** user — became a tab stop with a focus ring
that no key could activate, announced as a link. Both now follow `on_press.is_some()`
(`crates/freya/crates/freya-components/src/sidebar.rs`). Hover is untouched: a row you can
right-click is worth marking under the pointer, and the canvas paints one.

**The gitlink needs pushing to `github.com:alexparlett/freya`** before this branch can be cloned
fresh or run in CI (AGENTS.md §6).

## Handed on

- **Add and Edit are inert** (AGENTS.md §5) — the editor forms are 03's. All three affordances
  (header `+`, the empty state's CTA, the menu's *Edit connection*) render disabled; only the
  handler changes when 03 lands. See 03's file for the wiring notes.
- **The empty state and the rows both carry `PANE_BODY_MIN_W`**, and both are pinned by drag tests
  (`connections/interaction.rs`): a squeezed panel gives up the flexing name column and clips,
  it never spills the ⋮ and never wraps the empty copy to one letter per line.

## Acceptance
- [x] The rail button toggles the Connections pane; connections list with add/edit/remove.
- [x] A failed connection reads amber with the engine's reason; a re-scan (↻) clears it once the
      connection resolves.

## Freya / references
- Design: `Strata.dc.html` (`data-pane="connections"` + the `showConnections` header),
  `ActivityRail.dc.html`. DEV_TASKS U2/U3/W7. `docs/CONNECTIONS_SPEC.md` §1/§3.
