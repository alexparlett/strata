# QE-07 · Bound every schema surface: the shared collapse, the derived depth

**Workstream:** Query ergonomics · **Status:** ⬜ · **Depends on:** QE-03 (PR #163 — this
task moves its mechanism, so it lands after that merge)

## Goal

Promote QE-03's shape collapse from `describe_table`'s private rendering into the one shared
answer to a question three surfaces have now asked independently: *what does a bounded
rendering of a data-keyed Struct look like?* The invariant this task installs — and adds to
AGENTS.md §2 / INVARIANTS.md when it lands — is the schema-side restatement of the value rule
("a view of a value is bounded where the value is *encoded*"):

> **A surface that renders a schema bounds it.**

Three consumers have hit the unbounded schema so far, with three outcomes: the agent surface
was fixed twice (QE-01 for access, QE-03 for describe), and the sidebar **froze the window**
(2026-08-14, expanding `contentBlocks` on the real `config.json`; Alex had to force-kill).
QE-08 fixes the pane, on DB-05's new tree; this task builds the shared mechanism both it and
`describe.rs` consume, plus two corrections inside `describe.rs` itself (the derived depth,
the elided-shapes count) and the permanent probe that keeps all of it honest against the real
file.

## History — why the schema is shaped like this (settled; do not re-litigate)

- **`json_poly`'s founding commit** (`4115ff3`, 2026-07-29): `config.json` could not be
  *registered at all* — arrow's JSON inference admits five type combinations and hard-errors
  on every other pair, and the file has three conflicted paths across 5,171 occurrences of one
  recursive `content` tree. WJ-02 forked arrow's merge rule with `Text` as the absorbing
  conflict state. The acceptance, verbatim: "registers in ~3s, 19 columns, 241,425 nested
  fields, and `SELECT * FROM config` returns." **No presentation surface was ever accepted
  against the same file** — that gap is this task's reason to exist.
- **Struct-for-every-object was inherited, not chosen.** `infer` forks arrow's merge rule and
  arrow has no Map arm, so `datatype_of` (`json_poly/infer.rs:228`) says
  `Object → Struct` because the thing it forked did. Nobody weighed record-vs-map; the goal
  was "stop erroring".
- **Map inference is measured out, not just argued out** (probe, 2026-08-14). `Map<Utf8, V>`
  needs one `V`, and the head column carries **50 distinct value shapes** (below). Honest Map
  inference would union 50 shapes into one nullable-everything struct — destroying exactly the
  per-shape histogram that turned out to be the useful answer — or fall back to Struct anyway
  for the heterogeneous case. The real file defeats the Map fix for its most important column.
  This joins QE-01's considered-and-not-built list: **do not reopen record-vs-map at
  inference.** The type layer is right; the missing thing was always a presentation invariant.

## Measured facts (probe against the real 62 MB file, 2026-08-14)

One 65 MB JSON object on a single line; parse ~0.8 s, `json_poly::infer` ~1.6 s; 19 top-level
columns, 241,425 nested fields.

- `contentBlocks.contentBlocks`: **19,311 keys → 50 shape sets covering 19,213, 98 genuinely
  singular.** Power-law: `[9545, 2918, 1693, 955, 645, 615, 285, 266, 241, 205, 198, 140,
  127, 123, 115, 109, 109, 86, 56, 53, 51, 48, 44, 44, 42, 36, 35, 33, 30, 29, 29, 27, 26,
  21, 21, 19, 19, 18, 18, 17, 16, 16, 15, 13, 13, 12, 10, 10, 9, 8]`. The 15 sets an answer
  shows cover 18,071 keys = **93.6%**.
- `describe_table('config')`: 13,258 bytes in ~100 ms. `path=['contentBlocks']`: 11,779
  bytes, ~119 ms, **no paging** — one answer where the pre-QE-03 shape was 387 pages.
- Other wide containers behave: `schema.schemas.service.fields` 459 → 5 sets + 10 singular;
  `placement.placements` 49 → **0 sets** (a genuine record, correctly not collapsed).
- Nothing named `*variant*` is wider than 7 children (524 hits). The freezing column is
  `contentBlocks`; "contentVariants" was a recollection of shape, not a field name.

## Current state (verified 2026-08-14)

- The collapse is `describe.rs`'s: `slots()` / `Slot::{One, Keys}`, `same_shape` (exhaustive
  destructure over `ColumnInfo`'s derived `PartialEq` — everything but the name),
  `shallow` (one-level hash that only **buckets**; membership is always checked),
  `COLLAPSE_MIN = 8`, leaves never join a set, largest set first. QE-03's as-built
  corrections hold: it is a *cutting* strategy (an answer that fits complete is never
  collapsed) and the cutting rule reads over the **forest**, not the page.
- `NODE_CAP` (= `SCHEMA_BUDGET / NODE_FLOOR`) bounds every rung as it is built — an
  over-cap rung is abandoned, and past that many nodes no rendering fits whatever its names
  are, so no rung is refused that would have fitted.
- `SCHEMA_DEPTH = 5` — its own doc comment admits the number is where one known field
  landed, against a file that nests to 13.
- **The stated-shapes gap** (found by the probe): on `contentBlocks` the parent says
  `children_total: 19311` and the 15 shown entries' `keys_total` sum to 18,071 — a reader
  can infer 1,240 keys unshown, but nothing says they span **35 more shapes**. "Every elided
  set is replaced by a stated count" currently holds in key units and not in shape units.
- `strata-model` is serde-only (no logic — CLAUDE.md), so the shared home cannot be there.
  `strata-core` already exports `column_info` for exactly this kind of consumer
  (`engine/mod.rs:53`).

## Build

1. **The shared grouping in `strata-core`** — a new module beside `catalog`'s `column_info`
   (e.g. `engine::schema_shape`): move `same_shape`, `shallow`, `COLLAPSE_MIN` and the
   grouping itself (`group(&[ColumnInfo]) -> Vec<Group>`, `Group::One(&ColumnInfo)` /
   `Group::Keys(Vec<&ColumnInfo>)`), public. `describe.rs`'s `slots` becomes a thin adapter
   over it; the **wire dress stays agent-side** — `<key>`, `key_examples`, the byte budget
   and the ladder are `describe.rs`'s own, because budgets tune per surface while *the
   definition of a set* must not. The line: one copy of "what is a keyed set", N copies of
   "how much of it fits here".
2. **Derive the ladder's start depth.** `SCHEMA_DEPTH` stops being a constant: the ladder is
   already a search for the deepest rung that fits, and `NODE_CAP` made a failed rung cost a
   bounded count instead of a build — so start at the shown window's own depth (a bounded
   `counted_within`-style measure, not a full walk) and retreat exactly as now. The width
   decay (`schema_items`) stays. No magic number survives; on the real file the collapse
   frees enough budget that answers should land deeper than 5, and a shallow file never
   climbs past itself.
3. **Count elided shapes.** When the width sample cuts collapsed sets themselves (15 shown of
   50), the parent must state it — a `shapes_total`-shaped counting field on `ColumnWire`,
   present exactly when shown collapsed entries are fewer than the level's sets, absent
   otherwise, so **"an answer with no counting fields is complete" is untouched**. Exact
   name/placement is the implementer's; the convention is not.
4. **The permanent probe.** An `#[ignore]`d test in `describe.rs` (or beside the shared
   module) that reads `sample/config.json` via `CARGO_MANIFEST_DIR` and asserts the headline
   facts: registers/infers, the 19,311-key struct answers collapsed within budget with no
   paging, coverage ≥ 90% by the shown sets, `children_total` exact. **The repo's "an ignored
   test is one nobody runs" doctrine exists for tests CI could run; this file is a 62 MB
   non-distributable commercial document and cannot be in CI, so ignored-with-the-reason
   -stated is the honest form** — say so in the test's doc comment so nobody un-ignores it.
   The file is gitignored (`sample/.gitignore:5`); on dev machines it is symlinked from the
   main repo checkout into the worktree's `sample/`. The test must **fail with a clear
   message** naming the symlink step when the file is absent, not pass vacuously.
5. AGENTS.md §2 + INVARIANTS.md gain the invariant's one-liner and full entry;
   `docs/AGENT_ACCESS_SPEC.md`'s QE-03 paragraph gets the shapes-count sentence.

## Acceptance

- The grouping has one implementation, in `strata-core`, and `describe.rs` consumes it with
  wire behaviour byte-identical for every existing test (they all still pass unedited except
  where the shapes count adds a field).
- No `SCHEMA_DEPTH` constant; the depth-13 real file answers deeper than 5 where the budget
  allows (probe asserts it), a 3-level synthetic file never attempts depth > 3.
- A cut set list carries its shapes count; a complete answer still carries no counting
  fields.
- The probe runs by hand against the real file and passes; absent the file it fails naming
  the fix.
- Full check green.

## Files

`crates/strata-engine/src/schema_shape.rs` (new) + `engine/mod.rs` export ·
`crates/strata-agent/src/describe.rs` · `crates/strata-agent/src/wire.rs` (the shapes-count
field) · `AGENTS.md` / `docs/reference/INVARIANTS.md` · `docs/AGENT_ACCESS_SPEC.md`.
