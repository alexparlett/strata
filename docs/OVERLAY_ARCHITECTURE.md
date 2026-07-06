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
// `MenuItem` / `MenuSep` content primitives (`ui/overlay.rs`).
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

0. **Unwind the false start.** Remove the reducer version I began: the
   `AppState.popup` enum + field, `Action::ClosePopup`, and the variant-matching
   `ui/popup.rs`. Keep `Point`.
1. **`Popup` container + primitives.** Build `Popup`, `Btn`, `MenuItem`, `MenuSep`
   in `ui/overlay/`. Migrate the **catalog** + **tab** context menus: each owner
   (sidebar / workspace) holds a local `use_signal` and renders `Popup` with
   `MenuItem` content. Delete `OpenCatalogMenu`/`OpenTabMenu` actions + the
   `catalog::open_menu`/`tab::open_menu` handlers.
2. **Recents dropdown** → `Popup` in the header, local signal. Delete
   `ToggleProjectMenu`. (Page-size / format dropdowns follow — they want
   flip-up positioning; small follow-up.)
3. **`Dialog` container.** Migrate remove-confirm, cell view, command palette off
   their `AppState` bools + hand-rolled scrims onto `Dialog`.
4. **`Window` container** (drag + resize + geometry). Migrate Settings / Export /
   Configure; **fold in A4** (their form state → component-local). Non-modal.
5. **Cleanup.** Delete the leftover `*_open` bools, `overlay::close_all`, and the
   `if X_open` render pile in `app.rs`.

`OverlayScope` (shared stack for stacked-Esc + single-click switch) and
element-rect anchoring are optional refinements after the containers land.

---

## 6. What each buys

One place to restyle chrome + controls; consistency; new overlays are a container
+ a few primitives; theming free; and — because open/close is a local signal at
the trigger — no central enum to grow and content sits with its trigger. The
refactor removes code (per-modal scaffolding + the `AppState` overlay fields +
`close_all`) rather than relocating it.
