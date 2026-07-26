# P4-03 · Settings window shell

**Phase:** 4 · **Status:** ✅ · **DEV_TASKS:** W1 / U12 · **Depends on:** P4-01 · **Unblocks:** P4-04…P4-09

## Goal
The settings window frame: single canonical instance, category nav, draft/save, and live theme.

## Built (`crates/strata-freya/src/apps/settings/`)
- **Its own OS window** (`SettingsApp`, canvas 940×660, min 740×480), same chrome as the other
  windows: transparent titlebar + fullsize content view + hidden title, traffic lights inset into
  our own 50px strip (`with_traffic_light_inset(9., 11.)`).
- **A child window of its opener.** Settings is a panel over the window you were working in, so it
  is pinned above that window as a native `addChildWindow:ordered:` child: ordered above it, not
  coverable by it, travelling with it, while the opener stays fully interactive. Asking again from
  another window **re-points** it there. `platform::settings::open_settings` is the single entry
  point behind every trigger, so "already open" only ever means focus + re-pin.
- **Category nav** (Theme · System · Data display, under *Appearance & behaviour*; then Keymap;
  then Engine ▸ Properties), collapsible group headings. Rows are Freya's `SideBarItem` wrapped
  in the router's **`ActivableRoute`**, so the current route *is* the selection — deliberately
  **not** `SidebarRow`, whose own `Activable` would shadow it (`use_is_active` reads the closest
  provider). The catalog and launcher rails keep `SidebarRow`: they mark a selection, not a
  location, and have no route to read.
- **Draft / save:** controls edit `SettingsCtx::draft`, a working copy seeded from the committed
  settings. **Apply** writes it to the app-global config (publish + persist) and closes; **Cancel**,
  Esc and the red button close without writing. Apply is disabled while the draft is unchanged.
- **Theme is live:** the draft's theme half is mirrored into the app-global `ThemePreview`, which
  `use_strata_theme` resolves *ahead of* the committed settings — so every window re-themes at once
  while the pick is uncommitted. Dropping the slot on the way out is the revert.
- Entry points wired: project header gear · launcher rail Settings row · ⌘, in both · App menu
  **Settings…**. Dropped the "appearance & behaviour" subtitle (U12 drift).

## Corrections / decisions this task carried
- **Navigation is `freya-router`** (in-memory history), not local state: `Route` per category under
  the `SettingsChrome` layout, so the frame mounts once and only the pane swaps. Added the `router`
  feature to `strata-freya`'s `freya` dependency. Selection reads `use_route`, never a second
  copy of "which category".
- **The live theme needed a second derivation input, not a stored theme.** There is no shared
  `theme` signal in the Freya app (AGENTS §2: the theme is pure derived state), and `write_config`
  always persists — so an uncommitted-but-live theme is a *narrow* app-global preview slot
  (`state/theme_preview.rs`: `State<Option<ThemeSel>>`, theme id + sync_os) that the same
  derivation reads first. Deliberately narrow, and mirrored with `set_if_modified`: putting the
  whole draft there would wake every window's theme derivation on a keystroke in a text field.
  Pinned by `theme::tests::a_preview_outranks_the_committed_theme_until_it_is_dropped`.
- **The Settings window is in the live registry (`WindowKind::Settings`) but is not a workspace
  window.** `Windows::is_last()` counts projects and the launcher only — counting Settings would
  let the last project close onto an empty app, since Settings goes with it.
- **Closing with the owner is ours, not AppKit's.** AppKit closes a child window behind winit's
  back, and Freya only ever removes a window on a close it was asked for — so it would keep a live
  scope for a window no longer on screen. The rule is expressed in the app's terms instead: the
  owner leaving the live registry closes Settings through Freya's own path (`use_settings_pin`),
  which also covers the platforms where the child relationship is a no-op.
- **Fork addition:** `WinitPlatformExt::set_window_parent(child, Option<parent>)` +
  `RendererContext::set_window_parent`, with the AppKit half in
  `freya-winit/src/parent_window.rs`. winit can only express a parent window at creation time
  (`unsafe`, raw handle), so there was nothing to reach for — AGENTS §6, fix it in the fork.
- **`Settings` (strata-core) gained `PartialEq`** — the whole struct is the unit of "is there
  anything to apply?".
- **Theme:** new `settings` component theme, plus two palette slots (`accent_selection` .10,
  `accent_badge` .12) that de-duplicate accent tints this window would otherwise have repeated as
  `specific`s — `launcher.nav_background` and `catalog.part_background` now reference them
  (AGENTS §2, no visual change).

## Known gap: Apply commits the whole struct — **settled in P4-04** ✅
`SettingsCtx::apply` wrote the entire `Settings` from a draft seeded at mount, so a setting
another window committed meanwhile was silently reverted. Concretely: with Settings open, the T2
close confirm's "Don't ask again" writes `confirm_close_running`
(`views/dialogs/close_confirm.rs`), and Apply then restored the old value — a setting the user
changed, undone by a window that never showed it.

It was unreachable while no pane edited the draft, and was left for the first task that made the
draft editable. **P4-04 settled it** with the per-field diff (`Settings::merge_onto`) — see that
task's file for the shape and why `dirty` moved onto the seed with it.

## Not built here (owned elsewhere)
- **The five category panes.** Each route renders `Pane::not_built(..)` until its task lands:
  P4-04 Theme · P4-05 Data display · P4-06 System · P4-07 Engine · P4-08 Keymap. Nothing writes to
  the draft yet, so Apply is always disabled in this state — the first control to edit the draft
  makes it live with no change here.
- **The nav's search box** is P4-09 and is deliberately absent rather than inert: a search field
  that returns nothing is worse than none, and the index it filters is that task's to build.

## Acceptance
- [x] One settings window; re-invoking focuses it and re-pins it above the window that asked.
- [x] Draft edits; Apply applies app-wide + persists; Cancel/Esc/red button discards.
- [x] Theme changes preview live across windows (mechanism + test; the *control* is P4-04).

## Freya / references
- Design: `Settings.dc.html`. DEV_TASKS W1/U12. Router: `crates/freya/examples/feature_router.rs`.
