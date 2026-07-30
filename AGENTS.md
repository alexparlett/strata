# Strata — engineering practices

The **how-we-work** companion to [CLAUDE.md](CLAUDE.md) (which is the *what/where* map: build,
workspace layout, module map, docs index, backlog). Every rule here was settled deliberately during
the Freya rewrite — most after a wrong version was built and rejected in review — so treat them as
decisions, not suggestions. The *why* is kept wherever the reasoning **is** the rule.

**Upkeep:** when a review settles a new convention, or overturns one here, update this file in the
same change. Session memory may restate rules from this file; this file is the durable,
authoritative copy — if they disagree, trust the repo and fix whichever is stale.

**Scope:** the `strata-*` crates and app-level work. The Freya fork (`crates/freya`) carries its own
`AGENTS.md` with the upstream author's conventions (`just` commands not raw cargo, `crate::` imports
not `super::`, doc comments over inline comments, no em dashes, `KeyExt` on components) — follow
that file when editing fork code, and §6 here for how the fork relates to the app.

---

## 1. The engineering bar

- **Generic capability, not hardcoded subsets.** Build the real, general mechanism, not a tactical
  stub that happens to pass the current case.
- **Real end-states, not placeholders.** No TODO scaffolding left as the deliverable. (The one
  sanctioned exception is a deliberately **inert control** whose capability another task owns — §5.)
- **Native Rust tooling, not stray scripts.** Schema/codegen/tests live in the crate (e.g. the
  `schema_in_sync` test), not one-off Python.
- **Verify from source before agreeing.** If Alex asserts an API or behaviour, check it in the fork
  (`crates/freya/`) or the crate before confirming; correct it if it's wrong. Same bar for your own
  claims: don't enshrine or restate an API you haven't just looked at.
- **Framework-native idiom — never pattern-carrying.** When porting a feature, find the
  Freya/freya-query native shape first (fork examples, Valin) and build to that. The Dioxus app is
  a *behavioural* reference only, never an architecture to bridge to: no adapters, echo fields,
  parallel ids, or compatibility shims to keep old shapes alive ("I don't want to keep a pattern
  that worked with dioxus for the sake of it"). Prefer widening a native id over introducing a
  mapping. Breaking `strata-dioxus`'s build is expected — it is already broken on purpose.
- **Model impossible states out of existence; fail loud on the rest.**
  - A project can't exist without a folder, so `ProjectState.root` is `PathBuf` (not `Option`), has
    no `Default`, and is only built full from load/scaffold. Don't thread `Option`s or blank
    fallbacks to paper over failures.
  - Expected absences get defaults (missing session file → one blank tab; missing `.strata/` →
    scaffold). Unrecoverable faults (unopenable project dir, unparseable defs/session) are
    **surfaced** — interim `panic!` until P4-01's close-window handling — never a silent
    blank/rootless fallback.
  - Never shape a production signature (or add an `Option`) to satisfy a test — build the test's
    store inline instead. Pull deps like the project root from context
    (`use_radio_station::<ProjectState>`), not params-for-tests.
- **No over-engineering.** Private/internal app: use `pub` freely, don't hand-annotate visibility
  per field on struct-literal-built components (the linter widens them back anyway).
- **A path is qualified in the `use` and nowhere else.** Import the **item** and refer to it by its
  bare name; a qualified path in a signature, a body or a match arm is the smell, and so is
  importing a *module* to qualify through — `use crate::platform::{self, WindowKind}` plus
  `platform::open_export(…)` is the same rule broken, one step shorter. Not tidiness: the import
  block is the one place a reader checks what a file actually depends on, and a path spelled inline
  is a dependency that isn't listed there — which is how one item ends up reached three different
  ways in a single file (`crate::components::form::form_theme()` beside a `use` of the same module
  elsewhere). It also removes a class of bad call site outright: `platform::open_export(platform
  .clone(), launch)` reads as one name meaning two things, because the module and the local
  `Platform` are both spelled `platform`; `open_export(platform.clone(), launch)` cannot. The
  anchor is unchanged and unlegislated — `use super::` for a sibling, `use crate::` across the tree,
  both are in use here. Three things are **not** covered, because they are not shortenable code:
  a visibility modifier (`pub(in crate::apps::project::views::workbench)`), a rustdoc intra-doc link
  (`[`Subtree`](crate::platform::owner::Subtree)` — the full path *is* the link target), and the
  handful of `std` aliases whose module segment is what disambiguates them (`io::Result`,
  `fmt::Result`, `fs::write`; a bare `use std::io::Result` shadows the prelude). On a genuine
  collision between two of our own names, alias with `as` — never fall back to a reach through the
  crate root.
- **Valin-shaped.** Follow [`marc2332/valin`](https://github.com/marc2332/valin) (the Freya
  author's own IDE) for module layout, per-window data scoping, and stateful tabs.

## 2. Architecture invariants

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
  coerced on the way in — and the record view, `cell_pretty_json` and JSON/CSV export all read the
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
  path); `ProjectRoot` is the *open project* (engine, stores, autosave, catalog, views) and its
  `render_key` is that folder. So "open in this window" (`OpenPref::This`) is a plain `State`
  write: Freya drops the old subtree — flushing its session, dropping its engine, leaving the
  open-set — and mounts the new project through the very hooks that run at launch. Never add a
  second path that re-points a live store at another project: two ways to open one project drift,
  and the mutating one is how relative sources and partition columns get mangled. Anything that
  must survive a re-root (window fill state, the close-confirm slot, the registry entry) belongs
  on the **window** layer, and anything reading "which project" must read it reactively.
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
  false the moment the user switches tabs.
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
  `ProjectRoot`** so no call site can assemble a mismatched trio, with `use_owner_pin` the one
  predicate. Three things generalise. An owner that has closed *shows nothing*, so it fails the same
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

## 3. Freya component & UI conventions

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

## 4. State: where things live

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

## 5. Cross-task ownership

Cross-cutting capabilities (clipboard/copy, export, keyboard routing…) get **one** shared
implementation owned by their backlog task in `.claude/tasks/`. When your feature touches a
capability another task owns:

- Ship the UI affordance **inert** — no handler behind it — and add a "wire into X" note to
  **both** task files. Whether it also *looks* unavailable is a design call, not a rule: a menu
  item is **parked** (`MenuButton::enabled(false)`, `catalog/menu.rs`) because a menu is a list of
  things you can do right now, while a surface's **primary call to action keeps its full dress**
  (the inspector's scan card) because greying it out misrepresents the canvas the surface is built
  to. Either way the capability arrives with the task that owns it, and nothing at the call site
  changes but the handler.
- Do **not** build the shared mechanism early, do not fold a local one-off, and leave **no
  unreferenced pre-work** (pre-built helpers were removed for exactly this — "let the next task
  redefine that how it likes"). Record the intended shape in the owning task's file instead.

## 6. The Freya fork: when and how to change it

`crates/freya` is a git submodule of `github.com:alexparlett/freya`, resolved by **local checkout
path** — edits are picked up on the next `cargo build`, no push needed locally.

- **Fix limitations in the fork, not around it.** When an app design starts reaching for a
  workaround (a registry, a scale-factor correction, a duplicated theme token), the right move is
  usually a semantic fix in the fork — deterministic listener ordering, logical `root_size`,
  `SelectPlacement`, disabled colors on `ButtonColors`, `set_window_parent` all landed this way.
  The platform-specific half goes in its own `freya-winit` module beside `traffic_light.rs`
  (`cfg`-gated, a documented no-op elsewhere), the primitive on `RendererContext` (the only place
  that holds every window at once), and the discoverable API on `WinitPlatformExt` hopping to it —
  so app code never touches objc2 or a raw winit handle.
- Follow the fork's own `AGENTS.md` conventions when editing it; keep changes upstream-shaped
  (themed tokens, doc comments, examples).
- **After changing the fork, push it** — the committed gitlink must exist on the fork remote or
  fresh clones/CI can't init the submodule. This is not a formality: P4-03's `set_window_parent`
  commit was never pushed, so P4-04's worktree could not build the app at all (`no method named
  set_window_parent`), and no amount of `git submodule update` fixes it — the object isn't on the
  remote to fetch. If you hit that, the commit is in the **main repo's** `crates/freya` checkout:
  `git -C crates/freya fetch --no-tags /abs/path/to/main/repo/crates/freya <sha>` then
  `git merge --ff-only <sha>` (additive, and it keeps your own uncommitted fork edits as long as
  that commit touches different files — check with `git show --stat` first). Then push it.
- **Worktree traps — use the `freya-submodule` skill** (`.claude/skills/freya-submodule`), which
  owns the full sequence: `git worktree add` does not update submodules, so in any new worktree
  run `git submodule update --init --checkout` before the first build, then `git submodule status`
  (no `+` prefix). A `+` means the checkout is not the commit the superproject recorded; compare
  `git ls-files -s crates/freya` (the gitlink the index wants) against `git -C crates/freya log -1`
  before concluding anything about a build error in fork API. The skill also carries the recovery
  for the unpushed-gitlink trap above (fetch the sha from the main repo's checkout by absolute
  path, then update again). And every worktree has its **own** `crates/freya` checkout: when
  editing fork files by absolute path, confirm the path goes through *your* worktree, not the main
  repo's copy.

## 7. Git, worktrees, and verification

- **Formatting is the `fmt` skill, never `cargo fmt --all`.** `--all` means "all packages *and
  their local path-based dependencies*" (its own `--help` says so), and `crates/freya` is a path
  dependency — so `--all` reformats the fork, whose `rustfmt.toml` our stable toolchain does not
  apply. Measured once: 344 files, 4006 deletions, none intended, and invisible in
  `git submodule status` because the gitlink never moves. Use `.claude/skills/fmt`, which names the
  three it owns explicitly (and fails closed on a stale list — `cargo fmt -p` errors out entirely
  on a non-member, so a wrong list formats *nothing*).
- **Build + `schema_in_sync` is the check.** After any theme change:
  `UPDATE_SCHEMA=1 cargo test -p strata-freya schema_in_sync` (the committed
  `themes/theme.schema.json` must match `theme.rs`'s `REGISTRY`). Sandboxes that can't build verify
  against fork source and hand off to a Mac build (see CLAUDE.md's environment note).
- **CI runs that same check on every PR** (`.github/workflows/ci.yml`): `cargo test --workspace
  --locked` on **macOS** (the platform we ship — a green Linux build proves nothing about the muda
  menubar or the traffic-light gutter), with `submodules: true`, because the build resolves Freya by
  local path and without the fork checkout nothing compiles. `--workspace` and not a bare
  `cargo test`, which `default-members` would narrow to `strata-freya` alone. It asserts the
  submodule sits at the recorded gitlink **before** compiling, so §6's unpushed-fork-commit trap
  fails in seconds with that named as the cause instead of as a missing method 40 minutes in.
- **The release path is a script CI calls, never a pipeline written in YAML.**
  `scripts/bundle-macos.sh` builds the universal binary, assembles the `.app`, signs, notarizes and
  makes the DMG; `.github/workflows/release.yml` sets up secrets and runs it. So the build a
  laptop makes and the build a release publishes differ only in what is *configured*, never in what
  is *done* — a release path that exists only inside a workflow file is one nobody can run when it
  breaks. Two rules the script holds. Signing **degrades honestly and says which rung it took**:
  ad-hoc with nothing configured, real signature with a Developer ID, notarized when notary
  credentials exist — and it deliberately will **not** fall back to an *Apple Development*
  certificate, which signs but cannot be notarized, so it would buy a signature that still fails on
  a tester's Mac while reading like success locally. And **the tag is created after the build, not
  before**: a published release's tag cannot be moved or deleted, so `gh release create --target`
  mints it only once there is a DMG to attach.
- **The version lives in one file and is reached through one script; a bump rides the publish.**
  `scripts/version.sh` is the only thing that knows the number is in
  `crates/strata-freya/Cargo.toml` — the bundle script reads it through that, and the Release
  workflow resolves *and writes* through it. Writing, not only reading, is the fix for a real bug: a
  version passed to the workflow moved the tag and not the manifest, and the bundle script reads the
  manifest, so `v0.4.0` shipped `Strata-0.2.0-universal.dmg`. Resolving is a separate entry point
  (`--resolve` touches nothing and needs no cargo) so a typo or a taken tag is rejected before a
  runner installs a toolchain, and writing updates `Cargo.lock` because the release build passes
  `--locked`. Then the tag rule above, pointed at the commit: a bump is **refused without the
  release box** rather than performed and discarded, so "just build me a DMG" cannot move the
  repository's version; and the commit is **pushed after the build and never rebased**, because the
  tag names that commit and a rebase would make a permanent tag point at a tree nothing ever built.
  The release notes are the signing rule again — written by `claude-code-action`, `continue-on-error`,
  falling back to GitHub's changelog with a warning that says so, because better notes are a better
  release page and not a precondition for having one.
- **The app bundle is self-contained, and that is a claim each new asset has to keep.** Themes are
  `include_str!`'d and the two families the themes name (`themes/*.json` `fonts`) are
  `include_bytes!`'d and registered through `LaunchConfig::with_font` in `main.rs` — because
  neither IBM Plex Sans nor JetBrains Mono ships with macOS, and a font that is merely *installed
  on the developer's machine* fails silently and only on somebody else's, falling back to the
  system UI font with the whole type scale going with it. Naming a new family or weight in a theme
  means embedding it in the same change; the weights are 400/500/600 because that is exactly what
  `typography` and the component overrides ask for. The icon is the same rule pointed the other
  way: `assets/icon/strata.png` is the master and the `.icns` is **generated during the bundle**,
  so there is no committed second copy of the artwork to drift from the design.
- **One Strata window across every session — enforced.** Several sessions can be live in several
  worktrees, and each can build its own binary; a second instance clobbers the shared app config
  (read once at startup, last writer wins for recents / settings / the open-project set). So
  `.claude/hooks/block-second-strata.sh` refuses `cargo run` while any Strata is alive anywhere,
  naming the worktree that owns it. A **refusal, not a kill**: the running window may be what the
  user is looking at. This is a convention between agent sessions, *not* an app-level single-
  instance lock — that is a real feature (one process, N windows, a second launch focuses) and
  belongs to P4-01.
- **No destructive git — now enforced, not merely agreed.** `git checkout`/`restore`/`reset`/
  `clean` are **blocked outright** for agents by a `PreToolUse` hook
  (`.claude/hooks/block-destructive-git.sh`, wired in `.claude/settings.json`). It reads the whole
  command string, so chaining one behind `&&`, `;` or `$(…)` does not get past it — which is
  exactly how the rule was broken while it was only written down. Both hooks bound the verb with
  "not an identifier character" on **each** side: the git one originally required whitespace-or-end
  *after* the verb, so `git reset;`, `git clean|cat` and `$(git clean)` slipped through the very
  chaining forms it claimed to catch (found while building the Strata hook, which had copied the
  pattern). If you add a third hook, copy the fixed pattern and test the terminator forms. Ask the user to run it, or reach
  for something that destroys nothing: `git switch` to change branch, `git stash` to park work,
  `git diff` to inspect. Any other delete/overwrite of work you didn't just create still follows
  the original rule: **standalone**, with an explicit description, and not at all when there is
  substantial uncommitted work in the tree unless you have asked. Cleaning up a failed script means
  removing the exact files it created.
- **Task files are the working contract.** Each `.claude/tasks/` file is self-contained; keep it
  true — record corrections, wiring notes, and ownership seams there as part of the change (the
  `FetchCatalog` correction and the P4-01 fail-loud seam both live in task files because sessions
  read them cold).

## 8. High-risk areas

- **The editor's hover/pointer stack** (`hover`/`update_hover`, per-line pointer handlers, the
  diagnostics squiggle popup) is delicate; generalising it has broken diagnostics before, and every
  hover-docs/signature-popup variant was reverted. If function/symbol docs come up again, extend
  the **autocomplete** surface (signatures already render as the completion row's dim detail);
  treat any change to the hover model as prototype-behind-a-flag, and re-verify diagnostics.
- **Modifier tracking** (§3) and **global listener order** (§2, no-command-bus) both depend on
  fork semantics — if either misbehaves, suspect a fork-level fix before an app-level patch.
