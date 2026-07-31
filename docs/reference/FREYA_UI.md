# Freya component, UI and state conventions

The component/UI conventions (AGENTS §3) and the state-placement decision procedure (§4), in
full. [AGENTS.md](../../AGENTS.md) carries the one-line form of each. The framework-level
reference is the `freya:freya` skill and the fork source in `crates/freya/`; the full state
design is [FREYA_STATE_ARCHITECTURE.md](../FREYA_STATE_ARCHITECTURE.md).

## Component and UI conventions

- **Reusable UI is a `Component`**: `struct` + `#[derive(PartialEq)]` +
  `impl Component { fn render(&self) -> impl IntoElement }`. Plain functions only for the app root
  and stateless helpers. `mod.rs` builds children by **struct literal**, so fields stay visible.
- **Builder pattern**: chain methods; never store an element in a variable to mutate later. Use
  `.maybe(bool, |el| …)`, `.map(Option, |el, v| …)`, `.maybe_child(Option)`.
- **Standard components first.** Ghost icon buttons are `Button::new().flat()`, input-shell
  dropdowns are `Select`, text fields are `Input` — never hand-rolled lookalikes. The design comps'
  `data-hv` vocabulary maps 1:1 onto existing component themes; duplicating it drifts. Icon-button
  clusters are **28×28**. A missing component *state* (e.g. disabled) belongs on the component's
  own theme **in the fork** (`ButtonColors` grew `disabled_*` for exactly this) — never as a token
  on the consuming surface's theme. The same answer scales to a whole component: the Engine pane's
  properties grid is Freya's `Table`, and the four things it could not do turned out to be fork
  gaps rather than design limits (`TableRow` had a `pub theme` field with no builder, so a row
  could not carry a selection fill or decline the hover; only `TableCell` had `on_press`;
  `TableCell` hardcoded `main_align(End)`; `Table`'s rect had no flex content, so a stated height
  could not reach a scrolling body; and one `divider_fill` painted both the box and the rules
  between rows, so a theme could author the grid's outline or its row rules but never both — it
  grew a `border_fill`). Five small upstream-shaped additions beat a hand-rolled grid —
  but the test is whether the gap is in the *component*: what a table has no opinion about (which
  row is selected, what goes between two rows) stays composed in the app. And the other way round —
  a settings list is **not** a results grid, so it gets no zebra: banding is a reading aid for
  dense data, and on a form it only competes with the one row state the surface has. The Keymap
  grid (P4-08) is the second table in that window and takes the same answer to every one of these
  questions, down to the row height — one table dress per window, not one per pane.
  A **dashed** edge was the one thing neither table could get from anywhere: torin fills the region
  between an outer and an inner rounded rect, and a filled region cannot carry a pattern, so
  `BorderStyle::Dashed` strokes the outline's centreline with a Skia dash effect instead
  (`Border::dashed`, `Button::border_style` — the style only, so a dashed button keeps its variant's
  state-driven fill). Two named costs, because a stroke has one width and no squircle: a dashed
  border uses `width.top` for all four sides and ignores `CornerRadius::smoothing`. It is a fork
  addition rather than a solid approximation because the dash is the whole message — it says the
  slot is *open*, which is exactly what distinguishes "Add shortcut" from a bound control. And don't restate at a call site what a variant already
  resolves: `Button::new().filled()` *is* accent-over-inverse-text, so a `theme_colors` override
  naming those same two slots is a second copy of them. Override only for a genuinely different
  tone (the destructive action reading `cancel_button`).
- **A surface with its own component theme reads colours from that theme, not also from the
  sheet.** Once a component has a `define_theme!`, every colour it paints — surfaces, borders,
  hairlines, tints — is one of its own fields, authored as a `reference` to a sheet slot where it
  should track one. The sheet is reached for directly only where the value is **semantic**
  (`success` / `warning` / `error` / `info` — the status bar's state dot), because those must
  follow the app-wide ramp wherever they appear. Mixing the two sources in one component is how
  `colors.border` ends up beside a `border_fill` that already holds the same value.
- **A shared theme's fields are named for the role they play, not for whoever needed one first,
  and a component's own dress never becomes one.** The `drawer` theme dresses three bodies, so a
  field called `stats_color` is one the other two can never use — it is `value_color`, "a row's
  secondary fact", and History is merely the first row wanting all three text tones at once. The
  same question kills a field outright when the colour belongs to a *component*: the line-count
  pill's outline was briefly `badge_border_fill` on the drawer, but an outline is the badge's own
  dress, so `Badge::outlined()` derives it from its foreground exactly as the tint derives from
  `TINT_ALPHA` — and every surface that ever uses an outlined badge pays nothing. Before adding a
  field, ask which of the surface's other users could name it too; if the answer is none, it is
  either misnamed or it belongs to the component.
- **Fonts are never hardcoded.** Text goes through the typography role components
  (src/components/typography.rs); `Input`s are wrapped in `InputTypography::body(..)`/`::mono(..)`;
  `CodeEditor` pulls from the theme's code scale. Mixed-style inline text (one sentence changing
  style mid-run) is a `paragraph()` of spans dressed from the typography scale — not adjacent
  labels, which can't wrap or truncate as one line. Hooks that consume theme context must be called
  a **fixed number of times** per render — a variable number of calls breaks hook order.
- **Event props follow `Button`'s shape**: field `Option<EventHandler<Event<T>>>`, builder takes
  `impl Into<EventHandler<Event<T>>>`, and the handler is called with the triggering event even if
  callers ignore it. Never bespoke unit-payload shapes like `Option<EventHandler<()>>`.
  `Callback<A, R>` is a different tool, only for value-returning callbacks
  (e.g. `on_pre_key_down: Callback<Event<KeyboardEventData>, bool>`).
- **One handler per underlying event name.** A second registration silently **replaces** the first,
  and the sugar family shares names with the primitives: `on_secondary_down` is sugar over
  `on_pointer_down` (fork `freya-core/src/elements/extensions.rs`), so chaining it onto a node that
  already has `.on_pointer_down(..)` kills the first handler. Before adding any `on_*`, check which
  event name it registers under; if the node already handles that name, branch inside the one
  handler (match `e.data().button()` for right-click). Diagnostic fingerprint of a replaced
  handler: sibling events (hover) still fire, the press reaches ancestors, the node's own handler
  is dead.
- **A border is painted, never laid out — a bordered box whose children have backgrounds needs
  padding equal to the stroke.** torin has no notion of `border` at all (`BorderAlignment` exists
  only in `style/border.rs` and `elements/rect.rs`), so the default `Inner` alignment draws the
  stroke *inside* bounds the children already occupy, and children paint after the parent's
  background and border. A child at `width(fill)` with its own background therefore erases the
  border behind it. This is **not** CSS's border box, and the failure is partial and so reads as a
  rendering bug rather than a layout one: the export window's transfer panes kept their outline
  around the body (a wrapper with no background) and lost it across the header strip (which has
  one). Pad the bordered rect by the stroke width and subtract it from any child sized from the
  outer edge. Reach for `BorderAlignment::Outer` only when the box may genuinely overflow its
  slot.
- **A size lands on the node the parent lays out — a component that wraps its control must size
  the wrapper, not the control inside it.** A relative size is resolved *against a parent*, so a
  `flex(1.)` on a grandchild is not a flex child of the row at all: the row divides nothing, the
  wrapper hugs whatever the inner node resolved to, and the fixed sibling beside it is pushed off
  the surface. `ValueField` sized only its `Input` and not the `InputTypography` rect around it,
  which is invisible for a `px` width and broke the first row that put a browse button next to a
  field. So a component whose render adds a wrapper takes the caller's width on the **outer** node
  and fills the inner one. The tell is that a fixed width works and a relative one doesn't — that
  is the wrapper hugging, not a layout engine bug.
- **`Size::flex` is only divided by a parent whose `content` is `Flex`.** Without it the child
  takes a width of its own instead of the leftover, and a spacer meant to push a trailing item to
  the far end pushes it clean **off** the edge — which reads as an overflow bug rather than a
  missing property. The header bar's cluster is the worked example, with both lines. If a "push
  this to the right" spacer misbehaves, check `.content(Content::Flex)` on the row before
  anything else.
- **A focused `Input` owns the keyboard, so a surface built around one handles its keys in
  `on_pre_key_down`.** Freya's `Input` `stop_propagation`s **and** `prevent_default`s every key but
  Enter/Escape/Tab, and `prevent_default` on `KeyDown` cancels the derived `GlobalKeyDown`
  (`events/name.rs`) — so with the field focused, a global listener sees *nothing*. That is not an
  obstacle to route around: it is what makes a search-field surface a genuine modal barrier, which
  `Dialog`'s own docs note it is not (nothing moves focus into a dialog's card). What it costs is
  that every key the surface itself needs — ↑↓, Enter, Esc, and the chord that closes it — must be
  taken in the field's pre-handler, before the field processes them, and the chord must be resolved
  through `keymap::resolve` rather than matched literally. Returning `false` there means "the field
  does nothing further"; `true` lets the keystroke through as text. Keep the surface's own
  `GlobalKeyDown` barrier as well, for when focus is elsewhere, and put it on a **different node**
  from the one carrying the open chord — an element holds one handler per event name.
- **A disabled control gates its handlers; it does not go `interactive(false)`.** Wrap only the
  action handlers in `.maybe(enabled, …)` and leave `on_pointer_enter` / `on_pointer_leave`
  registered unconditionally, then decline to *dress* the hover while disabled — that is what
  Freya's own components do (`Switch`, `Card`). `interactive(false)` suppresses **every** pointer
  event including `pointer_leave`, so a node disabled while hovered keeps `hovering == true`
  forever and paints a stale hover the moment it is enabled again. Reach for it only for a
  genuinely pass-through overlay, which is the fork's own only use of it (tooltip, drag ghost,
  docking). Clearing the stuck flag in an effect afterwards is treating the symptom.
- **A settings-style surface is built from `components::form`, never from its own rows.** The
  export window, the config modal and the Settings panes are one surface drawn three times, and
  they kept arriving one at a time and re-typing each other's label metrics, field boxes and
  gaps. One module holds the whole vocabulary under one `form` component theme, composed the
  way a form actually nests: **`Form` > `Row` > control**, where the control is the row's
  *child*, so a row wraps a field, a `Switch`, a pill or a `Note` without knowing which. **One
  `Row`, never one per window**: the three presentation choices a row makes (how the label is
  set, how its explanation is shown, how rows are separated) always move together, so they are
  a `Variant` on the *form*, provided through context — `Fields` (eyebrow + ⓘ) or `Preferences`
  (title + inline subtext + rules), the split the design's *Settings consistency pass* settled.
  A second row type named for the window that first needed it is the failure mode here, not the
  fix.
  And **where the canvases genuinely differ, name the difference in `form/mod.rs`'s "known
  divergences" rather than averaging it**: a silent split-the-difference is how a surface stops
  matching the canvas it was drawn from, and a named one is a single constant to change when the
  design settles it.
  A row can also be **addressed**: `Row::anchor` names it and `form::reveal` carries the ask, so
  something outside the form (the Settings search) can have it scroll itself into view and flash
  once. That lives on the row rather than in the window that needed it first, for the reason above —
  a "jumpable settings row" would be a second row type — and it is two contexts because they have
  two lifetimes: `Reveal` is window-lived (it is written *before* the page holding the target has
  mounted, so a call into the row is impossible and a slot is the only shape that works) and
  `RevealScroll` is page-lived, since the page owns the `ScrollView`. Both optional, so a form with
  neither is a form of ordinary rows.
- **A field backing a draft publishes on every keystroke, and normalizes its box when it is
  left.** Freya's `Input` has no blur prop and only fires `on_submit` on Enter, so the tempting
  shape is "parse and publish when the field is left". It loses the value: the thing that commits
  a draft is a `Button`, and `Button` calls `a11y_id.request_focus()` and its `on_press` handler
  *in the same breath* — a focus-loss effect hasn't run when Apply reads the draft. So report per
  keystroke. But that leaves the box free to show something the caller never received (`abc`, an
  empty box, `9999` past the max), so **losing focus is when the text is re-echoed** — from what
  the field last *reported*, not by re-reading the parent, which keeps the field's one direction
  of travel. Watching for that means owning the `AccessibilityId` and calling `use_focus(id)`;
  both halves live in the shared `components::value_field::NumberField`, so a surface with a
  numeric setting reaches for that and writes neither. The comparison that decides "did this
  change?" belongs in **state**, never captured: `use_side_effect` builds its closure once
  (`use_hook`), so a captured value freezes at the first render and the field can never be typed
  back to where it started — and a plainly captured `EventHandler` (an `Rc<RefCell<dyn FnMut>>`
  snapshot) freezes the same way. Reactive values need `use_reactive`.
- **A built-in control's press reaches its ancestors, so never wrap one in a pressable parent.**
  `Switch`'s `on_press` does not `stop_propagation`, so a "click the whole row to toggle" ancestor
  takes the same click and toggles **twice** — back to where it started, which reads as a dead
  control. Make the row's label block a *sibling* of the control instead (Settings ▸ Appearance's
  Sync-with-OS row): the label takes the press, and the control keeps its own focus and keyboard
  operation. Check the component's source before assuming it consumes its press.
- **Pointer events carry NO modifiers.** `MouseEventData` is location + button only. Track
  shift/⌘/ctrl via `on_global_key_down`/`on_global_key_up` into shared state — and beware desync (a
  keyup lost while the window is unfocused leaves a modifier stuck). Reset defensively.
- **`stop_propagation` vs `prevent_default`**: `prevent_default()` in `on_pointer_down` suppresses
  the follow-up `on_press`/`on_global_pointer_press`. If a handler calls `prevent_default`, do
  double-click/press detection *inside* that same handler
  (`EventsCombos::pressed(loc).is_double()`), not via `on_press`.
- **`VirtualScrollView` memoizes its builder closure**, so snapshots captured in the closure go
  stale. Each child reads shared state reactively (`state.read()`) and computes its own view.
- **Reactivity**: `state()`/`.read()` subscribe (re-render on change); `.peek()` does not (use in
  event handlers/actions); `.set()`/`.write()` need `let mut`.
- **Logical units everywhere.** `on_sized` areas, authored offsets/positions/margins, and (since
  our fork fix) `Platform.root_size` are all logical. Never multiply/divide by the scale factor in
  component code — unit mixing here produced dropdowns that were only wrong on retina, and the
  wrong "fix" (dividing measured areas) halves correct values.
- **Naming**: plain nouns for structs (`CloseConfirm`, `Workbench`) — no role suffixes (`…Ui`,
  `…Manager`). DI handles end in `Ctx` (`EngineCtx`, `ThemesCtx`).
- **User-facing text reads like a standard IDE**, matching DataFusion's/JetBrains' register: terse
  plain sentences, single-quoted identifiers, no em-dashes/backticks/ellipsis/glyphs, no
  conversational hedges. ("Table or view 'nope' not found", "CREATE TABLE is not supported in the
  editor. Register tables in Table Config".) Merge or drop near-duplicate messages rather than
  stacking them.

## State: where things live

The decision procedure (full design: `docs/FREYA_STATE_ARCHITECTURE.md`):

- **State owned by one tab** → a field on `QueryTab` in the session store, under its **own granular
  `Chan` variant per concern**. Channel granularity is the leak-prevention mechanism: `request` sits
  on `Chan::Request(id)`, split from `Chan::Tab(id)`, so keystrokes never wake the results pane and
  one tab's press/cancel never touches another tab's results.
- **Shared reactive state with a small, known, shallow consumer set** → **struct-field props**
  (`State<T>` is `Copy` + `PartialEq`), e.g. the workbench's `running` mirror.
- **Context** (`use_provide_context`/`use_consume`) is reserved for DI handles (`EngineCtx`, theme)
  and deep/open-ended consumer trees (`Selection` across the datagrid layers).
- **A second surface that needs a settled query's outcome subscribes the query again** — same
  capability, same keys, same `stale_time`, which *is* a freya-query cache entry's identity — never
  a mirror of the result on a store or a prop threaded across the tree. A settled entry with
  `stale_time(MAX)` is never stale so it can't re-execute, and an execution in flight is attached
  to (our fork counts them). The Problems drawer reads a run's error exactly this way, off the same
  entry the results pane renders. Caveat: `enabled` is part of that identity, so `.enable(false)`
  reads a *different*, never-running entry — there is no "watch without running", and a surface
  that only sometimes has a key mounts its subscriber in a child that only exists when it does.
- **Never a shared map/registry value** (`State<HashMap<TabId, …>>`, a context registry) that
  threads every tab's data through one value into every consumer — that's the rejected
  "runs-by-id store" in every disguise.
- **Inside the fork**, `thread_local!` for shared component state is an antipattern. Use the
  lazily-initialized root-context pattern (`try_consume_root_context::<T>()` → on miss
  `provide_root_context`), as `Http` and `ContextMenu` do, or `State::create_global` for app-level
  multi-window state.

