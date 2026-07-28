# P4-06 · Settings ▸ System (+ history limit)

**Phase:** 4 · **Status:** ✅ · **DEV_TASKS:** W3 / U12 · **Depends on:** P4-03

## Goal
The System category, including the query-history limit.

## What landed
`views/system.rs` replaces the `Pane::not_built(..)` placeholder behind `Route::System`: the
canvas's five settings as a `Form::preferences` of `Row`s, in canvas order —

- **Reopen projects on startup** (`reopen_on_startup`) — a trailing `Switch`.
- **Default project directory** (`default_project_dir`) — a path box with the native folder
  picker beside it (`DirectoryField`, below).
- **Opening a project** (`open_pref`) — the three-segment form pill: Ask every time · This
  window · New window.
- **Confirm before closing a tab or window with a running query** (`confirm_close_running`) —
  a trailing `Switch`.
- **Query history limit** (`max_history`) — a `NumberField` reading `runs`.

Like P4-05, **every one of the five already had its reader** — startup routing reads
`reopen_on_startup`, `platform::pick_project_folder` the default directory,
`platform::open::decide` the open preference, the close confirm `confirm_close_running`, and
the history satellite `max_history` — so this task is the control, not the wiring. Nothing
downstream changed.

**Opening a project** is the row that was worth pulling early. The only thing that had ever
written `open_pref` was the This/New prompt's "Remember, don't ask again", which is one-way in
practice: once remembered, nothing in the app put it back to Ask. This control is how that
decision is undone.

### Decisions worth keeping

**`DirectoryField` is form vocabulary, not this pane's** (`components::form::field`). A path
box with a picker beside it is what every surface that names a location wants — the config
modal's source (P4-11) and the import options (P4-12) next — so it landed in the shared module
rather than as a private control here. It follows `NumberField`'s contract for the same
reasons: it owns the buffer (`Input` writes its bound state directly and has no on-change
prop), reports per keystroke (Apply is a `Button`, which moves focus and calls its handler in
the same breath), and never re-reads the caller. Two ways to set one value, so there is **one
buffer and both write it**: the picker sets the box and the box is what gets reported — a
button that reached past the box into the caller's state would leave the two free to disagree.
Unlike a number there is nothing to normalize on blur, because every string a user can type is
a legal path and one that does not exist yet is still the path they mean.

**The picker is not `platform::pick_project_folder`.** That one resolves what was picked to a
*project* folder and reports when it can't. This setting is where the picker **starts**, which
need not hold a project at all — it is usually the folder projects get made in.

**The placeholder diverges from the canvas, deliberately**: `/Users/you/data`, not `~/data`.
Nothing expands a leading `~` — every consumer hands the stored string to the picker's
`set_directory` as-is — so the canvas's example is one the app would silently ignore, from a
field whose own browse button only ever writes absolute paths. Supporting `~` is the other way
to close that gap, and a fine follow-up: it belongs at the *consumers*
(`platform::pick_project_folder` and the field's own picker start), never as a rewrite of what
the user typed.

**The history floor is one number: `strata_core::config::HISTORY_MIN`.** `history_cap` already
floored `max_history` at 1, so the field offers exactly the range its consumer honours — the
same rule P4-05 settled for the column-width bounds, and here the mismatch would be worse than
cosmetic: the cap drives the **rotation**, so a zero would have the next open rewrite
`history.jsonl` down to nothing. There is deliberately no ceiling; keeping more runs costs a
longer log and nothing else, and the canvas offers none either.

**A preferences row's title now wraps** (`components::form::row`). "Confirm before closing a
tab or window with a running query" is a whole clause, and `Strong`'s single-line default would
have clipped it mid-word at the window's minimum width. A fields-register eyebrow stays capped:
one that grew long would be the wrong label.

**`ValueField` sizes its wrapper, not just its `Input`.** Found by this pane's browse button
being pushed off the surface. `InputTypography` is a `rect()` around the input, so *it* is what
a parent lays out; sizing only the input left the wrapper hugging whatever that resolved to —
invisible for a `px` width, and wrong for a relative one, since a `flex(1.)` input inside a
hugging wrapper is not a flex child of the row at all. Fixed in the shared component: the
caller's width goes on the wrapper and the input fills it.

## Acceptance
- [x] System fields edit the draft; history-limit changes cap the History drawer.
- [x] Setting "Opening a project" to This/New stops the prompt appearing and lands opens there;
      back to Ask and the prompt returns.

## Freya / references
- Design: `Settings.dc.html` System. DEV_TASKS W3/U12. `Settings.max_history`.
