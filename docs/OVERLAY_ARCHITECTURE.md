# Strata — Overlay architecture (A3)

Two layers, kept separate:

1. **Chrome** — reusable, egui-style **container** components (`Popup`, `Dialog`,
   `Window`) in `src/ui/components/`. You hand each one content as `children`; it self-contains its positioning,
   scrim/catcher, focus, and dismissal. Nothing about *which* overlay is open lives in a container.
2. **Open-state** — owned in one of two places depending on the overlay's reach:
    - **Co-located popups** → a component-local `use_signal` at the trigger owner.
    - **App-global overlays** → a per-window **overlay store** (`crate::overlays`).

Overlay visibility is **never** in `AppState`. `AppState` is the domain/project state and is being decomposed, not
grown — its remaining ephemeral UI flags (`log_open`, `page_size_open`, `renaming_ws`) are debt, not a pattern to copy.

---

## 1. The three containers

|               | **Popup** (menu / dropdown)           | **Dialog** (confirm / quick view)          | **Window** (task panel)              |
|---------------|---------------------------------------|--------------------------------------------|--------------------------------------|
| Anchor        | cursor / fixed point                  | centred                                    | opens at a `WinGeom`, then **free**  |
| Backdrop      | invisible click-catcher               | dimming scrim, blocking                    | **none** (non-modal)                 |
| Move / resize | no                                    | no                                         | **yes** — drag titlebar, corner grip |
| Dismiss       | outside-click · Esc · pick            | backdrop · Esc · action                    | titlebar ✕ · Esc                    |
| Examples      | catalog / tab menus, project dropdown | remove-confirm, cell view, command palette | Settings, Export, Config             |

All three live in `src/ui/components/` (`popup.rs`, `dialog.rs`, `window.rs`,
`menu.rs`) and are re-exported from `components::{Popup, Dialog, Window, MenuItem,
MenuSep, Point, WinGeom}`.

### API (as built)

Each takes `on_close: EventHandler<()>` and is **mounted conditionally by the caller** — the container has no `open`
prop; when it's in the tree, it's shown.

```rust
// Anchored menu/dropdown. Owns the full-screen catcher, Esc, positioning.
// `card_class` picks the chrome (`ctx-menu` default, or `menu` for the richer
// dropdown); `width` fixes a px width. Content = MenuItem / MenuSep.
fn Popup(on_close: EventHandler<()>, at: Point,
         card_class: Option<String>, width: Option<u32>, children: Element)

// Centred, dimming scrim, blocking. `card_class` is the card chrome
// (e.g. "confirm", "cmdk", "modal cell-modal"); `z` its stacking order; `top`
// top-aligns instead of centring; `has_input` leaves focus to a body field.
fn Dialog(on_close: EventHandler<()>, card_class: String, z: Option<u32>,
          top: bool, has_input: bool, children: Element)

// Non-modal floating panel: no scrim, drag by titlebar, resize from the grip.
// Owns its geometry internally (a use_signal seeded from `init`).
fn Window(on_close: EventHandler<()>, title: String, subtitle: Option<String>,
          icon: Option<Element>, init: Option<WinGeom>,
          min_w: Option<f64>, min_h: Option<f64>, footer: Option<Element>,
          children: Element)

pub struct Point {
    x: f64,
    y: f64
}              // cursor/fixed anchor for Popup
pub struct WinGeom {
    x: f64,
    y: f64,
    w: f64,
    h: f64
}
```

**Dismissal is internal to each container:**

- `Popup` / `Dialog` render a focusable full-screen **catcher**; `onclick` (and
  `Popup`'s `oncontextmenu`) → `on_close`. The card `stop_propagation`s so inner clicks don't dismiss it.
- **Esc:** the catcher is `tabindex:0` and grabs focus on mount, so its
  `onkeydown` catches Escape → `on_close`. `Dialog`/`Window` also `stop_propagation`
  on Escape so the root `CloseOverlays` handler doesn't double-fire. A `Dialog` with a body input (`has_input`) doesn't
  steal focus — Escape bubbles from the input up to the catcher.
- `Window` is non-modal (no scrim); it closes via its titlebar ✕ and Esc, and owns its drag/resize geometry through a
  pointer-capture layer mounted during a drag.

---

## 2. Open-state: local vs. the store

### Co-located popups → local `use_signal`

Catalog + tab **context menus**, the **project dropdown**, **remove-confirm**, and the **cell view** are opened and
rendered by the *same* component (sidebar / workspace / header). They have a single owner, no cross-cutting triggers,
and no engine coupling — so their open-state is a local `use_signal`, and the content lives right at the trigger:

```rust
// a catalog row's context menu, owned locally by the sidebar
let mut menu = use_signal( | | None::<CtxTarget>);
// on a row:
oncontextmenu: move | e| { e.prevent_default(); menu.set(Some(CtxTarget::new(row, e))); }
// once, at the sidebar root:
if let Some(t) = menu() {
Popup { on_close: move | _ | menu.set(None), at: t.at, {catalog_menu_items(state, menu, t)} }
}
```

The action a row performs is still `dispatch(...)`; only open/close is local.

### App-global overlays → the overlay store

The **command palette, Settings, Export, and Config** are triggered from many places (header, sidebar, results toolbar,
⌘K/⌘,, the palette itself) and some are closed by the engine layer. Local signals don't fit that. They use a small,
focused, per-window **store** — the React/Zustand shape (a store read reactively, written from anywhere), *not* an event
bus.

```rust
// src/overlays.rs — per-window, because each project window is its own VirtualDom,
// and a GlobalSignal is scoped to its VirtualDom.
pub struct OverlayState {
    settings: bool,
    cmdk: bool,
    export: bool,
    config: bool
}
pub static OVERLAYS: GlobalSignal<OverlayState> = Signal::global(OverlayState::default);

pub fn toggle_settings();
pub fn set_settings(bool);
pub fn toggle_cmdk();
pub fn set_cmdk(bool);
pub fn open_export();
pub fn close_export();
pub fn open_config();
pub fn close_config();
```

Each app-global overlay is an **always-mounted host** — `CmdkHost`, `SettingsHost`,
`ExportHost`, `ConfigHost`, all mounted unconditionally in `ProjectRoot`. A host reads its store field reactively and
renders its `Dialog`/`Window` only when open — **visibility is derived during render**: no local signal, no `use_effect`
(per React's "don't sync state via effects").

```rust
#[component]
fn SettingsHost() -> Element {
    if !overlays::OVERLAYS.read().settings { return rsx! {}; }
    rsx! { SettingsModal { on_close: move |_| overlays::set_settings(false) } }
}
```

Triggers just call the helpers — `overlays::toggle_settings()` from the gear/⌘,,
`overlays::open_export()` from the toolbar. Because the helpers are plain functions, they're callable from the
**non-component action/engine layer** too:

- `run_export` closes Export with `overlays::close_export()` after its async file dialog — and because the host is
  *always mounted*, that spawn survives the window closing.
- The `Event::Registered` handler reads `OVERLAYS.peek().config` and closes Config with `overlays::close_config()` on a
  successful register.
- `OpenConfigNew` / `OpenConfigEdit` remain actions: they set up the form (`AppState.cfg`) and then call
  `overlays::open_config()`. So the sidebar / rail / palette triggers are unchanged — they still `dispatch` those
  actions.

The command palette carries a small `Effect` enum so a row can either `Dispatch(Action)`
or open a store window directly (`OpenExport`).

---

## 3. Esc & the shrinking `CloseOverlays`

Each container handles its own Escape (focus + `on_close`, with `stop_propagation`
on `Dialog`/`Window`). The root `handle_key` Escape → `Action::CloseOverlays` now only clears the leftover `AppState` UI
bits — the page-size dropdown and tab rename. There is no `EscStack`: only one overlay is open at a time in practice,
and each container's focused catcher + `stop_propagation` gives correct dismissal without a shared registry. (A LIFO
`EscStack` remains the upgrade path *if* real overlay stacking ever appears.)

---

## 4. Status & remaining work

**Done:** `Popup` / `Dialog` / `Window` containers; catalog + tab menus, project dropdown, remove-confirm, cell view on
local signals; command palette, Settings, Export, Config on the store via always-mounted hosts. `AppState` holds **no**
overlay-visibility flags.

**Remaining (non-blocking):**

- **A4 — form state.** Config's `cfg` and Export's `export` sub-structs still live in `AppState`; localizing them is the
  next decomposition.
- **Page-size / format dropdowns** → `Popup` (they want flip-up positioning).
- **Element-rect anchoring** for `Popup` (vs. the current cursor/fixed `Point`) — needs a DOM measure; optional.
- **Window geometry persistence** — `Window` owns geometry internally today; pass a geom signal in to persist per
  overlay.

**Rejected along the way (don't reintroduce):** a centralized `AppState.popup`
enum + reducer + a `match`-everything `PopupLayer`; and a `GlobalSignal` **event bus** + `use_effect` subscription (a
store beats an event-bus-plus-effect-mirroring for this — see the React docs on not syncing state via effects, and
Zustand's out-of-tree `getState`).

---

## 5. What the split buys

One place to restyle chrome (the three containers); one place per overlay for its content (the host or the trigger);
overlay visibility out of the `AppState`
monolith; and engine-driven closes that "just work" because the store is writable from the action/engine layer.
Truly-local overlays stay local — no ceremony for a menu that a single component owns.
