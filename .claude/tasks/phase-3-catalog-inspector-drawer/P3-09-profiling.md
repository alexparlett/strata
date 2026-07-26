# P3-09 · Column/table profiling (PROFILE zone)

**Phase:** 3 · **Status:** ✅ · **DEV_TASKS:** D4 · **Depends on:** P3-08

## Goal
A PROFILE zone in the inspector: a full-scan profile of a table's columns, on demand.

## As built

**The scan is a freya-query action keyed by its request.** The store keeps the *request*
(`TableRow::profile` / `ViewRow::profile` = `Option<ScanId>`), never the numbers — the same
division the Run trigger makes, so `ProjectState` still holds no query results. The numbers live
in the freya-query entry keyed by `ProfileSpec { owner, scan }`, with `stale_time(MAX)` (a settled
scan must never re-execute itself) and `clean_time(MAX)` ("cached until the entry changes", which
is what the confirm promises — and which also means a superseded entry is never swept, a
deliberate trade documented at `use_profile`). A ↻ re-scan mints a **new** `ScanId`, so it is a new
key and a new execution; invalidation is dropping the request. No profile cache, no dedup set, no
spinner flag was rebuilt (port plan §4).

Files:

- **`strata-core::engine`** — `Engine::profile(name)` + `Engine::cancel_profile(name)`, tracked in
  `Lifecycle::profiles` keyed by `fold_ident(name)` (tables and views share one namespace).
  Superseded-by-dispatch like `query`. A running scan **counts as work in flight** for the
  window-close confirm (T2) and deliberately not for the per-tab probe — a profile isn't a tab's.
  `register` / `create_view` / `drop_view` / `deregister` abort the entry's scan themselves, so no
  caller can forget to. `catalog::run_profile` lost its `#[allow(dead_code)]`.
- **`apps/project/query/profile.rs`** — the `ProfileEntry` capability, `ScanId`, `ProfileSpec`, and
  `use_profile` (the *one* place the `Query` is built, since the whole `Query` — stale/clean times
  included — is the cache key; two call sites building it differently would scan a table twice).
- **`state/project.rs`** — `profile_scan` / `request_profile` / `clear_profile`, plus the
  invalidation: `table_registered` / `table_failed` drop the table's request **and** the request of
  every view that reads it (`invalidate_readers`, D10's half); `view_registered` / `view_failed`
  drop the view's own. Usually a no-op in the driver path (a refresh re-creates the dependent
  views, which clears them on the *views* channel where the inspector listens) — it is there for a
  landing answer that does not re-create them.
- **`views/inspector/`** — `model::with_scan` folds the scan's facts into the one `FACT_ORDER`
  list; `column.rs` splits the zone into `Statistics` (no request → the scan card) and
  `ScannedStatistics` (subscribes, keyed on the request → running / settled / failed).
- **`views/sidebar/catalog/`** — the row menus' `Profile table` / `Profile view` items, **enabled
  only once the engine has answered for the row** (`CatalogActions::registered`): a def the engine
  refused has no provider, so a scan of it could only fail, and it would fail *out of sight*
  because the inspector shows a failed row's reason rather than a column a scan could report on.
  (Unlike `Refresh table`, which exists precisely to retry a broken row.) Also
  `ProfileStatus`, the per-row spinner (its own component, because subscribing is a hook and a row
  must only subscribe when there *is* a request — otherwise a sidebar full of tables would
  dispatch scans nobody asked for).
- **`views/dialogs/profile_confirm.rs`** — P3-10, and `ProfileActions`, the one entry point both
  surfaces call.

### Merge rules (`model::with_scan`) — the honesty half

- **The scan's row count wins, and `Nulls` follows it.** Not merely a fallback for sources that
  report none: the completeness bar *divides* the null count by the row count, and pairing the
  nulls a scan counted with the rows a footer reported is one ratio from two reads. (Caught by a
  test: it read ">99.9%" on a column that is a quarter null.) So `Nulls` is the one key where free
  does **not** win a tie — it is the bar's numerator, and it has to come from the same pass as its
  denominator. Where a scan described the column but counted no nulls, a free count is dropped
  rather than divided by a row count it never belonged to (`the_bar_never_divides_one_read_by_another`).
- **A nested field takes nothing but that row count.** The profile is keyed by top-level column
  name, so by leaf name `address.city` would collect an unrelated top-level `city`'s facts. The
  zone says so rather than looking like a scan that found nothing.
- **Free wins a tie, unless the free value is a bound.** An *inexact* footer stat yields to the
  computed one — `~Radia Perl` beside a scan that knows the whole value is a bound shown as a fact.
- A `DISTINCT` is a **count**, so it wears thousands separators like the ROWS row above it. Min /
  Max / Mean / Median are *values* and are printed exactly as produced.

### Deliberately not built: the canvas's distribution bars

The profile carries no distribution data. `core::profile::aggregates` computes distinct / min /
max / mean / median per type; bins need boundaries, which need min/max first — i.e. a **second
full pass** over data we have just warned the user is expensive to read once, against D4's "one
full scan, one aggregate" decision. The canvas's bars (and its `p95`, top-value counts and period
buckets) were prototype seed data, and the shipped Dioxus D4 never built them either. Agreed with
Alex on this pass; recorded in DEV_TASKS D4. If it ever lands, `CatalogProfile` grows the bins and
`column.rs::zone` grows a section — nothing else changes.

Also not built, and not wanted: the presence bar (filled/empty/null) — it is `Nulls` as a
percentage, which the completeness bar already is.

## Acceptance
- [x] Profiling a table shows per-type facts; a second request while running dedups; cancel works.
- [x] Registering/refreshing a table invalidates its profile.

Cancel is in the running row (engine abort + dropping the request, so the zone returns to offering
the scan). Dedup is the cache key's: the inspector's zone and the row's spinner subscribe to one
`ProfileSpec` and attach to one execution. Engine-side, a re-scan supersedes rather than
duplicating.

## Notes for later tasks

- **P4-11 (table config)** lands a table answer through `table_registered`, so it invalidates for
  free — but it must also notify `ProjChan::Views` if it does not re-create the views over the
  table, or a view-column inspector keeps showing scanned facts for a plan that moved.
- **The `view as query` tab** is `Origin::Scratch`, named `profile · <entry>`. It opens the SQL
  `core::profile::profile_sql` unparsed from the very `Expr`s that ran, so it cannot drift from the
  numbers on screen; the button is absent when the unparser couldn't render one.
- **W7 (connections)** will make a scan reach an object store. Nothing here assumes local files —
  the cost copy already says "reads every file" rather than quoting bytes.

## Freya / references
- Freya `use_query` (plan §4). Core `Engine::profile` / `cancel_profile`. DEV_TASKS D4 (per-type
  facts + the honesty calls). P3-10 is the confirm in front of it.
