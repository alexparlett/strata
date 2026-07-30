# P4-08 · Settings ▸ Keymap (rebindable)

**Phase:** 4 · **Status:** ✅ · **DEV_TASKS:** W4 · **Depends on:** P4-03, P2-20

## Goal
The Keymap category: rebind the shortcuts P2-20 wired.

## What landed

The canvas was **redrawn** between the last handoff and this task: the keymap category used to be
a list of cards (label + description on two lines, a pencil affordance, single-click to rebind) and
is now an **Action / Shortcut table** — a 32px header strip over 30px-floor rows, the description
moved into the row's tooltip, a smaller `Custom` chip, no pencil, and **double-click** to rebind
(`onDoubleClick`, `title="Double-click to rebind"`). So this is built on Freya's builtin `Table`,
the same one the Engine pane (P4-07) put through the fork, rather than on the shared form rows.

Files: `apps/settings/views/keymap/{mod,model,table}.rs`, plus `views/row_note.rs` (shared with the
Engine pane) and four `settings` theme fields.

### Every change is one funnel

Capture, the per-row reset ↺ and Reassign all go through the pane's `ask`, which calls
`strata_core::keymap::propose` and either commits with `apply` or raises the note. The policy and
its sentences live in **core**, beside `validate_bind`, because a hand-edited `config.json` meets
the same rules through `effective_chord` and the two must not drift. Core grew:

- `Rebind::{To(chord), Default, Off}` — the three things a keymap UI can ask for. `Off` exists
  because Reassign performs exactly that on the command it takes a chord *from*, so a steal is
  "unbind the holder, bind the asker" rather than a bespoke operation.
- `Bind::{Ready, Clash { holders, message }, Refused { message }}` — `Clash` is the one outcome the
  UI can offer to push through, which is why the holders are only in that arm.
- `propose` / `apply` / `reset_all` / `is_custom`.

**A reset is conflict-checked like a capture**, which is the non-obvious part: a command's default
chord can have been taken while it was away (bind Save query to ⌘G, bind Find to the ⌘S that freed
up, then reset Save query — `a_reset_is_conflict_checked_like_a_capture` pins it). `Rebind::Default`
also **removes** the override rather than writing the default chord into it, so the row stops
reading as custom and cannot freeze against a later change to `COMMANDS`.

Three things the review pass settled, each about a state a *hand-edited config* can reach that the
UI cannot:

- **An override equal to the default is not an override.** `apply(Rebind::To(default))` clears the
  entry rather than storing a copy, so pressing a command's existing shortcut is the no-op it looks
  like instead of marking the row Custom and growing a reset control for a chord nobody changed.
- **A fixed command is never custom.** `effective_chord` ignores any override of Esc, so
  `is_custom` says so too — otherwise the Dismiss row wore a CUSTOM badge beside its built-in chord
  on a row whose reset ↺ is gated off, with no way to clear it. One predicate drives both the badge
  and the control, so they cannot disagree.
- **A clash names *every* holder.** `Bind::Clash` carries a `Vec<Command>`, because two commands
  can already share a chord (that is what `duplicate_chords_resolve_in_table_order` describes) and
  a Reassign that freed only the first would hand the asker a chord `resolve` still gives to the
  second — reporting success while the shortcut does something else.

### The menubar is disarmed while a row is listening

The one thing that would otherwise have made capture useless on macOS: **the OS resolves a menu
accelerator before the window sees the key**, so with the menubar armed, pressing ⌘C to bind it
copies and the row goes on waiting. ⌘Z ⌘X ⌘C ⌘V ⌘A ⌘O ⌘Q ⌘, are all menubar accelerators — most of
what a user reaches for here. `MenuHandles::suspend_accelerators(bool)` holds them off for exactly
as long as the capture lasts, and it is a **held flag** rather than a `sync_chords(&Default)` call
so the focused window's routine sync cannot re-arm the menubar underneath a capture.

### Live menubar accelerators (the thing menu.rs deferred to this task)

`menu.rs` said out loud that accelerators were read at launch and "live menu updates can ride
P4-08". They now do: `MenuHandles` keeps every accelerator-carrying `MenuItem`, and
`sync_chords` re-applies all of them (and the enabled flag, for the items that ship disabled when
their command is unbound) off `ConfigChan::Settings` from the focused window — the same effect that
already pointed the File menu at it. The stakes are higher than stale text: a stale accelerator has
the OS *consuming* the old chord, so the item keeps firing on a shortcut the user rebound away.

`apply_chords` **destructures** `MenuChords` for the reason `settings_merge!` is a macro — a command
that grows a menu item and forgets the list is a build error, not an accelerator that silently never
updates.

The other reactivity was already in place and needed nothing: `use_hint` / `use_hint_title` /
`KeyHint` subscribe to `ConfigChan::Settings`, and the editor's `EditBindings` are synced by the
editor tab's own side effect. Only the muda menubar was launch-time.

### Fork additions (dashed borders)

The canvas draws both "empty slot" affordances with a **dashed** edge — the Press-shortcut pill and
the Add-shortcut button — and Freya had no dashed border at all: `render_border` *fills* the region
between an outer and an inner rounded rect, and a filled region cannot carry a pattern. Three small
additions rather than a solid-border approximation, since the dash is what says "this slot is open":

- `BorderStyle::{Solid, Dashed { dash, gap }}` on `Border`, with `Border::dashed(dash, gap)` and
  `Scaled` over the dash lengths (they are logical, like the width).
- `Rect::render_dashed_border` — **strokes** the outline's centreline with a Skia dash path effect.
  Two named consequences: one width for all four sides (a stroke has a single width) and no
  `CornerRadius::smoothing` (a squircle outline has no stroked equivalent).
- `PathEffect` re-exported from `freya-engine`.
- `Button::border_style(..)` — the style only, so a dashed button keeps its variant's state-driven
  fill and still answers hover and focus.

## Design divergences (each deliberate)

- **The intro sentence follows the gesture, not the canvas.** The canvas still reads "Click a
  shortcut to rebind it" from before the table, while the cell it describes carries `onDoubleClick`
  and the title "Double-click to rebind". The gesture is the deliberate half — a single click in a
  table row means "I am pointing at this", and a shortcut is too easy to knock off a command by
  pointing at it — so the sentence was updated to match.
- **The double-press is the row's, not the shortcut cell's.** The canvas hangs `onDoubleClick` off
  the 240px column alone; a row here is one command with one chord, so there is no part of it that
  means something else to press, and `TableRow::on_press` (a P4-07 fork addition) already carries
  it. The controls inside the row stop their press, so a single click on Reset or Add shortcut
  cannot also be half a rebind gesture. The description tooltip stays on the action's **name**
  rather than widening with the target: a tooltip spanning the row would nest inside the reset
  button's own and both would open over the same pointer.
- **No zebra.** The canvas bands these rows (`i % 2 === 1 ? var(--c-zebra)`). It banded the Engine
  pane's grid too, and P4-07 settled that a settings list is not a results grid (AGENTS.md §3). One
  answer for both of this window's tables.
- **34px rows, not a 30px floor.** The canvas's floor plus its own contents (24px caps in a 4px
  inset, a 26px reset button) lands its rows at 32–34 wherever they carry anything, and 34 is the
  Engine grid's row height — one height across the window's two tables.
- **The conflict box is a `RowNote` between rows, not a block inside the Action cell.** A cell
  stands at a fixed height so the columns line up, and the clash belongs to the row rather than to
  its label — the same reasoning that put the Engine pane's error strip there. Extracting `RowNote`
  is what made it shared; it takes **one** tone (wash, edge, glyph and message), where the canvas
  pairs a red-edged box with warm text. A box that is red at the edge and amber in the middle says
  both "this is broken" and "answer this", and only the second is true of a clash — so the whole
  note is `warning`.
- **The description tooltip hangs off the action's name**, not the whole row as the canvas's `title`
  does: wrapping a `TableRow` would put a node between it and `TableBody`, and the name is the part
  of a row you point at to ask what it does.
- **`Badge::tag` for the Custom chip.** The canvas sets this one marker two points smaller than the
  app's others and in the UI face rather than mono; taking `Badge` down to it would restyle every
  marker in the app to suit one row.
- **The action label names no colour.** The canvas gives it `--c-text2`, which is exactly what
  Freya's `table` theme already paints as its ambient `color` — so the label inherits rather than
  naming a `settings` field. (It was briefly `item_color`, the nav-row-at-rest tone, which is a step
  dimmer than the design.)
- **The accent is still read off the sheet** (`colors().primary`) in three places here — the CUSTOM
  badge, the capture pill, the Add-shortcut hover — which AGENTS.md §3 reserves for the *semantic*
  slots. The Engine pane does the same for its add tool, so fixing it properly means a `settings`
  accent field and both panes moving to it; left as it is rather than split between the window's two
  grids.

## Not built: a direct unbind control

The acceptance below asks for unbind, and the **state** is fully supported end to end —
`effective_chord` returns `None`, hints vanish, menu items ship disabled, and the row shows the
canvas's **Add shortcut** — but the only thing that *produces* it from the UI is Reassign taking a
chord away, plus a hand-edited config. **The canvas has no unbind affordance** (neither the old
card list nor the new table), so none was invented: a third control in a 240px cell is a design
decision, not an implementation one. Worth raising with the designer — the affordance for the
resulting state already exists, so it would be one button.

## Outstanding: the search index still has P4-09's placeholder

P4-09 left this note for P4-08, and it is **not done** — `apps/settings/search.rs`'s `PAGES` still
carries one "Keyboard shortcuts" entry pointing at this route, put there so a query for "shortcut"
answered with something while the pane was a placeholder. Now that the pane holds real content, the
index should carry what it actually holds — a command is findable by its own name, not by the page's
— and the page entry should go.

It was left out of this task deliberately rather than missed: a command row is not a
`components::form::Row`, so it is probably a new `Hit` kind rather than an `Anchor`, and the
flash/scroll half is `Row::anchor`'s and would have to be earned separately if a captured row wants
it. That is a design decision inside P4-09's mechanism, not a loose end in this one. Pick it up as
its own change, in either task's name.

## Acceptance
- [x] Rebind with conflict resolution (Reassign steals + unbinds the other / Cancel).
- [x] Reset one row and Reset all, both conflict-checked; no duplicate binding reachable.
- [x] Unbind supported as a state (reachable via Reassign and config; no dedicated control — above).
- [x] Changes reach every window: the shared settings, plus live menubar accelerators.
- [ ] **No keyboard route to start a rebind.** The shortcut cell is not focusable and the gesture is
      a double mouse press, so a row that already has a chord cannot be changed without a mouse (an
      unbound row's Add shortcut *is* reachable by Tab). The canvas has no keyboard affordance
      either, so closing this is the designer's call — same conversation as the unbind control.
- [ ] **Visually unverified** — a Strata from another worktree held the single-instance hook, so the
      pane has not been looked at on screen. Build and `schema_in_sync` are green.

## Freya / references
- Design: `Settings.dc.html` Keymap (the **updated** canvas — the local handoff bundle predates it;
  read it through `DesignSync` on the design project). Command table from P2-20 /
  `strata_core::keymap::COMMANDS`. Shared settings (P4-01). DEV_TASKS W4.
- The fork additions need pushing to `github.com:alexparlett/freya` before CI or a fresh clone can
  build this branch (AGENTS.md §6) — `Border::dashed` and `Button::border_style` are load-bearing.
