# P3-14 · Drawer — History tab

**Phase:** 3 · **Status:** ✅ · **DEV_TASKS:** U10 · **Depends on:** P3-11, P3-12, P4-14

## Goal
Query history in the History tab.

## What was built
`views/drawer/history.rs` — the drawer's third body, over the `History` satellite (P4-14), and the
tab that finishes the drawer's **Clear**.

- **The list.** Newest-first cards over the shared `DrawerBody`: the run's figures
  (`214 ms · 240 rows`), a line-count pill for a multi-line query, its age, and a two-line SQL
  preview. Empty state is the canvas's clock over "No queries run yet".
- **Press loads, double-press loads and runs.** Both in the *one* `on_press` (AGENTS.md §3 — a
  second registration under the same event name replaces the first), with `EventsCombos::pressed`
  deciding which. Loading is `actions::load_sql` (a plain `set_text`, so it is undoable, which is
  what makes replacing a tab's buffer a safe click rather than something needing a confirm);
  running is `actions::press_query`, the same funnel the toolbar's Run press uses — so a re-run
  from here is an ordinary press, with its own nonce, cache entry and keeper.
- **Clear** empties the satellite *and* removes `history.jsonl`
  (`state::history::clear_history` → `strata_core::project::clear_history`), so the rows don't come
  back on the next open. The header's button is no longer parked: both log tabs now enable off the
  mounted body's `DrawerCount`, so the number in the header and the button beside it can't
  disagree.
- **Theme.** Three fields on the existing `drawer` component theme rather than a second source:
  `stats_color` (the run's figures — a step forward from `meta_color`, which the timestamp beside
  them wears), `badge_border_fill` (the pill's outline) and `row_hover_fill` (the card's hover
  surface). `Badge` grew `.border(..)`, an outlined variant that drops the tint — a pill is read
  by its edge *or* its fill, and both is the same mark said twice.

## Dedupe: history is a list of queries, not of presses

Added after the first build put seven identical `select * from events` rows in the drawer. A run of
a query the log already holds **moves** its entry to the top with the newest figures, rather than
stacking a second row.

- **Dedupe comes before the cap, at every layer.** That is the whole point, not a detail: with a
  cap of 50, a query hammered 150 times must occupy *one* slot and leave 49 for everything else.
  So `History::push` removes the earlier entry before pushing (the cap never counts a repeat), and
  `load_history` collapses repeats *then* takes the newest `cap` distinct — a plain keep-last-N
  over an append-only log would have handed back a window of one statement repeated, quietly
  redefining what `max_history` means.
- **The key is `strata_core::util::collapse_sql`** — whitespace collapsed, nothing else — and it is
  the *same* function that renders the row's preview. Deliberately one function: a key looser than
  the preview merges rows a reader can tell apart, a key tighter than it lets two visually
  identical rows sit in the list. Case is never normalized (a quoted identifier is
  case-sensitive); re-indenting a query is the same query.
- **The saved log is deduped too, without giving up the cheap append.** A new query is still one
  `O_APPEND` line. A *re-run* moved an entry, which an append cannot express — it would leave the
  superseded line behind — so `push` reports whether it replaced, and `record_run` rewrites the
  whole (already capped) list through the new `project::save_history` in exactly that case. The
  file therefore never holds a duplicate, and the path that doesn't need a rewrite doesn't pay for
  one. `load_history` keeps its own dedupe regardless, since it must also cope with logs written
  before this.
- **A deduped row has a real identity**, so the drawer keys rows by the collapsed SQL rather than
  by position: a re-run moving to the top carries its own scope with it instead of shuffling every
  row below through its neighbour's.

## Decisions worth keeping

- **No status dot.** The canvas's leading dot encodes ok / cancelled / failed, and the satellite
  records **only successful data runs** — so the dot would have exactly one value. A mark that
  implies a distinction the data doesn't carry is decoration, not a fact (P3-08's rule). What is
  left is what was really measured. A failed run isn't silently missing from a list claiming to be
  complete: it was never history, and Events beside it is where it is recorded.
- **Clear keeps the dedup guard.** `History::clear` empties `entries` but not `seen`: `seen` is the
  guard for *runs*, and the pin holding a cleared run is still mounted, so forgetting it would let
  the run re-record itself on the next render and put back an entry the user just cleared.
- **Clearing is a `remove_file`, and an absent file is success.** "No log" is already how
  `load_history` spells "no runs yet", so removing it reaches that state by the path that already
  exists, and the next append recreates it. A failure is logged as an **event**, not a `tracing`
  line: the list on screen is already empty, so a silent failure would leave the surface
  disagreeing with the file behind it until the next open.
- **Rows are keyed by position.** A `HistoryEntry` has no id (it is a line in a `.jsonl`), and the
  same SQL run twice is two entries differing only in their numbers. Position is stable for
  everything the list does — a new run prepends, and Clear empties it.
- **No per-row tooltip.** The canvas's `title="Click to load · double-click to load & run"` was
  dropped: nothing else in the app tooltips a list row (the catalog's, Problems', the tab strip's
  are all bare), and a hover card over a two-line preview covers the thing it is describing. The
  single press does something visible, which is the affordance.
- **The age is shared, and does not tick.** `strata_core::util::ago` is now the one spelling of
  "3 h ago", used here and by the inspector's `scan_age` (which used to carry its own copy). Coarse
  enough to stay true between repaints, so nothing re-renders on a clock.

## Acceptance
- [x] Past queries list, capped at `max_history`; click loads into the editor; double-click loads + runs.
- [x] Clear empties the history.

## Freya / references
The `History` satellite (`state/history.rs` → `.strata/history.jsonl`). Canvas `onLoadHistory` /
`onRunHistory`. Design: `DrawerHistory.dc.html` (the rows live in `Strata.dc.html`, region
`drawer-history`). `Settings::max_history` cap.
