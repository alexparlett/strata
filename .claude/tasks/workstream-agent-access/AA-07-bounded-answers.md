# AA-07 · Tools that page or filter, and truncation that names a real recovery

**Workstream:** Agent access · **Status:** ✅ built 2026-08-11 (design settled 2026-08-10) ·
**Depends on:** AA-02 (built), AS-02 (built)

**As built, and what the implementation corrected** (the rest of this file is the settled
design, kept because its measurements and reasoning are the reference):

- `crates/strata-agent/src/describe.rs` is the walk; `wire.rs` carries `functions_result` /
  `tables_result` and the new params/result shapes; the run ceiling is
  `assistant/dispatch.rs`'s `MAX_RUN_ROWS` over `StrataTools::resolved_page_size`; the
  per-tool cut note is `turn::recovery`.
- **The budget, not a depth, decides whether a schema answer is cut.** The ladder's first
  rung renders the *complete* subtree and keeps it whenever it serializes inside
  `SCHEMA_BUDGET` — so a small schema stays complete however deep it nests, which the
  planned fixed-`SCHEMA_DEPTH` attempt would have cut needlessly. Only past the budget do
  depth 3 → 2 → 1 engage, with depth 0 the unmeasured floor.
- **A saved query's SQL stays whole in `list_tables`.** The plan's per-entry bound clipped
  every SQL body; `describe_table` does not answer for saved queries, so a preview there
  would have made the full text unreachable — the one bound honesty forbids. A view's SQL
  is previewed (its full text is `describe_table`'s answer); a page of large saved queries
  can therefore still trip `turn::bounded`, whose note now names this tool's real recovery.
- **A review pass over the built change corrected six more things**, all folded in: matches
  page at `MATCH_PAGE` (25, half the column page — a match carries its whole path, and 50 of
  the fixture's 13-segment paths measured past the result cap); the window rule is one
  helper (`wire::windowed`, saturating arithmetic, and `page`/`page_size` present exactly
  when the answer shows fewer than the total — a requested page of a complete answer stays
  total-free); the search streams (counts every hit, builds paths only inside the window —
  an empty needle is legal and bounded); the complete rung is gated by a node count before
  any tree is built (`NODE_FLOOR`); a describe answer's `sources` is elided past 100 with
  `sources_total`, because `list_tables`' elision points here; `FunctionWire::signatures`
  became `Option` so a names-only absence cannot read as "no declared arity"; and a cut
  **error** gets its own note (`turn::FAILED_CUT`) rather than the cut tool's
  success-shaped recovery.

## Goal

Four of the ten tools answer with a list that has no bound but the user's data, and the
assistant's loop caps a tool result at 24,000 bytes — so it cuts them positionally and tells the
model to recover in a way three of the four cannot. Give the list-shaped tools a way to answer
the question the model actually asked, and make a truncation name a recovery that exists.

The design below was settled by a planning session (exploration verified against source, then an
adversarial stress pass); the corrections it produced are folded in and marked. Implement it as
written — the numbers were measured, not guessed.

## Current state, measured

Against the shipped registry and realistic data, serialized exactly as `dispatch::encode` does
(`serde_json::to_string`), against `turn::MAX_TOOL_RESULT` (24,000):

| tool | overflows | measured |
|---|---|---|
| `list_functions` | **always** | 63,729 B — **2.66x**, and fixed: it does not depend on the user's data at all |
| `run` / `read_page` | past ~330 rows | 100 rows x 8 cols = 7,151 B (0.30x); **`MAX_PAGE_SIZE` (10,000) = 811,756 B, 33.8x** |
| `describe_table` | past ~90 columns | 100 cols with statistics = 27,321 B; 400 cols = 109,221 B (4.55x); 1,000 cols = 11.4x |
| `list_tables` | past ~170 tables | 200 tables = 27,993 B (1.17x); 1,000 tables = 140,793 B (5.87x) |

`list_projects`, `validate` and the three session tools are bounded small and are not in scope.

Where `list_functions`' bytes sit: descriptions 33,994 (53%), signatures 10,339, JSON structure
~15,500, names 3,070, returns 872. **Dropping descriptions entirely still lands at ~25,000** —
over the cap. Enumerating 319 functions does not fit at any useful level of detail, which is why
this is a shape change and not a trim.

**The nested worst case is real, not hypothetical.** The reference fixture (`sample/config.json`:
62 MB, one JSON object on one line, so one row) infers to a merged Arrow schema of 19 top-level
columns carrying **241,425 nested fields** at depth 13 — one column (`contentBlocks`) is a struct
of **19,311 UUID-keyed children** (the numbers strata-core's own comments cite:
`crates/strata-core/src/engine/json_poly/mod.rs:25`, `engine/serialize.rs:174`).
`ColumnWire::from(&ColumnInfo)` recurses with no bound, so today `describe_table` on that table
materializes all 241k nodes into JSON on every call — multi-megabyte, and the assistant's cut
hands the model the first 24,000 bytes of column one. The fixture is untracked and 62 MB, so it
is **not** a test fixture — tests model its shape synthetically (below).

## The two defects

**A truncation names a recovery the tool does not have.** `turn::bounded` ends every cut result
with "Read the rest with read_page, or run a narrower query." That is true for `run` and false
for the other three: there is no snapshot behind a function list, a catalog listing or a table
description, so `read_page` answers not-found and the model has spent a round learning nothing.

**A cut list is a prefix, not a sample.** All three truncate positionally, so a 400-column table
describes as columns 1-90 and the model reasons about that as the schema. The `truncated: true`
flag `bounded` now emits is the only thing standing between that and a confidently wrong answer.

## The settled design

One narrowing mechanism family across the list-shaped tools (AGENTS.md §1: generic capability),
and one convention over all of it: **an answer with no totals is a complete answer.** Every new
field is `Option`/`Vec` with a serde skip, present only when something was elided — so a small
schema or catalog serializes byte-identically to today, and a stated total is itself the tell
that narrowing happened. A narrowed answer always states what it matched against; a filter that
silently truncates is the same defect one level in.

### 1. `describe_table` — bounded walk, path drill-down, name search

The hard one: one object with a recursive tree inside (`ColumnWire::children`), not a list.

New params (`DescribeTableParams`, `crates/strata-agent/src/wire.rs`):

```rust
pub struct DescribeTableParams {
    pub name: String,
    /// Segments exactly as a previous answer printed them; never dotted.
    #[serde(default)] pub path: Option<Vec<String>>,
    /// Case-insensitive substring over field names, scoped under `path` when given.
    #[serde(default)] pub matching: Option<String>,
    /// 1-based window over the walk root's direct children, or over matches.
    #[serde(default)] pub page: Option<usize>,
    #[serde(default)] pub project: Option<String>,
}
```

**The bounded walk** lives in a new module (`crates/strata-agent/src/describe.rs` — it is a real
algorithm with its own tests; `wire.rs` is long already). Input `&[ColumnInfo]`, output
`ColumnWire` — it belongs to strata-agent because wire shapes must not leak into strata-core. It
mirrors the value encoder's discipline (`crates/strata-core/src/engine/serialize.rs:147-300`,
rules at `docs/reference/INVARIANTS.md` "A view of a value is bounded where the value is
encoded") with its own constants and a comment citing that precedent (serialize.rs's consts are
private and bound values, not schema):

```rust
const SCHEMA_DEPTH: usize = 3;                       // attempted; retries 2 -> 1 -> 0 under budget
fn schema_items(level: usize) -> usize { (30 >> level).max(3) }   // width decay: 15/7/3
const SCHEMA_PAGE: usize = 50;                       // level-0 window; also the match-page size
const SCHEMA_BUDGET: usize = 16_384;                 // bytes for the columns/matches portion
const MATCH_LIMIT: usize = 50;                       // one page of matches
```

- **Fixed depth alone is not enough — this was the stress pass's correction.** At depth 3 with
  width decay, `contentBlocks` alone emits ~26 KB (15 UUID-named children × ~7 grandchildren ×
  3); even depth 1 grazes the cap on UUID names across 19 columns. So the walk *attempts*
  `SCHEMA_DEPTH` and retries shallower (2 → 1 → 0) until the serialized columns portion fits
  `SCHEMA_BUDGET`, measured with `serde_json::to_string` — the same encoder `dispatch::encode`
  uses, so the measurement is the truth. Depth 0 is bounded by `SCHEMA_PAGE` alone and always
  fits: there is always something to show. The budget is 16,384, not 24,000, because the
  envelope also carries table facts and, for a view, the whole SQL.
- **Every elided child set is a stated count.** `ColumnWire` gains
  `children_total: Option<usize>` (skip-if-`None`; present only when `children` shows fewer).
- **`path` roots the walk at a nested column**, resolved by the by-name walk the inspector
  already uses (`crates/strata-freya/src/apps/project/views/inspector/model.rs` `resolve`). The
  answer shows **the addressed node itself** — one element in `columns`, subtree bounded below
  it, `page` windowing *its* children — so a `path` to a leaf answers as itself (name, dtype,
  kind, nullable, stats), never as an empty list. The path vocabulary is closed by construction:
  the walk prints `ColumnInfo.name` verbatim and invents nothing. `column_info`/`nested_children`
  (`crates/strata-core/src/engine/catalog.rs:984-1008`) name a List child whatever the file's
  schema says (`item` on arrow-written files, `element` on Spark parquet — **never** document a
  constant name) and skip a Map's synthetic `entries` level, and the resolver walks the same tree
  that printed the names, so producer and consumer cannot disagree. Duplicate sibling names
  resolve to the first, as the inspector does; say so in the walk's doc.
- **`matching` searches field names** depth-first in document order (deterministic) and answers
  with `matches: Vec<MatchWire { path: Vec<String>, dtype, kind }>`, capped at `MATCH_LIMIT` per
  page (`page` windows the match list too), with `matched_total` **always** present when
  `matching` was given — `Some(0)` on no hits, because an absent total must be indistinguishable
  from "no search ran"; this one field is never skip-on-zero. This is the load-bearing recovery
  for the config shape: the default answer samples ~15 of `contentBlocks`' 19,311 children, and
  `matching` is how the other 19,296 are reachable. Scoped under `path` when both are given
  (composition, not a refused combination); `matching` under a leaf is `matched_total: 0` for
  free.
- **`DescribeResult` gains** `columns_total`, `page`, `matches`, `matched_total`, all
  skipped-when-absent. `impl From<Described> for DescribeResult` becomes a fallible,
  parameterized function — `describe_result(described, &params) -> Result<DescribeResult,
  AgentError>` — because the projection now takes params and can refuse. Table and View arms
  both route through the walk (a view's columns are the same `Vec<ColumnInfo>`); Failed/Pending
  bypass it entirely — the state is the answer, never a path NotFound on a table that exists but
  has no schema yet.
- **Errors.** An unresolvable `path` is `AgentError::NotFound`, its message rendering the path
  as a JSON array of segments (copy-pastable back into the param; **never dot-joined** — names
  come from the user's files and may contain dots, the settled `ColRef` rule at
  `crates/strata-freya/src/apps/project/views/sidebar/catalog/columns.rs:1-11`) and naming
  describe_table's own recovery: call without 'path', or with 'matching', to find it. Extend
  `NotFound`'s doc comment (`crates/strata-agent/src/error.rs`), which currently claims the
  listing tool is the recovery. `page` past the end is clamped/empty exactly like `read_page`
  (`params.page.max(1)`, empty window, totals still stated) — an honest answer, not a fault.
- **Stats need nothing**: they exist only on top-level columns by construction
  (`catalog.rs:993-995`); the walk carries whatever is there.
- **The `Host` seam is untouched.** `Host::describe` already returns the full owned tree (the
  241k-node clone in the in-app driver is paid today); the walk applies at the wire conversion.
  If that clone ever matters, the fix is `Arc<Vec<ColumnInfo>>` inside `Described` — a separate
  change, not this task's.

### 2. `list_functions` — one detail rule, no modes

New `ListFunctionsParams { matching: Option<String>, project: Option<String> }` — its **own**
struct: `ProjectParams` is shared with `open_query_session`/`list_query_sessions`, which must
not grow `matching`.

- `matching` is a case-insensitive substring over function **names** (descriptions are not
  searched — predictable beats clever). Filtered in `tools.rs` over `engine.functions()`; the
  seam is untouched.
- **One rule, not filtered/unfiltered modes**: the (filtered or whole) set gets full detail
  (signatures, returns, description) when it is at most `DETAIL_LIMIT` (~30) functions; above
  that, names only. `FunctionWire.signatures` gains `skip_serializing_if = "Vec::is_empty"` so a
  names-only entry is `{"name": …}`. A 20-function project gets full detail unfiltered; the
  319-function registry answers with every name.
- `FunctionsResult` gains `total: usize` (what the filter matched against — always stated) and
  `note: Option<String>` naming the recovery when detail was withheld (call again with matching
  for a subset in full).
- Byte math holds: 319 names-only ≈ 8 KB; 30 detailed ≈ 7.5 KB (descriptions average 107 B).
  "does `date_trunc` exist and what does it take" is one round.

### 3. `list_tables` — deterministic per-entry bounds, then paging

New `ListTablesParams { matching: Option<String>, page: Option<usize>, project: Option<String> }`.

- `matching` is a case-insensitive substring over entry names — tables, views and saved queries
  alike.
- **Per-entry bounds make a page's size deterministic** (today an entry is unbounded through a
  view's SQL body and a table's source list):
  - `View`/`SavedQuery` `sql` becomes a one-line clipped preview — `clip(&collapse_sql(sql), N)`,
    both already in `strata_core::util` (the clip is visible through its trailing ellipsis
    character, which is data, not UI prose). Full SQL stays reachable per entry through
    `describe_table`, which already returns it.
  - `Table` `sources` capped at the first few plus `sources_total` (skip-when-absent).
- `page` (1-based, mirroring `read_page`) windows entries at `TABLES_PAGE` (~50) per page;
  `TablesResult` gains `total` plus a `page`/`page_size` echo, mirroring `RunResult`'s shape. A
  catalog of at most one page — the common case — answers complete in one round.

### 4. `run`'s half — the assistant asks for less

`MAX_PAGE_SIZE` (10,000) is **unchanged on the wire**; an MCP client decides for itself what to
do with 811 KB. The ceiling is the assistant's, because a conversation carries the result
forever (`Conversation` is append-only by design — a dropped tool result is the malformed shape
every provider rejects).

- Extract the page-size resolution out of `run_as` (`tools.rs`: asked → clamp to
  `MAX_PAGE_SIZE`; absent → host default; default 0 → `MAX_PAGE_SIZE`) into one method on
  `StrataTools`, still called by `run_as` — one construction site.
- The assistant's `dispatch` `"run"` arm resolves through that same method, applies
  `min(_, MAX_RUN_ROWS)`, and passes `Some(page_size)` on. This closes **both** doors: the model
  asking for 10,000, and — the second door the planning pass found — a host whose row-limit
  setting is 0 resolving to 10,000 with no ask at all.
- `MAX_RUN_ROWS` ≈ 250, a documented const in the assistant module (100 rows × 8 cols ≈ 7,151 B;
  the cap crosses at ~330 rows). **Deliberate divergence from this file's earlier sketch, which
  said "on the assistant's `Scope`"**: a `Scope` field nobody sets differently is dead config;
  the const satisfies "the assistant asks for less", and promoting it to a `Scope` field is a
  two-line change if AS-04 ever needs per-pane ceilings. Do not build the field now.
- `read_page` needs nothing: it has no size param and inherits `last.page_size` from the run,
  which is now capped. `RunResult` already echoes the size actually used, so the clamp is
  visible in the answer — `MAX_PAGE_SIZE`'s own precedent.

### 5. `turn::bounded` — a recovery that exists, per tool

`bounded(answer)` becomes `bounded(tool_name, answer)` (one call site in the round loop). The
note names the named tool's own recovery: `run`/`read_page` → read_page pages this result, or
run a narrower query; `list_functions` → call again with matching; `list_tables` → matching or a
later page; `describe_table` → matching, path or page; anything else — errors pass through
`bounded` too — a generic narrower call. After 1-3 the list tools are bounded by construction,
so `bounded` becomes a backstop (a run page of enormous cells can still trip it), not the normal
path.

Model-facing prose kept true in the same change: `assistant/system.md` (the `list_functions`
line and "More rows are 'read_page'"), the `#[tool]` doc comments (wire strings — no backticks),
and the params-struct doc comments, which are the model-facing schema. `RunParams::page_size`'s
"capped at 10,000" stays true on the wire; the assistant's lower ceiling is stated in the
assistant's own prose, not the shared wire doc.

## Acceptance

- The unfiltered call to each of the three list tools returns a **complete** answer under the
  cap, or an answer that states what it left out and how to reach it — never a positional prefix
  whose only tell is a flag. An answer with no totals is a complete answer.
- A `matching` call returns the matches with full detail, and the total it matched against —
  `matched_total: Some(0)` on zero hits, never absent.
- A conversation can ask "does `date_trunc` exist and what does it take" and get an answer, in
  one round, on a project with the full 319-function registry.
- A 400-column parquet table can be described usefully by an assistant turn (pages of 50, or
  matching); a config-shaped schema (19 columns, ~241k nested fields, one 19,311-child struct)
  describes at whatever depth fits the budget, with counts everywhere it sampled, and any field
  in it is reachable in two rounds: `matching` to find the path, `path` to land on it.
- Every truncation note names a recovery that tool actually offers; `read_page` is named only
  where a snapshot exists.
- The assistant cannot ask for a page it cannot hold, and a row-limit-0 host cannot hand it one
  either; an MCP client still can (`MAX_PAGE_SIZE` unchanged on the wire).
- `tests/facade.rs`'s manifest assertions still pass with the updated property lists —
  `describe_table` = `[matching, name, page, path, project]`, `list_tables` =
  `[matching, page, project]`, `list_functions` = `[matching, project]` — a new optional
  parameter is not a new tool, and the ten stay ten.

## Tests

- Walk unit tests in `describe.rs`: depth attempt + retry-shallower under budget; width decay;
  stated totals; leaf-path answers as itself; path resolve through List (**both** `item` and
  `element` element names) and Map (entries-skip); matching scoped under path; matching zero-hit
  = `Some(0)`; page past the end = empty window with totals. Fixtures are **synthetic, built
  through the real `column_info`** (the pattern in `sidebar/catalog/columns.rs` and
  `inspector/model.rs` tests) — `config.json` is untracked, 62 MB, and absent from worktrees.
  One fixture must model its shape: a struct column with thousands of children, depth past
  `SCHEMA_DEPTH`.
- A size regression test that builds the live registry via a real `Engine` (strata-agent already
  reaches core) and asserts the unfiltered `list_functions` answer encodes under
  `MAX_TOOL_RESULT`. This test *is* the first acceptance bullet for that tool.
- `turn::bounded` unit tests — none exist today; add per-tool note coverage in `turn.rs`'s
  `mod tests`.
- Existing suites that move: `tests/facade.rs` property lists; `wire.rs`'s
  `every_result_schema_describes_an_object` needs no new entry (`MatchWire` is nested-only) —
  verify; dispatch's `every_manifest_tool_has_an_arm` passes as-is (new params all optional).

## What is NOT this task

- **Raising `MAX_TOOL_RESULT`.** A 63 KB result is re-sent every round *and every later turn*,
  and a `Conversation` cannot be trimmed. The cap is the point.
- A `Scope` field for the run ceiling (see §4 — a const until AS-04 needs otherwise).
- Profiling, statistics computation, or anything that scans — `describe_table` reports what
  registration read for free and that does not change.
- A second results pipeline. `run`/`read_page` already page a snapshot; nothing here duplicates
  that.
- Pushing the walk or the filters into the `Host` seam, or `Arc`-ing `Described` — the seam
  hands back owned data and the wire layer narrows it; the clone is paid today.
- The chat pane's rendering of any of this (AS-04).

## Docs to keep true in the same change

- `docs/AGENT_ACCESS_SPEC.md`: the ten-tools table rows for the three list tools, plus a bounds
  paragraph beside the existing `page_size` one.
- `crates/strata-agent/src/assistant/system.md`.
- This file: mark done, and record anything the implementation overturned.

## Notes

- Both halves were found by the second adversarial pass over AS-02 (2026-08-10), which fixed the
  cut result's *shape* (it used to be sliced mid-object into unparseable JSON) without noticing
  that three of the four tools it cuts have nowhere to go afterwards.
- The design above was settled 2026-08-10 (planning session: three source-verified explorations
  plus an adversarial stress pass over the candidate design). The stress pass's corrections,
  folded in above: fixed depth without a byte budget fails on the config fixture; a `path` to a
  leaf must answer as the node itself, not an empty children list; `From<Described>` cannot
  survive a parameterized, fallible projection; List child names are the file's own, never a
  documented constant; `NotFound`'s doc wording needs the column-path recovery.
- The measurements are reproducible: build the wire values at scale and `serde_json::to_string`
  them, which is what `dispatch::encode` does. Line numbers cited here are as of 2026-08-10.
