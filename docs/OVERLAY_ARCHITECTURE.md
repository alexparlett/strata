# Strata — Overlay architecture (A3) — container model

**Model:** egui-style **containers**. `Popup`, `Window`, and `Dialog` are reusable
components you **hand content to** (as `children`); each **self-contains** its
positioning, chrome, and dismissal. State is a plain `open: Signal<bool>` the
caller owns (a local `use_signal`) — **no central `AppState` enum, no reducer, no
`PopupLayer` that `match`es variants.** You drop a container in *at the trigger*.

This is a deliberate exception to A2 ("every UI event through `dispatch`"): overlay
open/close is transient, single-owner UI state, so it lives local — exactly the
spirit of A4. Content composes from the primitives in §4.

---

## 1. Three concerns

| | **Popup** (menu / dropdown) | **Window** (task panel) | **Dialog** (confirm / quick view) |
| --- | --- | --- | --- |
| Anchor | cursor point or element | centred, then **free** | centred, fixed |
| Backdrop | invisible click-catcher | **none** (non-modal, egui-style) | dimming scrim, blocking |
| Move / resize | no | **yes** (drag titlebar, corner grip) | no |
| Dismiss | outside-click · Esc · pick | close button · Esc | backdrop · Esc · action |
| Concurrency | one at a time in practice | one+ | stacks over windows/dialogs |
| Examples | catalog / tab menus, dropdowns | Settings, Export, Configure | remove-confirm, cell view, command palette, B10, S14 |

## 2. Current overlays → concern

Popups: `ctx_menu`, `tab_menu`, project/recents, page-size, format. Windows:
Settings, Export, Configure. Dialogs: remove-confirm, cell view, command palette
(+ future B10 / S14). The bottom drawer (`log_open`) is docked layout — out of
scope.

---

## 3. The containers (the API)

Each is **controlled** by an `open: Signal<bool>` owned by the caller, takes its
content as `children`, and manages the rest itself.

```rust
// Anchored menu/dropdown. Owns: catcher, Esc, positioning. Mount it conditionally
// (`if let Some(t) = menu() { Popup { … } }`); it calls `on_close` to dismiss.
// Implemented: `Popup(on_close: EventHandler<()>, at: Point, children)` + the
// `MenuItem` / `MenuSep` content primitives (`ui/components/`).
#[component]
fn Popup(open: Signal<bool>, at: Anchor, children: Element) -> Element

// egui-style in-app floating panel. Owns: drag/resize geometry, close, Esc.
#[component]
fn Window(open: Signal<bool>, title: String, #[props(default)] init: WinGeom, children: Element) -> Element

// Centred, scrim, blocking. Owns: scrim, centre, Esc, focus.
#[component]
fn Dialog(open: Signal<bool>, title: String, #[props(default)] footer: Option<Element>, children: Element) -> Element

pub enum Anchor { Cursor(Point), Fixed(Point) }   // element-rect anchoring: fast-follow (needs DOM measure)
pub struct Point { x: f64, y: f64 }
```

**How they self-contain state:**
- **`open`** is the caller's `Signal<bool>`. The container early-returns `rsx!{}`
  when `!open()`. To close, it calls `open.set(false)`.
- **`Window` geometry** is the container's *own* `use_signal(init)` — drag/resize
  mutate it; the caller never sees it. (Persistence later = pass a geom signal in.)
- **Dismissal is internal** — no reducer:
  - `Popup`/`Dialog` render a full-screen **catcher** div behind the card; its
    `onclick` / `oncontextmenu` → `open.set(false)`. The card `stop_propagation`s.
  - **Esc:** the catcher is focusable (`tabindex:0`, focused on mount) with
    `onkeydown` Escape → `open.set(false)`. Dialogs with inputs autofocus the
    first field instead and let Escape bubble to the catcher's `onkeydown`.
  - `Window` is non-modal (no catcher/scrim); it closes via its titlebar ✕ and Esc.

**Why no shared scope (for v1):** the catcher already gives outside-click dismiss
*and* prevents double-open — right-clicking a second trigger hits the open
catcher (which closes the first), so two menus never coexist. Esc is handled by
the focused top catcher. A shared `OverlayScope` (stack of open overlays) would
add single-click trigger-switching and strict stacked-Esc precedence, but it's not
needed to be correct — deferred.

**Usage** — the whole point is this reads at the call site:

```rust
// a catalog row's context menu, owned locally by the sidebar
let mut menu = use_signal(|| None::<CtxTarget>);   // Some(target+point) when open
// on a row:
oncontextmenu: move |e| { e.prevent_default(); menu.set(Some(CtxTarget::new(row, e))); }
// once, at the sidebar root:
if let Some(t) = menu() {
    Popup { open: /* derived bool */, at: Anchor::Cursor(t.at),
        MenuItem { icon: icons::play(14), onclick: move |_| { menu.set(None); dispatch(state, Action::LoadSelectStar(t.name)) }, "View table" }
        MenuItem { danger: true, onclick: ..., "Drop table" }
    }
}
```

> Note the menu **content lives with its trigger**, not in a far-off `match`. The
> action a row performs is still `dispatch(...)` — only the *open/close* is local.

---

## 4. Building blocks (content primitives)

Thin wrappers over the existing `.btn / .field / .toggle / .menu-item …` CSS, so
container bodies compose instead of copy-pasting classes; theme comes for free.

```rust
Btn       { variant: Accent|Default|Ghost|Danger, size?, icon?: Element, kbd?: String, disabled?: bool, onclick, children }
IconBtn   { icon: Element, title: String, active?: bool, onclick }
MenuItem  { icon?: Element, label: String, meta?: String, danger?: bool, disabled?: bool, onclick }
MenuSep   {}
Field     { label: String, hint?: String, children }
Input     { value: String, placeholder?: String, oninput, onkeydown? }
Toggle    { on: bool, onclick }
Segmented { value: String, options: Vec<(String, String)>, onclick }
Section   { label: String, children }
DialogFooter { children }
```

A **Dropdown** = a `Btn` trigger + a local `open` signal + a `Popup` of
`MenuItem`s. Page-size / format / recents all become this.

---

## 5. Build & migration order (each step compiles)

0. **DONE — Unwind the false start.** Removed the reducer version (the
   `AppState.popup` enum + field, `Action::ClosePopup`, the variant-matching
   `ui/popup.rs`). Kept `Point`.
1. **DONE — `Popup` container + primitives.** Built in `ui/components/`
   (`popup.rs` + `menu.rs`, with `card_class`/`width` for the richer dropdown
   chrome). Catalog + tab context menus migrated to sidebar/workspace-local
   `use_signal`s; `OpenCatalogMenu`/`OpenTabMenu` actions + handlers deleted.
2. **DONE — Recents dropdown** → `Popup` in the header, local signal;
   `ToggleProjectMenu` deleted. (Page-size / format dropdowns still pending — they
   want flip-up positioning; small follow-up.)
3. **DONE — `Dialog` container** (`ui/components/dialog.rs`; centred scrim, focus,
   Esc with `stop_propagation`, `has_input` to defer focus to a body field).
   remove-confirm → sidebar-local, cell view → workspace-local, command palette →
   **root-local** (open flag owned by `ProjectRoot`; both ⌘K and the header search
   button drive it; it closes via an `on_close` callback). Removed the
   `remove_*`/`cell*`/`cmdk_*` `AppState` fields, the
   `RequestRemove`/`CancelRemove`/`OpenCellPopover`/`ToggleCmdk` actions, and their
   `CloseOverlays` coupling.
4. **`Window` container** (drag + resize + geometry). Migrate Settings / Export /
   Configure; **fold in A4** (their form state → component-local). Non-modal.
5. **Cleanup + unified Esc.** Once every overlay is a container, introduce the
   **`EscStack`** (a LIFO registry in context; one root handler pops the top on
   Escape) and have `Popup`/`Dialog`/`Window` register on mount / unregister on
   unmount. Then delete `overlay::close_all`, the root `CloseOverlays` Esc
   fallback, the remaining `*_open` bools, and the `if X_open` pile in `app.rs`.
   This is where the deferred `OverlayScope` finally lands — until then Esc is
   focus + bubbling (correct while only one overlay is open at a time).

Element-rect anchoring (vs. the current fixed/cursor `Point`) is an optional
refinement after the containers land.

---

## 6. What each buys

One place to restyle chrome + controls; consistency; new overlays are a container
+ a few primitives; theming free; and — because open/close is a local signal at
the trigger — no central enum to grow and content sits with its trigger. The
refactor removes code (per-modal scaffolding + the `AppState` overlay fields +
`close_all`) rather than relocating it.
