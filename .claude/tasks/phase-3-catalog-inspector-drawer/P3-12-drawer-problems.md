# P3-12 · Drawer — Problems tab (and the diagnostics architecture behind it)

**Phase:** 3 · **Status:** ✅ `[core ✓]` · **DEV_TASKS:** U10 · **Depends on:** P3-11, P2-18, P2-01

## Goal
Live per-tab diagnostics in the Problems tab. The first build scoped the *view* to the active
tab; review established that the limit was not the drawer's but the **producer's**, and the task
grew into the diagnostics architecture. Design + diagrams: the approved plan for this change.

## The problem it fixed
`use_validation` was a hook inside `EditorTab`, which mounts only for the tab on screen. So a tab
whose SQL arrived **without being typed** — restored at project open, reopened with ⇧⌘T, opened
from a saved query or to edit a view — was never validated, and its empty `diagnostics` read as
*clean* when it meant *nobody looked*. Two more holes had the same cause: the catalog was not a
dependency at all (a table registering after a pass left a phantom "not found" until the next
keystroke), and a pass cancelled by a tab switch left a verdict on text the tab no longer held.

## Shipped

### The stamp — the idea the rest falls out of
Each tab carries `validated: Option<Stamp>` (`state/session.rs`) — the buffer revision its
diagnostics were computed from and the catalog epoch they were resolved against. Those are
validation's only two inputs, so `SessionState::stale_tabs(epoch)` is the **whole** work list and
there is no list of entry points to keep true: restored, reopened, opened-from-a-view,
duplicated, edited, and cancelled-mid-pass are all "the stamp does not match". `None` means
**unchecked**, which is distinct from `Some(_)` with an empty vec — *clean* — and that
distinction is exactly what the old view could not make.

### One driver
`state/diagnostics.rs` — `use_diagnostics()`, one hook in the window root. Three fixed
subscriptions: `Chan::Text` (a new synthetic fan-in every `Chan::Tab(_)` write derives, so **one**
subscription watches any tab's buffer — without it the driver would need one per tab, a variable
hook count), `Chan::Tabs`, and the catalog. One cancel-and-rearm task drains the stale list
**serially, active tab first**, so a twenty-tab project open doesn't put twenty dry plans on the
engine's two workers ahead of the user's first Run. `use_validation` and `query/validate.rs` are
deleted; `EditorTab` is a pure view. There is exactly one writer of diagnostics in the app.

The 700ms surface hold survives but is now **typing-only** (`hold`, unit-tested): a first look at
a restored tab and a re-check after a catalog change are not half-written, and holding them would
only delay the truth.

### The catalog as a gate
`CatalogState { Scanning, Settled(u64) }` replaces `CatalogScan: State<bool>` — one value for
both "can I resolve against it" and "has it changed". `Engine::register` **deregisters before it
re-infers**, so mid-scan `table_exist` is false for every table being rebuilt; gating means a
false "not found" is never *produced* rather than produced and retracted, and the squiggles on
screen hold rather than blank. Releasing into a new epoch re-derives every tab — which is how a
problem fixed in Table Config clears without opening the tab. `catalog_settled` bumps for the
discrete mutations (save-as-view, drop) *after* the engine answers, because validation resolves
against the engine, not the defs.

**An epoch, not a fingerprint over the rows:** registration writes `ProjChan::Tables` once per
table, so a fingerprint would fire N times during one scan and queue N × M dry plans.

> **The seed is `Settled(0)`, and that is load-bearing.** It must be *settled* because
> `claim_scan` claims from settled — seeding `Scanning` deadlocks the window's one scan driver at
> mount and strands every catalog row in `Reg::Loading` for the life of the window (shipped once,
> caught in the app). Epoch **0** means "no pass has completed", so `epoch()` is `None` and
> nothing validates before registration lands. The open-time race is closed by value, not by
> which side effect happens to run first. Regression test:
> `the_seed_is_claimable_but_nothing_validates_against_it`.

### Known, accepted: the drain is head-of-line during a typing burst
`use_diagnostics` drains one stale tab at a time with the active tab first, and `settle` only
returns once that tab has been quiet for a full `DEBOUNCE`. So a long continuous burst in the
active tab delays validation of any *other* tab that went stale meanwhile — a catalog epoch bump
from a save-as-view or a ↻ — until the user pauses.

Left as is, deliberately. It is self-healing and produces nothing wrong, only delayed freshness
for tabs that are not on screen. Fixing it means either inverting the active-first priority (which
costs the common case, where the active tab is the only stale one, to help a rare one) or teaching
the drain to yield mid-settle and resume — real complexity in the part of this task that has
already produced two regressions under review. If it ever bites, the shape to reach for is
"drain background tabs while the active tab is still settling", not a shorter debounce.

### The view
`views/drawer/problems/` is now a pure view over `problem_groups()`: a group per tab (sticky
header of file glyph · tab name · `N problems`), rows of severity glyph · message · `line L:C`,
**pressable** to switch to the owning tab (canvas `onProblemJump`). No `use_query`, no per-tab
branching, no merge; `PlainProblems`, `RunProblems` and `problems/model.rs` are gone. The drawer
header tally and the new **rail badge** (`ProblemsBadge`, its own leaf so a settling tab doesn't
re-render the other four toggles) are both `error_count()` — one function, so they cannot
disagree. Errors only: a keyword-typo warning lists without claiming the query is broken.

The `Diagnostic` carries **no** `TabId` — the group supplies it, so there is no second copy to
disagree with the tab the row is stored on, and `strata-model` stays free of app concerns.

### Run failures are not here
Deliberate, and a change from the first build. A failure belongs to a *run*, not to the text: it
can describe SQL the buffer no longer holds, it can't self-clear by typing, and `cancel` /
supersede settle `Err("cancelled")` / `Err("superseded")` that no user should read as a problem.
Putting it in a cross-tab view costs either a copy on the store that outlives the run, or one
freya-query subscription per tab in the drawer *and* in the rail badge. The results pane already
renders it in full — banner, code frame, caret, hint. `DiagSource` and
`Diagnostic::from_query_error` are deleted with it.

### Two defects fixed on the way
- **A run must survive a tab switch.** `run_query::query_for` is now the single spelling of a run
  subscription, with `clean_time(MAX)`. The execution was always safe (`spawn_forever`, root
  scope) but an *evicted* entry reads `Pending`, which is stale regardless of `stale_time`, so
  returning to a tab would silently re-run its query and retire the snapshot its cached pages
  describe. That it can't happen today is a fork accident (`update_tasks` counts the dying
  scope's own effect contexts); this makes it a contract.
- **Autosave gates on content.** `Chan::Tab(id)` derives `Persist`, so hovering a squiggle or
  moving the caret rewrote a byte-identical `session.json` — and the driver's decoration writes
  across every tab would have multiplied that. `use_autosave` now compares the snapshot it is
  about to write against the last one it wrote.

## Acceptance
- [x] Every open tab's diagnostics are live, not just the active one's.
- [x] They resolve as the user types, and when the user fixes a catalog issue.
- [x] No false diagnostics at project open, and no badge spike that drains.
- [x] No Clear button; the empty state shows the clean message.
- [x] Rows switch to the owning tab; header tally and rail badge agree.

## Freya / references
- state-arch §8/§9 (rewritten by this change). Core `sql::validate`. Design:
  `Strata.dc.html` `data-rg="drawer"` (1267–1348) + the rail badge (353–357). DEV_TASKS U10.
