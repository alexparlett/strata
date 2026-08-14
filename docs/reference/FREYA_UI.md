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
  The sidebar's data-sources tree (DB-05) takes the same answer at the row level: it is the fork's
  `TreeItem` / `Disclosure` / `TreeConfig`, themed by the `catalog` component theme and given the
  app's own chevron through `TreeItem::arrow`, with the one gap it had — a pressable row's `Link`
  role, tab stop and focus ring, which `SideBarItem` already carried — fixed in the fork rather
  than around it. What it does **not** use is the `Tree` wrapper, and that is a scope call rather
  than a preference: `Tree` is `VirtualScrollView` over a flat list of visible rows, so it needs
  the row count up front, and this tree's rows fetch as they open (a status glyph subscribes, a
  scan dispatches, a remote relation introspects). Answering the count would mean mirroring those
  query results into a pane-local map, which is the one thing the state rules forbid — so the rows
  compose as nested components under the pane's own `ScrollView`, and `Tree` stays where its
  contract fits (the results record view).

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
- **Every padding, gap and corner radius comes off the scale in `components::metrics`, and the
  three exceptions say so at the site.** The design fixes a nine-step spacing scale on a 4px grid
  and a five-step radius scale (`Design.dc.html` §03); `metrics` is that scale as constants,
  named for the design's own tokens (`SP_1`…`SP_9`, `R_XS`…`R_4`) so a call site picks a step
  rather than a number this repo invented. They are **not** theme fields on purpose: a step does
  not vary by theme, and a theme author who could retune one could reflow every surface from a
  JSON file — the theme layer is colour and typography, and `theme/components.rs` reads the scale
  like any other consumer. A surface that wants to name its use of a step does it locally
  (`const CELL_INSET: f32 = SP_4;`) — that is the application, not a second scale, and it is why
  three panes each having a `PAD` is fine while three panes each having a `12.` is not. The
  exceptions are **pills and circles** (the design's `999px` / `50%`, which a px radius states as
  `metrics::pill(extent)`), **hairlines** (`HAIRLINE` — a 1px rule is a stroke that occupies a row,
  not the smallest gap; snapping it would double every rule in the app) and **alignment nudges**
  (a 1px optical lift, a rail's centring arithmetic), plus the one whole surface off the scale,
  the Settings theme preview's miniature, which is a drawing of a window rather than one. Beneath
  the scale the same module holds the **fixed sizes more than one surface agrees on** — a toolbar
  button, a title bar, a panel header — because a constant scoped to one surface is one every
  other consumer reaches *into* that surface for: this landed with four separate 26px title-bar
  buttons, two `HEADER_HEIGHT`s with different values, and three docs each claiming to match the
  other two. Rehoming does **not** renumber: where two copies genuinely disagreed the values stay
  as distinct named constants and the canvas call is P5-05's.
- **A surface with its own component theme reads colours from that theme, not also from the
  roles.** Once a component has a `define_theme!`, every colour it paints — surfaces, borders,
  hairlines, tints — is one of its own fields, mapped onto a role in the static table
  (`theme/components.rs`). `use_roles()` is reached directly only where no component theme
  covers the surface, and the four **semantic** tones (`success` / `warning` / `error` /
  `info` — the status bar's state dot) only through the shared `tones()` hook, because those
  must follow the app-wide ramp wherever they appear. Mixing the two sources in one component
  is how `roles.get(Role::Border)` ends up beside a `border_fill` that already holds the same
  value.
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
- **A panel has no usability floor, only a stub floor — and a chrome row folds rather than
  spilling.** The design canvas has no answer here: `Strata.dc.html` declares `min-width: 1180px`
  on the app root and scrolls the page below it, so every narrow state had to be designed. The
  reference is RustRover, and it is the opposite of a minimum — JetBrains state that *"it is not
  possible to enforce minimal tool window size"*, and a window squeezed to ~680px keeps both tool
  windows open while the editor between them is a ~45px stub. So: a floor exists only so a panel
  cannot become a sliver too thin to grab; **space is given up in a stated order** (the
  proportional main pane first and entirely, then the pixel side panels in equal measure, which is
  the sizing model rather than a policy anyone writes down); **pressure never collapses a panel,
  only a drag does**; and a chrome row shrinks its flexible run, then folds its actions into one
  `⋯` menu, then drops them. The fold is `components::toolbar`, one policy for every row rather
  than a breakpoint per surface — because with no floors there is no row for which "it always
  fits" can be argued. Its arithmetic is over the item list, so adding a button moves the fold
  point with nothing restated, and **an item is declared once**: it knows its width, its inline
  form and its menu-row form, so the overflow menu is a second *rendering* rather than a second
  copy. The measured width is local, per-mount state — a fold verdict is derived, like the theme,
  and `Chan::LayoutSize` has no subscribers by design anyway. Two traps generalise. `Overflow`
  has **no `Scroll` variant** and defaults to `None`, which lets children paint *outside* their
  box, so a `main_align(SpaceBetween)` header over the default `Content::Normal` draws its two
  clusters straight through each other once it narrows — the fix is `Content::Flex` plus a
  flexing, ellipsizing leading run, never a clip. And a panel's `min_size` was only ever a *drag*
  clamp: it never reached the layout node, so a shrinking container measured flex panels toward
  zero and past it (torin applied a negative `flex_available_width` with no clamp) — both were
  fork fixes, and the negative one is the single most direct cause of overlapping content.
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
- **A completion is an edit at the caret: replace the token's span, then put the caret after what
  was inserted.** One rule, and both completing surfaces are held to it — the SQL editor through
  `CodeEditorData::replace_range` (one undo step, caret at the end of the insert, sealed on both
  sides), and the chat composer's `@`-mentions through `mention::complete` over the fork's
  `Input::caret`, a two-way binding in UTF-16 code units. The mention list was built first without
  one, because `Input` published no caret: the token was defined as the run from the last `@` to
  the **end of the buffer**, which reads as working until a mention is typed mid-sentence and the
  sentence carries on past it (`@F fake me` offered nothing, and a whole-buffer rewrite left the
  caret inside the name it had just spliced in). Do not answer that with a policy about where the
  caret goes on an external write — a caret policy is not a completion mechanism, and shipping one
  beside `replace_range` would be two answers to one question. Give the control a caret and edit
  the span. Convert bytes ↔ UTF-16 **once**, at the `Input` boundary (`mention::byte_of` /
  `utf16_of`), so nothing above it counts in two units at the same time.
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

  **The handler halves were still captured until the pre-release review**, in the shared fields
  themselves: `NumberField`, `PathField`, `FieldControl` and `ValueField`'s `max_len`. Each got
  away with it for a different accidental reason — the current callers' handlers close over `Copy`
  context handles, and `FieldControl` is keyed by group label so a format switch remounts it — and
  none of those reasons is a property of the components. The first caller whose handler closed over
  a row id, an index or a cloned draft would have got silently stale calls with no diagnostic at
  all. Shared machinery takes the rule rather than the luck; the same review found the live
  instance of it in the chat composer's `ModelPicker`, where an un-keyed component froze its
  provider at mount and never refreshed the model list after a repick.
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
- **Two siblings on the same layer have no paint order — set a layer, don't rely on document
  order.** `RenderPipeline` walks `tree.layers` in sorted key order, and a layer's nodes are an
  `FxHashSet`: **unordered**. So an element that must paint over a sibling has to say so, and
  "it is declared second" is not saying so — it works until the hash order changes. The symptom
  is distinctive and misleading: the covered element appears to have **alpha**, because all you
  see of it is what shows through the semi-transparent parts of whatever painted on top. Use
  `Layer::Relative(1)` for "in front of my siblings"; `Layer::Overlay` is a big jump for content
  that must clear the whole window (a dialog, a menu), and reaching for it to fix a local
  ordering problem puts a tooltip over your modals. The chart's hover readout is the case
  (`results/chart/paint.rs`).
- **A `canvas` paints from a slot, and repaints only when asked.** `RenderCallback`'s `PartialEq`
  is **always true**, so `CanvasElement::diff` never sees a new closure as a change and the
  callback stored in the tree stays the one from the first render — the same staleness
  `VirtualScrollView`'s builder has, but silent, because the element is still on screen and still
  painting. A callback that captured its data by value therefore paints the first frame forever.
  Put the frame in a `State` the callback **peeks**, and have the side effect that fills it also
  call `Platform::get().send(UserEvent::RequestRedraw)` — nothing else schedules a paint (this is
  `examples/feature_plot_3d.rs`'s idiom; a resize repaints the tree anyway). Build that effect
  with `use_side_effect_with_deps`, not `use_side_effect`, for the reason below: a plain effect's
  closure is built once and would capture the first frame too. `CanvasContext::size` is logical
  and the canvas is pre-scaled, so everything drawn inside is in logical units. First used by the
  results Chart body (`results/chart/paint.rs`).
- **A task spawned from a handler belongs to the scope that pressed it, so a press that unmounts
  its own control cancels its own work.** `spawn` binds `scope_id: current_scope_id()`, which
  during dispatch is the scope owning the handler's *element*
  (`freya-core/src/lifecycle/task.rs`); dropping that scope removes the task **before its first
  poll**, with no error and no log — the handler ran, and nothing happened. Three presses in AS-07
  hit it, each looking perfectly ordinary: a menu item that closes its menu (Export chat silently
  did nothing), a dialog button whose press clears the slot that mounts it (Delete and Clear left
  the file on disk), and the composer's Stop, which flips the control back to Send and so unmounts
  itself. The tell is that the press *changes state that removes the control it is on* — which is
  most confirm and menu presses.

  **`spawn_forever` is not the escape.** It is root-scoped, so it outlives the project subtree
  whose `State` the task writes after an await, and `State::write` panics on a `GenerationalBox`
  whose owning scope is gone (`freya-core/src/lifecycle/state.rs`) — a re-root or engine restart
  racing the task is a real crash. Root-scoping is right only for work that genuinely must outlive
  the pane, and then the task has to stay cancellable by something the subtree still holds (AS-02's
  turn task keeps `Chat::running` until its record is written, which is what `Chats::stop_all`
  reaches it by).

  The shape that works: **the press records the intent in a `State`, and a `use_side_effect` in a
  component scope that outlives the control performs it.** The intent must ride *in* that state and
  never be captured — `use_side_effect` builds its closure **once** (`use_hook(|| Effect::create(..)
  )`) and re-runs it when a state it `read()`s changes, so a value cloned out of the render freezes
  at the first render's. That is not a theoretical trap: the first fix for the confirm captured its
  target while it was still `None`, which made Delete and Clear do nothing at all — the same
  silence the bug it was fixing produced. `use_side_effect_with_deps` / `use_reactive` are the
  other way to say it when the value is a prop.
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

