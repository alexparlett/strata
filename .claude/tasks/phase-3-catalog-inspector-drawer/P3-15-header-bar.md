# P3-15 · Header bar (title bar · brand · project switcher · action cluster)

**Phase:** 3 · **Status:** ✅ (two placeholders, one seam — below) · **DEV_TASKS:** U2 · **Depends on:** — · **Feeds:** P6-01 (palette), P4-03 (settings), P4-13 (open project)

> **Why this file exists:** the header was the one app-shell surface with no task of its own — the
> rail and the panel frame came in with **P3-01**, the header was left as a bare 48px strip. This is
> that missing task, written as it was built.

## Goal
The window header **and** the window's title bar: brand · project switcher · the ⌘K / ⌘, cluster,
over the strip the macOS traffic lights float in — behaving like a real title bar (drag, fill,
traffic-light gutter).

## Current state (built)
`views/header/` — `mod.rs` (the bar) + `project_menu.rs` (the switcher).

- **Title-bar behaviour.** The window ships transparent-titlebar + fullsize-content-view + hidden
  title, so this bar *is* the title bar. `title_bar_press` (the fork's `WindowDragExt::window_drag`
  recipe, kept app-side — see below): press-drag moves the window, double-press **fills** it to the
  current monitor (macOS *zoom*) or restores the previous size. Fill ≠ native fullscreen — that
  stays the green button's. Every interactive child (`Button::on_pointer_down` →
  `stop_propagation`) opts out, so a press on a control never drags.
- **Our fill / fullscreen are not persisted; a user-sized fill is** (normal IDE behaviour).
  `use_autosave` takes the geometry the window was created with and only writes the last geometry
  the window had while in neither state. The subtlety: macOS's `isZoomed` — what the fork's new
  `Platform::is_maximized` mirrors — is a **frame comparison**, not a state flag, so a window the
  *user* tiled to the screen (macOS 15 edge-tiling / the green button's *Fill*) reports zoomed
  exactly like our double-press. That size is one they chose and must persist, so the header marks
  the fills **it** initiates (`filled_by_app`, owned by the window root, read by both) and the save
  keys off that plus `Platform::is_fullscreen`. Leaving fill by any route clears the mark, so a
  stale flag can't freeze the geometry. Both flags are refreshed on `WindowEvent::Resized`
  (companions to the `window_position` mirror added for the same feature).
- **Brand** — the app mark (the dock icon's bands, `icons/strata.svg` scaled to a 24 viewBox) in a
  22px rounded, clipped tile, plus the wordmark in the scale's `Title` role.
- **Project switcher** — ghost `flat_button` trigger (folder glyph · project name · ⌄) opening the
  comp's 328px dropdown: **Open…**, `OPEN PROJECTS` (the app-global open set, this window's marked
  current), `RECENT PROJECTS` (recents minus what's open), each row an initials avatar + name +
  path. Data is real, from `ProjectState` (`ProjChan::Meta`) + `AppConfig`
  (`ConfigChan::Recents` / `Open`).
- **Action cluster** — 30×30 standard `button`s: Search and Settings, each wearing its live chord in
  the tooltip (`use_hint_title`).
- Colours: **all** of it is the `header_bar` theme component — the bar's surface plus the
  switcher's dress (`accent`, `menu_*`, `avatar_*`), the way every other surface (`tab`,
  `status_bar`, `record_view`, …) carries its content colours. Both theme files + the generated
  schema updated (`UPDATE_SCHEMA=1 cargo test -p strata-freya schema_in_sync`).
- `Divider::menu(color)` moved into the shared component (the tab menus' `menu_sep` now delegates).
- Icons added: `Folder`, `Gear`, `StrataLogo`.

## Deliberately not wired here
- **Search → command palette (P6-01)** and **gear → settings window (P4-03)**: the buttons are
  placeholders that log; their chords are already consumed by `project.rs`'s catch-all stub.
- **Open… / a project row → open that project (P4-13, with P4-01's window model).** Opening is one
  mechanism — folder pick → `.strata/` load → this-window-or-new-window per `OpenPref` → the
  re-open-in-place guard. The rows log and close; wire them at that seam, not with a header-local
  open path.

## Remaining
- [ ] Wire the three seams above as their tasks land.
- [ ] Window **title** follows the project name (P4-13 build item 4).
- [ ] Recent-row **branch glyph** — deferred in the Dioxus app too (no git integration).

## Freya / references
- Design `Header.dc.html` → `Strata.dc.html` `data-rg="header"` (+ the project-switcher dropdown
  right below it). Dioxus parity: `crates/strata-dioxus/src/ui/header.rs` (+ `.ps-header` /
  `.proj-btn` / `.proj-item` in `assets/main.css`, incl. the macOS 82px traffic-light gutter).
- Freya: `Attached` (+ the fork's new `offset`, the dropdown's gap off its trigger),
  `Menu`/`MenuButton`/`MenuItem`, `Button::flat`, `TooltipContainer`, `EventsCombos::pressed`.
  DEV_TASKS **U2**.
- **Fork changes this needed** (`crates/freya`): `Platform::is_maximized` / `is_fullscreen`
  mirrors refreshed on `WindowEvent::Resized`; `Attached::offset`; a `WindowDragExt` doc spelling
  out fill ≠ fullscreen.
