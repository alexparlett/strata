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
  holds connections and defs the engine refused and `.strata` files a failed write left behind —
  in **registration order** (connections → tables → views), so a table broken *by* a bucket with
  no credentials reads below its cause rather than being the only thing said (W7). Three kinds of
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
- **A refusal is stored whole; the surface that cannot hold it clips it, and says where the rest
  is.** A limit is a property of the surface that has it, never of the string every other surface
  reads. `register_error` capped its pass-through at 240 characters with a trailing `…` for one
  reason — the catalog row's tooltip and a11y label, which a sentence cannot be read in — and so
  clipped the message *everywhere*: the Problems row, which wraps precisely so a refusal can be
  read, and the copy button beside it, which exists so a refusal can be pasted into a search, both
  handed back the same cut. It was not a hypothetical cut. An unreachable bucket reports past 240
  characters and names its cause in the **last** clause, so what survived was `object_store`'s
  retry bookkeeping and what went was the answer. The cap now lives at the tooltip
  (`catalog::row`'s `TIP_CHARS`), which shows a short refusal entire — most are one sentence
  Strata wrote — and otherwise clips and names Problems, so the pointer appears exactly when
  something was left out. A **connection** row is the same slot and the same rule since DB-05
  folded the connections into the tree: it used to show a fixed "see Problems" pointer instead,
  on the argument that a connection's refusal is always the engine's own prose where a catalog
  row's is usually ours — but that is an argument about how *often* a message fits, and the cap
  answers it per message.
- **A wrapper is not a diagnosis, so peeling one is not diagnosing — but every literal you peel
  has to be read off the crate that writes it.** DataFusion and `object_store` prepend a name per
  crate boundary crossed, which is what makes a refusal read as a stack trace: `External error:
  Object Store error: Generic S3 error: Error performing GET <uri> in 5.383s, after 10 retries,
  max_retries: 10, retry_timeout: 180s  - …` in front of the one clause that says what happened.
  `catalog::readable` strips those layers (`LAYERS`), the store wrapper (`STORE_WRAPPER`) and that
  retry bookkeeping (`RETRY_PREFIX`/`RETRY_CAUSE`) and **nothing else** — it loops because they
  nest, stops at the first thing it does not recognise, and keeps the raw line when peeling would
  leave nothing. It sits on the pass-through path deliberately, beside the mappers that do
  diagnose and under the same rule they follow: an unfamiliar error is never translated, because a
  confident wrong diagnosis is worse than a raw one the user can search for. It is reached by the
  **table** funnel and the **view** funnel, because both land in one Problems list and a view
  wearing the stack beside an unwrapped table reads as two apps.
  Two things this got wrong first time, both worth keeping: the strings were taken from a **doc
  comment** rather than from `object_store`'s `Display` impls, so three of them matched text the
  crate has never emitted and the peel was a no-op on the one path it exists for — while its unit
  test, whose fixture had been written to match the code, passed. And `Generic {store} error: ` is
  a **format with an open store name**, not a list: enumerating it shipped `GoogleCloudStorage`
  (the crate says `GCS`) and omitted `HTTP` outright. Match the pattern; quote the source.
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
  duplicates (`ChartData::Duplicates`, refused in favour of the user's own `GROUP BY`). Over a cap it answers
  `ChartData::OverCap`, which carries no data at all — a truncated chart is not a state that can
  exist. The histogram's binning is the one engine computation over the *data* (no `width_bucket`
  in DataFusion 54), and the scatter trendline's `Engine::trend` fit (Chart 11, `CHART_SPEC.md`
  §10) is the one sanctioned computed *overlay*: a function of the encoding rather than the
  query, its own read keyed by the two encoded columns, so toggling it never re-reads the
  points. What was **built and withdrawn** is the first design's engine-side aggregation pipeline —
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
    the type three times and lets the three disagree. **A time column is two roles, not one**
    (04): `Instant` (Date32/Date64/Timestamp) and `Clock` (Time32/Time64) are identical on an
    axis — same default X, same default mark, read together by `config::is_time` — and differ
    wherever a **stride** does. Measured: DataFusion refuses a day-wide `date_bin` over a `Time`
    column ("DATE_BIN stride for TIME input must be less than 1 day"), so any SQL the chart
    generates over a time axis has to know which it has. Nothing in V1 generates such SQL — the
    split arrived with the cut scaffold below and was kept anyway, because the only other way to
    recover the distinction later is the type's spelling, which this same entry rules out, and
    the `DataType` is here and gone.
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

  And one **Copy Image** settled (Rz2/08):

  - **A chart image is the chart, so the capture and the paint are one draw body.** Copy Image
    renders the *same* `Frame` the visible canvas is painting — held as an `Rc` and handed to
    both, so the toolbar item and the plot cannot describe different charts — through the same
    `marks::draw`, which takes a canvas and a `FontCollection` rather than a `CanvasContext`
    exactly so the offscreen path has nothing to reimplement. `draw` **returns** its hit regions
    instead of writing them through a handle, so a capture cannot overwrite what the visible plot
    last recorded for its pointer. And it needs no paint pass at all: the `FontCollection` is a
    root context (`consume_root_context`, the same one `freya-code-editor` measures against), so
    a press renders on its own rather than raising a flag the next paint has to notice. The
    capture is a fixed 1600x900 at 2x — the pane's own size would copy whatever labels a narrow
    drag had thinned away, and drawing the export's pixels as logical units would leave a 10pt
    tick label lost in a chart at twice the size. The background is filled first (the live canvas
    is transparent over the pane, which paints it), and the pixels are converted to
    **unpremultiplied RGBA** on the way out, because `raster_n32_premul` is the platform's native
    order (BGRA on Apple) and a raw read puts a blue-for-red chart on the pasteboard. Nothing is
    written to disk: the clipboard grew image support in the fork rather than the app growing a
    save-to-PNG stopgap. It grew it **inside the existing shape** — the platform integration still
    provides a `Box<dyn ClipboardProvider>` into the root context and `Clipboard` still reads it
    from there; what changed is that the trait is the fork's own and covers images as well as
    text, and copypasta was **replaced** by arboard rather than run beside it, because text and
    images are one clipboard and a second backend is a second claim on the same selection. The
    Linux trade is stated where it lives (`ClipboardContext`): arboard reaches Wayland over
    `wlr-data-control` / `ext-data-control` and otherwise falls back through XWayland, where
    copypasta used the standard `wl_data_device`. No crate speaks that protocol *and* carries
    images, and a second provider for text alone is the thing being avoided.
    The item is **absent** over a notice rather than disabled, because there
    is no chart to copy and a greyed control says there is one that is merely unavailable.

  And one the **guardrails** settled (Rz2/04):

  - **A refusal names its fix in prose, and V1 puts no control behind it.** The chart aggregates
    nothing, so over-cap and pivot-duplicates are both fixed by changing the query; the overlay
    says so and stops. An *Aggregate in SQL* press — a `GROUP BY` composed from the resolved
    encoding over the run's SQL, opened unrun in a new tab — was **built and cut**, and the
    reasons are worth keeping because the capability will come back up. The mechanism was sound
    and the placement was not: the same capability exists in DBeaver's Grouping panel and as
    "eject to SQL" in Metabase, Superset and Looker, and every one of them puts it in a menu or
    a surface of its own, never among the encoders — which is where it landed here, the one
    control in the strip that *left* the chart rather than changing it. It was also standing in
    for the chart having no aggregation of its own (`CHART_SPEC.md` §8), and a shortcut that
    makes that gap tolerable is a reason not to close it. What survives is the role split above.
    Re-litigate the *placement* only with a surface that isn't the strip — **which happened**
    (Chart 09, settled in planning 2026-08-07, shipped 2026-08-12): the **Shape panel** is that
    surface, a modal working panel off the results toolbar that composes visible SQL (group
    columns with `date_bin` strides, per-measure aggregates, an explicit `ORDER BY`) and opens
    it unrun in a new tab, seeded from the resolved encoding when the press came from the
    Chart view. Its aggregate vocabulary is UI-local text rendering
    (`results/shape/compose.rs`) and enters no engine type — renderer-first stands untouched,
    the refusal overlays still keep no control behind them, and the strip is still not the
    place (`CHART_SPEC.md` §8 records the same).

  And four the **interactivity** pass settled (Rz2/06), all about which side of the read a
  control sits on:

  - **A chart's controls are repaints, and the bin count is the one exception — because the
    engine does the counting.** `ChartConfig` grew four channels and only `bins` reaches a
    `ChartQuery`: a new bin count is a new cache entry, exactly as it should be, while `hidden`,
    `log_y` and `sort` are transforms over data already in hand. The cap is
    `engine::MAX_BINS`, `pub` and clamped at **both** ends of the wire, because a box that
    accepts 5 000 over a read that answers 200 shows one thing and means another. An empty box
    is `None` and `None` is the engine's `√n` — reachable by clearing the field, which is why
    the strip owns a small buffer of its own rather than reusing `NumberField` (a number field
    has no state for "deliberately no number"). It **bounds its box and normalizes it when it
    is left** (AGENTS.md §3): `max_len` is the cap's own digit count, and losing focus re-echoes
    what was committed — without both, the box showed `5000` over a 200-bin chart, which is the
    "shows one thing and means another" failure the shared cap exists to prevent. The parse is
    wide and the clamp comes after it, or a count over 65 535 would fail a `u16` parse and read
    as Auto rather than as the cap.
  - **A hidden series keeps its slot, and the order it is hidden in is sorted-then-hidden.**
    Hiding blanks a series' `values` to all-`None` rather than removing it (`chart::hide`), so
    positions — and therefore `Dress::series` colours — never move under a legend press, and
    `marks` needs no idea it happened (a `None` cell is already a gap and hit regions are built
    per finite value). It is keyed by **name**, so a NULL-valued series and a literal `"(null)"`
    one toggle together: accepted coarseness, because the name is what the user pressed and a
    position-keyed legend forgets the choice the moment the SELECT list changes. `sort::sorted`
    runs **before** `hide::applied`, or hiding the first series would silently reshuffle a
    `ByYDesc` chart's whole category axis. `resolve` drops the set for a mark whose legend cannot
    un-hide, exactly as it drops `bins` for a mark with nothing to bin — a pie's Y is an ordinary
    measure a bar may have hidden earlier, and honouring it there blanks the pie with no control
    on screen to bring it back. ⌥-press **edits** the set rather than rebuilding it from the
    current legend, so a name this result cannot answer survives the gesture the way it survives
    an ordinary press; on the sole visible series it shows them all again, so the gesture cannot
    empty the chart. And the **legend survives the one notice that names it**: the all-hidden
    notice says "press a legend entry", and built only on the drawable path the legend vanished
    exactly when its own message named it — a dead end the tab carried across a re-run and a
    restart, because `hidden` is persisted. Only that one: every other notice draws no plot and
    offers no way back through the legend, so keying colours beside one would name colours
    nothing on screen is wearing.
  - **A log axis never refuses; it says why it could not and draws linearly.** `ValueCoord`
    (`chart::axis`) is one plotters `Ranged` with a linear and a log arm, so no mark has to be
    generic over its Y — the alternative was splitting every mark into a build half and a draw
    half. It is offered only where a mark plots **position** rather than extent (`log_axis`:
    line, scatter, histogram; a bar and an area are read as area from a baseline, which a log
    axis has none of), and `log_fallback` answers **which** of two reasons sent it back to
    linear, because a banner that blames zeros that are not there is worse than none. A value at
    or below zero is one; the other is a span whose **ratio** overflows — a log axis is bounded
    by `end/start`, not by `end - start`, and `LogCoord::key_points` turns an overflowed ratio
    into a `usize::MAX` tick count it then counts down one at a time on the render thread (a
    column holding 1e-300 and 1e300 reaches it). A histogram's **empty bins are not such a
    value**: a zero count paints nothing on either axis, and blocking on one would take the log
    scale away from exactly the long-tailed distributions it exists for. And a result with
    **nothing positive in it at all** gets no banner — `log_span` answers `None` for that and
    for an unusable ratio alike, and reporting the ratio's message told a user whose every value
    was NULL that their data spanned too many orders of magnitude. `log_span` rounds out to
    whole decades and takes the *next* decade out when a bound already sits on one — the log
    version of `EDGE_AIR`, and without it the commonest log histogram there is draws every count
    of 1 as a bar of no height.
  - **The crosshair rules through the hovered mark, and its pieces are absolute siblings of the
    plot.** Through the **mark**, not under the pointer, and that is a cost model rather than a
    simplification: Freya has no incremental rendering (`render_pipeline.rs` repaints every node
    every frame) and `CanvasElement::render` calls its `on_render` on each pass, so *any*
    reactive write here re-runs `marks::draw` — a full plotters replot plus a rebuild of every
    hit region, on the render thread. A crosshair that followed the pointer did that on every
    mouse sample; riding on `hover` costs nothing beyond what the readout already costs, for the
    same reason `Hit::anchor` exists. The price is that the axis can only be read at a mark,
    which is where the numbers are. The value is **carried on the `Hit`, never inverted out of
    the pixel row** — that round trip put `11.01` under a tooltip reading `11` — and `PlotArea`
    (plotters' own `plotting_area().get_pixel_range()`) comes back **with** the hit regions, in
    `draw`'s own answer rather than a second slot, only so the rules span the plot rather than
    the pane — the capture gets one too and drops it, which is the point of returning both. The value label
    **flips below its rule** rather than off the top of the plot, because a maximum that is
    already a nice number puts the tallest mark exactly on `frame.top`. The three pieces hang off the canvas root,
    not off a wrapper: an absolutely positioned node resolves against its parent's area, and a
    wrapper would be a *stacked* sibling of a fill-height plot — measured, its horizontal rule
    came out one whole pane below the pointer.

- **A snapshot read has no order of its own; order is the ordinal column.** (`SNAPSHOT_SPEC.md`
  §9; lands with the workstream re-cut.) Above 10 MB an Arrow File scan range-splits and a bare
  `LIMIT/OFFSET` read sits over a `CoalescePartitionsExec` — measured: at 3M rows the *same page
  re-read returns different rows* (page 1 arrived starting at row 1 843 201 on one read and 101
  on the next), and a 200k-row snapshot with a text column pages stably but starting at row
  57 345, so `fetch_page`'s pages disagree with the spooled page 1 — rows duplicated and missing
  as the user pages, and the page cache freezes whichever answer a read happened to get. The fix
  is written order: `materialize` appends `__strata_ord` to each batch **as the writer spools
  it**, numbered from the count already written (a **UInt64, 1-based** column — nothing reads its
  values, only their order), after `QueryOutput::columns` is captured, name-escalated on
  collision and recorded in `SnapshotStats.ord: Option<String>`. The value is therefore the row's
  literal position in the file, which is exactly what every reader's `ORDER BY __strata_ord`
  means — the property holds by construction rather than by measurement. **It was a plan-level
  `row_number() OVER ()` until DB-02 and must not go back**: a window in the plan is the
  *query's* to evaluate, so a read over a federated database had Strata's own bookkeeping pushed
  across the wire for Postgres to compute, numbering the remote result rather than the stream the
  writer consumes; it also dragged the scan into DataFusion 54's unparser along a derived-table
  path that does not rebase outer column qualifiers, so every federated read failed. The full
  argument, and the measurements the window was originally adopted on, are `SNAPSHOT_SPEC.md` §9.
  Two results spool **without** an ordinal, `None`, and read unordered as at base: an
  `EXPLAIN`/`EXPLAIN ANALYZE` (the planning constraint that forced this is gone with the window;
  the exclusion stays because a handful of plan rows cannot reach the nondeterminism the ordinal
  exists for) and a result with duplicate column names (name-keyed reads would mis-map a
  duplicate onto the ordinal's slot). The registration **declares** the file's order (`with_file_sort_order`), so an ordered
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
- **The vocabulary is public methods and `#[tool]` is a wrapper over them; the model-facing
  manifest is derived from the router that serves MCP.** (AS-01) The eleven tools *are*
  `StrataTools`' own public methods — plain arguments in, wire result types out, no rmcp type
  in any signature — and each `#[tool]` item does only what a semantic call cannot: resolve
  which agent the *request* is (`Caller`) and hold it against the idle sweep (`Busy`), then
  delegate. A session-scoped tool has a private `_as` core taking the `AgentId`, so the wrapper
  passes the request's agent and the public method passes the value's own; that is
  `open_query_session`'s original split generalized rather than a new mechanism, and it makes
  the in-process caller the **owned** case by construction — its `AgentId` retracts by RAII,
  there is no roster entry, and there is nothing for the sweep to reap. The alternative, an
  in-process shim that re-implemented anything, is the second vocabulary this crate exists to
  not have, and it would put the policy gate on the far side of a copy. `manifest()` is that
  same offer as plain data (name, doc-comment description, schemars argument schema), read off
  `Self::tool_router().list_all()`, so a tool added to the router reaches a model with no
  further edit; a hand-kept list would be right on the day it was written and wrong on the day
  a tool was added — silently, and in the direction of advertising a capability that is not
  there. rmcp's own dispatch is deliberately *not* reused for the in-process path:
  `ToolCallContext` needs a live `Peer`, and it answers in content blocks rather than typed
  values. Binding a model's tool call **by name** belongs with the loop (AS-02), where the
  provider's tool-call type and a bad-arguments message a model can act on both live.
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
  to **query sessions** of the agent's own, kept entirely out of the user's window. The fix for
  the sharp edge
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
  is a connection, and an idle sweep (`retire_idle`, `STATELESS_IDLE` = 30 minutes) where
  there is not — skipping any agent with a call in flight, because retiring one mid-run aborts
  its own query and reports that back as "you stopped this", and running a final sweep from
  `AgentServer::drop`, because `shutdown_background` never polls the sweep task again. That
  window is **not** parity with rmcp's `keep_alive`, though it was for its first five minutes:
  `keep_alive` times out a *session worker*, which is the branch retracted by a `Drop` anyway,
  while a stateless request holds no rmcp state between calls at all — so this sweep is the
  only bound there and has nothing to agree with. Five minutes measured thinking time between
  calls and retired sessions out from under clients reasoning over a large result;
  `open_query_session`'s description, the assistant's system prompt and
  `docs/AGENT_ACCESS_SPEC.md` state the 30 as a ceiling ("may be retired"), because one text
  reaches connected callers too.

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
- **The vocabulary's one write is always available, and its whole fence is the path.**
  `export_result` (QE-05) writes a query session's settled result to a file the caller names, and
  it is *not* a loosening of `run` — the classification, the policy gate and `Blocked::CopyTo`
  are untouched. A consent surface was considered and **declined on the observation that killed
  it**: `read_page` already hands over every byte of that result, so a gate on writing the same
  bytes to the user's own disk protects no data the read surface has not already exposed, and
  per-call confirmation was never a candidate (a tool call must not block on a dialog — the same
  reason profiling is not exposed at all). What needs protecting is the write, so
  `export::check_destination` refuses **by name**: never into `.strata/` or the snapshot spool
  (the settled resolved-target gate, literally `ddl::copy`'s own function), never over an
  existing file with no overwrite flag, never creating folders — and the shape rules that make
  those answerable at all. Two of them are about the caller (absolute, local: a relative path
  resolves against a cwd the caller cannot see and a remote one has no local file to ask about);
  the other three are read off DataFusion's `FileOutputMode::single_file_output` and
  `ListingTableUrl::parse`, which decide what the target *is* — a `?`/`*`/`[` makes it a glob and
  the write lands in the directory before it under a generated name, and a trailing separator
  **or a last segment with no extension** makes it a collection. Both of the last two were
  measured writing a directory of part files while the answer claimed one file, `bytes` reporting
  the directory inode's size. Two shapes
  are the precedent for any later curated write: it reaches the **engine directly** like
  `read_page`, because the source is the session's own snapshot and no window state is touched
  (so all three deployments answer it with no `Host` method and no channel hop), and it is a
  **third gesture into `Engine::export`**, composing the spec with no options to get wrong so the
  pin, the background-work count and the ordinal's exclusion are the window's own, unchanged.
  Related: `resolve` returns the canonical path itself when nothing is left to join — `join("")`
  leaves a trailing separator and `stat("file/")` is `ENOTDIR`, which read an existing file as a
  free name. `starts_with` is component-wise and never noticed.
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
- **The assistant's brain is one table and a per-send value; whether a knob exists is ours,
  what a rung means is the provider's.** `strata_agent::assistant::provider::PROVIDERS` is the
  one place a kind's label, base-URL policy, key policy, **effort ladder** and `genai` adapter
  are written — Settings' roster (AS-03) and the composer footer (AS-04) read it and neither
  restates it, and `Brain::resolve` is the single site a client is built. `Selection` is plain
  data handed in with **every send**, so several chat panes on different providers is several
  values rather than a mode anywhere: the def/runtime split, one layer down. Effort splits in
  two on purpose. *Whether the control is offered* is decided **per model** by the kind's
  `Efforts` rule (`Never` / `Always` / `Only`), because reasoning is a model capability and not
  a provider one: `claude-opus-4-5` takes an effort and `claude-sonnet-4-5` cannot be offered
  one, because genai would enable thinking it then cannot return on the next tool round — so a
  per-kind answer is wrong in both directions, hiding controls that work and offering ones that
  break the turn. The rules are name fragments and will fall behind what the providers ship,
  which is why `Only` is **default-closed**: falling behind costs a knob the user cannot reach
  yet, never a menu whose settings the provider refuses. *What a rung means* for a model that
  has one stays `genai`'s, which already downgrades `xhigh` below Opus 4.7 and knows `gemini-3`
  takes a thinking level where 2.5 takes a budget; restating that would be a second copy of a
  mapping that exists. Three fields are refused rather than dropped (a base URL on a kind that
  owns its endpoint, a key on a kind that sends none, an effort the *model* does not offer),
  because a field silently ignored is a lie on screen; the compatible kind
  has **no env fallback**, because `genai`'s default would post the user's `OPENAI_API_KEY` to
  whatever host they typed; and `check_base_url` normalizes the **trailing slash**, without
  which every adapter's path join reaches a URL the user never wrote.
- **A model is picked from what its provider serves, and the list it is picked from is a
  satellite refreshed where it is shown.** There is no free-text model box in the app (AS-06).
  `genai` prescribes no models — a name is an opaque string that goes into the request payload,
  and `AdapterKind::from_model` is a routing heuristic Strata bypasses by always naming an
  adapter — so nothing here is protecting the user from a name a static list would have caught,
  and the provider can simply be asked. A typed name buys nothing and costs a turn:
  `gpt-5-turbo-imaginry` is accepted by every layer we own and refused by the vendor, after the
  send, in a transcript.

  **The offer is `Listings::offer`: reported ∪ {the current pick}**, and it lives beside the
  data because two surfaces pick a model (Settings ▸ AI ▸ Chat's default, the composer footer's
  per-conversation pick) and a rule about what may be selected has to be one rule. The union is
  load-bearing in both directions: the list endpoint is not the chat endpoint — a proxy or a
  private deployment can serve `/chat/completions` and no `/models` at all (genai carries
  hardcoded lists for Cohere and Baidu for exactly that reason), and an offline laptop serves
  neither — so a strict picker would strand a setup that works and silently retarget a
  conversation the first time a fetch failed. The list stays **unfiltered**: the provider names
  every id it has, so OpenAI's carries `whisper-1` and `dall-e-3`, and a static tidy-up list
  here would be the prescribed-model table this design avoids and would hide a new chat model on
  the day it ships. A non-chat pick fails on the first send in the provider's own words; if that
  becomes a real irritation the fix is a capability the adapter reports, upstream, never a list
  here.

  **The cache is `strata_core::models`, a satellite beside the config and never a field of it** —
  history's precedent. A fetched list is a cache of a remote fact rather than something the user
  edited, so routing it through `write_config` would persist and broadcast a change nobody made
  and wake every reader of every setting. Same mechanism (`preferences`, the app's own `AppInfo`,
  the key `"models"` beside `"config"`), a missing file is an empty `Listings`, and it holds
  **names and timestamps only** — asserted on the serialized bytes, because the module next door
  is `secret`.

  **Refreshed at the point of use, not at launch.** A startup dial-out spends a round trip and
  puts a key on the wire per configured provider on every start, for a session that mostly never
  opens a model picker — and a read the user waits for has to be an *arm* rather than a freeze,
  which at startup has no surface to be an arm on. So `main` loads the file and calls nothing.
  Where a list is *shown* the cached one renders immediately and one background refresh runs per
  shown provider whose listing is absent or older than `STALE_AFTER` (**24 hours**, stated where
  the poll is: rosters move on the order of weeks, so a day behind costs one missing name and
  the recovery — Test — is already built). Enabling a provider is the other point of use, since
  that is the moment a person is setting one up and expecting it to reach out.

  **One request, two keeps, never two caches.** `probe::refresh` is the whole mechanism and every
  fetching gesture is a call into it — the Test press, a picker's staleness kick, the enable
  toggle — with the in-flight guard inside it rather than at each call site. The **names** go to
  the satellite, because they outlive the window and the run of the app; the **outcome** stays as
  a window-scoped `Probe`, which is why it carries a count and not a list: a probe restored from
  disk would be a "verified" nothing had checked, and a probe holding the names again would be
  the second cache. Retraction is one line for both (`SettingsCtx::forget_provider`), called
  wherever the address or credential moves, because dropping either alone leaves the other making
  the claim.
- **A cancelled turn is a drop, and the conversation it leaves behind must still be sendable.**
  Dropping the tool future *is* the engine's abort (`DispatchGuard`) — never a second abort
  path. But an assistant message carrying tool calls with no results is a request every provider
  rejects, so a cancel answers the outstanding calls with "the user stopped this turn" before it
  settles. A cancelled turn settles as `Cancelled`, never `Failed`.
- **A statement the user can run is a tool call, not a formatting convention — and the check in
  front of it is the *editor's* policy.** `offer_sql` is the assistant's own tool, dispatched by
  the loop and **never registered on the router**, so `tools/list` stays the router's own and no MCP
  client is offered a tool it has no transcript to use. A tagged markdown fence was built first
  and withdrawn: a fence is taught only by prose in the system prompt, which small local models
  follow unevenly, and it cannot check anything before the text is on screen. The tool validates
  first, so a card cannot offer SQL that will not parse — against the **editor's** capability,
  because the card runs in the user's editor, which is what lets the assistant hand over a write
  it is itself refused. SQL it is merely explaining stays an ordinary code block; telling the two
  apart is the whole point.
- **A window holds conversations, the pick is per conversation, and a step card is a citation.**
  `state::chat`'s `Chats` is the transcript satellite — several conversations, both the list and
  each conversation's turns capped, in the image of `state::agents` and `state::log`. Nothing here
  reaches `session.json` and nothing reaches `history.jsonl` (the **adoption** rule: a promoted
  tab's own Run press is what records).

  Each conversation carries its own `Pick`, seeded from Settings through `seed_pick`, which drops
  a provider that is no longer enabled — in Settings a disabled provider also loses its key, so
  "disabled" and "no longer usable" are one state rather than two the pane has to tell apart. The
  composer picks a **model**, grouped under the enabled providers, because a model belongs to
  exactly one provider and two pickers can disagree in a way that offers selections which cannot
  be sent; effort renders only when `efforts(kind, model)` is non-empty, and a rung the newly
  picked model does not offer is dropped rather than kept out of sight (`Brain::resolve` refuses
  one before a socket opens, and the control that set it is gone by then).

- **A conversation survives its window, and what has to survive is both lists — the turns the pane
  paints *and* the `Conversation` the model reads back.** AS-07's store is `.strata/chats/<uuid>.json`,
  a satellite on `history.jsonl`'s terms (one document per conversation, because a single file
  would make every turn in every chat rewrite every other one), gitignored through
  `ensure_gitignore` because a transcript quotes the user's own data and `.strata/` sits in
  people's repos. Storing only the transcript restores a conversation you can read and cannot
  continue — the *appearance* of one — because the resolved `@`-mention bodies, the tool results,
  the captured reasoning parts and the `offer_sql` call/response pairs exist **only** in the
  model's list, and a failed turn plus the differing caps make the two genuinely diverge. The seam
  is `Conversation::{to_json, from_json}`, **JSON-valued** so `genai` still stops at
  `strata-agent`'s edge; what rides on disk is therefore genai's own serde shape at the pin, and an
  upgrade that moves it bumps `CHAT_VERSION` or degrades to `Read::Memoryless`. Three tiers, one
  rule: the worst outcome is losing what the model remembered, never what the user wrote.

  The writes hang off the three points a conversation is known to have stopped changing — the turn
  task after its settle (race-free: AS-02 commits to the memory *before* it emits `Settled`), the
  stop press, and the subtree teardown, which writes **synchronously on the render thread** like
  `use_autosave`'s own `use_drop`, because a task spawned there dies with the scope and "it is
  there after a quit" is the feature. Only `dirty` conversations write, and `dirty` is set by
  everything that changes what would be stored — including a **pick**, through `Chats::repick`,
  which is the one funnel all three composer controls edit through.

  **A task that writes this subtree's state after an await must be cancellable by this subtree.**
  The standalone presses (open a stored conversation, delete, clear) use scope-bound `spawn`, like
  `clear_history`'s own writer: root-scoped, they would still be holding `Chats` and the report
  satellites if a re-root dropped the subtree mid-write. The turn task is the one that genuinely
  must outlive the pane, and the only thing that can reach it is `Chat::running` — so it holds
  that handle until its record is written and releases it with `Chats::finish`, not at the settle.
  A turn is not over until it is on disk. On the cancel paths the cancelled turn's commit may still
  land after the write; both interleavings are valid provider tails, so the bounded cost is a
  stopped turn the model does not remember — recorded, not fixed, because awaiting the settle would
  contradict "a cancel is a drop".

  Over-cap eviction **demotes to the shelf** rather than dropping, since the document is already
  stored — and `evict` *answers* what it shed, because `Chats` does no IO and a conversation still
  dirty when it left would leave its row pointing at a file older than the row claims. Reopening is
  a **read**: no run, no scan, no snapshot, no network. A restored `offer_sql` card is re-checked
  once with `tools.validate` (a dry plan, the one host call a reopen makes) and a stale one
  **degrades silently** to an ordinary code block — the user never ran it, and a complaint that the
  catalog moved is not news. That mark is **never stored**: it is an answer about the catalog as it
  stands, and persisting it would leave a card retired after the table it named came back.
  Retention is `Ai::max_chats`, rotated down on load like history's; **Clear is per project**,
  because the files are a project's and a Settings button is app-global. Both it and the per-row
  delete ask through one confirm mounted at the **window root** — a dialog mounted inside the pane
  it belongs to is a key barrier over nothing, since listeners fire in document order.

  A turn's blocks stay in **arrival order** — the model speaks, calls a tool, speaks again, and a
  transcript that hoisted every card to the bottom would separate its reasoning from its evidence.
  Every figure on a step card is the engine's own (`elapsed_ms`, the exact total, the stop's own
  wording), which is what makes AS-02's no-number-without-a-run prompt rule auditable; an
  `offer_sql` card is executable *instead of* a step card, never beside one. Promotion is
  `actions::open_sql` — a **new** tab, focused — and **never** a write to the user's buffer,
  which is often their only record of how a number was reached.
- **A turn is cancelled by dropping its task, and a dropped run still settles.** The send funnel's
  task owns AS-02's `Running`, whose `tokio_util` drop guard *is* the turn's cancel and the
  in-flight tool's engine abort — so there is no second stop path to keep in step, and the reply
  keeps everything that had already streamed, marked stopped (a conversation that erased what it
  was doing when you stopped it is one you cannot audit).

  One layer down, `agent::directory`'s `SettleOnDrop` sends the stop settle when a run's future is
  dropped, in the engine's own `CANCELLED` wording, disarmed on the normal path. A `Drop` rather
  than a `select!` arm because there is nothing to select on: a cancelled future never resumes to
  run a cleanup branch. Without it a stopped run left its satellite row reading `Running` for the
  window's life — AA-03c reaps such a row when a **connection** ends, which covers an MCP client
  hanging up and not the assistant, whose connection is its own mount in the window.
- **The in-app assistant is held like any other agent and told apart only where the user is owed
  a different sentence — and the mark is minted, never claimed.** The assistant is part of the
  app and its runs render as step cards in its own transcript; an MCP client is working in the
  project from somewhere else. It stays one more agent to everything below — its own `AgentId`,
  its own query sessions, the same gate — and the satellite **holds** it like any other, because
  the ownership check, the per-agent session cap and the teardown all have to work for it and
  `list_query_sessions` has to answer for it.

  The one place the two are told apart is `Agents::sessions_of`, which the close confirm asks so
  it can say "the assistant is running a query" rather than "an agent is" — the second would
  send the user looking for a client that is not connected. The mark is `Agent::in_app`, minted
  by `StrataTools::in_app` and delivered on the call that first tells a host an agent exists, so
  nothing holds an id to compare. Keying on `AgentIdentity::assistant()`'s name instead would let
  any MCP client claim its way across that line at `initialize`, which is the worst possible
  version of this rule.

  **Nothing lists agents at all.** The Agents pane (the sidebar tool pane, its rail toggle and
  its live-agent badge) and the header's agent-access status dot were removed on request: the MCP
  server still runs, and the app shows neither who is connected nor whether it is listening. A
  server that cannot bind reports through `tracing` and nowhere else. `state::agents` is now pure
  bookkeeping with no reader on screen, so it holds **only what the bookkeeping reads** — a run
  is a `seq` and an outcome, and the query text does not travel `AgentAsk::RunStarting` at all.
  A record kept for a surface must go when the surface does, or it is retention nothing can
  justify: the dispatch still gets the SQL, because that was never this channel's to carry.

  The satellite draws the line in **one** place. `Agents::held` is the unfiltered iterator,
  which `list_query_sessions` answers from (an agent must see its own sessions) and which the
  event log attributes from (the assistant is never out of the record). `Agents::sessions_of`
  is the line itself, for the close confirm, which asks whose work it is about to destroy and
  must say "the assistant" rather than "an agent" — sending the user looking for a client that
  is not connected is the failure that arm exists to fix. The pane's own `agents` / `len`
  projection went with the pane. The ownership check and the session cap are inside the
  satellite and read the field directly, so they never had a filtered view to avoid. And for
  the same reason, the log says the assistant **stopped** rather than disconnected: it never
  dialled in, so its "connection" is its own mount in the window.
- **A root-scoped task outlives the project subtree, so it asks before it writes one.**
  `spawn_forever` pins a task to `ScopeId::ROOT`, which is the right home for work that must
  outlive the *tab* or the *dialog* that ordered it — a drop confirm closes itself in the same
  tick it presses, and a scope-bound task there is cancelled before its first poll. But the
  project subtree is keyed on `(root, generation)` and unmounts wholesale on a re-root **and** on
  an engine restart (which is what a `runtime.*` Settings apply is), while the `EngineCtx` clone
  the task carries keeps the outgoing engine alive — so the call completes and comes back to a
  store whose owner has been dropped. `State::write_unchecked` panics on a freed box, and the
  release panic hook ends the process.

  Cancelling is one answer and the one `Chats::stop_all` takes, because a streaming turn holds a
  provider connection and an engine run that genuinely should stop. It is the *wrong* answer for
  a drop that is deleting a table's data, which has to finish. So the fork grows the predicate the
  situation actually wants — `State::is_alive` / `RadioStation::is_alive`, a liveness question the
  API could not previously express — and `refresh_table_rows` and both `drop_row` arms ask it
  after the await. The work happens either way; only the reporting is skipped, because there is
  nobody left to report to.
- **The catalog is the `ProjectState` store, not a query.** Never build a `FetchCatalog`
  capability: introspecting DataFusion hides the defs whose registration **failed** — precisely
  the rows the catalog exists to show, because a table that is merely broken has no engine
  presence at all and so is indistinguishable from one that was never defined. (This used to be
  argued from the `__snap_*` result snapshots surfacing too. ED-03's provider hides them, so that
  ground is gone and the rule does not need it — a `Reg::Failed` row is something no
  introspection can ever answer. Saved queries aren't a DataFusion concept either, and the store
  is also the ⌘S save target, so a cached second copy would be two sources of truth.) Mutations
  call the engine, then the store's own method on the matching `ProjChan`; nothing refetches.
- **An expensive, opt-in *result* is freya-query keyed by the request; the store holds the
  request.** Profiling (P3-09) is the shape: the row keeps `Option<ScanId>` — a nonce minted per
  ask — and the numbers live only in the cache entry that key names, with `stale_time(MAX)` (a
  settled scan must never re-execute itself) and `clean_time(MAX)` ("cached until the entry
  changes"). A re-scan is a *new* nonce, so it is a new execution; invalidating is dropping the
  request. Never a `profile` field holding results on the store, never a dedup set, never a
  spinner flag — the cache key is the dedup and `query.read().state()` is the spinner. And the
  `Query` (stale/clean times included) is the identity, so it is **built in one place**: two call
  sites spelling it differently are two entries, i.e. the same table scanned twice.

  **The holder of the request is whoever owns the surface, which is usually but not always the
  store** (DB-07). A relation inside a database connection's catalog has no `ProjectState` row to
  keep a request on — a database answers for itself, so there are no defs under it — and the rule
  generalizes rather than being excepted: the request goes in a window-side satellite
  (`state::catalog`'s `RemoteScans`, keyed by `RemoteRef`), never a remote row minted into the
  store. What does not move is everything else: the nonce, the cache identity, and the numbers
  living only in the freya-query entry. Its invalidation is a **reconciliation**, not an event —
  the entries whose connection is no longer `Ready` are dropped, which covers a Forget and a ↻
  (whose `reload_connections` passes every row through `Loading`) without either being noticed
  specially, and leaves a single table's Refresh alone because it touches no connection.
- **One entry point per expensive action, with the confirm in front of it.** Every trigger for a
  scan calls `ProfileActions::ask`, which raises P3-10's confirm on a first scan and goes straight
  through on a re-scan; confirming calls the same `start` the ↻ calls. Adding a surface means
  calling `ask`, never reaching for the store directly — the same rule the drop confirm holds.
  Every one of those calls takes a **`ProfileTarget`**, which is what says where the request is
  kept, how the name is rendered for the engine, and what a scan of it can compute: before it had
  two arms, a confirmed scan of a remote relation fell through the workspace lookup and started
  *nothing* — a cost the user had read and agreed to, with the panel still offering it.
- **A profile's expression set and its `FROM` renderer are one decision, made where the scanned
  name's identity is known.** `Profiled::{Workspace, Database}` (`engine/profile.rs`) is picked by
  `run_profile` off `providers::in_workspace`, and it decides both: a workspace entry executes
  here, so it gets the whole set and the fold-preserving `quote_ident`; a name in a database
  connection's catalog federates into **one statement the server runs**, so it gets the restricted
  set and the case-preserving `sql::qualified`. The median is the difference — DataFusion's is
  `approx_percentile_cont`, which has no PostgreSQL spelling and no dialect hook in DF 54, and a
  federated subplan has no per-expression fallback, so including it would fail the whole scan of
  any remote table with a numeric column rather than costing one fact. It is **dropped and
  stated** (`stats_footnote`), never substituted with a spelling nobody verified.
- **Def/runtime split.** `strata-model` holds pure serde defs only (exactly what
  `.strata/project.json` stores — no runtime caches, no UI flags). The Freya store wraps defs in
  rows with `Reg<T> = Loading | Ready(T) | Failed(String)`, making invalid combos unrepresentable;
  `defs()` is a pure projection for saving. **Identity:** tables/views are keyed by **name** (their
  engine/SQL identity, one shared namespace, case-insensitive compare); saved queries by a stable
  **`Uuid`**. Renames route through the store (a view rename rewrites tab `Origin::View` keys).
  **Connections** (W7) are the fourth def and follow the same split: `ConnectionDef` in the model,
  a `ConnRow { def, reg: Reg<()> }` in the store on its own `ProjChan::Connections`, keyed by
  **`ConnectionDef::url()`** — scheme *and* authority, which is what the object-store registry keys
  on. Not the bucket: `s3://lake` and `gs://lake` share one and are two connections, so a
  bucket-keyed fold lands both answers on whichever row comes first and leaves the other `Loading`
  for the life of the window, with no error anywhere to say so. The `()` is not laziness —
  connecting registers an object store and asks whether its bucket answers, but it infers nothing
  *about* the bucket, so there is no value to carry and the three `Reg` states are the whole answer
  (the pane's status dot). They live in the committed `project.json` beside the rest, which
  `CONNECTIONS_SPEC.md` §5 had left open against the gitignored session: a def carrying only a
  profile *name* and a key *file path* holds nothing a colleague may not have, and a catalog whose
  tables live in a bucket is not shareable if the bucket isn't.
- **A connection registers a bucket — or, for a database, a catalog — and it registers before
  anything that reads one.** A table's
  source path resolves through the object store registered for its bucket, so `register_pass` runs
  connections as its **first** phase — otherwise a perfectly correct table def fails with "no
  suitable object store found" and the diagnosis lands on the wrong row (and a view over
  `pg.public.orders` fails to plan). Connections need no
  ordering among themselves and get no fixed-point retry (each registers one bucket and reads
  nothing the pass provides). A **whole-catalog ↻ re-connects; a single table's Refresh does not** —
  a re-connect is what fixes the case ↻ exists for (fill in the region, run `aws sso login`), and
  putting a credential round trip behind a one-table gesture buys nothing. **Ambient and Named
  profile are two providers, not one chain with a setting**: `aws-config`'s default chain is
  unconditionally `Environment → Profile → …`, so naming a profile on it lets an exported
  `AWS_ACCESS_KEY_ID` sign instead, silently, with the row still green.
  And because a connection's identity is its **URL** while its list is sorted by **address**, the
  two keys are never interchangeable: `upsert_connection` replaces on the URL and inserts at the
  address's slot, an **edit that moves either half deregisters the old URL itself** (`connect` is
  additive and only ever sees the def it is given, so nothing else ever would), and the editor's
  Save asks for a **whole-catalog** pass — the width connections belong to, and the one that
  re-registers the tables that were reading the store it just replaced.
- **Connecting asks the bucket, because a description can be well-formed and wrong.** `connect` is
  `prepare` — the provider's naming rules, the client options, the registry key, and a store built
  from all three — followed by `reachable`: the **first page** of `list(None)`, taken with `next()`
  and the rest of the stream dropped unpolled, so it costs one request however large the lake.
  Deliberately **not** `list_with_delimiter`, which reads like the cheaper call and is the more
  expensive one — it drains the whole paginated stream to assemble a `ListResult`, a round trip per
  thousand top-level prefixes, which is the exact cost this probe is shaped to avoid.

  This **overturns** the rule that stood here before it, and the reversal is worth stating plainly
  because the original had a real argument. `connect` used to stop at the credential chain, on the
  grounds that asking the bucket is a network round trip per connection on *every project open*.
  It is. What that argument did not cover is a def that is **well-formed and wrong**, which no
  local check can see: `AmazonS3Builder` validates a bucket name against nothing, so a mistyped
  region built a perfectly good store, registered **green**, and left every table under it failing
  with `object_store`'s bare "Received redirect without LOCATION" — a message that names no bucket,
  no region and no connection, and which the sidebar then truncated at the comma, discarding the
  only clause that hinted at the cause. One request per connection at open buys a status dot that
  means what it says. A cross-region refusal is reworded **by name**, naming the region, because
  `object_store` can only guess at a field it has never heard of and we have the value in hand.

  **It asks whether the connection is *described* right, not whether it may do everything**, and
  that line decides which failures are fatal. It has to be drawn there: `connect` is
  `register_pass`'s **first** phase, so no table has registered and there is no prefix to probe
  with, leaving a **root** listing as the only question available — a far stronger demand than
  Strata makes of the bucket. So `Generic` (the bare redirect of a wrong region) and `NotFound`
  refuse, while `PermissionDenied` and `Unauthenticated` **register**, matched on
  `object_store::Error`'s own variants rather than on its prose. The two allowed arms are ordinary,
  not exotic: an `s3:ListBucket` conditioned on `s3:prefix: ["team/*"]` is AWS's documented way to
  hand somebody a folder and answers 403 at the root while `s3://lake/team/events/` reads fine, and
  a published dataset granting `GetObject` but not `ListBucket` does the same — refusing either
  would take a working project's every table down with the connection. What that gives up is
  catching **rejected credentials** here; they fail at the first table instead, which is exactly
  where they failed before the probe existed, so this declines a new win rather than losing an old
  one. One case cannot be helped: S3 answers 403 rather than 404 for a bucket that does not exist
  when the caller lacks `ListBucket`, so a mistyped bucket under a scoped policy still registers.

  **HTTP is exempt**, and not out of laziness: `object_store`'s HTTP store lists over WebDAV
  `PROPFIND`, which most origins serving files do not implement (MinIO included — it is why that
  integration test's HTTP arm reads a single object). Probing one by listing would refuse working
  connections for a verb the server was never going to answer, which is a worse lie than the one
  this exists to remove. An HTTP connection names an origin and its table names the object, so the
  table's own registration is where its reachability is genuinely tested.

  The **connection editor needs nothing of its own**, and that is worth recording because the
  obvious design is wrong. A Save-time `Engine::check_connection` — the same judgement without
  registering, so an unreachable bucket was never written to `project.json` — was built first and
  withdrawn. It is **redundant**: Save already writes the def, asks for a whole-catalog pass, and
  watches its row, and `connect` running in that pass now asks the very same question; the probe
  was a second round trip to learn what the first was about to say, and a def written and then
  refused is exactly what already happens for a credential chain the server rejects. And it was
  **expensive in the wrong place** — it put a network call carrying `object_store`'s ten retries
  behind a button three interaction tests press, taking `strata-freya`'s suite from 7 seconds to
  308. A refusal arriving one round trip earlier does not buy that.

  The cost to carry: **`connect` now does network I/O**, so a test about naming or keying must go
  through `store::settle` (the all-or-nothing half, shared with `connect` rather than restated) and
  not through `connect` itself — otherwise the unit suite dials out to buckets nobody owns and
  fails offline. The network half belongs to `tests/object_store_minio.rs`, against a bucket that
  is really there.
- **A database connection is a fourth `Provider` arm that registers a catalog, and the whole
  database comes through it — discovery gets catalogs, declaration gets defs.** (DB-02.)
  `Provider::Postgres(PgStore)` is the same `ConnectionDef`, the same `ConnRow`/`Reg<()>`, the same
  editor window, the same `register_pass` phase 1 and the same Forget: the `TableOrigin` lesson,
  applied. What differs lives in `engine::db` and nowhere else — the def's `catalog` field (the
  first on any provider that is an **SQL identifier**, because SQL cannot address
  `postgres://host/db` and relations must be reachable as `pg.public.orders`), a connection **pool**
  whose construction *is* the probe (DNS, TCP, auth, `SELECT 1`, all-or-nothing exactly like
  `store::connect`, so there is no separate `reachable` step), and a catalog provider registered on
  `StrataCatalogList`. Both arms settle through **one** `connect::settle`, which takes the
  take-back as an argument: the registries differ, the contract does not.

  **No per-table defs, no manual adds** (settled with Alex, 2026-08-13). Connect enumerates every
  schema the role can see and every relation in them, in one round trip, and lists them lazily —
  three-part names, remote schemas preserved. A def per remote table was considered and rejected:
  it restates configuration the server owns, goes stale silently, costs an introspection per def
  per pass, and mints failure states for things whose only real failure is the connection's. The
  line is *discovery gets catalogs, declaration gets defs*: a bucket cannot say what its tables
  are — someone must declare globs, a format and options, and that declaration can fail, which is
  what the `Reg` rows exist to show — while a database answers for itself. Pinning one remote
  relation into the workspace is a **view**, which needs no new machinery at all.

  **The provider is ours, and the three reasons are in the crate's source.** Its own
  `DatabaseCatalogProvider` snapshots the schema and table list at construction (a ↻ could not
  refresh it), builds plain `SqlTable`s with the default unparser dialect, and skips the federation
  wrapper — so the generic path silently forfeits exactly the pushdown this workstream exists for.
  Ours builds one level below `PostgresTableFactory` — that factory's own three steps written out
  (`engine::db::federate`, DB-08), so a rewrite hook can ride the executor — and **caches a
  provider per
  relation**, so diagnostics' validation costs one remote introspection per relation per connect
  rather than one per keystroke; `SchemaProvider::table_type` is **overridden** to answer from the
  cached `relkind`, because its default is `self.table(name).await` and `information_schema.tables`
  calls it for every relation in every catalog — with the override `SHOW TABLES` costs zero remote
  calls. (`information_schema.columns` still builds providers; bounded by the cache, and accepted.)
  The listing reads **`pg_class`** (`relkind IN r,p,v,m,f`), not the crate's `pg_tables`: remote
  views, matviews, partitioned and foreign tables must show and resolve, or the tree lies about
  what is queryable. `UnsupportedTypeAction::String` maps `jsonb` and other exotics to `Utf8` JSON
  text the app's own accessors already read — the crate's default (`Error`) would make any table
  with one such column entirely unreadable, and this is representation honesty rather than silent
  corruption: the value is intact, only the type is wider.

  **Schema visibility scopes display, never resolution.** `PgStore.schemas` is committed
  configuration (DataGrip's "N of M schemas"); registration exposes every schema regardless, so a
  query naming a non-enabled one still runs. `Engine::db_listing` is the one read every surface
  shares and it answers **scoped and tagged** (`Live | EnabledButMissing | NotEnabled`), so no
  consumer re-derives visibility. It reads the connect-time enumeration, which is why a ↻ — a
  re-run of the registration pass — *is* the refresh.

  **Read-only against the database in v1**, and a reconnect **replaces**: `db::connect` deregisters
  whatever that URL last registered, under the name it went in under, so the editor's rename (same
  URL, new catalog name) is handled by construction rather than by a surface remembering.
- **A JSON accessor over a remote column is rewritten into the server's own operator, and a family
  member with no faithful spelling is refused by name rather than approximated.** (DB-08,
  `engine::db::json`.) `->` / `->>` / `?` are planned as `datafusion-functions-json` UDF calls, and
  a UDF call unparses **by name** — so without a rewrite a federated subplan carries
  `json_as_text(payload, 'type')` to a server that has no such function, and federation has no
  per-expression fallback to catch it. The rewrite is an AST pass on the executor's `ast_analyzer`
  seam, which is why the provider is constructed one level below the crate's factory; it is
  reachable only from the remote SQL path, so local JSON is untouched.

  **One table is the whole of it**, and it is the only source of "mapped": `json_as_text` → `->>`
  (an arrow chain for a path), `json_contains` → `IS NOT NULL` over that chain, everything else
  unmapped. Unmapped is a **judgement per member, stated**, never an omission — `json_get` returns
  Arrow's JSON union, which no Postgres expression produces; `json_get_str` is NULL where `->>`
  stringifies an object; `json_get_json` hands back the source slice where `->` hands back
  normalised `jsonb`; `json_length` counts object keys as well as array elements. A mapping that is
  close enough makes a query's answer depend on where it ran, which is worse than a refusal —
  which is also why `json_contains` is **not** `?` (Postgres's `?` is true for a string array
  element and takes no integer index; the local function is false for both).

  The refusal names the function, the connection and the way out (copy the rows in with a CTAS,
  and for `->` the spelling that does push down), minted beside the table because the table is
  what knows — and a *mapped* accessor called with no key to look up says **that**, rather than
  that the accessor is unsupported, which the same call with a key would disprove. The failures
  only the server can catch — a created SQL macro that survived `simplify`, an accessor over a
  `text` column — keep Postgres's own words with that same way out after them, wrapped where the
  error is born (the executor knows it is remote and which connection it is), never by
  string-matching in the generic run path. Recognised by the **code**, `SQLSTATE: 42883`, which
  the provider crate renders into every server error it hands back: the prose has at least three
  wordings (`function … does not exist`, `operator does not exist: …`, `could not identify an
  equality operator for type …`) and matching those would miss the third while firing on any
  message where the words merely co-occur.
- **A connection's password lives in the OS keystore under a ref *derived* from the connection's
  identity, and the def stores only the expectation.** (DB-02, and the deliberate rewrite of W7's
  no-secrets rule — see the entry below on `strata_core::secret`.) `PgPassword::{None, Keystore}`
  is the whole of what `project.json` carries; the reference is
  `SecretRef::derived("pg-password", def.url())`, a `Uuid::new_v5` over a fixed namespace.

  **Not a minted ref, and the reason is git.** A minted `SecretRef` in a *committed* file would be
  rewritten by every colleague who entered their own password — two machines ping-ponging one id
  through the project file forever. A derived one is the same slot on every machine while each
  machine's keystore holds its own entry, and storing it beside the fields it derives from would be
  two statements of one fact that can disagree the moment the identity moves.

  Three consequences ride with it, and each is carried rather than papered over. An identity edit
  (address or user) moves the ref, so whoever moves it **migrates** the entry
  (`secret::migrate_derived`: get → put → delete, best-effort about absence, loud about a keystore
  that refuses) — exactly as a moved URL already deregisters itself. A Forget deletes the entry
  without needing a stored ref. And on a machine with no entry the row settles **failed naming the
  fix** ("No password is stored on this machine for '…'"), the same honest shape as an expired SSO
  session — and re-entering it touches nothing in git.

  The value itself is read **per pool connection** and never cached (`db::KeystorePassword`, on
  `spawn_blocking` because a keystore call is a blocking platform call), and the password seam is
  an **argument** to `db::connect` rather than something it reaches for — which is what lets the
  integration test substitute the *keystore* (`keyring_core::mock`) while still driving the real
  bridge through `Engine::connect`.
- **A table reads through a connection by naming it, and the composition happens once, in
  `resolve_source`.** `TableDef::connection` is the connection's `url()` and nothing else about it
  (W7 · 04): a *reference*, because the bucket, the provider and where its credentials come from
  all belong to the connection, and a second copy here is two statements of one fact that can
  disagree. It is also the **one** field that says a table is remote — a source is bucket-relative
  exactly when it is `Some` — so the two halves cannot contradict each other, and the LOCATION
  toggle that produces it is an explicit choice rather than a scheme parsed out of a typed path
  (spec §4). `resolve_source(root, connection, source)` takes the connection precisely so a caller
  cannot reach for the wrong rule: `s3://` is not an absolute *path*, so the local rule silently
  turns a bucket-relative source into `<project>/events/2024/` and reports a missing folder on the
  user's own disk. The engine needs **nothing** for this beyond the composed string — the store
  went in under that same URL in the pass's first phase — and `relativize` is skipped on the way
  back out, because a bucket-relative path has nothing to do with the project folder. Forgetting
  a connection therefore has a **consequence**, and the confirm names it: the tables whose def
  reads through it (`tables_over`) and the views behind those (`views_over`), which is the reading
  a table drop already reports. In the Configure window the two locations hold **separate** paths
  (`ConfigureDraft::local_sources` / `remote_source`, projected by `nonblank_sources`) and the
  toggle moves none between them: a list written against the disk and one written against a
  bucket are different answers to different questions, so carrying the first over wrote
  `/data/events.parquet` under a bucket that had nothing to do with it — and, from an empty list,
  seeded a blank row in the one section whose toolbar is absent, so a remote table's only path was
  a row nobody added and nobody could remove. Nothing seeds either arm; an empty one blocks Save.
- **A connection's address is its provider's own, and every rule about it lives in one place.**
  The field is `address` rather than `bucket` because the three providers do not address the same
  thing: S3 and GCS name a bucket whose scheme the provider states, while HTTP names a **whole
  origin URL** — its scheme is the user's answer, not the provider's, since `http://` and
  `https://` are two different origins. So there is no `Provider::scheme`, no prefix chip and no
  scheme picker; a path is **refused naming the part to drop**, never trimmed, because the
  registry keys on scheme and authority. What a legal address is, is `Provider::check_address` —
  called by `engine::store::connect` *and* by the editor, so a name refused at the field is
  refused by the store in the same words. The two bucket rule sets are the providers' published
  ones and are genuinely different (GCS takes underscores and a 222-character dotted name and
  reserves `goog`/`google`; S3 takes neither and refuses `..`), which is exactly why one copy.
  **Client options are the def's other half** (`client_config`, `object_store`'s own
  `ClientConfigKey`s): on the def rather than in a provider, because all three stores are built on
  one HTTP client; offered from `CLIENT_KEYS` and refused by `check_client_config`, the same call
  on both sides again. `allow_http` is not offered — it is the S3 endpoint's toggle, and on an
  HTTP connection it is **derived from the scheme the user typed**, since reqwest is built
  `https_only(!allow_http)` and would otherwise refuse every plain-`http` request before it left
  the process. Full model:
  [ENGINE.md](ENGINE.md), spec: [CONNECTIONS_SPEC.md](../CONNECTIONS_SPEC.md).
- **An internal table is an ordinary def whose data Strata owns, and `TableOrigin` is a flag on
  that def rather than a second kind of thing.** A `CREATE TABLE` / CTAS spools its result into
  the project's `.strata/tables/<slug>/` as Arrow IPC and then registers it through
  `register_external` like any other table (ED-04), so the store fold, the persist funnel, the
  scan driver, the headless host and replay all handle it with **zero new code** — which is the
  whole argument for a flag: splitting the type would make every reader match on two shapes to
  ask nothing. What the flag answers is three questions and no more: may a write statement target
  it (`Engine::is_internal`, an engine-side set of folded names rebuilt by the same registration
  pass — never a second catalog), does dropping it delete data (ED-05), and can Configure edit it
  (no — the item is *absent* from the row menu, which is what makes the window structurally
  unable to receive an internal def). **The def travels and the data does not**: `project.json`
  carries it, `tables/` is gitignored, and a clone gets an honest `Reg::Failed` row saying the
  data is local to the machine that created it — never the external vocabulary ("no source at
  …"), which invites the user to repair a path that was never theirs. The row says which origin
  it is, because that is what stands between the user and a drop that means two different things.
  **The spool is the parsed plan, never re-rendered SQL** (`CopyTo` over
  `CreateMemoryTable.input`), so the query that runs is the query the user wrote and DataFusion's
  own exhaustive clause refusals come for free. Spec: [STATEMENTS_SPEC.md](../STATEMENTS_SPEC.md)
  §6.1 + §7.
- **An internal table is a third LOCATION in the Configure window, not a surface of its own.**
  `Where::{Local, Remote, Internal}` — the word the catalog row's `INTERNAL` chip and
  `TableOrigin::Internal` already use, so one thing has one name. Creating a table Strata stores
  is the same question that window already asks (what is it called, what is in it) with a
  different answer to *where*, so it belongs in that control; what changes below it is which
  sections have anything to ask. On `Internal` the FORMAT picker, SOURCE PATHS, the import
  options and HIVE all draw nothing, and a **COLUMNS** list takes their place — built from the
  paths list's own `Table`, `+`/`−` `ToolButton` toolbar and two-way-synced bare fields, because
  it is that control with two boxes instead of one. The segment is **shown and inert** on an
  edit: a table that already has files cannot be turned into one Strata stores without silently
  discarding the def that points at them, and a segment that vanished would change the control's
  shape. **A modal panel behind a two-item menu on the catalog's `+` was built and rejected**
  (Alex, 2026-08-13): it asked the same question on a second surface, in a dress that matched
  nothing else in the app. Do not re-propose it.
- **A surface that makes a table composes a statement and folds it through `settle`; it does not
  register anything itself.** Configure's Save branches on `Internal` (`views::footer`): it
  composes one visible statement — the same one the COLUMNS list describes — dispatches it
  through `Engine::run` on a **minted** `WsId` (a tab's would abort whatever that tab is
  running), and hands the report to `apps::project::state::settle`. That last step is the
  load-bearing one: the `StoreEffect` is what puts the row in the store, the def in
  `project.json`, the epoch bump behind every tab's diagnostics and the entry in the log, so a
  gesture that ran the statement and stopped has made a table the catalog never learns about.
  Never a second `apply`, persist path or epoch bump — and no def is written here at all, because
  the spool that gives the table its data has none to be written from. The wait is the
  **existing** one: `Status::Registering(name)` makes Save read "Validating…", and
  `use_watch_registration` closes the window when that row lands `Ready`, which the fold makes
  true in the same breath. Nothing new watches anything. **And the window will not close while
  that create is in flight** (`Status::Creating`, answered once by `Status::holds_window` for
  both Cancel and Esc): the fold runs *after* the spawned task's await, and
  `ddl::tables::create` publishes its spool by **rename** before its own last await, so a window
  dismissed mid-create would leave a data directory under `.strata/tables/` that no def points at
  and `tidy_strata_dir` never sweeps. The engine's abort is not the gap — dropping `Engine::run`'s
  future runs `DispatchGuard`'s drop, which aborts the detached task — but an abort is delivered
  at the next **await**, and `create` has none left after `register_external`. `Registering` is
  deliberately *not* held: that pass is the project window's and answers on the catalog row
  whether this window is watching or not. **And a window that refuses to close has to say so to
  winit as well as to itself** — Esc and the Cancel button are in-app presses, but the native
  traffic-light button and ⌘Q both route through `process_close_request`, which closes
  unconditionally when a window registered no `on_close`. So the predicate is mirrored into an
  `Arc<AtomicBool>` built with the `WindowConfig` and read by a `with_on_close` hook, the shape
  `project::close::close_bridge` already uses. Gating only the in-app paths leaves the two that
  do not pass through them.
- **A form over a statement authors only what a form can be wrong about; every other refusal is
  the arm's own, reached rather than restated.** The panel's own vocabulary is four sentences (a
  row with no name, a row with no type, a table with no name, a table with no columns).
  Everything else is the engine's: `ddl::tables::duplicate_column` for a repeated column,
  `unenforced_clause` for the constraint and default the create arm refuses, `fold_ident` for
  *which* names collide (a case-insensitive compare refuses `"my col"` beside `"MY COL"`, which
  the create accepts), and `ProjectState::name_taken` for a name the catalog already has —
  shared with the Configure footer, which asks the same question. **The type field is free text
  probed per row**: there is no Arrow → SQL inverse to author a picker from (the mapping is
  many-to-one, and `map_string_types_to_utf8view` and `execution.time_zone` make it
  config-dependent), so `Engine::column_type` plans `CREATE TABLE __strata_probe (c <typed>)`,
  executes nothing, and answers in `short_type`'s spelling — the one the grid and the inspector
  will show, so the form promises exactly what the user is about to get. **Eager per row is the
  requirement, not a nicety**: validation deferred to the press means filling eight rows and
  hunting for the one that was wrong, which is worse than typing the statement by hand. The probe
  therefore also runs the arm's clause refusals and requires exactly one planned field, because
  `INT PRIMARY KEY` plans clean and `INT, b INT` plans two columns.
- **A table is dropped in one place, on both origins, and a confirm is a gesture in front of that
  place — never a second implementation of it.** `ddl::tables::drop_table` (ED-05) is the whole
  drop: resolve the target against the engine's own namespace (an unknown name errors, `IF EXISTS`
  reports a no-op with nothing for the store to fold, a view says which statement drops it),
  **deregister first** so no plan built afterwards can resolve the name while a scan already
  running finishes against its own provider, delete `.strata/tables/<slug>/` **only** where the
  def is internal, and answer with `StoreEffect::TableRemoved`. The catalog pane reaches it through
  `Engine::drop_table` after its store-first write (the `save_view` order — a drop the project file
  never heard about comes back on the next open); a typed `DROP TABLE` reaches it through the
  router. **That sharing is the point, not tidiness.** As this task was first drafted the two were
  separate: the statement deleted the data directory and the pane's `Engine::deregister` did not,
  which orphans an internal table's data forever — no def points at it and `tidy_strata_dir` only
  sweeps `.tmp-*`. The engine's own bookkeeping (`cancel_profile`, the internal-name set) is
  applied by `Engine::settle_effect` off the returned value, once, for the same reason. **No
  cascade**: dependent views are *named* in the report, read from the providers before the
  deregister — a `ViewTable`'s plan was inlined when it was created and goes on executing until
  reload (D11), and the epoch the fold bumps is what re-derives every tab's diagnostics, which is
  the surface that actually tells the user. **The two wordings are the engine's**
  (`ddl::drop_intent` before the fact, `drop_report` after), because a fixed "the source files on
  disk are not deleted" in the confirm was reassuring the user at exactly the moment the action
  became destructive.
- **A table's data is discarded by rename, and the drop that does it is background work.**
  `ddl::tables::discard` moves the directory into a `.tmp-…` sibling and only then walks it — the
  mirror of the spool, which publishes by rename for the same reason. A `remove_dir_all` in place
  is interruptible at every step, and what an interruption leaves is a half-emptied directory under
  the table's *real* name that nothing ever collects: the def naming it is already gone and
  `tidy_strata_dir` sweeps only `.tmp-…`. After the rename the data is unreachable under that name
  whatever happens next, and what is left is exactly what the sweep exists for. **The rename is the
  operation; the removal is housekeeping** — a failure to finish it is logged, never returned, or
  the app would report a failed drop for one that plainly succeeded. A failure of the *rename*
  is returned, and **puts the provider back**: the deregister comes first so nothing can plan
  against a table whose files are going, which leaves the one fallible step after it, and a
  `discard` that could not even start destroyed nothing. `deregister_table` hands back what it
  removed, so the same `Arc` goes home and the drop is all-or-nothing on the engine — otherwise
  the report says "failed" while the irreversible half has landed, and the def sits in
  `project.json` naming a table the session can no longer resolve. And because an `INSERT` is one
  file with no compaction, a heavily written table is a directory of thousands of files: the delete
  is not instant, so `Engine::drop_table` holds a `BackgroundGuard` for its whole await. That guard
  is `Lifecycle::background`, the count `export` already used — one counter, because every reader
  asks the same question ("is anything the user would rather finish still going?") and the
  close-while-running flag is the only consumer. The confirm's copy follows: `whose_work` answers
  `Mine`/`Agent`/`Background`, since "Queries are running" shown to somebody deleting a table sends
  them looking for a query they never started.
- **A write statement only ever reaches files Strata owns, and the gate is the *parsed* target.**
  `INSERT` (ED-05) plans the statement — side-effect free — and gates on what the plan names: a
  target outside `Engine::is_internal` is refused (`Blocked::InsertExternal`, which covers a view
  too, neither being a directory a `CREATE TABLE` wrote), and a write op that is not `Append` is
  refused (`Blocked::InsertOverwrite`, which the router already produces for `INSERT OVERWRITE`
  off the bare statement, and which the arm produces for `REPLACE INTO` — DataFusion folds both
  onto the one thing the Arrow sink has no implementation for). Everything else is DataFusion's
  own INSERT path: the column list, the source query, the schema check
  (`logically_equivalent_names_and_types`) and the single LZ4-frame IPC file the sink appends.
  **The plan that was judged is the plan that runs** — driving it *is* `execute_logical_plan`'s
  own arm for a DML node, so re-dispatching the text through `ctx.sql` would only gate one value
  and execute another. **One file per statement and no compaction**, stated in the module doc:
  `DROP TABLE` plus a `CREATE TABLE AS SELECT * FROM t` is the compaction story until a task owns
  one. The row count on the catalog row comes back through `StoreEffect::RescanTable`, never from
  the store adding up what a statement claimed.
- **An append re-reads the table's facts; it does not re-register it, and it leaves the views
  alone.** `StoreEffect::RescanTable` folds through `refresh_table_rows` → `Engine::table_meta` →
  `ProjectState::table_registered`, not through the scan pass. The distinction is the one D10/D11
  settled: a `ViewTable` captures its sources by `Arc` at creation and never re-resolves, so what
  strands a view is a **provider replacement** — which is what `register_external` does, and the
  only reason a table Refresh re-creates the views above it. An `INSERT` replaces no provider, and
  could not invalidate one if it did: `logically_equivalent_names_and_types` runs before the sink
  writes, so the shape a view captured is the shape still there, and the provider re-LISTs per
  scan (no `ListFilesCache`) so it finds the appended file unaided. Going through the pass broke
  every view above the table and repaired it again for nothing, re-inferred a schema that could
  not have moved, and flashed each affected row through `Loading`. The count is still *read* —
  `table_meta` re-LISTs and totals the footers, of which only the new file's is uncached — so
  "never store-side arithmetic" holds. No epoch bump either: the data moved, not what a name
  resolves to. Stale *profiles* on dependent views are unaffected, being
  `ProjectState::invalidate_readers`', which its own doc says exists "for the landing path that
  does **not** re-create them". A table **Refresh** keeps the full pass, because it re-infers from
  whatever is on disk now and so genuinely may move the schema — the caller is what knows. And
  because the re-read runs **outside the scan driver's claim**, it lands through
  `table_reread`, which stands down for a row a pass has put back to `Loading`: otherwise a ↻
  pressed while the read was in flight would be silently undone by the state from before it.
- **A typed `CREATE EXTERNAL TABLE` is Table Config's registration written down, and its `OPTIONS`
  are the table's reader — never the store's.** `ddl::external::create` (ED-10) reads the parsed
  statement into a `TableDef { origin: External }` and hands it to `register_external`, so the
  store fold, the persist funnel, replay and the headless host need no code of their own and the
  settle is CTAS's. DataFusion's own `ListingTableFactory` stays unused for §3's reason once more:
  it registers a provider behind the store's back, and the **def** is the durable artifact — a
  table that existed only in a `SessionContext` would appear in no catalog row, no `project.json`
  and no clone. The statement is *read*, exhaustively and with no `..`, so every clause a `TableDef`
  cannot carry (`TEMPORARY`, `UNBOUNDED`, `WITH ORDER`, constraints, a **data** column list) is
  refused by name; `STORED AS` is exhaustive by name too, so a format with no reader fails rather
  than falling through or minting the `Unknown` variant a *legacy def* needs; and a partition name
  repeated in `PARTITIONED BY` is refused, because Arrow's `Schema` permits duplicate fields and
  would have carried the column twice. The def reaches the funnel through **`register::table_spec`**,
  the mapping the registration pass already owns, so what the statement writes composes exactly as
  the next open will replay it. An **internal** table's name is fenced off from a *replacement* —
  pointing it at the user's own directory would strand `.strata/tables/<slug>/` with no def naming
  it and nothing left that could delete it — and from a replacement only, since `IF NOT EXISTS` and
  a plain create perform none and would otherwise be answered with advice to drop a table the
  statement asked not to touch.
  **`OPTIONS` is two vocabularies wearing one syntax, and that is where this statement collides
  with connections**: `datafusion-cli` writes the reader's settings and the object store's into one
  list, where Strata keeps them in two files on purpose. So a `format.` key the def has a field for
  is read onto it (the key set **is** the def — every `CsvRead`/`JsonRead` field has a DataFusion
  name and nothing else does), a store namespace or a client option (`CLIENT_KEYS`, shared) is
  refused toward Connections **on the key alone** — the value is never read and never echoed,
  because it may be a secret — and everything else is refused by name, which is what keeps the
  mechanism total rather than a list of the keys we thought of. A `LOCATION` with a scheme is
  `resolve_source` read backwards (`project::split_remote`, round-trip asserted) onto a connection
  **this project has**, refused by name otherwise: a statement cannot mint one (it says no
  provider, no region and no credential, and must never carry the last), and refusing here is what
  keeps DataFusion's "No suitable object store found" off a table row. Membership is
  `Engine::connections`, noted by `connect` **whatever the outcome** and removed by `disconnect` —
  the internal-name set's shape, names and nothing else — because whether a connection resolved a
  credential today is not whether the project has it. It **resolves** rather than tests: the match
  falls back to case-insensitive, since `Url::parse` lower-cases a scheme and a host on the way
  into the registry, and it answers with the *connection's* spelling, which is the string the def
  stores and the string every other surface addresses it by. None of this contradicts Configure's LOCATION
  toggle being an explicit choice: that rule exists so a typed **path** is never re-read as remote,
  and here the scheme is the only thing the statement says about where the files are (a `file://`
  URL is refused naming the plain-path form rather than decoded).
- **A view is Save's artifact, and typed view DDL is a second gesture into that funnel — one
  body, views indistinguishable by origin.** `ddl::views::create` is what
  `Engine::create_view` spawns for ⌘S *and* what a typed `CREATE VIEW` reaches through the router
  (ED-06), so one store row, one `project.json` entry and one set of deps serve both, and either
  gesture edits the row the other made. The statement is never run natively for two reasons, each
  disqualifying on its own: DataFusion's `CREATE OR REPLACE VIEW` over a **table** name silently
  replaces the table (`context/mod.rs`, the `(true, Ok(_))` arm never checks `table_type`), so a
  typo would turn a registered parquet table into a view while its def went on naming files
  nothing reads; and the store write-back needs a `ViewMeta`, which introspecting for afterwards
  is the refetch the catalog invariant forbids. `Blocked::CreateView` / `DropView` stay defined as
  the **agent** path's refusals.
  **`ViewDef` is `{ name, sql }`, so the arm has to arrive at exactly that pair**: the folded name
  (`TableReference::parse_str`, DataFusion's own normalization) and the definition **query's**
  canonical rendering, which is what makes the row round-trip — it is the string ⌘S would have
  saved from a tab holding that query. And because the statement is *rebuilt* around that query,
  DataFusion's own clause gate never sees the user's spelling: every clause `CREATE VIEW` can
  carry is therefore refused **by name**, from a destructure with no `..`, or it would be accepted
  and silently ignored (`CREATE TEMPORARY VIEW` creating a permanent one). A clause sqlparser
  learns later is a compile error, the rule the router's wildcard-free match keeps from the other
  end. A table name is refused whether or not `OR REPLACE` is written, and a plain create over an
  existing view points at `OR REPLACE` rather than at DataFusion's "Table 'v' already exists".
  **A view drop names its readers and does not cascade**, in the table drop's own words
  (`ddl::left_invalid`, shared so the two cannot describe one consequence two ways) — from
  `catalog::dependents_of_view`, which reads the **aliases** half of `PlanDeps` where the table
  drop reads the tables half: the inliner leaves a view's name behind as a `SubqueryAlias` and its
  base tables at the leaves, which is the same split the store keeps (`ViewInfo::deps` vs
  `view_deps`) and so makes the report and the pane's warning one fact. That half is **raw** and
  **over-reports on purpose**: a plan cannot tell an inlined view from `FROM t AS v` or a CTE
  named `v`, and a *missed* reader is a destructive action reported as consequence-free where a
  spare one is a name the user can look at. It is not a divergence from the pane either — the
  store's filter keeps an alias only where a view row of that name exists, which is always true of
  the name being dropped. A redefined or dropped
  view's profile is cancelled by `Engine::settle_effect` off the returned effect, because the
  statement runs in a task that cannot reach the lifecycle; the direct gestures cancel in
  `create_view` / `drop_view`, which never produce an effect. Replay needs no code of its own — a
  typed view is a `ViewDef`, and `register_pass`'s fixed point orders a chain from cold exactly as
  it does a saved one. Spec: [STATEMENTS_SPEC.md](../STATEMENTS_SPEC.md) §6.3.
- **A name is rendered into a statement by one of two renderers, and which one is decided by
  whose identity the name is.** `engine::quote_ident` is **fold-preserving**: it renders a name
  so DataFusion resolves it to `fold_ident(name)`, which is the identity a workspace def has
  actually been registered under — so it lower-cases `DailySales` on purpose, and a name that
  already worked keeps its exact old identity. `sql::quote_verbatim` is **case-preserving**: the
  name survives the parser exactly as spelled, which is what a relation whose spelling belongs to
  a *server* needs, and `sql::qualified` renders a dotted name **segment by segment** through it
  (quoting `pg.public.orders` whole would make it a bare relation with dots in it). The two are
  not interchangeable and the wrong one is silently wrong in opposite directions: the folding one
  over a remote `"Orders"` asks the server for a relation it may not have, and the preserving one
  over a workspace def re-keys a def every sibling addresses by its folded name.
  `export::quote_col`, which quotes unconditionally, is a third rule for a third reason (a
  projection list, where nothing has to resolve back to a stored identity). The pair lives in
  `sql::ident` — it is the completion insert's own rule, lifted out of it at DB-06 so the
  data-sources tree's gestures compose their `FROM` through the same function rather than a
  fourth spelling of it; DB-06's own Pin as view uses **both**, one per name in its
  `CREATE VIEW`, which is the clearest statement of the rule there is.
- **A typed `COPY` is DataFusion's own write behind the two checks the Export window used to
  stand in for, and the Export window is unchanged.** The write is not ours and never becomes
  ours: `ddl::copy::copy_to` plans the statement once, gates that plan, and drives it — no text is
  re-rendered, so the plan that was judged is the plan that runs (the `INSERT` arm's rule). What
  the editor adds is the pair of refusals that stop a statement which would otherwise *succeed*
  and produce something wrong. **A partition identifier has to be one bare word**, asked of
  `export::partition_columns_are_bare_words` — shared, not copied — because DF 54's COPY parser
  renders each one with `Ident::to_string()` and the planner then looks it up by that string, so a
  quoted name arrives still carrying its quotes and fails about a column nobody named. **A NULL in
  a partition column is refused**, in `export::partition_null_refusal`'s words, for the reason the
  export gives: DF 54 has no `__HIVE_DEFAULT_PARTITION__` and files the row under a neighbouring
  value's directory. The mechanisms differ because the sources do — the window reads the snapshot
  write pass's exact counts for free, a typed COPY counts over the planned input and pays one
  extra scan, the honest price of the same guarantee over an arbitrary query — and the rule is
  identical: **proceed only on an exact zero**, an unreadable count being a reason to decline just
  as a positive one is. A `__snap_` source is the router's refusal (`Blocked::ReservedName`), which
  is what keeps `__strata_ord` out of a user's file. The effect is `None`: a COPY changes nothing
  the catalog holds, and history and the event log record it like any successful run.
  `Blocked::CopyTo` stays defined as the **agent** path's refusal.
  **And a partitioned write states `keep_partition_by_columns` in the statement, never in the
  session.** DF's physical planner reads that key out of the COPY's own `OPTIONS` and only falls
  back to the session config when it is absent, so `run_export` sends it as an option; the `SET`
  it replaces was global and never restored, which was invisible only for as long as no statement
  could read it back (ED-08) and would otherwise have made one export decide the answer for every
  later one. Namespaced (`execution.…`) rather than bare because `TableOptions::set` skips that
  whole namespace, which is what lets the key reach the planner without a format refusing it.
  Spec: [STATEMENTS_SPEC.md](../STATEMENTS_SPEC.md) §6.4,
  [EXPORT_OPTIONS.md](../EXPORT_OPTIONS.md).
- **A typed `SET` is a session overlay in front of Settings, and the overlay wins for its keys
  until `RESET` or restart.** Neither statement runs natively, and the two reasons are opposite
  halves of one rule — Settings stays the durable config authority. Native `SET` applies
  `datafusion.runtime.*` **live**, rebuilding the `RuntimeEnv` under the session, which is exactly
  the discipline `restart_owed` exists to hold; native `RESET` restores **DataFusion's** default
  rather than the value Settings names, so a user who set `batch_size` in Settings, typed `SET`,
  then typed `RESET` would land on 8192 with their own setting silently gone. So `ddl::session`
  applies a `SET` through the same `ConfigOptions::set` call `Engine::set_config` makes — one
  funnel, so the two ways an option moves cannot land differently — records it in `SessionScope`'s
  overlay, and a `RESET` drops the entry and re-applies `config::effective` over the engine's own
  overrides (DataFusion's `reset` only for a hand-typed key the catalogue names no default for).
  Four key classes are **refused** rather than overlaid, each toward the surface that owns it and
  on `RESET` as much as on `SET`: `is_owned_key`, `datafusion.runtime.*` (a restart), and the two
  the app reads from the **Settings store** rather than from the session —
  `datafusion.format.*` (`config::is_display_key` — the grid formatter and the chart read's cache
  identity) and `config::DIALECT_KEY` (the language service carries the dialect on its own
  `Catalog` snapshot, built from Settings, while the validator and the planner read it live, so a
  session value leaves completion lexing the buffer by rules the planner has stopped using —
  WJ-04, and silent: nothing fails, the two layers just disagree). Those last two are one rule
  with two surfaces, which is why each gets its own sentence and not its own mechanism. The
  overlay is **engine-wide**, because every tab and every agent read plans against the one
  `SessionState`, and the precedence rule lives in the one place the two writers meet:
  `set_config` skips a key the overlay holds, recording the new baseline for the eventual `RESET`
  to land on rather than overwriting what the user just typed. `restart_owed` is untouched — a
  runtime key can never enter the overlay. The statement is **planned**, never read off the AST,
  because the planner is what refuses scope modifiers and `HIVEVAR`, folds `SET TIMEZONE` onto
  `datafusion.execution.time_zone`, lower-cases the key and renders the value.
  **And writing the option is only half of applying it.** `NowFunc` captures
  `execution.time_zone` when it is *registered* and bakes it into the literal its `simplify`
  returns (the `to_timestamp` family too), so every writer also calls
  `engine::refresh_config_dependent_udfs` — which is what DataFusion's own `set_variable` /
  `reset_variable` do after the same `options.set`, and what `SessionStateBuilder` does at
  construction, which is why a launch override always worked and a live change silently did not.
  Skipped, a `SET` reports success, moves `SHOW`, and leaves `now()` in the zone the engine was
  built with until a restart. The Settings Apply had the same gap and is fixed with the typed
  statements, because "the two ways an option moves cannot land differently" is worth nothing if
  both land wrong.
  Spec: [STATEMENTS_SPEC.md](../STATEMENTS_SPEC.md) §6.5.
- **`PREPARE` runs natively because DataFusion owns the plan; the fence and the mirror are ours,
  and the fence can be nowhere else.** `SQLOptions::verify_plan` descends into a `Prepare` node's
  input but an `Execute` node has **no inputs**, so a DML/DDL body refused at `PREPARE` or it is
  never refused at all: the router answers off the parsed statement
  (`Blocked::PrepareNonQuery`) and the dispatch verifies the plan under `dml=false, ddl=false,
  statements=true`. Storing it is `execute_logical_plan`'s own arm, so the optimizer pass, the
  arity check and the duplicate-name error stay DataFusion's — which is why the mirror is written
  **after** the dispatch and never before it, and why `DEALLOCATE`'s "does not exist" is
  DataFusion's too. The mirror exists at all only because `SessionState::prepared_plans` is
  `pub(crate)` — there is no public enumeration — and completion has to offer the names; it holds
  types, rendered through `short_type` at the boundary so the language service never depends on
  DataFusion's, exactly as `FunctionSym` does not. Both statements carry
  `StoreEffect::PreparedChanged`: nothing persists, and it is still an effect for the reason
  `FunctionsChanged` is one — `EXECUTE p` resolves now and did not a moment ago, so the catalog
  epoch has to move with it. **A restart clears all of it by construction**, not by a teardown
  step: the remount builds a new `Engine`, whose `SessionScope` is a fresh `Default`.
  Spec: [STATEMENTS_SPEC.md](../STATEMENTS_SPEC.md) §6.5.
- **A created function is a SQL macro, its catalog is swappable, and the name it may take is
  fenced against the built-ins.** `CREATE FUNCTION` runs natively over DataFusion's own seam for
  it — a `FunctionFactory`, installed at `build_context` on **every** engine, so the headless host
  runs the statement identically — and the UDF it returns implements nothing but `simplify`, which
  substitutes the call's arguments into the stored body: the function is inlined once per plan and
  never invoked per batch. `Definition::read` is the **one** judgement of the statement, called by
  the arm for the sentence the user reads and by the factory to build from, so a form the arm
  accepts is a form the factory can build.
  **The body is an expression over the arguments and nothing else, and the standard spelling of one
  does not plan.** DataFusion plans the body against an *empty schema* with the argument list
  supplied as placeholder types, so it accepts `$1` and `$x` and refuses the bare `x` that is
  standard SQL and is what a user writes; `bind_parameters` says the bare form in the planner's own
  vocabulary **on the parsed statement, before planning**, so all three land on one planned body and
  `simplify` has one substitution to make. A bare `Column`, a subquery or a `$n` past the arity is
  refused — a body reading a table is a hidden dependency that nothing persists and no `DROP TABLE`
  can name. `AS '<string>'` is refused because in this dialect family `AS` takes a *string literal*,
  so `AS 'x + 1'` would create a function returning the text; every clause the planner drops
  silently (`STRICT`, `SECURITY`, `SET`, …) is refused off the parsed statement from a destructure
  with no `..`, which is `views::definition`'s rule from the same position. The name is folded on
  both statements, because DataFusion's planner takes the identifier verbatim on each.
  **A built-in is refused to both statements**, because DataFusion's registry cannot tell one from
  a session's own function and its `DROP FUNCTION` deregisters across *all five* registries at once
  — scalar, aggregate, window, table, higher-order: `DROP FUNCTION abs` would take the built-in
  away for the rest of the session with nothing able to put it back. `Functions::created` — the
  folded names this session made, held beside the catalog — is what makes the difference nameable;
  a name it holds is the user's to redefine under `OR REPLACE`, any other registered name is not.
  Same shape as `CREATE OR REPLACE VIEW` over a table name, read from the other side.
  **`registered_function` asks all five**, three of which are one method call away and two of which
  are not: `array_filter`, `array_transform` and `array_any_match` are registered *only* as
  higher-order, so a three-registry fence read them as free names — takeable, then destroyable by
  the matching drop. The predicate is "what would the drop clear", never "what happens to be
  callable"; `range` escaping a narrower fence by having a scalar twin is an accident, not a rule.
  **And the drop's own statement is read, not trusted to the planner**: DataFusion's `DropFunction`
  arm takes `func_desc.first()` with no length check, binds `drop_behavior: _` and never reads a
  `FunctionDesc`'s argument list, while sqlparser parses the comma list in every dialect — so
  `DROP FUNCTION a, b` planned as a drop of `a` alone and reported success. Refused off the parsed
  statement from a destructure with no `..`, the same rule the create arm keeps.
  **The catalog is re-walked by the statement that moved the registry and by nothing else.**
  `functions::snapshot` used to run exactly once at `Engine::new` into an immutable field, which was
  true of the registry until this statement existed; `Functions` holds it as an
  `Arc<FunctionCatalog>` and `Engine::functions()` hands out the handle, so the built-in set still
  costs one walk and the language service's memoized snapshot stopped deep-copying a thousand
  symbols per catalog epoch. There is **no revision counter beside it**: `FunctionsChanged` bumps
  the catalog epoch, which is what every consumer already keys on, and a second signal would have
  had no readers. A restart clears the created functions by construction — a new `Engine` is a
  fresh walk of the built-in registry.
  Spec: [STATEMENTS_SPEC.md](../STATEMENTS_SPEC.md) §6.6.
- **A re-scan means "list the sources again", so this engine runs no list-files cache.** DataFusion
  54 turns one on by default — 1 MiB, **infinite TTL** — and with it every re-listing answers with
  the file set from last time: the catalog's ↻, the Configure window's re-inference and
  `CREATE OR REPLACE TABLE` all silently return the previous state. `ENGINE_KEYS` names `0` as the
  default for `datafusion.runtime.list_files_cache_limit` and `build_runtime` applies it **before**
  any override, which is why it always builds a `RuntimeEnv` rather than falling back to
  DataFusion's. It stays a default and not an owned key — a project over a slow bucket with a
  fixed file set is exactly what the cache is for.
  **The per-file *statistics* cache is the opposite call, and the two must not be confused.**
  That one answers "what is in this file", keyed per object and invalidated by `is_valid_for` on
  size and mtime, so a re-listing still finds new files and a replaced file still re-reads — only
  an unchanged file is spared. `register_external` hands the table the runtime's own
  (`ListingTable::with_cache`), which `SessionContext::register_listing_table` does for itself,
  so snapshots always had it and only our hand-built config did not. Without it statistics are
  re-read on **every scan** (`free_stats` reaches them through `list_files_for_scan`) *and* every
  registration — and since an `INSERT` asks for a re-scan, the *k*th write re-read *k* footers.
  Hand-building a `ListingTable` means opting into every default the convenience constructor
  applies; this is the second one that had to be applied by hand, after `collect_stat`.
- **A reader that outlives one Run pins the snapshot it reads.** A snapshot belongs to its
  workspace and is retired the moment that workspace dispatches another run (SNAPSHOT_SPEC §4),
  which is right for the grid and wrong for anything longer-lived. `Engine::pin_snapshot` hands
  back an RAII `SnapshotPin` that **defers** the retire to the last release — so the export
  window (P4-10) writes the result it was opened on even if the user re-runs the query behind
  it. RAII rather than a pin/unpin pair for the same reason cache entries are held by a mounted
  subscriber: lifetime is a held handle, never imperative bookkeeping. Never answer this with a
  warning or a staleness check instead — "your results moved" is a worse product than results
  that don't move, and a check races the very dispatch it is checking for.

  **And a hold that protects spawned work belongs to that work, not to the call that started it.**
  `Engine::export` plans on the caller's future but *drives* the write on a spawned task, and the
  export window's press is a scope-bound `spawn` — so closing the window drops the caller while
  the write is still streaming. A guard living in that future releases the pin and the in-flight
  count right there: a re-run in the owning tab then retires the snapshot the `COPY` is reading
  (deregistering the table and unlinking the IPC file), and the user's file ends truncated or its
  Hive tree half-built, with nothing to report it — while the close-while-running flag has already
  gone false, so quitting mid-write does not ask either. `ExportHold` is therefore owned and moves
  into the task, released when the write ends however it ends. It holds a **`Weak<Engine>`**, which
  is load-bearing rather than cautious: the task runs on the runtime the engine owns, so a strong
  `Arc` would close a cycle (engine → runtime → task → hold → engine) and the engine would never
  drop. The write does not need it either — `run_export` holds its own `SessionContext` clone.
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
  `ProjectState`/`SessionState`. It records **only successful runs** — rows or an intercepted
  statement (ED-02: a typed `CREATE TABLE` is as much a query you may want back as the `SELECT`
  inside it, and its `count` is the rows it moved, `0` where it counts nothing). *Successful* is
  the load-bearing half, and a claim the surface keeps: the History drawer shows no status mark,
  because the canvas's ok/cancelled/failed dot would have exactly one value. Its **Clear** unwrites the file as well as
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
- **Strata owns the catalog list, catalog and schema providers, for identity and visibility —
  never lifecycle. The workspace has one catalog with one schema; the *session* has as many
  catalogs as there are database connections.**
  `engine::providers` installs a `StrataCatalogProvider` (one schema, `public`, `register_schema` /
  `deregister_schema` refusing) and a `StrataSchemaProvider` (one map, keyed by `fold_ident`) in
  `build_context`, before anything registers. The scoping word is load-bearing since DB-02: a
  Postgres connection registers a **sibling** catalog with as many schemas as the server has
  (`engine::db`), and what stays one-catalog-one-schema is the *workspace*, whose flat bare-name
  namespace is the deepest assumption in the app. Registering a catalog programmatically is
  Strata's own act and was never what `register_schema` fenced — that refusal is about *user-typed*
  `CREATE SCHEMA`, and it is unchanged.
  Two jobs and no third: DataFusion 54's
  `SchemaProvider::register_table` is **sync** and carries **no caller identity**, so it can neither
  spool a CTAS result (already whole in RAM by then) nor authorize a `DROP` (`Engine::register`,
  snapshot retirement and DF's own `CREATE OR REPLACE VIEW` all deregister routinely — a provider
  that deleted files there would delete user data on a sidebar refresh). Lifecycle is intercepted in
  front of `ctx.sql` instead; `STATEMENTS_SPEC.md` §3 is the full argument and it is settled.
  What the traits *can* do is the whole reason they are ours:
  **`table_names()` hides the `__snap_` result snapshots while `table()` still resolves them.**
  Every `information_schema` view and every `SHOW` form enumerates through `table_names()` and
  nothing else (`datafusion-catalog-54.0.0/src/information_schema.rs:96-216` — a separate snapshot
  *schema* would hide nothing), so one filter covers all of them, and `__strata_ord` never reaches
  `information_schema.columns`. Paging, chart, export and retirement address a snapshot **by name**
  and are untouched. That is what makes it safe to default `datafusion.catalog.information_schema`
  **on** — set in `build_context` *before* the override loop, so it is a default and not an owned
  key, and named `true` in `ENGINE_KEYS` so a removed override lands back on it. `SHOW TABLES` and
  `DESCRIBE` work on a fresh project; the store stays the catalog authority, so a `Reg::Failed` def
  is absent from `SHOW` (it was never registered) exactly as it should be.
  **`CREATE SCHEMA` is impossible by construction; `CREATE DATABASE` is not, and cannot be.** DF's
  `create_catalog` registers into the `CatalogProviderList`, whose `register_catalog` returns an
  `Option` — a refusing list could only lie ("already exists") or silently no-op, both worse than
  the router's refusal. `Blocked::CreateDatabase` is the gate for it, and the first line for
  `CREATE SCHEMA` too. **`StrataCatalogList` changes none of that** (DB-02): it is
  `MemoryCatalogProviderList` plus the one operation DataFusion has no trait method for —
  *removal*. `CatalogProviderList` can register a catalog and never take one back, so forgetting a
  database connection could not make its catalog stop resolving, and a removed source that stays
  silently queryable is the exact inverse of the catalog-is-the-store rule. Installed on the
  `SessionStateBuilder` so the workspace catalog lands in it rather than in a list thrown away
  afterwards, keyed by `fold_ident` (catalog names are unquoted identifiers, and DataFusion looks
  one up already folded), and refusing nothing.
  Everything else is `MemorySchemaProvider`'s behaviour verbatim, duplicate-name error included, so
  every reader, `find_and_deregister`, `table_exist` and snapshot retirement work with **no**
  call-site changes. The `fold_ident` keying is what makes the one namespace genuinely
  case-insensitive rather than case-insensitive-if-you-came-in-through-a-`&str`: the fold-preservation
  oracle now pins the stored **identity** (`registered()`) rather than which spellings resolve,
  because every spelling of a name now does.
- **One classification with a capability axis, in front of dispatch.**
  `sql::validate::classify(stmt, Capability) -> Verdict` is the whole statement policy:
  `Query` (the snapshot pipeline, unchanged), `Intercept(StmtKind)` (the editor implements it as an
  engine method and the store folds the outcome), `Refuse(Blocked)` (rendered per surface). It
  matches the *parsed* statement, so it is a classification and not a leading-keyword sniff, and it
  is **a pure function of that statement** — a refusal needing context the statement does not carry
  (an INSERT target's origin, a SET key's class) belongs to dispatch, decided with the same
  `Blocked` vocabulary so the wording still has one home.
  The two answers are **columns of the same match arm** (`classify_form` returns
  `(Verdict, Option<Blocked>)`), not two functions kept in step: an arm cannot answer one surface
  and forget the other, and the agent column is AA-01's shipped answer written beside the editor's
  new one. That is what makes parity a test of a table (`the_capability_axis_keeps_the_agent_surfaces_answers`)
  rather than of discipline. `Capability::Agent` is still **read-only v1** — every non-query a
  refusal, message-identical, including the two forms where the editor now diverges
  (`INSERT OVERWRITE` refuses as `InsertOverwrite` in the editor and `Insert` for the agent;
  `EXECUTE` is a query for the editor and stays `Unsupported` for an agent that cannot `PREPARE`).
  **The editor's refusal set is a short list** — `CREATE DATABASE`/`SCHEMA`, the context-dependent
  refusals, unsupported clauses inside accepted statements, and unknown kinds. `Blocked`'s older
  variants (`CreateExternalTable`, `CreateTable`, `Insert`, `CreateView`, `DropView`, `Drop`,
  `CopyTo`, `Set`, `Reset`) stay defined as **the agent path's error messages**, unreachable from
  the editor: `strata-agent` names them directly, so a deletion is a compile break rather than a
  silent rewording. Default stays deny — a parse failure is the caller-side `Err`, the sqlparser
  wildcard is `Refuse(Unsupported)`, and the five-variant DFParser match is wildcard-free so a new
  DataFusion statement is a compile error.
  **Reserved names, read and write**: a `__snap_`-prefixed identifier **in the workspace catalog**
  anywhere in a statement the user typed — targets included — is refused, because the same prefix
  hides the collision from every catalog reader and the collision itself is unrecoverable either
  way (the provider answers "already exists", so it is the *Run* that fails, on a name the user
  cannot see). The predicate is one function (`engine::query::is_snapshot_ref`, beside
  `snapshot_name`) so the naming rule, the refusal and the hiding rule cannot drift; the write
  targets sqlparser does not annotate for `visit_relations` (`CREATE VIEW`'s name, `DROP`'s name
  list) are named explicitly rather than assumed.

  This fence covered only the **intercepted** forms until the pre-release review, on the stated
  grounds that a query may read a snapshot because snapshots are how results are addressed at all.
  They are — but that addressing is `fetch_page`'s, the chart's and the export's, every one of
  which reaches the snapshot through `ctx.sql` and never passes the router, so the allowance bought
  nothing the app uses. What it did buy was a typed `SELECT * FROM __snap_3`: another tab's
  retained result, with `__strata_ord` showing as an ordinary column, which the **Export window**
  then writes into the user's file — the ordinal reaching a user's file down a route the COPY
  fence never sees, which is the single thing that fence exists to prevent. So the refusal covers
  `Verdict::Query` too, on both surfaces (the agent's own refusals keep their wording and come
  first), and `names_reserved` descends into an `EXPLAIN`'s inner statement, or that would be the
  one spelling left that still resolves the name. No Strata surface composes SQL naming a snapshot,
  so nothing in the app is refused by it.

  The two funnels a def can reach without a statement carry the write half themselves:
  `register_external` for tables, and `ddl::views::create` for views — which had no such backstop,
  so a view saved as `__snap_7` through ⌘S or a hand-edited `project.json` registered into the
  reserved namespace and cost a Run the first time the counter reached 7.

  **The namespace is the *workspace catalog's*, and the DB workstream is what made that a real
  question.** The rule was the prefix alone, on any part of any name, which was exactly right
  while `strata` was the only catalog there was. A database connection can hold a relation
  somebody called `__snap_3`, and there the name reserves nothing, hides nothing and collides
  with nothing: it is not the namespace a Run mints into, the workspace schema provider is not
  what enumerates it, and reading it hands back that server's rows rather than another tab's
  result. So `is_snapshot_ref` is `is_snapshot_name` under `providers::in_workspace` — one
  predicate, the same one `ddl::bare_name` uses to decide what an intercepted statement may
  target, so the naming rule, the refusal, the hiding rule and the management rule cannot drift
  apart. Writing to a remote `__snap_3` is still refused, for being remote, which is the true
  reason. `validate::is_reserved` reads the qualifier through DataFusion's **own**
  `object_name_to_table_reference`, so the reference judged is the reference the planner would
  resolve; the scoping is deliberately **syntactic** (the three workspace spellings in,
  everything else out), because `classify` is a pure function of the parsed statement and asking
  the session which catalogs exist would make it a question about now. A qualifier naming no
  catalog resolves nowhere anyway — `bare_name` refuses it by name, and a query naming it does
  not plan. A database connection's schema provider correspondingly grew **no** hiding filter.

  **`in_workspace` compares each part the way the thing that resolves it compares**, and the two
  halves differ on purpose. `StrataCatalogList` keys catalogs by `fold_ident`, so a *quoted*
  `"STRATA"` — carried verbatim past the parser's own folding — resolves to the workspace
  catalog and must answer true; the first version compared it raw, which let that one spelling
  out of the workspace and therefore out of the reserved namespace, so
  `SELECT * FROM "STRATA".public.__snap_3` planned and handed back another tab's snapshot with
  `__strata_ord` on it. The old any-part prefix test had caught it by accident. `StrataCatalogProvider::schema`
  compares its one schema **exactly**, so a `"PUBLIC"` resolves to nothing and answering false
  about it is honest. The same fold belongs in `ddl::database_catalog`, or a quoted workspace
  spelling skips its guard and then *matches the workspace's own entry* in the catalog list —
  telling the user their project's catalog is a database connection, which
  `PgStore::check_catalog` makes impossible for any real one.
- **A name qualified into a database connection's catalog is read like any other name and managed
  by nothing, and the refusal is minted once — in `ddl::bare_name`, which is already in front of
  every arm that resolves a target.** v1 is read-only against a database, so the sentence says
  which of the two halves is true (`'pg.public.orders' is in the database connection 'pg'. Strata
  reads remote tables; it does not create, drop or write them`) rather than naming a surface to go
  and use: there is no Strata surface that creates a remote table, and the server's own client is
  not something this app can point at. It is **not** parameterised by what the statement makes,
  because the answer is about the catalog and not about the kind of thing being made in it; a
  qualifier that resolves to *no* catalog keeps `elsewhere`'s older wording, which is a different
  fact with no connection to name. The catalog **list** is what is asked — it is what resolves the
  name, it holds a database's catalog exactly while the connection is live, and it answers with
  the spelling that connection was registered under. Two consequences worth stating: `INSERT`
  reaches this **before** `Engine::is_internal`, since ownership is not a question to ask about a
  relation whose data Strata could never own; and `CREATE`/`DROP FUNCTION` need no fence of ours,
  because DataFusion's planner already refuses a qualified function name in words that name the
  fault. Reading is never refused — a plain query, a cross-source join, a `COPY`'s source and a
  `PREPARE`d body all resolve a remote relation normally, which is the whole point of the
  connection and the thing an over-broad gate would break.
- **A view's dependencies are two lists, because only one of them is checkable against the
  project's own rows.** `PlanDeps` keeps workspace scans **bare** (`tables`) and non-workspace
  scans **qualified whole** (`remote`), split by the same `in_workspace`, and `ViewMeta` and the
  store's `ViewInfo` carry the split through as `deps` / `remote_deps`. Recorded by bare component
  — which is what `plan_deps` did before the DB workstream — a cross-source view's
  `pg.public.orders` is indistinguishable from a workspace table called `orders`: dropping that
  table names a view that never read it, `view_problem`'s missing-dependency check cries wolf over
  a relation the store has no row for (a triangle on every working cross-source view), and a
  forget of the connection matches nothing anywhere. `dependent_views`, `readers` and
  `left_invalid` needed no change once the split existed, which is the tell that the split is
  where the fact belongs. An agent asking what a view *reads* is handed both halves, because that
  question is not about rows.
- **A remote relation that vanishes server-side is a reconciliation, and its staleness bound is
  stated where the message is built.** Nothing on our side can observe a server-side rename or
  drop: the view goes on answering from the plan it inlined at creation, and the first Strata
  hears of it is the next registration pass failing to re-plan. DataFusion's answer there (`table
  'pg.public.orders' not found`) is true and reads like a bug in the SQL, so `catalog::view_error`
  — the view funnel's counterpart to `register_error`, one diagnosis in front of `readable`'s
  unwrapping, matching the literal off the crate that writes it — rewrites it to name the
  connection and the fix. What that sentence reports is bounded by the **last connect**, which is
  why the fix it names is a refresh: a connection's relation list is its connect-time enumeration,
  nothing polls, and a ↻ re-runs the pass, which re-connects. A workspace name keeps DataFusion's
  words, because the catalog pane has a row for it and that is a better thing to be pointed at.
- **A `COPY … TO` may not land in storage Strata owns, and the gate is the *resolved* target.**
  The reserved-name half of that statement is the router's and covers the **source**; nothing looked
  at where the write went. A `COPY … TO '<project>/.strata/tables/sales/extra.arrow'` drops a file
  inside an internal table's directory, which that table's next scan lists — schema-matched it is
  phantom rows, mismatched it is a table that has started failing, and silent corruption is refused
  rather than warned about. The project's `.strata/` and the snapshot spool are the two fenced
  roots, because they are the two places a stray file changes what Strata later *reads*; everywhere
  else is the user's own disk, and a `COPY` that overwrites their file is the statement doing what
  it says. Compared resolved and never as text: a relative `output_url` is the process's cwd away
  from an absolute one, `.strata/../.strata/tables` names the directory without sharing its prefix,
  and the target need not exist yet — so the path is made absolute, its `.`/`..` folded, and both
  sides anchored on the deepest ancestor that does exist, which is what makes a symlinked project
  folder compare equal. A target carrying a non-`file:` scheme belongs to an object store and is
  not a path into this machine at all.
  Every interception is a **second gesture into a funnel that already exists**, never a second
  implementation: typed view DDL onto the body ⌘S runs (`ddl::views::create`, ED-06), a `SET` onto
  the `ConfigOptions::set` call `Engine::set_config` makes (ED-08), typed
  `CREATE EXTERNAL TABLE` onto Table Config's own def-first registration.
  (ED-01 landed classification, ED-02 the dispatch below; each `StmtKind`'s implementation is its
  own ED task, and until one lands its statement classifies, draws no squiggle, and fails at Run
  with `ddl::execute`'s stub refusal naming the statement.)
- **`Engine::run` routes, and only its query arm touches the snapshot lifecycle.** The Run press
  is one statement of unknown kind, so the app stops choosing a method and the engine classifies:
  `sql::classify_one` parses through the **one** parse funnel `policy_verdicts` uses (a dialect
  the two gates resolved differently would be a statement judged as one form and executed as
  another), then `Verdict::Query` delegates to `query()` byte-for-byte, `Intercept(kind)` goes to
  `engine::ddl::execute`, and `Refuse(b)` returns `b.editor_message()` — the words the squiggle
  showed, and returned *before* anything reaches `ctx.sql`, because DataFusion executes DDL
  eagerly inside it. **One statement per Run**, refused here with a policy sentence rather than
  left to DataFusion complaining about its own parser. The `SQLOptions` triple in
  `query::materialize` is now defense in depth behind the classification, not the gate: it can
  refuse a class of plan, never name the surface that owns the capability. It stays all-false for
  every read but one — `EXECUTE`, whose plan *is* a `LogicalPlan::Statement` — and that widening is
  a **`ReadPolicy` on the dispatch** (`sql::read_policy`, beside `classify`), never a mode the read
  path offers: it is sound only because `PREPARE` verified the prepared plan under the read triple
  and `verify_plan` cannot see through an `Execute` node (it has no inputs) to do it again. So
  `Engine::query` stays the read-only entry every other caller keeps and the widened body is
  private — one body either way, because a second copy of the snapshot lifecycle is what the whole
  discipline exists to avoid.
  Because only the query arm spools, "DDL does not retire snapshots" is true **by construction** —
  an intercepted statement rides `Engine::bookkeep`, the in-flight bracket `explain` shares
  (supersede, abort handle, `DispatchGuard`, settle by `dispatch` and never by `tag`), which
  never touches `Lifecycle::current`. That bracket is what keeps `cancel` / `is_running` / the
  close-while-running confirm honest over a CTAS, which is a full scan.
- **A statement's outcome is a value the app folds, and one fold serves every effect.** An
  intercepted statement returns `StatementReport { kind, message, count, elapsed_ms, effect }`;
  `StoreEffect` is applied by `state::statement` in exactly the `save_view` shape — store upsert
  on the matching `ProjChan` → `persisted_defs` at the mutation point → `catalog_settled` (a
  `RescanTable` asks the scan driver instead, which bumps the epoch on its own way out) → the
  event log. Never introspection and never a refetch: the store is the catalog authority, which
  is the whole reason lifecycle is intercepted rather than left to DF's provider traits. Adding a
  capability is a `StoreEffect` arm, never a second persist path. There is no `StoreEffect::None`
  beside the `Option` — one way to say "nothing changed", not two. The fold is driven from the
  tab's **request keeper**, beside history and the log and for the same reason (a backgrounded
  tab's `CREATE TABLE` still has to reach the sidebar and `project.json`), and it **owns the log
  entry**: `run_event` returns `None` for a statement, because a message claiming something
  durable must not be logged over a `project.json` write that failed — the `save_view` lesson.
- **A secret Strata must keep lives in the OS keystore, and config holds a reference to it —
  which is a property of the types, not a rule to remember.** `strata_core::secret` is the one
  mechanism (AS-05): `SecretRef` is a minted id, `Clone + PartialEq + Serialize + Deserialize` so
  it rides `settings_merge!` like any other field, and `Secret` — the pasted value on its way to
  the store, or one just read back — derives **no** `Serialize`, has no `Display`, and prints
  `Secret(<redacted>)`. So a provider key reaching `config.json` is not carelessness, it is a
  program that does not compile. This extends the connections posture ("no arm of `engine::store`
  takes a secret **value**") to the case where the app really must hold one: third-party API keys
  for the assistant's provider roster — and, since DB-02, a database password, which is where the
  connections rule was **rewritten** rather than routed around. The rule was never "Strata never
  holds a secret" on principle; it was a consequence of this module not existing when W7 was built,
  and of object stores happening to have host-side credential chains where a database does not. The
  standing form is: *no def field is a secret value; a secret Strata must keep lives here and is
  read per use.* `engine::store` is unchanged — it still needs no secret at all.
  The agent-access bearer token stays a plain config string on
  purpose — locally minted, for our own loopback server, worthless elsewhere — and "stored like
  the token" was the wrong precedent to extend to a billing credential; migrating it here is a
  recorded follow-on that needs a config upgrade path.
  Four consequences worth naming. **Empty is not a secret**: `Secret::new` returns `None` for a
  blank field, which is what makes the Settings draft rule fall out of the types rather than
  being restated — a cleared field yields no `Secret`, and no `Secret` is a `delete`. **Absence
  is not an error**: `get` answers `Ok(None)` for a marker whose entry is gone, because "no key
  set" and "the keystore is broken" are different sentences on screen; `SecretError` is
  `Unavailable` (unlock it, allow it) or `Failed` (report it), and never a plaintext fallback,
  which is the exact failure the module exists to prevent. **The store is opened once**, by
  `open_keystore` in `main` — explicit rather than lazy, because `keyring-core`'s default store is
  process-wide and a module that installed itself on first touch could never be handed the mock
  that proves a refusal surfaces. That is also why the app links `keyring-core` plus a per-target
  platform store instead of the all-in-one `keyring` crate, whose `Entry::new` installs its own
  store from a `LazyLock`. **And the service is the app id**: `secret::APP_ID` is the macOS bundle
  identifier, read out of that constant by `scripts/bundle-macos.sh`, because Keychain access is
  scoped per code signature and a bundle claiming a different identity than the items it writes is
  a bug nobody would go looking for. Every call blocks — `task::offload`, like any other blocking
  read.
  **In memory the value is zeroed, not guarded, and the difference is stated rather than blurred.**
  `Secret` zeroes its buffer on drop and `get` zeroes the string the store returned once it has
  been wrapped; that narrows a window and is described as nothing more. mlock/mprotect-style
  guarding (`secrets`) was rejected: it would guard **one link of six** — the text field's own
  `String`, the draft, `security-framework`'s buffer, then the HTTP header and TLS write buffers —
  and a measure that reads as stronger than it is, is worse than none; the threats it addresses
  (swap, core dumps, cross-process reads) are already macOS's; and it links libsodium, which the
  self-contained universal bundle cannot have for free, while setting `RLIMIT_CORE` to 0 for the
  whole process on the way past. What reduces exposure here is **lifetime** — read a key per use,
  never cache one, never let it reach a buffer that outlives the call. Reopen only with a change
  that closes the chain, not a better allocator for one link of it.
- **The config file is read three ways and written atomically, and a file this session could not
  read is never written over.** `config::load` used to be `AppConfig::load(..).unwrap_or_default()`,
  which read *absent*, *unparseable* and *unreadable* as one thing — and because `write_config`
  fires on every project-window mount, the defaults it returned were persisted over the real file
  within seconds of launch. One transient read failure therefore cost every keybind, every engine
  override, the whole AI provider roster (whose `SecretRef`s then name keystore entries nothing
  can reach again), the agent-access token and the recents list, with no copy kept anywhere.
  `session.json` has had the corrupt-vs-unreadable split and the kept-aside file since P4-14; the
  one file that cannot be regenerated by re-running anything had neither.

  So: **absent** is an ordinary first launch. **Unparseable** — which includes a strict enum token
  from a newer build, since one bad `Command` or `ProviderKind` fails the whole document — moves
  the bytes to `<config>.corrupt` before returning defaults, so the settings are recoverable by
  hand and the next write has nothing left to destroy. **Unreadable** returns defaults for the
  session and latches `WRITABLE` off; it is only ever cleared, never set, so a restart is what
  re-asks the question. The latch is deliberately not a `Result` threaded to the nine call sites:
  the rule is about the file rather than any one write, and a per-call answer would be recomputed
  identically at all nine.

  The write is `util::write_atomic` — the same temp + `sync_all` + rename every `.strata/` write
  uses. `preferences`' own `save` is `File::create` then `to_writer`: truncate first, no fsync, no
  rename, so a kill mid-write leaves a config that parses as nothing, and two instances writing at
  once race the same way. The crate keeps `AppInfo` and the JSON shape; `app_dirs2` (already in
  the graph as `preferences`' own dependency) resolves the path, and a test asserts our path is
  byte-for-byte the one it computed, so an upgrade cannot silently orphan every existing user's
  settings.
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
- **A theme is roles; a component's colour is a role reference in one static table.** A theme
  file authors the closed ~100-name role vocabulary (`roles!` in `strata-core`), `syntax`,
  `fonts` and `typography` — components are **not** in the file. Every component field is fixed
  onto a role by the mapping table (`strata-freya/src/theme/components.rs`): built-ins as
  partial retunes over fork defaults, Strata components as whole-cloth struct literals the
  compiler keeps total, `role(Role::…)` the only colour-reference constructor so the table
  cannot hold a typo'd name. The old per-theme `components` sections are how the two built-ins
  drifted (same field `specific` in one, `reference` in the other; a dead group nothing
  validated; per-theme palette aliasing) — do not reintroduce a per-theme override layer, and
  do not put a literal colour in the table where a role exists (`Color::TRANSPARENT` for
  structural absence and non-colour layout constants are the exceptions). The fork stays
  untouched: `bridge_sheet()` feeds its `ColorsSheet` by each slot's behaviour in fork
  defaults, dotted names resolve through the pluggable `Palette::color` seam, and
  `StrataPalette::color` answers magenta (never `None` — Freya's `primary` fallback would hide
  the typo). A role that turns out to be shared by two things that must differ is **split** —
  add to `roles!`, retarget the table rows, author two values, `schema_in_sync` regenerates —
  never worked around with a call-site literal. Values in the file are literal colours; there
  is deliberately no in-file aliasing, which is exactly how the old palette rotted.
- **Panel layout lives on `SessionState`** (not a peer store), so it rides `SessionSnapshot` +
  autosave and survives restart. Two channels, both `Persist`: `Chan::Layout` = structure,
  `Chan::LayoutSize` = sizes (nobody subscribes; a resize drag persists without re-rendering the
  shell). `ResizableContainer` owns live resizing — we persist only the last size. Keep panels
  **keyed** with fixed `.order()` so the `Workbench` subtree survives a sibling collapsing.
- **Each edge of the shell offers one pane at a time, and a rail is what picks it.** The left side
  is `Layout::sidebar: Option<SidebarPane>` — one variant since DB-05's data-sources tree absorbed
  the connections pane (and the agents pane went with AA-03's surfaces), and it stays an enum:
  `None` still means collapsed, the rail still toggles it, and `sidebar_pane`'s retired-name
  tolerance is what keeps a `session.json` written while Connections was open from being moved
  aside and costing the user every tab. The right, since
  AS-04, is `Layout::right: Option<RightPane>` — the column inspector **or** the chat, never both.
  Two independent flags would let a 1180px window carry two rails, a sidebar, two right panels and
  the drawer at once, which is the arrangement the canvas declares a minimum width to avoid; one
  slot is also RustRover's own right edge. Each pane keeps its **own** remembered width
  (`inspector_w`, `chat_w`) because they share a position and nothing else, and the shell keys the
  panel per pane so switching remounts rather than inheriting the other's scroll offset.

  The rail's press is `toggle_right_pane` (pressing the lit one collapses); a surface that *names*
  a pane calls `open_right_pane` — the distinction `open_drawer` already draws, because a catalog
  row saying "Ask about this table" has to mean it.
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
- **An update is installed by a quit, and what makes one installable is the signature rather than
  where it came from.** Three rules, and each replaced a tempting shortcut.
  **The bundle is never mutated while it is running.** The press records the swap in a
  process-global slot and calls the ordinary `platform::quit()` — every close confirm keeps its
  say and the open-set persists exactly as on any quit — and `main` performs it *after*
  `launch` has returned, which on macOS it does (winit 0.30's `run_app` is `run_on_demand`, and
  Freya's renderer calls `event_loop.exit()` rather than ending the process). A cancelled quit
  clears the intent, and that clearing lives in `end_quit` because that is already the one call
  every dismissing path has to make. The swap itself is copy-to-sibling, rename-aside,
  rename-in, so a failure at the last step renames the original back: the outcome is the new app
  or the old one, never half of either, and the relaunch is unconditional because the user asked
  for a restart.
  **Authenticity is Apple's chain, checked by us, and it fails closed.** `codesign --verify
  --deep --strict`, then `TeamIdentifier` against `secret::TEAM_ID`, then the staged
  `Info.plist`'s `CFBundleIdentifier` against `secret::APP_ID` — in that order, because the
  strict verify is what seals the plist the third check reads. Each refusal has its own wording,
  and an ad-hoc signature (no team, or the literal `not set`) is refused by name so a locally
  built app can never be offered as an update. TLS and an https-only redirect policy cover the
  transport and nothing else: a MITM can withhold an update, never substitute one. The offer
  requires a **strictly newer** semver, so a replayed listing cannot walk a running app
  backwards, and the archive is unpacked with `ditto` because anything that drops extended
  attributes produces a bundle that no longer verifies. There is no system check behind ours —
  a file the app downloads itself is never quarantined, so Gatekeeper never assesses it.
  **A worker outlives the window that started it.** The blocking calls run through
  `task::offload`, but a task is bound to its window's root scope and the launcher closes the
  moment a project opens, so the worker parks its settled status in a process-global and
  whoever reaches it first takes it — the awaiting task, or the next window to mount. Nothing
  polls: there is always a workspace window, and its mount is the second wake. A status can
  therefore never strand on `Checking` or `Downloading`, which for a third-of-a-gigabyte
  download would orphan a verified bundle nobody could reach.
  Outside a bundle the whole thing is **inert** — `update::site()` finds no `.app` in a
  `cargo run` build, so there is no startup check and no offer, and that is what says so rather
  than a debug assertion. `strata-core` is versioned independently of the app, so the running
  version is an **argument** to the check and never the core's own `env!`.
- **What the app offers about an update is one pure answer every surface reads, and the question
  it raises is per window while the status behind it is the app's one.**
  `updater::Affordance::of(status, site)` takes the app-global status and the cached install
  site and answers what to draw and what a press means; `updater::press` is a thin match over
  that answer, each arm one call into `state::updates`. Three surfaces read it — the launcher
  rail's version line (UP-03's affordance, because that is the one place the app already talks
  about its version, and it prints `state::updates::CURRENT` so the number shown and the number
  compared are one), App ▸ *Check for Updates…*, and the restart dialog — and none of them
  restates a rule. (A **palette row was built and removed**: those two are already where the app
  talks about an update, and a third to keep in step with them bought a gesture nobody reaches
  for by name. `press` is still the funnel, so putting it back is one method.) That is what
  makes three things impossible
  to get wrong in one place and right in another: a dev build offers **nothing** (the mechanism
  is inert there, so an enabled control would be the "looks live, does nothing" failure the
  menubar's `Gate` exists to prevent), a release carrying no archive **or** a bundle that cannot
  be replaced degrades to the release page rather than promising an install, and a staged update
  is a **restart** rather than a second download. Because `press` is that match and nothing
  else, "check for updates" pressed over a staged update raises the restart question instead of
  starting a check.
  **The status is app-global and the question is not.** One running app has one answer about a
  newer release, but two project windows must not both raise the dialog for one press — so
  `UpdateConfirm` is one component mounted at both workspace roots over a per-window `AskSlot`,
  carried on `use_register_window`'s `MenuScope` because the menubar needs it and a panel has
  none (it mounts no dialog).
  **And the menubar item records its press rather than performing it.** It carries no chord, so
  unlike every other custom item it cannot reach a window through the keyboard pipeline — but it
  cannot simply call `press` either: `handle_menu_event` runs on the renderer thread, outside
  Freya's current context, and two of `press`'s arms reach `spawn_forever`, which panics there
  (in a release build freya-winit catches it and exits the process). So the press sets
  `AppCtx::update_request`, a plain `bool` a `State::set` can write with no context, and the
  **focused** window drains it from `use_file_menu`'s effect, where there is a scope to spawn in.
  That is AGENTS.md §3's rule for a press with no scope of its own, and it is the same edge Open
  Recent sits on — which is why *that* item hand-rolls its open rather than calling
  `OpenCtx::apply`, whose `NewWindow` arm is a `spawn_forever`.
  **The slot lives on the project *window*, not its subtree**, because an update is a fact about
  the app: a re-root must not drop a question the user has already been asked.
  And confirming asks *only* "restart now?" — it is an ordinary quit, so every close confirm
  still gets its say afterwards, and re-asking "lose the running query?" here would be a second,
  weaker copy of that dialog. The card says a window that *would* ask before quitting still
  asks, never that one with a running query does: `confirm_close_running` can be off, and a
  dialog that promised a prompt the user had switched off would be a false reassurance.
  **A failure is not silent just because it is not chrome.** The rail deliberately draws
  nothing for `Update::Failed`, so `state::updates::failed` is the one constructor and it logs —
  otherwise a refused signature and a finished download would be indistinguishable, with nothing
  to diagnose from.
- **The rail is quiet because nobody asked it; the menubar item asked, so it is owed an answer —
  including "nothing to install".** The quiet rule is about *nagging*, and it was over-applied:
  `Idle`, `Checking`, `UpToDate` and `Failed` draw nothing anywhere, so App ▸ *Check for
  Updates…* over an up-to-date app was a menu item that did nothing at all — which is the
  "looks live, does nothing" failure the item's own `Gate` exists to prevent, arrived at from the
  other side. So the rail keeps its silence and the menubar gets `updater::raise`: a second thin
  match over the same `Affordance`, which raises `UpdateAsk::Report` on the pressing window's
  slot and *then* runs the check, so the answer lands in the card the press opened. It checks
  **over an offer it already has**, because the item says *check*: an `Available` learned at
  startup can be a release behind by the time somebody asks, and reporting it unasked would
  answer with a fact nobody re-established (`check` stands itself down while a job runs, so a
  second press costs nothing). Three arms divert — a staged update is still the restart question
  (that one is `press`'s), a download in flight only reports, since the offer in hand is the one
  being installed, and a dev build offers nothing — and the rail's own action is untouched,
  because there pressing *is* the offer. The affordance is bound **before** the match: a `peek`
  guard resolved in a match *scrutinee* lives for the whole match, and the `check` in an arm
  writes that same state, which is the generational-borrow panic the confirm dialogs record —
  this function shipped it once and it panicked on the ordinary press.
  `UpdateAsk` is therefore an enum of two questions: the restart carries its version so the card
  cannot name a different one, and the report carries nothing, because it is a view of the
  app-global status rather than a question about one release.
  **The card's words are one pure match and its action is the affordance's.** `Report::of(status,
  offer, tones)` answers glyph, tone, title, subject and body together, so the card cannot end up
  with a tick beside "the update failed"; the subject line *is* `Affordance::note`, so the card
  and the rail describe one update in one vocabulary; and the one accent action is
  `Affordance::action` pressed through `press` — this card can offer nothing the rail would not,
  and a download started in it reports its own progress in place.
  **And a staged update is asked about once.** When that download lands the offer becomes
  `Affordance::Restart`, and the card the user is already looking at *becomes* the restart
  question — it does not grow a press that raises a second card. "Restart to update" followed by
  "restart now?" is one question asked twice, and the second card would say nothing the first had
  not; `report_card` delegates to `restart_card`, so the two are one body rather than two that
  have to be kept saying the same thing.
  **What changed is the release's own Markdown, rendered by the viewer the app already has.**
  `update::Offer::notes` is GitHub's release body, carried by the check that already read it
  (never a second request) with its line endings normalized, since a `\r` reaches the text shaper
  as a glyph; a `null` body is `Option<String>` at the parse, or one release without notes would
  refuse the whole list. It rides all three offer states, so the panel does not vanish when the
  download starts, and it rides `Affordance::Restart` and `UpdateAsk::Restart` on to the restart
  card, which can be the **first** sight of it: a download started from the rail never raises the
  report card, and asking somebody to restart into a release whose notes the app is holding and
  never shows is the same silence this whole entry is about. **Both** cards draw it through the
  chat pane's `MarkdownViewer` — one Markdown dress in the app — at the type scale's small sizes,
  per instance, through
  `MarkdownViewer::theme`: a fork addition, because that component was the only themed one
  without the per-instance setter every other one has (§6's rule, not an app-side token). The
  well is a **fixed height and scrolls**, so a long changelog cannot push the action strip off
  the window.
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

