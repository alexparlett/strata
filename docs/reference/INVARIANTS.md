# Architecture invariants

Things that must not regress. Each was fought for once already, most after a wrong version was
built and rejected in review — so the reasoning is kept wherever the reasoning **is** the rule.
[AGENTS.md](../../AGENTS.md) §2 carries the one-line form of every rule here and links back to
this file; read the full entry before extending, arguing with, or overturning one.

Things that must not regress. Each was fought for once already.

- **The engine is a direct-call async facade** (`strata_core::engine::Engine`): private multi-thread
  Tokio runtime, each call spawned onto it, caller awaits the `JoinHandle`. No UI-side runtime, no
  channels, no request ids, no router/demux — the Dioxus-era `Command`/`Event` protocol was deleted
  with P2-01 and must not be rebuilt. DataFusion is touched **only** in `strata-core`.
- **Results are freya-query off the tab's SQL.** Each `QueryTab` owns its Run trigger
  (`QueryTab::request: Option<QuerySpec>` on `Chan::Request(id)`). The store holds **specs, never
  results** — rows live only in the freya-query cache keyed by `QuerySpec`. No runs-by-id store.
  Two lifetime rules keep that cache honest: a Run subscription is built **only** through
  `QuerySpec::query` (a `Query`'s settings are cache identity — a hand-built variant is a
  different entry, i.e. a duplicate execution), and cache-entry lifetime is **subscriber
  presence**, held for background tabs by the window's request keepers (`views::keeper`, mounted at ProjectRoot —
  one invisible pin per open tab's current press, which also owns history recording). Never
  manage entry lifetime imperatively; mount or unmount a subscriber. Fork-side, freya-query
  never cleans an entry whose execution is in flight and never cancels one on unmount — a
  remounting subscriber attaches to it (`RunningGuard`).
- **Diagnostics are a reconciliation, not an event.** Every open tab's diagnostics are a pure
  function of two inputs — its buffer revision and the catalog epoch — and each tab records a
  `Stamp` of the pair its current rows describe. `SessionState::stale_tabs` is the whole work
  list, and the window's **one** driver (`state::diagnostics::use_diagnostics`, a hook in the
  window root) drains it. Never add a second producer, and never enumerate entry points: a tab
  restored at open, reopened, opened from a view or saved query, duplicated, edited, or left
  behind by a pass a tab switch cancelled are all the same thing — the stamp does not match. It
  is one hook rather than a component per tab because `Chan::Text` is a fan-in (like
  `Chan::Persist`) that lets one subscription watch every tab's buffer. **The catalog is a gate,
  not just an input**: `Engine::register` deregisters before it re-infers, so nothing validates
  mid-scan and no false "not found" is ever produced.
- **A log is recorded by its observer; there is no producer to register with.** The event log
  (`state::log`, the drawer's Events tab) is the mirror image of the rule above, and the contrast
  is the reasoning: a diagnostic is a pure function of two live inputs, so one driver can
  re-derive it and no entry point needs enumerating — an **event** can be re-derived from nothing,
  because it describes something already finished that may no longer exist to be re-read. So
  whichever layer watched the fact records it (the scan pass per def, Save and the drop confirm
  per mutation, the request keeper per settle, `cancel_run` per cancel), by capturing the `LogCtx`
  at render time and calling `log_event`. Never add a producer hook, never re-derive an event from
  live state, and never let a *log* entry be the only copy of a live fact — a registration failure
  belongs on its catalog row, a run failure in the run's own query entry. Two corollaries that
  cost time to rediscover: a cancel is logged at the **cancel**, since clearing the tab's trigger
  unmounts the press's keeper in the same pass and the `Err("cancelled")` settle lands
  unsubscribed; and an entry carries a **level** (the sheet's four semantic slots) but no
  `origin`, because the message already names its subject and a structured copy of that is a
  second copy that can disagree with it.
- **A stopped run is not a failed one, and `engine::stopped_on_purpose` is the only thing that
  knows which is which.** The engine settles **three** such strings, not one — `cancelled` (an
  abort), `superseded by a newer run` (a press that finished after a newer one replaced it) and
  `superseded by a newer scan` (the profile equivalent) — each behind a named const beside the code
  that produces it. Never string-match the engine's prose at a call site: the event log tested
  `e == "cancelled"` and so logged a *supersede* as a red error reading "superseded by a newer run",
  while the inspector's scan zone kept a second copy of the rule (`== "cancelled" ||
  starts_with("superseded")`) that happened to be right. Two copies, one already drifted; both now
  call the predicate. A surface showing a settled `Err` must map every one of them to something the
  user reads as "you stopped this", never as a fault — and none of them may reach Problems.
- **Problems holds *conditions*, at two scopes; a run failure is the results pane's.** The test
  for admission is not "is it about SQL" but **"is it true right now, and does it retract itself
  when it stops being true"** — which is why the drawer's header carries a scope strip
  (`Queries` · `Project`, P4-15) rather than one list. Queries holds the SQL diagnostics; Project
  holds defs the engine refused and `.strata` files a failed write left behind. Three kinds of
  state sort themselves on that test: a **reconciliation** is re-derived from live inputs (a
  diagnostic, a `Reg::Failed` row), a **remembered condition** cannot be re-derived but still
  retracts (a write fault — an observer records it, a later successful write clears it), and an
  **event** describes something already finished and belongs in the log, not here.
  A **run failure** fails that test and stays the results pane's: it describes SQL the buffer no
  longer holds, it can't self-clear by typing, and `cancel`/supersede settle
  `Err("cancelled")`/`Err("superseded")` that no user should ever read as a problem. Putting it
  in a cross-tab view costs either a copy on the store that outlives the run, or one freya-query
  subscription per tab in the drawer *and* in the rail badge. The results pane renders it in full.
  Two corollaries the split makes load-bearing: the rail badge and the header must total **every**
  scope from the same functions the scopes use, or the badge goes quiet while the project under it
  is broken; and a repeating writer must record its **transition** as the event and hold the rest
  as the condition — both in the log and in the store, since re-recording an identical fault wakes
  every subscriber as surely as re-logging it buries every other row.
- **JSON is read by our own `FileFormat`, and a replaced reader inherits the replaced reader's
  diagnostics.** `engine::json_poly` is now the *only* JSON reader — arrow's `JsonFormat` is not
  constructed anywhere. It exists because arrow's inference admits five type combinations and
  errors on every other pair, so a type-discriminated union fails registration outright; ours
  stringifies **only** the paths arrow would have rejected and infers everything else identically
  (asserted against arrow's own inference, not argued). Three things generalise. A reader swap is
  also a **diagnostics** swap: `catalog::json_shape_error` keys off arrow's `Json error: ` prefix
  and its exact `Expected JSON record to be an object, found Array` wording, so ours speaks that
  dialect deliberately — replacing a reader must not quietly replace the message the user reads.
  A `FileSource` **must** handle its own projection: leaving `projection()` at its `None` default
  does not mean "plan a projection above the scan", it means `FileScanConfigBuilder::build` fails
  the plan with `does not support projection pushdown`. And each normalization rule was found by
  running the real file, not by reading the spec — arrow can infer a schema its own decoder then
  refuses to read (a scalar promoted into a list), which no amount of design review surfaced.
- **The snapshot is Arrow IPC, so a result's type survives it.** Every run materializes to a
  snapshot before the grid sees a row, which makes the snapshot's format a constraint on the whole
  type system. Parquet was that format and is narrower than Arrow: it cannot write a union at all
  (`arrow_to_parquet_schema` **panics**, ARROW-8817) nor a zero-field struct, so results were
  coerced on the way in — and the record view, `cell_preview_json` and JSON/CSV export all read the
  *re-read* snapshot, so they read the coerced form, not what the query produced. Each new exotic
  type meant another arm in a gate that was twice found incomplete. IPC round-trips anything the
  engine emits, and **compressed it is the same size** (measured over 1M–20M rows in three shapes:
  raw IPC is 1.4–4.4x parquet, LZ4 IPC is 0.46–0.73x — i.e. half of the *uncompressed* parquet it
  replaced, which is what our snapshots were). The one thing parquet's footer gave us was exact
  null counts for the partitioned-export gate; those are now counted during the write pass
  (`query::SnapshotStats`) because `materialize` already streams every batch and
  `Array::null_count` is a stored field — free to produce, and held in `Lifecycle` for exactly the
  snapshot's lifetime rather than in a footer or a sidecar. What remains of the old gate is
  `json_unions_as_text`, which is now **presentation, not storage**: `json_get`'s union renders as
  `{str=x}` and nobody typing `content -> 'type'` wants to read that.
- **A chart renders the result in result order; it computes nothing SQL can say.** (Rz2 — the
  renderer-first design, `docs/CHART_SPEC.md`; lands with the workstream re-cut.) `Engine::chart`
  is a projected, ordinal-ordered, capped read of `__snap_{id}` plus a long→wide pivot: columns
  map onto marks, multiple Y columns are multiple series, a series column pivots — and the pivot
  is the only operation that can conflate rows, so it is the only thing that refuses on
  duplicates (`ChartData::Duplicates`, CTA into the SQL scaffold). Over a cap it answers
  `ChartData::OverCap`, which carries no data at all — a truncated chart is not a state that can
  exist. The histogram's binning is the one engine computation (no `width_bucket` in DataFusion
  54). What was **built and withdrawn** is the first design's engine-side aggregation pipeline —
  `AggFn`/`Bucket`/`Stride`, auto-stride resolution, engine-imposed category order. Withdrawn on
  two grounds, both evidenced: every hard defect of two adversarial reviews clustered in that
  machinery (NaN comparator panics, `date_bin` overflow, cap arithmetic, bin-key collapse), and
  its ordering fought the user's own `ORDER BY` — a `GROUP BY` has no output order, so
  re-aggregating an already-shaped result destroys the order the user asked for, and the
  measure-descending rule this entry used to state was a workaround for that self-inflicted loss.
  Two measurements stay recorded because they motivate the ordinal (next entry): `min(row_number()
  OVER ())` follows file order only below `repartition_file_min_size` (10 MB) — a 200k-row
  snapshot perfect, a 3M-row one 2 975 424 of 3 000 000 rows out of order — and a chart that
  re-aggregates cannot preserve order even over an ordered read. Do not resurrect either idea
  without new evidence.

  Two facts the **renderer** settled (Rz2/02), both about identity rather than drawing:

  - **A column's chart role comes from the Arrow `DataType`, resolved in `column_info`.**
    `ColumnInfo` carries `role: ChartRole` beside `kind`, and `engine::catalog::chart_role`
    matches the type itself — its measure arm **is** `DataType::is_numeric`, the same predicate
    `engine::chart`'s read gates a Y on, so an encoder cannot offer a measure the read would then
    refuse. Neither of the two things already in hand is the source: a type's *spelling* is a
    rendering of a type (and `short_type` folds `LargeUtf8` into `Utf8`, every list into `List`),
    and `Kind` is the **display** taxonomy — deliberately coarser, and it reads a union as a
    string, which is harmless for a swatch and wrong for an axis. Every `ColumnInfo` fixture in
    the workspace is built through `column_info` for the same reason: a hand-written row states
    the type three times and lets the three disagree.
  - **A chart read's cache identity is `(snapshot, query, display config)`.** Axis labels render
    through `CellFormat` — the engine's live `datafusion.format.*` — which `set_config` changes
    with **no restart and no new snapshot**, so an entry keyed on the pair alone serves labels
    rendered under a format the user has since changed. `ChartSpec` carries
    `config::display_subset` of the app's engine overrides, which makes a format change a new
    entry rather than a stale one and keeps `stale_time(MAX)` honest. It is read from the **app
    config** (the store `use_engine_config` drives the engine from), not from the engine's own
    copy: that is the reactive source, and Freya's runner drains a write's dirty scopes before it
    polls the tasks queued alongside them, so the driver's `set_config` has landed by the time the
    capability runs.

  And two the **encoder** settled (Rz2/03), both about the config being intent rather than a
  resolved read:

  - **A chart config is intent; resolving it against the result is a read-time fallback, never a
    write.** `ChartConfig` (on `QueryTab`, `Chan::Chart(tab)`, persisted via `TabSnapshot`) says
    what the user chose and *whether* they chose: `mark`/`ys` are `Option`, and X is a three-state
    `ChartX` because "not chosen" and "chosen to be the row index" are different answers — an
    `Option<String>` would let the next result's date column overrule a deliberate row-index axis.
    `config::resolve` merges the schema's defaults **under** the choices and drops any reference
    this result cannot answer; nothing writes that fallback back, so a column that disappears from
    one result and returns in the next brings the user's choice back with it. The same rule is why
    a mark that draws one Y (pie, scatter, histogram) narrows the *encoding* and leaves the config
    holding all four — flipping to a pie and back costs nothing. What each control offers is
    `config`'s per-mark option sets (spec §4 as functions), so an invalid encoding is unreachable
    rather than reported: the engine's own three refusals around a series column (needs an X, not
    the X, a category) are option-set arithmetic, and `encode` stays the one `ChartQuery`
    construction site.
  - **The chart's sort is a view transform over the settled data, and its comparison is total in
    both directions.** `ChartSort` never reaches `ChartQuery`, so flipping it permutes what is already
    in hand — no re-read, no change to cache identity — and it is offered only for the marks whose
    data has an order to permute (points are documented unordered, bins ascending). The
    comparator takes a `descending` flag rather than being reversed at the call site: reversing it
    reverses where the **gaps** go, which put every NULL and NaN at the head of a value-descending
    chart the first time it was written. A gap is not a small value; `total_cmp` and a stated place
    for the missing ones are what keep the withdrawn pipeline's `sort_by` panic from coming back.
- **A snapshot read has no order of its own; order is the ordinal column.** (`SNAPSHOT_SPEC.md`
  §9; lands with the workstream re-cut.) Above 10 MB an Arrow File scan range-splits and a bare
  `LIMIT/OFFSET` read sits over a `CoalescePartitionsExec` — measured: at 3M rows the *same page
  re-read returns different rows* (page 1 arrived starting at row 1 843 201 on one read and 101
  on the next), and a 200k-row snapshot with a text column pages stably but starting at row
  57 345, so `fetch_page`'s pages disagree with the spooled page 1 — rows duplicated and missing
  as the user pages, and the page cache freezes whichever answer a read happened to get. The fix
  is written order: `materialize` adds `row_number() OVER ()` to the spool query itself (the
  column is a **UInt64, 1-based** — nothing reads its values, only their order), aliased to
  `__strata_ord` after `QueryOutput::columns` is captured, name-escalated on collision and
  recorded in `SnapshotStats.ord: Option<String>`. Two plans spool **without** one, `None`, and
  read unordered as at base: an `EXPLAIN`/`EXPLAIN ANALYZE` (DataFusion requires those at the
  plan root, so the window would fail a statement the DDL policy promises to run) and a result
  with duplicate column names (name-keyed reads would mis-map a duplicate onto the ordinal's
  slot). The registration **declares** the file's order (`with_file_sort_order`), so an ordered
  read plans as a stream, not a sort — measured: a page at offset 2.9M of a 3M-row snapshot is
  543 ms as an undeclared TopK holding every candidate row, 97 ms declared, with shallow pages
  planning as scan-level limit pushdown and exports streaming into their `COPY`. Unsorted reads
  `ORDER BY` it; user sorts append it as the tie-break (stable across page windows); **every**
  reader projects it away, and export selects explicit columns so a `COPY` never writes
  bookkeeping into the user's file.
- **A view of a value is bounded where the value is *encoded*, never afterwards — and it expands
  breadth-first.** The record view opened on a `config.json` row (19 struct columns, 241,425 nested
  fields) froze the window for a second or two, and the freeze was the **materialization**, not the
  formatting: bounding the output of a whole-value serializer changes what is displayed and none of
  what is paid, so `cell_preview_json` (P2-24) walks the Arrow arrays and never builds the value at
  all. Narrowing a list to one index is an O(1) slice, so a container can be **measured without
  being read** — which is what makes a count (`[ … 5171 items … ]`) cheaper than a truncation.
  Four things generalise, and the first two were each got wrong once. **Collapse the value, not its
  parent** — a boundary that renders `"contentBlocks": { … 19311 keys … }` throws away the one level
  the reader is looking at, where IntelliJ shows every key at that level with each *value* collapsed
  (`"0004d823-…": { 2 keys },`). So a container's entries are **sampled** — the first N, then
  `… 19296 more keys` — and only what hangs below them is counted. **The depth is fixed and the
  budget is a backstop, never a target to fill**: searching for the deepest level that fits sounds
  thrifty and is exactly wrong, because on a wide document the deepest *uniform* level that fits is
  a narrow one, so the preview walks five levels down one branch and never shows the second key.
  Sample width decays with depth (wide at the top, `PREVIEW_ITEMS_MIN` at the bottom) so the budget
  buys breadth where a reader scans. And an **empty container is its own summary** — `{ … 0 keys … }`
  is a count of nothing standing where `{}` belongs, so the emptiness test comes before the depth
  test.
  **Reuse the encoder library's own per-index primitive for the leaves** (arrow-json's public
  `make_encoder` + `NullableEncoder::encode`) instead of re-rendering scalars by hand, or a preview
  and a copy of the same decimal eventually disagree. **Clip the one unbounded leaf before encoding
  it** — a 30MB string measured by encoding it costs exactly what the budget exists to avoid.
  And the shallowest render is a **floor that ignores the budget**, because a budget too small for
  the count marker should still leave something to read. Measured on the file that prompted it: all
  19 columns bounded is 1.6ms/22KB, the same row unbounded is 1.29s/128MB — the JSON of one row is
  twice the file, which is why bounding it *afterwards* was never going to work. The corollary on
  the UI side is the plain version of the same rule: a `render` body (or a press handler, which is
  the same thread) never serializes a whole value — it reads a cache keyed on what the value depends
  on, and **that cache is synchronous, not `use_memo`** (`Runner::handle_events` returns the moment
  a scope is dirty and only polls tasks once none is, so a `use_memo` paints its *previous* value
  for a frame — `record_view.rs`'s `PreviewMemo` and `find.rs`'s `PageMemo` are the same answer to
  the same question). And the leaf-nullity trap generalises past arrow: **a library's "is this
  null?" is the one the library's own writer asks**, not the one the data structure exposes — an
  all-null column infers as `DataType::Null`, whose nulls are *logical*, so `Array::is_null` says
  false for every index and `NullEncoder::encode` is `unreachable!()`. Found by running the real
  file, like every `json_poly` rule.
- **An inspector reads the Arrow arrays; only a *document excerpt* goes through text.** The nested-cell
  tree (P2-25, `engine::value_tree`) addresses a node by a path of **entry indices** — not names, so a
  duplicate or reordered key cannot mis-resolve, and a list has none — and resolves it with O(1) Arrow
  slices, so the last 30 of 19,311 siblings costs what the first 30 does (11µs vs 13µs). It emits no
  JSON, and not as an optimisation: a tree already carries the structure braces exist to express, so
  encoding to text is work done to be re-parsed by the eye, and it *loses* what the tree needs — a leaf
  arrives quoted and a node's type lives in the schema rather than anywhere in the JSON. Leaves go
  through the **same `ArrayFormatter` the grid formats a cell with** and the same `util::clip`, so one
  value cannot be described two ways. The record view's sampled preview stays JSON because it is a
  document *excerpt*, where the braces are the point. Two traps generalise. A window must carry
  **absolute** indices, or a path built from a second page addresses the wrong entry. And clipping is
  only a bound if it happens **before** materialization — `ArrayFormatter` renders through `Display`, so
  asking it for a 30MB leaf and clipping afterwards is the unbounded copy this design exists to remove,
  reintroduced one row at a time; read the `&str` off the array and clip the borrow.
- **A virtualized list scrolls its cross axis already; a row that `fill`s is what stops it.**
  `VirtualScrollView` offsets its content by the cross-axis scroll, measures that scrollbar from the
  content size, and applies X wheel delta — `ScrollView` uses the identical structure, and its own
  comment says the content box is *fill-sized to the viewport, its offset scrolls its children, not
  itself*. So a row sized `Size::fill()` clamps itself to the visible width, nothing ever exceeds the
  box, and there is nothing to overflow; a row that **hugs** is the whole fix. Three attempts to
  "add" the capability were wrong and each is a general trap: sizing the content rect explicitly makes
  `area == inner_sizes` so `is_scrollbar_visible` finds nothing — **you cannot widen the box whose
  narrowness is what scrolls**; `min_width(Size::percent(100.))` does not floor a hugged width in
  torin, it **adds** to it (a 900px row became 1400, a 50px row 550); and a `Fill` child resolves
  against the space *available to its parent*, never a hugged parent's final width, so "content hugs,
  rows fill it" is inexpressible. Verify a layout question with a `torin/tests` case before building
  on the answer — all three of those took one small test to settle and a build each to guess wrong.
  The accepted cost is that a hugging row's hover fill stops at its content, which is a **theme**
  decision at the consumer (Strata's `tree` sets those fills transparent), not a component change.
- **A recursive `Debug` is not a cheap way to get a type's name.** `catalog::short_type` was
  `format!("{dt:?}")` with the first word taken off the front; `DataType`'s `Debug` recurses, so on a
  19,311-key struct that one call rendered the entire subtree as text to discard nearly all of it —
  18ms, and `column_info` makes it per field all the way down, i.e. quadratic in the schema. Matching
  the composite variants by name took it to 3.8µs and ~19% off every query on that file. Leaf variants
  keep the generic path because their `Debug` is a single term.
- **An agent's tools are the app's own semantics, and the gate in front of them runs before
  dispatch.** `strata-agent` (AA-02) is the read-only vocabulary, frontend-agnostic by
  construction — no Freya crate in its graph, which is what lets one surface serve the MCP
  server, the headless host and, later, the chat pane rather than each re-implementing it.
  Four rules it holds, each the reason something above is not duplicated. **The policy gate
  is the editor's predicate, asked before the press**: `Engine::query` does not enforce the
  managed-DDL policy — the editor simply never dispatches what validation flagged, and an
  agent cannot be trusted with that discipline — so `run` asks `Engine::policy_verdicts` and
  refuses on any non-clean answer, *including* an unjudgeable one, so the gate fails closed.
  **`run` never rewrites SQL**: no injected `LIMIT`, because the press must materialize
  exactly what a person's would (same cost, same snapshot); the *response* is bounded by
  `page_size` and `read_page`, and the total stays exact. **A stop is a status, not an
  error** — `stopped_on_purpose` is asked through one predicate and never re-derived from the
  engine's prose, and its three strings become an outcome the agent reads as "you stopped this". And **"your result was
  replaced" is asked of the engine, never of its prose**: a retired snapshot answers
  `fetch_page` with DataFusion's own "table not found", so `Engine::snapshot_live` exists to
  be asked *after* the read fails (which is also what keeps it from racing the dispatch that
  retired it). The one thing `read_page` deliberately does **not** do is pin: a pin is right
  for an export window, which owes the user the rows it was opened on, and wrong for a
  long-lived server, where the honest answer is that the query session has moved on.
- **An agent drives the app through the app's own funnels, and works in a surface of its own;
  only a *gate* may be skipped, and only when the gate is a question for the user.** The in-app
  host (AA-03/AA-03b) is a `Host` impl over a cross-thread service directory each project window
  registers with, plus a serial driver on the window. An agent's run is dispatched **straight at
  the engine**, on its query session's own `WsId` — a real execution (same snapshot lifecycle,
  same supersede, same cancel) that touches nothing of the user's — and the window brackets it:
  `RunStarting` before (does this agent hold this session, and record what ran), `RunSettled`
  after. Bringing an agent's query into the user's editor is the *user's* gesture, a press on a
  run row through `actions::open_sql` — which opens a **new** tab, never the active one: the
  History drawer loads into the tab you are in because being there *is* the ask, while an
  agent's run arrives in a surface you were only looking at, and overwriting that buffer is the
  exact harm the pane exists to prevent. There is no double-press-to-run either; promoting puts
  a query where it can be read, and pressing Run is the user's next decision. The one thing
  skipped is the **T2 confirm**, because it asks the *user* whether to destroy work and neither
  answer serves a tool call. Four more rules the seam holds. A settle **names its run** by a
  sequence number the dispatch minted, never "the newest", or an agent that presses on before a
  slow query finishes has the older outcome stamped on the newer row. A registration is per
  **mount** of the project subtree and keyed by a minted id rather than by the project root,
  since an engine restart remounts at the same root. The **channels are two**, and the split is
  load-bearing: asks are bounded and awaited (honest backpressure for a tool call), notices are
  unbounded and one-way, because the most important of them is sent from a `Drop` — a connection
  ending, with nothing to await on and nowhere to report a failure to. And this directory is
  **not** the registry §4 rules out: that rule governs reactive UI state threaded through one
  value, and this is a DI seam between threads, where context cannot reach.
- **An agent that is not *in* the window does not write the window's state; it gets a surface of
  its own — and the scoping is a type, not a check.** AA-03 landed MCP runs in the user's own
  `QueryTab`s ("the investigation trail *is* the tab strip") and using it showed the premise was
  wrong for a frontend that lives in a terminal: the user is not watching, so an investigation
  moved the editor out from under them, left tabs they had to close, and cost a diagnostics pass
  per tab **on the engine their own press needed**. Worse, `list_tabs` handed an agent *every*
  open tab and a `run` on one replaced the buffer the user was typing in. AA-03b moves those runs
  to **query sessions** of the agent's own, shown in an Agents pane. The fix for the sharp edge
  is structural rather than a guard: `StrataTools` **is** one agent, and every session-scoped
  tool is scoped to that id, so an agent is never handed a handle on another agent's work, let
  alone on a tab. **Which agent, though, comes from the request and not from how long a value
  happens to live** (AA-03c) — see the entry below. A handle it does not own
  answers exactly as one that never existed, deliberately: a distinct "that is not yours" would
  confirm the session exists. The runs stay *real*, which is the half of the original decision
  that was right. The chat pane is the opposite case and keeps the tab gesture, because it is in
  the window and the user is looking at it. The general rule: **a surface's state belongs to
  whoever is looking at that surface**, and "shared, last-writer-wins" is a fine rule for
  *content* and a bad one for *attention*. Its corollary reaches the T2 confirm: the gate stays
  the engine's own answer over *both* sets of workspaces, and only the **sentence** changes —
  "Queries are running" shown to somebody who pressed Run on nothing sends them looking for a
  query they never started. Not confirming at all for agent-only work was considered and
  rejected: it costs the one property that makes the confirm trustworthy.
- **An agent's identity comes from the request, and a teardown that cannot happen yet is owed to
  whoever finishes last** (AA-03c). Two holes the AA-03b seam left, and they are one family: the
  seam identified things by something that was not their identity.

  *Identity.* rmcp 3.0.1 serves Streamable HTTP two ways. On the session lifecycle the service
  factory runs once per MCP session and the value it returns lives as long as the client does,
  so an `AgentId` minted in that value and retracted by its `Drop` is exactly right. On the
  **stateless** branch — taken by a client negotiating `2026-07-28` or sending per-request
  `_meta`, and gated by `use_session = legacy_session_mode && is_legacy_request(…)` —
  `get_service()` runs per *request* and the value dies with the response, so every request is a
  different agent: `open_query_session` mints a session and the next `run` answers `No open
  query session`, silently, for that client's whole life. Two remedies look obvious and are
  both wrong. **There is no stopgap in the config**: `legacy_session_mode` already defaults to
  `true` and can only turn sessions *off*, never force them on. And **keying on `Mcp-Session-Id`
  fixes only the path that already works**: SEP-2567 removes sessions from the discover
  lifecycle, and rmcp sets that header on exactly one response, inside the session branch — it
  is absent precisely where identity is broken. Worse, it is not even the discriminator:
  `is_legacy_request` reads the request's `_meta` and protocol version and never consults that
  header, so a client that still echoes a stale session id while sending per-request `_meta`
  takes the stateless branch and would be misread as owning its value — the bug, restored for
  the client most likely to hit it. So `tools::Caller` **mirrors rmcp's own predicate**: no HTTP
  `Parts` in the extensions at all means stdio or the in-process pane, where the value's
  lifetime genuinely *is* the connection; a request whose `_meta` lacks the `2026-07-28`
  required keys and whose version is older took the session branch, where it is too; anything
  else is stateless and falls back to the only durable thing such a client sends, its `_meta`
  `clientInfo`. That last is a real loss of resolution — two windows of one client share an
  agent — and it is the honest maximum, because the protocol carries nothing else. Read from
  `_meta` and **not** from the peer: rmcp reconstructs stateless `peer_info` with
  `Implementation::default()`, which is `from_build_env` and therefore reads `rmcp` and the rmcp
  version, so falling back to it labels every un-introduced client with the name of the MCP
  library it happens to use. `clientInfo` is *optional* on that lifecycle, and a **blank
  identity is refused the session-scoped tools rather than pooled**: one minted id behind two
  different processes would let each list, page and close the other's query sessions, since that
  id is the whole of both isolation checks — the AA-03 hole reopened by a bucket meant for
  display. The **project-scoped** tools keep working; the line is whether a tool must know whose
  agent is asking, not `read_only_hint` (which `list_query_sessions` and `read_page` both carry,
  and both are refused). Retraction follows the same split: RAII where there
  is a connection, and an idle sweep (`retire_idle`, matched to rmcp's own `keep_alive`) where
  there is not — skipping any agent with a call in flight, because retiring one mid-run aborts
  its own query and reports that back as "you stopped this", and running a final sweep from
  `AgentServer::drop`, because `shutdown_background` never polls the sweep task again.

  *Teardown.* MCP permits concurrent requests on one connection and the dispatch is the
  caller's, so a `close_query_session` can land between a run's `RunStarting` and its
  `engine.query`. Tearing the workspace down there aborts and retires **nothing** — the engine
  has not been given the work — and the dispatch then registers a snapshot and an in-flight
  entry on a `WsId` no later close, retraction or cap eviction can name, so it leaks for the
  engine's life and feeds a phantom into the T2 confirm. The fix is a **tombstone**
  (`agents::Closed`): the handle stops answering at once, the row is kept, and the workspace is
  retired by whichever `run_settled` lands last. Not a lease, which would put the driver back to
  waiting on a query — the very thing AA-03b moved out. What reaps a tombstone whose settle
  never comes is already there: the row stays in the agent's session list, so `gone` hands it
  back like any other when the connection ends, and the engine side of a dropped run future is
  `DispatchGuard`'s. The adjacent rule is kept apart rather than blanket-refused: the session
  **cap** never evicts a working session (the engine settles a torn-down workspace as
  `cancelled`, which the vocabulary would report as "you stopped this" for a cancellation the
  *app* performed), while an explicit close of a running session is the agent's own decision
  about its own work and is allowed. `is_running` is **any** run in flight, not the newest —
  every consumer of it destroys work when it is wrong.
- **Poll only what nothing on our side can observe, and name the reason where the poll is.** The
  header's agent dot is the app's one sampled fact: how many MCP clients are paired lives in
  rmcp's `LocalSessionManager`, and a session is created inside `service.handle(req)` — below our
  own `serve`, with nothing of ours to notice. The two reactive alternatives are worse, and that
  is the argument rather than the conclusion: wrapping `SessionManager` is ten pass-through
  methods to learn a number that is already `pub`, and a channel needs a receiver, which can be
  taken exactly once — impossible for a status every project window shows. So it samples, with
  three properties that keep it honest. `try_read`, never a wait, because the caller is the
  render thread and a status light is not worth a frame; a failed sample keeps the last answer
  rather than reporting a disconnection that did not happen. The **timer exists only while the
  feature is on**, because the component owning the hooks is mounted conditionally — the default
  app runs nothing. And the staleness is bounded and stated: a client that dies without its
  `DELETE` is over-reported until rmcp's `keep_alive` reaps it, five minutes, and never
  under-reported.
- **A second deployment of the vocabulary answers the same questions from what it already has,
  and owns nothing of the app's.** The headless host (AA-05, `strata mcp <project>`) is a
  `Host` over a plain `Engine` with AA-01's `register_project` replayed on it, serving
  `StrataTools` over rmcp's stdio transport — no port and no token, because the client spawns
  the process and owning it *is* the access control. Four things make it a host rather than a
  second implementation of the feature. **Registration outcomes are the catalog**, folded once
  at open into the same `CatalogEntry`/`Described` shapes the app projects from its store — so
  a def the engine refused is a `failed` row here too, and neither host asks DataFusion.
  **The pass finishes before anything is served**, which is why this one needs no equivalent of
  the app's scan claim: `Engine::register` deregisters before re-inferring, and there is no
  second pass to race. **It has one project by construction**, so the `project` argument is not
  consulted anywhere in the impl — `host::resolve` only ever hands back a project the host
  listed, and a lookup would be a check that can only pass. And **it owns nothing of the
  user's**: no app config (so no `datafusion.*` overrides — the engine runs defaults, and
  `default_page_size` is `Settings::default().row_limit`, the shipped value reached without
  opening the file), no `session.json`, no history, and a folder with no project in it is
  refused rather than scaffolded — the GUI open path scaffolds, but a server the user cannot
  see must not create the files the app owns. Running beside the live app is safe for the
  reason two windows are: every engine lock-claims its own snapshot directory. The CLI branch
  is taken **first in `main`**, ahead of the theme registry, app config, the windows registry
  and the fonts, none of which exist for it — and **stdout belongs to the transport**, so
  logging is a parameter (`init_logging(Log::Stderr)`) rather than a constant: one stray log
  line on stdout is a parse error at the client.
- **The catalog is the `ProjectState` store, not a query.** Never build a `FetchCatalog`
  capability: introspecting DataFusion would surface the `__snap_*` result snapshots and hide defs
  whose registration failed — precisely the rows the catalog exists to show. Mutations call the
  engine, then the store's own method on the matching `ProjChan`; nothing refetches.
- **An expensive, opt-in *result* is freya-query keyed by the request; the store holds the
  request.** Profiling (P3-09) is the shape: the row keeps `Option<ScanId>` — a nonce minted per
  ask — and the numbers live only in the cache entry that key names, with `stale_time(MAX)` (a
  settled scan must never re-execute itself) and `clean_time(MAX)` ("cached until the entry
  changes"). A re-scan is a *new* nonce, so it is a new execution; invalidating is dropping the
  request. Never a `profile` field holding results on the store, never a dedup set, never a
  spinner flag — the cache key is the dedup and `query.read().state()` is the spinner. And the
  `Query` (stale/clean times included) is the identity, so it is **built in one place**: two call
  sites spelling it differently are two entries, i.e. the same table scanned twice.
- **One entry point per expensive action, with the confirm in front of it.** Every trigger for a
  scan calls `ProfileActions::ask`, which raises P3-10's confirm on a first scan and goes straight
  through on a re-scan; confirming calls the same `start` the ↻ calls. Adding a surface means
  calling `ask`, never reaching for the store directly — the same rule the drop confirm holds.
- **Def/runtime split.** `strata-model` holds pure serde defs only (exactly what
  `.strata/project.json` stores — no runtime caches, no UI flags). The Freya store wraps defs in
  rows with `Reg<T> = Loading | Ready(T) | Failed(String)`, making invalid combos unrepresentable;
  `defs()` is a pure projection for saving. **Identity:** tables/views are keyed by **name** (their
  engine/SQL identity, one shared namespace, case-insensitive compare); saved queries by a stable
  **`Uuid`**. Renames route through the store (a view rename rewrites tab `Origin::View` keys).
- **A reader that outlives one Run pins the snapshot it reads.** A snapshot belongs to its
  workspace and is retired the moment that workspace dispatches another run (SNAPSHOT_SPEC §4),
  which is right for the grid and wrong for anything longer-lived. `Engine::pin_snapshot` hands
  back an RAII `SnapshotPin` that **defers** the retire to the last release — so the export
  window (P4-10) writes the result it was opened on even if the user re-runs the query behind
  it. RAII rather than a pin/unpin pair for the same reason cache entries are held by a mounted
  subscriber: lifetime is a held handle, never imperative bookkeeping. Never answer this with a
  warning or a staleness check instead — "your results moved" is a worse product than results
  that don't move, and a check races the very dispatch it is checking for.
- **Silent corruption is refused, never warned about — and the refusal is checked against read
  data, not declared metadata.** DataFusion 54 misfiles a NULL partition value into a neighbouring
  value's directory, so a Hive-partitioned export whose key column has nulls writes rows under the
  wrong key with nothing to tell the user. A banner is the wrong answer to that (it stands there on
  every export, warning about what usually cannot happen, and is still only a suggestion when it
  can) — `Engine::export` refuses and names the column. The *check* is the transferable part:
  schema nullability answers nothing here (every column reports nullable), so
  `partition_columns_have_no_nulls` reads the parquet footer's null count and proceeds **only on an
  exact zero**, which disposes of the `Precision::Exact`/`Inexact` ambiguity in the same move. That
  is why `snapshot_writer_props` sets `EnabledStatistics::Chunk` explicitly rather than trusting a
  default — a gate is only as good as the footer it reads.
- **History is a satellite**, persisted to `.strata/history.jsonl` — never a field on
  `ProjectState`/`SessionState`. It records **only successful data runs**, which is a claim the
  surface has to keep: the History drawer shows no status mark, because the canvas's
  ok/cancelled/failed dot would have exactly one value. Its **Clear** unwrites the file as well as
  emptying the satellite (an emptied list that refills on the next open is a surface disagreeing
  with its store), and keeps the `seen` dedup guard — that guard is about *runs*, and the pin
  holding a cleared run is still mounted, so forgetting it would put the entry straight back.
- **History is a list of queries, not of presses — and dedupe comes before the cap.** A re-run
  moves its entry to the top with the newest figures instead of stacking a row, keyed by
  `util::collapse_sql`, which is the *same* function that renders the drawer's preview (a key
  looser than the preview merges rows a reader can tell apart; a tighter one lets two identical
  rows sit in the list). The ordering is the load-bearing part: a query hammered 150 times must
  occupy one slot of `max_history`, not all of them, so a keep-last-N over the raw log is wrong —
  collapse, *then* cap. The persisted log follows the same rule without losing the cheap write: a
  new query is one `O_APPEND` line, and only a run that *replaced* an entry rewrites the file
  (`project::save_history`), because an append can add a line but not move one.
- **A window's project subtree is keyed on the project folder; there is no reopen-in-place path.**
  `ProjectApp` is the *window* (theme, app-globals, close bridge, menubar, the `OpenCtx` open
  path); `ProjectRoot` is the *open project* — the once-per-mount load and its three arms,
  `ProjectLoading` while the read is off on its own thread, then `ProjectLoaded` (engine, stores,
  autosave, catalog, views) or the `ProjectLoadFailed` fault dialog — and its `render_key` is that
  folder. So "open in this window" (`OpenPref::This`) is a plain `State`
  write: Freya drops the old subtree — flushing its session, dropping its engine, leaving the
  open-set — and mounts the new project through the very hooks that run at launch. Never add a
  second path that re-points a live store at another project: two ways to open one project drift,
  and the mutating one is how relative sources and partition columns get mangled. Anything that
  must survive a re-root (window fill state, the close-confirm slot, the registry entry) belongs
  on the **window** layer, and anything reading "which project" must read it reactively.
- **Nothing blocking runs on the render thread, and a read the user has to wait for is an *arm*,
  not a freeze.** Freya is one event loop drawing every window and its `spawn` polls on that very
  thread, so an `async` block around a synchronous call moves nothing: one blocking `std::fs` read
  is not a slow frame, it is the whole app — every window, the menubar, the traffic lights. That
  stops being theoretical the moment the path comes from the user, because a network mount that
  went quiet blocks in the kernel with no timeout and cannot be interrupted. `task::offload` is
  the one way across — **a thread per call**, since a pool or a single worker would let one wedged
  mount hold up the next project's open, which is this failure moved one step along rather than
  removed. And the wait has to live somewhere a window can *show* it: `ProjectRoot`'s load is
  `use_future` over the offloaded read, so `Pending | Loading | Fulfilled` are the subtree's three
  arms (P4-01) and the fault dialog's Try again can no longer re-enter a blocking call. Three
  things generalise. **Cancelling is dropping the answer, never stopping the work** — the thread
  runs on into a dropped receiver, which is why a deadline buys a window rather than a freed
  thread, and why the parked-thread cost is named rather than designed away. A value needed
  *before* a window exists cannot be reported by one, so it gets a **deadline** instead
  (`window_geometry`, 250ms, because Freya places a window as it creates it or not at all) — and
  then whatever consumes it must be safe against the empty answer, which is why the autosave seed
  is taken from the session the project loaded and not from that read: seeding `None` would let
  the first save overwrite a remembered size with the default the window opened at. And the
  engine's private Tokio runtime is **not** the home for this, tempting as it looks: an engine
  exists only after a successful load, and the loads are exactly what needs the hop.
- **Which window an open lands in is one decision in one place** (`platform::open`). `decide` is
  pure over plain values and is the *whole* rule; acting on it is split off (`OpenTarget`) because
  a window holds a `Platform` and the menubar handler holds a `RendererContext`. Two rules outrank
  the preference and are not among its outcomes: the project a window already shows is a no-op,
  and a project another window already has is focused — two windows on one project would both
  autosave over its `session.json`.
- **Every path that destroys a window's work asks on the same terms.** The T2 confirm is not the
  close button's — it is the gate for *any* action that aborts running queries. Re-rooting
  (`OpenPref::This`) is one, since the remount drops the engine; so is an **engine restart**
  (`CloseTarget::Restart`, P4-07), for the identical reason. Adding such an action means adding a
  `CloseTarget` variant and routing through the one dialog, never a second confirm and never a
  silent abort. The predicate is always the engine's own answer (`guard.running` /
  `Engine::is_running`) plus `confirm_close_running` — never derived from mounted UI, which goes
  false the moment the user switches tabs. One boundary, settled by review: **a question already
  answered is not re-asked.** The subtree's two **engineless** arms drain the confirm slot and act
  (`use_engineless_close`, shared by `ProjectLoading` and `ProjectLoadFailed` — the once-only close
  and the drain always travel together, so they are one hook rather than a copy per arm), because
  `guard.running` can only be true there for runs orphaned by a
  stop the user already confirmed — the re-root or restart that replaced the subtree asked this
  very question, and the engine's deferred `Drop` merely hasn't finished honouring the answer —
  or under a pref that asked never to be asked, which gates every writer of the slot alike.
- **An engine's config is a launch value; a live change is `set_config`, and a runtime key is a
  restart — which is the remount, not a second path.** `Engine::new(overrides)` is the *only* place
  a `RuntimeEnv` is built, so an engine is only ever born with a full set;
  `EngineCtx::new(overrides)` takes the app's, and `use_engine_config` keeps the rest in step off
  `ConfigChan::Settings`. Three rules that each cost a bug to find. A **removed** key is set back to
  its `ENGINE_KEYS` default rather than skipped — leaving the engine on the value the user just
  deleted is the one outcome nobody asked for, and it is expressible precisely because the keys
  `ConfigOptions` accepts are the ones the catalogue names a default for. `restart_owed` is measured
  against `built_runtime` (what the context was *built* with), never against the previous map — a
  user who declines the restart keeps the new values, so map-to-map would report "nothing changed"
  and never offer it again. And the restart itself is a bump of `ProjectRoot`'s diff key
  (`EngineRestart`, owned by the *window* so it survives the remount it causes), because the
  re-root mechanism already drops the engine and re-registers the project through the launch hooks
  — a `restart()` that rebuilt a live store in place would be the second way to configure an engine
  that the rule above exists to prevent.
- **A setting the user edits through more than one gesture gets one funnel, and the policy lives
  next to the resolution it has to agree with.** Settings ▸ Keymap (P4-08) changes a binding four
  ways — capture a press, reset a row, take a chord off another command, reset every row — and all
  of them are `keymap::propose` then `keymap::apply` over a `Rebind`. The check is in
  **strata-core**, beside `validate_bind`, because a hand-edited `config.json` reaches the same
  rules through `effective_chord`, and a second copy in the pane would be the copy that drifts. Two
  consequences worth keeping. **A reset is a proposal like any other**: a command's default chord can
  have been taken while it was away (move Save query off ⌘S, bind Find to the ⌘S that freed up, then
  reset Save query), so a reset that just dropped the override would create the duplicate the whole
  policy exists to prevent. And a *steal* is expressed as the bindings it actually changes — unbind
  **every** holder, bind the asker — rather than as one write, because a write that only recorded the
  winner would leave two commands claiming one chord for `resolve` to settle silently by table order,
  and freeing only the *first* holder does the same for a chord a hand-edited config had already
  duplicated. The same rule reaches the display: an override is only "custom" if it **takes effect**,
  so an override of a fixed command is not (`effective_chord` ignores it, and a badge saying
  otherwise would sit on a row whose reset control is gated off), and a bind to a command's own
  default clears the entry instead of storing a copy of it. One predicate behind the badge and the
  control, or a row wears a mark it has no way to remove.
- **An app-global surface that follows the focused window is pointed by *every* window, and the
  obligation rides the call each window already has to make.** The menubar is one bar for the whole
  app, so its File and Window halves are only ever about whoever has focus — which means a window
  that never says what its menubar is doesn't leave the bar blank, it leaves it showing the *last*
  window's. Configure and Export shipped without the call, so with either focused the menubar still
  carried the owner project window's File menu: Close Project (and its ⇧⌘W) closed the focused panel
  while naming the project, and Open… sat enabled with no listener to reach. The fix is not to
  remember harder — `use_file_menu` moved *inside* `use_register_window`, which every window root
  must call anyway, and takes a `MenuScope` (`Project(OpenCtx)` · `Launcher` · `Panel`), so a new
  kind of window cannot ship without answering the question. The scope is also the second half of
  every gated item's enabled state: an item that reaches its window through the keyboard pipeline is
  live only with **both** a chord to synthesize and a window that listens for it, which is why one
  `apply` writes accelerator and enabled state together rather than two syncs racing on
  `set_enabled`. Close Project is *removed* rather than greyed, because it routes at the focused
  window directly and greying it would still leave the wrong window named. The gate is **four
  independent flags, not a rank** (`workspace` · `project` · `workbench` · `cyclable`), because
  where a command's listener lives differs per item and the differences do not nest: Close Project
  is mounted on the window root and so works on a window whose project failed to load, while New
  Query and Save Query are in the *workbench* and do not — the shape no "how much of a project
  window is this" scale expresses. Whether AppKit claims a *disabled* item's key equivalent or lets
  it fall through to the window is **unverified**, and nothing here depends on the answer: every
  greyed item's command has no listener in the window that greys it, so both resolutions end in the
  same nothing.
- **A menubar accelerator is state, not decoration — and it must be disarmed while a chord is being
  captured.** The OS resolves an accelerator *before* the window sees the key, which makes both
  halves of this sharper than they look. A stale accelerator does not merely show the wrong text: it
  keeps firing on a chord the user rebound away, and swallows the new one. So `MenuHandles` keeps
  every accelerator-carrying item and `sync_chords` re-applies all of them off
  `ConfigChan::Settings` from the focused window (the same effect that points the File menu at it) —
  and the list is a **destructure** of `MenuChords`, so a new menu command that forgets it is a build
  error, for the reason `settings_merge!` is a macro. The capture case is the same fact pointed the
  other way: with the menubar armed, pressing ⌘C to *bind* it copies instead, and ⌘Z ⌘X ⌘C ⌘V ⌘A ⌘O
  ⌘Q ⌘, are most of what anyone reaches for, so `suspend_accelerators` holds them off for the
  capture's lifetime. A held flag, not a `sync_chords(&Default)` call — otherwise the routine sync
  re-arms the menubar underneath the capture.
- **An app-wide flag held to protect one window's listener is released on losing focus, not only on
  finishing.** The half of the rule above that was wrong first time: the Keymap pane suspended the
  menubar on "a capture is in progress" alone, and Settings is deliberately *not* modal, so clicking
  the project window behind it mid-capture left the flag stuck — every gated menu item lost its
  chord *and* its enabled state, in every window, until that capture was finished or the window
  closed. The condition has to name both halves ("a capture is in progress **and** my window is
  focused"), which is not defensive bookkeeping but the actual invariant: the listener being
  protected is that window's and cannot fire while another has the keys, so there is nothing to
  protect. Generally — when a flag's *scope* is wider than the state that justifies it, its
  condition must include whatever makes that state reachable, and the release path has to fire on
  every way of leaving it (`use_drop` covers a window that goes; only focus covers one that stays
  open behind another).
- **A name two surfaces have to agree on is generated from one table, not typed twice — and
  navigating to something is never editing it.** The Settings search (P4-09) indexes a setting by an
  `Anchor` *variant*: one table generates the enum, the list of every anchor, and each setting's
  route, label, subtext and keywords, and the pane builds its row from the same entry
  (`Anchor::row()`). That is not tidiness — the failure it rules out is silent. An anchor spelled one
  way in the index and another in the pane is a jump that routes and then singles nothing out, and
  nothing but trying it would ever say so; the same goes for a label, which titles the hit *and*
  heads the row. Two consequences. The **category** is not restated in the index at all (a hit
  resolves its page through `model::category`, the tree the rail and the breadcrumb already read),
  and the engine's properties are indexed off **`ENGINE_KEYS` entire** rather than a chosen few,
  because a hand-picked subset of a catalogue is a second list to keep in step. And **following a
  result only navigates**: it may single a setting out where there is something to single out, but it
  must not write. Adding a pre-filled grid row for a property with no override (the canvas's "search
  doubles as add a known property") was built and rejected — a named row with an empty value still
  projects into the draft, so merely following a result left Apply live for a change nobody asked
  for, and the grid claiming to list the overrides in force listed one that wasn't.
- **A free-form list setting is edited as rows and committed as a map.** `Settings::engine` is a
  `BTreeMap`, which cannot hold the row you have not named yet or the duplicate you are halfway
  through fixing — so the Engine pane's model is an ordered list of rows under ids minted by a
  counter (never the name: the name is the thing being retyped), projected back into the draft on
  every edit. The window's one commit path is untouched. The list lives on `SettingsCtx`, not the
  pane, for two reasons that generalise: navigating away and back must not discard a half-finished
  edit, and the footer has to answer "what is blocking Apply?" (`blocker()`) without the pane being
  mounted to answer it — a button disabled for a reason the user cannot see reads as broken.
- **Managed DDL policy.** The editor runs `SELECT`/`EXPLAIN`/`SHOW`/`DESCRIBE` only. Views are
  Save's artifact: ⌘S wraps the buffer's plain query in `CREATE OR REPLACE VIEW`
  (`Engine::create_view`); typed DDL is blocked with validation pointing at the owning surface
  (Save / the catalog / Table Config).
- **One app-global config store.** `RadioStation<AppConfig, ConfigChan>` created once in `main`,
  shared into every window (`use_share_config`). Disk is a startup input, read **once** — no file
  watching, ever; after launch only the UI writes. `write_config` (src/state/config.rs) is the
  **sole** write path: mutate + notify + persist; nothing re-reads the file to answer a question.
  Settings is the `ConfigChan::Settings` **channel**, not its own global — one struct = one load,
  one write, no field clobbered by a partial save. `use_config(chan)` to subscribe;
  `use_config_station()` when a handler must only `peek` (key chords, close guard).
- **A draft of shared state commits a per-field diff against its seed, never the whole struct.**
  The Settings window's draft is a snapshot taken when it opened, and another window can commit a
  setting of its own before Apply is pressed — the close confirm's "Don't ask again" writes
  `confirm_close_running` from a window that never shows it. Writing the draft wholesale carries
  its stale copy of that field back over the top: a change the user made, undone by a window that
  never displayed it. So `SettingsCtx` keeps a `seed` beside the `draft` and commits through
  `Settings::merge_onto` (strata-core), which only writes fields the draft actually changed. Two
  consequences. The field list is generated by `settings_merge!` and made **exhaustive by the
  compiler** (`let Settings { … } = self` names every field, so a new setting that isn't merged is
  a build error, not a control that silently never commits) — hand-writing the merge is the
  failure mode, not the macro. And "is there anything to apply?" is `draft != seed`, never
  `draft != committed`: the latter enables Apply for someone else's change, which the merge then
  correctly commits nothing for.
- **The theme is pure derived state — deliberately not stored.** Every window root mounts
  `use_strata_theme(themes, config, preview)`, which derives the effective theme id from the
  settings global (+ `Platform.preferred_theme` while `sync_os`) and resolves through the shared
  `ThemesCtx`. Don't add a stored applied-theme-id global back, and don't store other derivable
  settings projections — subscribe to the channel and compute. Gotcha: copy `theme.peek().name` out
  before `theme.set(...)` — an if-condition temporary holds the read borrow across the write
  (runtime borrow panic on the same GenerationalBox).
- **An uncommitted value that must be live everywhere is a second *input* to the derivation, never
  a stored result.** The Settings window's theme pick has to repaint every window while it is still
  uncommitted, and `write_config` persists — so it rides one narrow app-global slot
  (`state/theme_preview.rs`: `State<Option<ThemeSel>>`, theme id + `sync_os`) that
  `use_strata_theme` resolves *ahead of* the settings. Two rules keep it honest. It stays
  **narrow**: the rest of the draft is the Settings window's own `State`, because putting the whole
  draft in the slot would wake every window's theme derivation on a keystroke in a text field
  (mirror with `set_if_modified` for the same reason). And **dropping it is the revert** — Cancel,
  Esc and the red button all just clear the slot, so there is no restore path to keep in step with
  the commit path.
- **A repeated colour is a palette slot, never a repeated `specific`.** A theme file's colour
  source is the 27-slot `sheet` **plus** its own `palette` of app-named slots, together forming the
  `Palette` a `Theme` resolves references against (fork-side: `Theme.palette: Box<dyn Palette>`,
  `sheet()` required so a custom palette can never break a built-in, `color()` open and consulted
  only for non-core names). Authoring the same hex in two fields is the smell the palette exists to
  remove — name it once and reference it. Two consequences to hold: `reference` is an **open**
  namespace, so the schema can't enumerate targets — an unresolvable name paints magenta and warns
  via `unresolved_references` (`references_resolve` pins the built-ins); and a colour is only one
  token if it is one *per theme*, so collapse on the design source of truth (Midnight) and let the
  others normalize onto it rather than freezing each theme's drift into separate specifics.
- **Panel layout lives on `SessionState`** (not a peer store), so it rides `SessionSnapshot` +
  autosave and survives restart. Two channels, both `Persist`: `Chan::Layout` = structure,
  `Chan::LayoutSize` = sizes (nobody subscribes; a resize drag persists without re-rendering the
  shell). `ResizableContainer` owns live resizing — we persist only the last size. Keep panels
  **keyed** with fixed `.order()` so the `Workbench` subtree survives a sibling collapsing.
- **A window that belongs *to* another window is a child window, and its lifetime is ours.** The
  Settings window is one app-wide, pinned above whichever window opened it (the fork's
  `set_window_parent`), re-pointed when another window asks — with one entry point
  (`platform::settings::open_settings`) so "already open" can only mean focus + re-pin. Two things
  don't come free with the AppKit relationship. It must **not** count as a workspace window
  (`Windows::is_last()` skips it, or the last project closes onto an empty app), and **closing with
  the owner has to be the app's rule, not AppKit's**: AppKit closes a child behind winit's back and
  Freya only removes a window on a close it was asked for, so it would keep a live scope for a
  window that is no longer on screen. Express it in the registry's terms — the owner leaving closes
  the child through Freya's own path — which also covers the platforms where the child relationship
  is a no-op.
- **A window's lifetime must be at least as short as the shortest-lived thing it holds — and for a
  child window that is a *mount* of the project subtree, not a window id.** Export and Configure are
  their own OS windows, so they cannot inherit the project window's context and carry its store, log,
  catalog and scan counter as launch values — all created inside `ProjectRoot` and all
  `GenerationalBox`-backed. Both things that remount that subtree free them while leaving the owner
  window open under the same id: a re-root changes the folder, an engine restart changes neither. A
  child left open across one holds dangling handles, and the failure is a panic on whichever read
  repaints first — a keystroke is enough — or a Save into a store nothing is left to serve. So the
  pin is over `platform::owner::Subtree`, which is `ProjectRoot`'s own diff key (folder +
  generation) plus the live `EngineRestart` to read the current generation back, **provided by
  `ProjectLoaded`** (the loaded arm — the fault arm has no handles to lend) so no call site can
  assemble a mismatched trio, with `use_owner_pin` the one predicate. Three things generalise. An owner that has closed *shows nothing*, so it fails the same
  comparison and "my owner closed" needs no clause of its own — one predicate, not three. The
  generation is the one handle here safe to hold across a remount, for precisely the reason it exists
  (owned by `ProjectApp`, above the subtree). And this is why `WindowKind` carries **less** than it
  used to: `Configure`'s `project` and `Export`'s `owner` were the old pins' inputs, so once the pin
  reads its owner from the launch value they were unread copies of a fact that could go stale — the
  registry keeps only what it is *asked*, which is `is_workspace()` and Configure's focus-if-open
  keying (`owner` + `target`, since one owner window shows one project). Anything that later hands a
  child window a subtree handle takes a `Subtree` and calls that hook rather than growing a third
  copy of the rule.
- **Window geometry** is read via `Platform::root_size` and the fork-added
  `Platform::window_position` (both logical); never reach for the raw winit handle. There is no
  runtime resize/move from the app — restore geometry only at window **creation**
  (`WindowConfig::with_size` + `with_window_attributes(with_position(..))`), which is why launch
  inputs (project root, geometry) are resolved *before* the window opens.
- **No command bus.** App-level shortcuts are distributed `on_global_key_down` listeners per
  feature (helper: `strata-freya::keymap::on_command`), resolving through the central
  `strata-core::keymap` table. Precedence = document order; a modal barrier = an early-mounted
  consuming listener. Never a root-level handler registry — registries/buses are god-objects that
  centralize what the tree already expresses, and when a design reaches for one to work around a
  Freya limitation, fix the limitation in the fork instead (§6).
- **The command palette is a *registry of offers*, not a dispatch layer — and it is not a function
  of the keymap.** The rule above is about *shortcut dispatch*, and the palette does not do any:
  it is a list the user picks from, so it holds no chords and intercepts nothing. Two things keep
  it from becoming the bus that rule forbids. **Every command's body is one call into a funnel that
  already exists** (`actions::run_query`, `close::close_project`, the catalog's `view_row`), so a
  palette row is a second way to *ask* and never a second implementation — and where that logic was
  inline somewhere the palette can't reach, it **moves to the funnel** rather than being copied,
  which is how the ⌘↵ in-flight gate and the close-while-running predicate came to live beside
  their neighbours. And a command's chord (`CommandRoute::key`) renders its hint and nothing else:
  synthesizing it — the trick `menu.rs` uses correctly, because a muda handler has no stores —
  would make a command the user unbound unreachable from the one surface that exists so you needn't
  know the chord. Registration is `strata-command-macro`'s `#[command_router]`, rmcp's declaration
  shape (id from the method name, subtext from the doc comment, nothing typed twice) with the
  `HashMap<name, Arc<dyn Fn>>` deliberately left behind: string dispatch exists because an MCP
  client names a tool over a wire, and a palette already *holds* the row that was picked. The macro
  generates the **enum** instead, one variant per method, so dispatch is total by construction —
  "registered but unrunnable" is not expressible — and a route is a plain `fn` pointer in a `const`
  slice, with no router to rebuild per keystroke. Adding a command is one method; if it needs a
  funnel that doesn't exist yet, build the funnel.

