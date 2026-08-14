# AM-05 · Memory management panel

**Workstream:** Assistant memory · **Status:** ⬜ · **Depends on:** AM-03

## Goal

The prune surface auto-extraction owes the user: a modal working panel off the chat pane's
header listing every memory — kind, text, tables, age — with per-row edit and delete, a
recipe's SQL expandable read-only, and Clear behind the window-root confirm. Every change
goes through the same `Memories::apply` funnel extraction uses, so an edited text's vector
goes stale by the same mechanism (AM-01's null-on-text-change rule).

## Current state (verified 2026-08-13)

- Memory is **per-project**, so the surface lives in the project window — not Settings
  (app-global). The chat pane's header (`views/chat/header.rs`) already holds the
  conversation gestures (switcher, New, export, delete, pane ×) and is where the `Memory`
  press belongs.
- The modal rule (AGENTS §3 + the modal-base memory): working panels get their own card on
  the `components::modal` base — open/closed contract, Esc-as-close-request owned by the
  base — never the 420px confirm. Enter-confirm belongs to the surface's card.
- Clear-style destructive confirms mount **at the window root** (the chats-clear precedent,
  `state/chat.rs` / AGENTS §2's chat bullet: "Clear and the per-row delete ask through one
  confirm at the window root").
- Settings-style rows come from `components::form` (`Form` > `Row` > control); fields
  publish per keystroke and normalize on leave; metrics off `components::metrics`
  (`SP_*`/`R_*`); user-facing text in the terse IDE register (single-quoted identifiers, no
  em-dashes).
- The window's `Arc<Memories>` slot lands with AM-03 (`app.rs`, beside `AssistantCtx`); the
  panel reads through it (`Memories` methods are direct-call async — awaited from a Freya
  task, no `offload` needed). A press that closes its own surface must not own the spawned
  work (AGENTS §3's scope-bound-task rule): deletes/edits record intent in a `State`, a
  `use_side_effect` in the pane's scope performs them.
- The chat theme is `views/chat/mod.rs`'s `define_theme!` (:55); the panel dresses from it
  (a surface with its own component theme reads that theme, not the roles).

## Build

1. **`views/chat/memory.rs`**: `MemoryPanel` on the `components::modal` base. Header: title
   + count + `Clear`. Body: a scrollable list — kind chip (`fact` / `recipe`), the text
   (editable `Input` under `InputTypography`, publishing to a draft, committed on blur/Enter
   through an `Update` op), tables as chips, relative `updated_ms`; a recipe row expands to
   its SQL read-only; per-row delete. Empty state: one line ("No memories yet. They are
   learned from conversations in this project.").
2. **The press**: a `Memory` item in `header.rs` beside export — opens the panel (state on
   the pane, open/closed only, per the modal base's contract).
3. **The funnel**: every gesture is `MemoryOp`s through `Memories::apply` — edit = `Update`,
   delete = `Delete`; the list refreshes from the store's answer (no shadow copy that can
   drift). Clear asks through the window-root confirm ("Delete all memories for this
   project? They are relearned from future conversations.") and is `Memories`' clear (AM-06
   owns its store-side wording if it lands first, a plain delete-all `apply` otherwise).
4. **Liveness**: a distill settling while the panel is open refreshes the list (the `State`
   bump AM-03's hook provides, `is_alive`-guarded).
5. **Tests**: what is testable headless is the funnel — ops from panel gestures hit `apply`
   with the right targets; the rest is a build + manual pass.

## Acceptance

- Every memory the store holds is listed and editable; an edit persists across reopen and
  nulls the row's vector (verified through the store, AM-01's rule).
- Clear asks at the window root and empties the store; Esc closes the panel and never asks.
- The panel matches the chat pane's dress; nothing hand-rolled where a standard component
  exists.
- Full check green.

## Files

`crates/strata-freya/src/apps/project/views/chat/memory.rs` (new) ·
`crates/strata-freya/src/apps/project/views/chat/header.rs` (the press) ·
`crates/strata-freya/src/apps/project/views/chat/mod.rs` (theme fields if any, module) ·
tests beside the state funnel.

## Freya / references

- `components::modal` (the base's open/closed + Esc contract); the Shape panel
  (`results/shape/`) is the working-panel precedent on that base.
- `components::form` rows; `Input` + `InputTypography`; chip dress from the composer's
  (`views/chat/composer.rs`).
- The window-root confirm: the chats Clear path (`state/chat.rs` + its confirm mount).

## What is NOT this task

Any store logic (AM-01's funnel is reused, never reimplemented). Re-embed/Clear store-side
semantics (AM-06). A memory *browser* beyond this list — search/filter inside the panel is
future polish, not v1.
