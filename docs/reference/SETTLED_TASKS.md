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

**Connections 03 (the editor, W7)** is ✅ — the window the pane's `+`, its empty-state CTA and a
row's *Edit* all open, plus `upsert_connection` and the AWS profile discovery behind it. Configure's
window shape throughout (child of the project window, pinned to the *subtree*, single-instance per
target, writing that window's store through `persisted_defs`), so what is worth recording is only
where it differs.

**Save asks for a whole-catalog pass, and no `ScanScope::Connection` was added.** `plan_scan` puts
connections in `All` alone, and its own doc names the re-connect case — a corrected region, an
`aws sso login` — as "exactly what ↻ is for". That case *is* this window, so Save is the ↻ the user
would otherwise press with the def written first; it is also the honest width, since every table
over the bucket was registered against the store the save replaces. **An edit that moves the bucket
or the provider deregisters the old URL in the footer** — `connect` is additive and only ever sees
the def it is given, so nothing else ever would. **The window then watches its own row** rather
than awaiting a second registration path: `Ready` closes it, `Failed` keeps the engine's sentence
beside the field that caused it, which is worth more here than anywhere else because a credential
refusal names a control still on screen.

Three canvas departures, each a state removed. **A new S3 connection opens with a blank region**,
where the canvas seeds `us-east-1`: that seed is arrow-rs#2795's silent default in a user's
handwriting, and the credential probe would still pass, so the connection registers green over the
wrong region. **A field's error is the footer's**, one value that both disables Save and explains
it, so a form cannot hold two accounts of its own validity. **HTTP shows the URL box and nothing
else** — an auth pill that can never mean anything is not a control, the same call Configure made
about its one-option LOCATION toggle.

Two smaller things worth not re-deriving. **`ProviderId` is the discriminant a picker needs**, and
`Provider::id()` / `Display` / `scheme()` now delegate to it — so `GCS` and `gs` are each written
down once across the badge, the picker and the registry key, rather than a third time here. And the
**Named-profile picker is a real discovery** (`Engine::aws_profiles`, parsed by `aws-config`
because `[profile x]` versus `[x]`, `AWS_CONFIG_FILE` and the two files merging are not the ini
rules they look like) with three distinguishable states, because an empty dropdown cannot tell "no
profiles" from "not read yet".

A later pass over the same task settled four more, and they are the ones a reader is most likely
to re-derive. **The def's field is `address`, not `bucket`** (aliased for old files), because the
providers do not address the same thing — and **an HTTP connection's address is a whole URL**,
scheme and all, in one box: `http` and `https` are two different origins, so there is no chip, no
picker and no `Provider::scheme`. A **path is refused naming the part to drop**, never trimmed,
since the registry keys on scheme and authority. `allow_http` for HTTP is **derived from the typed
scheme**, and that one is worth not rediscovering: `ClientOptions` builds reqwest with
`https_only(!allow_http)`, so a plain-`http` origin failed before any request left the process,
with a "builder error" that named nothing. Only the MinIO test found it.

**Every rule about an address lives in `Provider::check_address`**, called by the store and by the
editor, because S3's naming rules and GCS's are genuinely different and two copies would drift.
**Client options** (`client_config`) sit on the def rather than in a provider — one HTTP client
serves all three stores — offered from `CLIENT_KEYS` and refused by `check_client_config`, one
call on both sides again. And the editor's form keys **every** row by the provider: keying only
the rows that come and go still leaves the ones that stay sitting at a new index, which Freya's
differ records as *moved* and then unwraps a scope the move left behind.

Two silent failures the review pass caught, and both are migration-shaped. **`serde(alias)`
migrates a field's name, not its value**: an HTTP connection stored before its address carried a
scheme held the authority alone and derived `https`, so after the rename it read as a URL with no
scheme and was refused — `ConnectionDef::migrated` runs in `project::load_defs`, the one path defs
come off disk. And **a plain-`http` endpoint without `allow_http` is refused by name**, because
`ClientOptions` builds reqwest `https_only(!allow_http)` and every request then fails with a bare
"builder error" that names neither the host nor the control to change.

**Connections 04 (the LOCATION toggle, W7)** is ✅ — the Configure window's object-store arm, and
the last piece of the data path: a table def can now name a connection. Four things it settled.

**A table names its connection; it does not carry a composed URL.** `TableDef::connection` is the
connection's `url()` and nothing else about it, which makes it the *one* field that says a table is
remote — sources are bucket-relative exactly when it is `Some`, so the two halves cannot disagree,
and the LOCATION choice stays explicit rather than a scheme parsed back out of a path (spec §4).
**`resolve_source` takes the connection**, rather than a local resolver with a remote sibling
beside it: `s3://` is not an absolute *path*, so the local rule silently answers
`<project>/events/2024/` and reports a missing folder on the user's own disk — a wrong answer that
looks like a broken table. One function is what makes reaching for the wrong one impossible. The
engine needed nothing at all: the store went in under that URL in the pass's first phase, so
`table_spec` composing the string is the whole of it.

**A forget now has a consequence, and it is two lists.** Nothing reads an object store by name, so
the shared `consequence` could not say this: what breaks is the tables whose def names the
connection (`tables_over`) and then the views behind those (`views_over`), which is the reading a
table drop already reports. `forget_consequence` says both in one sentence; stopping at the tables
would have under-reported a forget against the drop it is otherwise identical to.

**A def over a forgotten connection keeps naming it.** Rewriting it to "local disk" would re-point
the table at a relative path on the user's machine; the footer says which connection is missing and
blocks Save until one is chosen — the treatment `FormatId::Unknown` already gets, for the same
reason. **New connection… sets the project window's slot** rather than opening an editor of its
own, so there is still one open path, the editor outlives a Configure window closed under it, and
what it saves appears in the picker with no reopen.

---

**Assistant 04 (the chat pane, AS-04)** is ✅ — the surface the whole workstream was for: a
conversation in the project window, over AS-02's loop and AS-01's vocabulary.

**The right edge is a rail, and it holds one pane.** The design canvas grew a second 48px strip
after the task was written, mirroring the left one, so `Layout::inspector_open` became
`Layout::right: Option<RightPane>`: the column inspector and the chat are *alternatives*, not
neighbours. That is what keeps a 1180px window readable with two rails, a sidebar and the drawer
up, and it is RustRover's own right edge. `views::right_rail` is `views::rail`'s mechanism —
`ToggleButton`s whose `on` is derived from the layout, a press routing through the store's toggle
— and the shell keys the right panel **per pane** rather than per side, because the two share a
position and nothing else.

**A window has conversations, plural, and the pick is per conversation.** `Chats` is the
satellite: a capped list, a live id, and a `Pick` (provider · model · effort) on each, seeded from
Settings' defaults through `seed_pick` — which drops a provider that is no longer enabled, since
in Settings a disabled provider also loses its key. Deleting the last conversation opens a fresh
one, which is what lets `active` be an id rather than an `Option`. Nothing reaches `session.json`
and nothing reaches history: the second is the **adoption** rule and stays, and the first became
`.strata/chats/` with **AS-07** (below) rather than a session field.

**Provider is picked by picking a model.** The footer's model list is grouped under the enabled
providers, so choosing a model chooses both — one control instead of two that can disagree about
selections which cannot be sent. Effort is its own control and renders **only** when
`efforts(kind, model)` is non-empty; a rung the newly picked model does not offer is *dropped*,
because `Brain::resolve` refuses a selection carrying one before a socket opens and the control
that set it is gone by then.

**A turn's blocks are in arrival order, and its two cards mean different things.** A `Step` is a
**citation** — every figure on it is the engine's own, which is what makes AS-02's
no-number-without-a-run prompt rule auditable. An `Offer` is **executable**, and arrives only from
`offer_sql`, which checked the statement against the catalog and the *editor's* policy before the
card existed; it deliberately has no step card beside it. SQL the assistant is merely *explaining*
stays in the prose as an ordinary markdown code block with no Run press — the distinction the tool
exists to make. Both promote through `actions::open_sql`, never into the user's buffer.

**Cancel is dropping the task, and it settles anyway.** The turn task owns AS-02's `Running`,
whose drop guard *is* the cancel and the engine's abort, so there is no second stop path — and the
reply keeps what had streamed, marked stopped. The other half of that landed one layer down:
`agent::directory`'s `SettleOnDrop` sends the stop settle in the engine's own `CANCELLED` wording
when a run's future is dropped, disarmed on the normal path. AA-03c reaped such a row only when a
*connection* ended, which covers an MCP client hanging up and not the assistant at all.

**`probe::refresh` moved rather than being copied.** The in-flight guard, the two keeps (names to
the satellite, outcome to the probe) and the retraction rule now live in `state::listings` with
`needs_asking`, the staleness guard both model pickers kick with; Settings keeps only
`Ask::from_draft`, which is the one part that is about a draft. The composer's is
`Ask::from_config`.

**Prose is the fork's own `MarkdownViewer`**, on the `markdown` feature and pointedly not
`markdown-code-editor` — that would pull in freya's code editor, which this app does not use. A
fenced block renders as a themed mono panel, which is what a transcript wants.

**What the canvas asked for and could not have:** its "Thought for 4s" collapsible. AS-02's stream
loop folds reasoning chunks into the captured content that rides the next request rather than
emitting them, so there is no `TurnEvent` to render — building the control would mean inventing
the fact. It is AS-02's to enable, and the transcript grows the block when the event exists.

**Two layout bugs, one rule, both now pinned.** Adding the right rail after the resizable middle
put the middle's `expanded()` in a `Content::Normal` row, which claims the whole width and lays
the rail off screen; the pane's own column did the same to its composer. `Size::flex` is only
divided by a parent whose `content` is `Flex` (AGENTS.md §3) — the rule was already written, and
both sites now state it. The composer's is a test that measures the field's rect against the
pane's height, because "it rendered" and "it is on screen" are different questions and only the
second is the one a user asks.

**Assistant 07 (conversations survive the window, AS-07)** is ✅, and it closed **AS-03** behind
it. The store is `.strata/chats/<uuid>.json` — a satellite on `history.jsonl`'s terms, gitignored
through `ensure_gitignore` so a transcript quoting the user's data never surfaces in a committed
project.

**What has to survive is both lists, and that is the correction the task file itself carried.**
AS-07 was written to persist the transcript; the transcript alone restores a conversation you can
read and cannot continue, because the resolved `@`-mention bodies, the tool results, the captured
reasoning parts and the `offer_sql` call/response pairs live **only** in the model's
`Conversation` — and a failed turn plus the two different caps make the lists genuinely diverge.
The seam is `Conversation::{to_json, from_json}`, **JSON-valued** so `genai` still stops at
`strata-agent`'s edge; the consequence is that a `genai` upgrade moving that serde shape is a
change to this document, absorbed by `CHAT_VERSION` or by the `Read::Memoryless` tier.

**Three degradation tiers, and none of them takes the pane down**: an unknown version is skipped
with a log line, an unparseable file is skipped, and a memory this build cannot read still yields
the transcript with a fresh memory under it. The worst outcome is losing what the model
remembered, never what the user wrote.

**Reopening is a read.** No run, no scan, no snapshot, no network — the step cards are recorded
values. The one thing it asks the catalog is whether each restored `offer_sql` statement still
plans (`tools.validate`, a dry plan), and a stale one **degrades silently** to an ordinary code
block: the user never ran it, so a complaint that their catalog moved is not news. Two other
corrections settled the same way — over-cap eviction **demotes to the shelf** rather than dropping
(the document is already stored, so a discard would be the window forgetting something still
listed), and **Clear is per project**, in the pane's own ellipsis menu, because the files belong to
a project while a Settings button is app-global and would promise a sweep it cannot perform.
Settings keeps the **cap** alone, rotated down on load like history's.

**The cancel race is recorded rather than fixed.** Writes hang off the fold's `Settled` arm
(race-free: AS-02 commits to the memory *before* it emits `Settled`), the stop press, and the
subtree teardown — synchronous there, like `use_autosave`'s own `use_drop`, since a task spawned
in teardown dies with the scope. On the two cancel paths the turn's own commit may land after the
write; both interleavings leave a valid provider tail, so the bounded cost is a stopped turn the
model does not remember. Closing it would mean awaiting the settle, which contradicts "a cancel is
a drop".

**Export is Markdown, and is not the store.** The pane's ellipsis menu writes the readable
conversation — prose, the statements, and each step's own engine figures — because the JSON is
already on disk and a second copy of it is not a deliverable. A stopped turn exports marked
stopped, in the settle's own words.
