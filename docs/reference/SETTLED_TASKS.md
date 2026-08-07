# What the completed tasks settled

The corrections and standing rules each finished migration task carried out of review. The
one-line version of each is in [AGENTS.md](../../AGENTS.md) §2; this is the narrative, kept
because several of these were built the wrong way first and the reasoning is what stops that
repeating. Per-task detail lives in `.claude/tasks/`.

The near-term critical path is done: P2-01 (engine facade + snapshots, `docs/SNAPSHOT_SPEC.md`
agreed), P2-02 (results driven by `use_query`) and P2-03 (grid renders the real `QueryPage`;
fixture deleted; snapshot page reads via `FetchSnapshotPage`, paged from the status bar) are ✅ —
sort/filter/export now rest on the snapshot read model. Results are **freya-query** off the tab's
SQL (no runs-by-id store, no query *results* on the session — state-arch §2): each `QueryTab`
owns its Run trigger (`QueryTab::request: Option<QuerySpec>`, on the dedicated
`Chan::Request(id)` channel, so one tab's press/cancel never touches another tab's results and
keystrokes never wake the results pane); the `running` mirror is threaded as **struct-field
props** — props for known shallow consumers, context only for DI handles (`EngineCtx`) and deep
trees (`Selection`).

Phase 3 has started: P3-01 (layout shell) and **P3-02 (catalog sidebar)** are ✅. Note the
correction P3-02 carried: **the catalog is the `ProjectState` store, not a query.** Earlier drafts
of `FREYA_STATE_ARCHITECTURE.md` (and the P3-03 / P3-06 task files, now fixed) described a
`FetchCatalog` freya-query capability — it was never built and must not be. Introspecting
DataFusion would surface the `__snap_*` result snapshots and hide defs whose registration failed,
which are precisely the rows the catalog exists to show. Catalog mutations call the engine and
then `ProjectState`'s own methods on the matching `ProjChan`; nothing refetches.

**P3-12 (Problems drawer)** is ✅, and carries the phase's third standing rule: **diagnostics are
a reconciliation, not an event.** Every open tab's diagnostics are a pure function of two inputs —
its buffer revision and the catalog epoch — and each tab stamps the pair its rows describe, so
`SessionState::stale_tabs` is the whole work list and the window's **one** driver
(`state::diagnostics`) drains it. Never add a second producer and never enumerate entry points: a
tab restored at open, reopened, opened from a view or saved query, duplicated, edited, or left
behind by a pass a tab switch cancelled are all the same thing — the stamp does not match. The
catalog is a **gate** as well as an input (`Engine::register` deregisters before it re-infers, so
nothing validates mid-scan and no false "not found" is ever produced). Problems is the
**SQL-validation** surface across every tab; a run failure belongs to a run, not to the text, and
stays the results pane's.

**P3-13 (Events drawer)** is ✅, and is the standing rule above read backwards: **a log is
recorded by its observer.** An event can't be re-derived from anything — it describes something
already finished — so `state::log` has **no producer hook**; the scan pass records each def the
engine answered for, Save and the drop confirm record their mutations, the request keeper records
each run's settle, and `cancel_run` records a cancel (the `Err("cancelled")` settle lands
unsubscribed, because clearing the trigger unmounts the keeper in the same pass). It also builds
the store state-arch §8 only sketched — deleting the dead `strata-model::log` vocabulary — and
wires the drawer's first working **Clear**. An entry carries a level, not an origin; see its task
file.

**P3-14 (History drawer)** is ✅, and completes the drawer. Query history is the *persisted* log
next to P3-13's ephemeral one, so its **Clear** has to unwrite `.strata/history.jsonl`
(`strata_core::project::clear_history`) and not just empty the satellite — while keeping the
satellite's `seen` dedup guard, because the pin holding a cleared run is still mounted and would
otherwise re-record it. The rows are *actionable*, which is the one thing that shapes the surface:
a press loads into the active tab, a double-press loads and runs, both through the editor's own
`actions::load_sql` / `actions::press_query`, so a re-run from the drawer is an ordinary press with
its own nonce and cache entry. And there is deliberately **no status dot** — the satellite records
only successful runs (rows or a statement, ED-02), so the canvas's ok/cancelled/failed mark
would have exactly one value
(P3-08's "only real facts", applied to a glyph).

History is a list of **queries, not presses**: a re-run moves its entry to the top with the newest
figures, keyed by `util::collapse_sql` — the same function that renders the row's preview, so no
two rows can read identically and none a reader can tell apart are merged. **Dedupe runs before
the cap** at every layer, which is the point rather than a detail: one query pressed 150 times has
to occupy one slot of `max_history`, not all of them. The log on disk holds to the same rule
without giving up its cheap write — a new query is one `O_APPEND` line, and only a run that
*replaced* an entry rewrites the file (`project::save_history`), because an append can add a line
but not move one.

**P3-08 (column inspector)** is ✅, and carries the phase's other standing rule: **only real
facts.** Every number in the inspector was *read* from the source (footer statistics via
`ColumnInfo.stats`, the row count via `TableMeta.rows`) — never derived from the rows on screen,
which is what the Dioxus panel used to do. So the facts box is a dynamic list rather than a grid
of blanks, inexact footer values render `~value`, and the completeness bar needs a real exact null
count or it doesn't appear.

**P3-09 (profiling) + P3-10 (cost confirm)** are ✅, and land the scan tier in that same list
(matched on `StatKey`, so a fact can never appear twice; free wins a tie *unless* it is an inexact
bound). The scan is a **freya-query action keyed by its request**: the store row keeps
`Option<ScanId>` and the numbers live only in the cache entry that key names (AGENTS.md §2) — so
invalidating a profile is dropping the request, which `table_registered` does for the table *and*
for the views that read it. `Engine::profile` / `cancel_profile` own the engine side; a running
scan counts as work in flight for the window-close confirm and not for the per-tab probe. The
canvas's **distribution bars are deliberately not built** — the scan has no distribution data and
an honest histogram needs a second full pass (P3-09's file has the reasoning).

**P4-07 (Settings ▸ Engine)** is ✅, and is where Freya's builtin `Table` earned its first use.
The investigation it started with is the transferable part: the table gave the bordered box, the
shared column widths and the per-row rule, and the five things it could not do were all **fork
gaps rather than design limits** — `TableRow` had a `pub theme` field with no builder (so a row
could not carry a selection fill, nor decline the hover a selectable table doesn't want), only
`TableCell` had `on_press`, `TableCell` hardcoded `main_align(End)`, `Table`'s rect had no flex
content so a stated height could not reach a scrolling body, and one `divider_fill` painted both
the box and the row rules so a theme could never author them apart. Five small upstream additions,
not a hand-rolled grid; what a table has *no opinion* about (which row is selected, what goes
between two rows) stayed composed in the app. It also wired the setting for the first time — the
engine was being built with `Default::default()` — and settled how an engine config change lands:
`Engine::set_config` writes the `ConfigOptions` half live (a **removed** key going back to its
catalogue default, not skipped), and a changed `datafusion.runtime.*` is a **restart**, which is a
bump of `ProjectRoot`'s diff key through the one T2 confirm rather than a second way to configure
a live engine. See its task file; AGENTS.md §2 carries the rules.

**P4-08 (Settings ▸ Keymap)** is ✅, and completes the Settings window. It is the second use of
that same builtin `Table` — the canvas was **redrawn** between the last handoff and the task, from
a list of two-line cards into an Action/Shortcut grid with the description in a tooltip and a
double-click to rebind, so the local handoff bundle's `Settings.dc.html` is stale for this pane
(read the design project). Three things it settled. **One funnel**: capture, the per-row reset and
Reassign all go through `keymap::propose` then `keymap::apply` over a `Rebind`, with the policy in
**strata-core** beside the `validate_bind` a hand-edited config already meets — which is what makes
a *reset* conflict-checked like a capture (a command's default chord can have been taken while it
was away) and a *steal* two bindings rather than one write. **A menubar accelerator is state**: it
now follows the keymap live (`sync_chords`, the list a destructure so a new menu command can't
forget it), and — the load-bearing half — it is **suspended** for the life of a capture, because the
OS resolves an accelerator before the window sees the key, so an armed menubar makes ⌘C copy instead
of bind, and ⌘Z ⌘X ⌘C ⌘V ⌘A ⌘O ⌘Q ⌘, are most of what anyone reaches for. And **dashed borders are a
fork addition** (`BorderStyle::Dashed`, `Button::border_style`): torin fills the region between two
rounded rects and a fill cannot carry a pattern, so a dashed edge strokes the centreline instead —
worth the addition because the dash is the message, distinguishing an open slot from a bound
control. The one thing deliberately **not** built is a direct unbind control: the state is supported
end to end and reachable via Reassign, but the canvas has no such affordance and inventing one is
the designer's call. See its task file.

**P4-10 (export window)** is ✅ — rebuilt from the canvas rather than ported, because the Dioxus
modal had drifted from the design and reached its screen through hardcoded `match` arms per
format. Four things it settled. **Options are data**: `ExportDraft::groups` hands the view a
`Vec<Group>` and each option carries the `Edit` it performs, so adding one is a table row and no
control can write the wrong field. **A window opened on a result pins what it reads** — the
snapshot pin (AGENTS.md §2) exists because of this window: a re-run in the tab behind used to
retire the table mid-`COPY`. **NULL partition values are refused, not warned about**: DataFusion
54 misfiles them into a neighbouring value's directory, which is silent corruption, so
`partition_columns_have_no_nulls` reads the parquet footer and proceeds **only on an exact zero**
(schema nullability is useless here — every column reports nullable), which is also why
`snapshot_writer_props` sets `EnabledStatistics::Chunk` explicitly. And the form vocabulary it
grew — the labelled row, the mono value box, the bounded number — went to
`components::form` with P4-05 rather than staying the export's own.

**P4-09 (Settings search)** is ✅, and settles that **a setting's name has one home**. The index
(`apps/settings/search.rs`) is one table generating the `Anchor` enum, the list of every anchor, and
each setting's route / label / subtext / keywords — and the panes build their rows from it
(`Anchor::row()`), because the failure it rules out is invisible: an anchor spelled one way in the
index and another in the pane is a jump that navigates and then singles nothing out, and only a
person trying it would ever know. The category is never restated either — a hit resolves its page
through `model`'s `category`. Three more things it settled. The engine's properties are indexed off
**`ENGINE_KEYS` entire** rather than the canvas's hand-picked eleven, so a subset can't drift from
the catalogue. **Following a hit is navigation and never an edit** — the first cut added a pre-filled
row for a property with no override (the canvas's "search doubles as add a known property") and was
rejected: a named row with an empty value still projects into the draft, so merely following a result
left Apply live for an override nobody asked for. And a **revealed row belongs to the form, not to
this window**: `Row::anchor` names it, `components::form::reveal` carries the ask (a window-lived
slot, because it is written before the target's page has mounted, plus the page-lived scroll frame),
and the row scrolls itself in and flashes once — the app's first use of `freya::animation` and of
`ScrollController::scroll_to_item` outside the tab strip.

**P4-16 (child-window lifetimes)** is ✅, and settles what a child window is actually pinned to:
**not the window it sits above, but the mount of `ProjectRoot` whose handles it borrowed.** Export
and Configure carry that subtree's store, log, catalog and scan counter as launch values — all
`GenerationalBox`-backed — and both things that remount the subtree free them while leaving the
owner window open under the same id. A re-root changes the folder, which the Configure pin happened
to catch; an engine restart (P4-07) changes neither, and nothing caught that, so the next repaint
panicked on a reclaimed box and a Save wrote into a store nothing was left to serve. The fix is one
value and one predicate (`platform/owner.rs`): `Subtree` is the subtree's own diff key plus the live
`EngineRestart`, **provided by `ProjectLoaded`** so no opener can assemble a mismatched trio, and
`use_owner_pin` replaces the two near-verbatim pins. Three things it settled. An owner that has
closed *shows nothing*, so it fails the same comparison — one predicate, not three clauses. The
generation is the one handle safe to hold across a remount, for exactly the reason it exists (owned
by `ProjectApp`, above the subtree). And `WindowKind` now carries **less**: `Configure`'s `project`
and `Export`'s `owner` were the old pins' inputs, so once the pin read its owner from the launch
value they were unread second copies of a fact that could go stale.

**P6-02 (native menubar)** is ✅ — App · File · Edit · Window over muda, through the fork's `menu`
feature (freya#782, ours to offer upstream). Three things it settled, two of them corrections.
**The Edit menu is custom items, not muda's predefined set** — which overturns both this task's own
plan and F8's: the predefined items send Cocoa first-responder selectors (`undo:` / `copy:` / …)
that a Skia view never receives, so each item instead synthesizes its command's effective chord into
the focused window's keyboard pipeline (`NativeEventExt::send_key_press`) and a menu click takes the
identical path as a typed key. That is what dissolved F8 rather than answering it: with one Edit
menu routing through the focused element there is no per-window divergence, so the muda-handler
shims, the `global-hotkey` layer and the whole "menu follows the opener" design have no job. **What
does vary is narrower**: which File and Window items the focused window can carry out
(`MenuScope::Project(OpenCtx) | Launcher | Panel`, resolved into a **four-flag** `Gate` — `workspace`
· `project` · `workbench` · `cyclable` — because where a command's listener lives differs per item
and the differences do not nest: a project window whose load failed can close and open but has no
workbench to put a tab in) — and Settings is a `Panel`, deliberately not "matched to its opener",
because it has no listener for any of these commands. `Command::CycleWindow` was **built** here
rather than left as the stub that would have made its menu item a lie. And **an app-global
surface that follows the focused window must be pointed by every window**: Configure and Export
shipped without the call, so the bar kept the owner project window's File menu and Close Project
closed the focused *panel* while naming the project. `use_file_menu` now lives inside
`use_register_window`, which every root must call and which takes a `MenuScope`, so forgetting is a
build error rather than a wrong window closing. Scope and chord are one enabled state, written by
one `apply`. Known limitation, recorded in the task file: muda's predefined items own their own
accelerators (⌘H, ⌥⌘H, ⌘M) and `set_accelerator` is `MenuItem`'s alone, so those three chords are
reserved and `suspend_accelerators` cannot reach them.

**Connections 01 (model + object stores, W7)** is ✅ — the fourth project def, and the engine half
of remote reads. No surface: 02–04 own the rail button, the pane, the editor and the Configure
LOCATION toggle. Four things it settled, three of them decisions the spec had left open or drawn
differently, and none worth re-opening without new information.

**The whole def rides the committed `project.json`**, closing `CONNECTIONS_SPEC.md` §5's open
question against splitting the per-machine `profile` / `saPath` into the gitignored `session.json`:
a def carrying only a profile *name* and a key *file path* holds nothing a colleague may not have,
and a catalog whose tables live in a bucket is not shareable if the bucket isn't. **The bucket is
the authority alone and the scheme comes from the provider**, where the v11 canvas stored the
scheme-qualified string — two statements of one fact can disagree, and `s3://acme-lake` under a GCS
provider is a def that reads one way and registers another; `ConnectionDef::url()` is the derived
registry key, and the form owns the prefix chip. **The auth reference lives inside the auth
variant** (`S3Auth::Profile { name }`) rather than beside it, so a profile named on an Ambient
connection is not a state that exists. **`connect` probes the credential chain before registering,
and is all-or-nothing**: without the probe a credential-less connection registers happily and the
diagnosis lands on every table over the bucket — one opaque signing error each, in the wrong place
— and registering-then-failing would make a connection both refused and live, which is exactly what
`Reg<T>` exists to make unrepresentable. The probe resolves once and discards; the installed
provider resolves **per request**, so SSO / assumed-role credentials still refresh themselves.

Two consequences worth carrying: connections are `register_pass`'s **first** phase (a table
registered before its bucket's store fails on a def that is perfectly correct), and the new
`aws-config` tree raised the workspace's effective MSRV to **rustc 1.94.1**.

Two things the adversarial review caught, both of which had shipped as written and are now fixed —
they are the ones most worth not rediscovering. **Ambient and Named profile are two providers, not
one chain with a setting.** `ConfigLoader::profile_name` configures the default chain's *Profile*
arm and does not move it in front of `Environment`, so the first version signed as the exported
`AWS_ACCESS_KEY_ID` while the pane showed the profile the user picked — Profile and Ambient were
the same connection wherever ambient credentials existed, and a misspelled profile name still
registered green, defeating the probe. Named profile is now `ProfileFileCredentialsProvider` alone.
And **a connection's identity is `url()`, never `bucket`**: `s3://lake` and `gs://lake` share a
bucket and are two connections over two stores, so the bucket-keyed fold the first version used
answered one row twice and left the other `Loading` for the life of the window with no error
anywhere. Both have regression tests (`engine::store::tests`, `register::tests`).

---

