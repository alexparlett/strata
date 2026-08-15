# Query ergonomics (QE) — deep-JSON feedback workstream

Field feedback (2026-08-13) from users driving Strata's agent surface over large object-keyed
JSON — the same shape as `sample/config.json` (62 MB, 19 top-level columns, 241,425 nested
fields, one struct of 19,311 UUID keys). Twelve reported gaps, triaged here against the source:
six are ours to build (the tasks below), five are upstream DataFusion 54 behaviour we can at
most mitigate (the ledger at the bottom), and one was already built when the feedback arrived
(identifier casing — a settable key; its gap is discoverability, folded into QE-06).

**The headline:** the whole `contentBlocks` story (feedback items 1+2) reduces to one small
UDF family (QE-01), Arrow-side first. `engine::json_poly` always infers an object as a
**Struct** (`infer.rs:247-258`; there is no Map arm anywhere), so `struct_keys` answers "which
keys does this row have" straight off the null bitmaps, `struct_entries` walks a
same-shaped keyed map with the values still typed Arrow, and `struct_get` indexes by a
computed key (which `get_field` cannot) — no serialisation in the common case, which is the
form the feedback itself proposed. `to_json(x) → Utf8` is the total
fallback for heterogeneous structs (and item 2's direct ask): `datafusion-functions-json`
0.54.2 is registered whole (`engine/mod.rs:1985`) and its `json_object_keys`/`json_get`
family speaks JSON text, so `to_json` hands any subtree to it — and its metadata-free Utf8
also sidesteps the recursive-CTE unification bug (ledger item 4). QE-01 opened with a spike on
`datafusion-contrib/datafusion-variant` and **rejected** it (2026-08-13): the published 0.1.0
pins DF 52 / arrow 57 — a second DataFusion in the graph — and has no key-enumeration function
at all, only its unreleased git HEAD builds against our pin, and against the fixture a Variant
column reaches the grid, the inspector and export as hex while a keys-only read costs 58.7 ms
where the null-bitmap walk costs 19.75 µs. The evidence, the one capability it has that we do
not (dynamic access to a *heterogeneous* struct), and the revisit condition are in the task.

## Tasks

| ID | Task | Feedback items | Status |
|---|---|---|---|
| QE-01 | [Struct UDFs: `struct_keys`, `struct_entries`, `struct_get`, `to_json`](QE-01-struct-udfs.md) | 1, 2 (mitigates 4) | ✅ |
| QE-02 | [`regexp_extract_all` UDF](QE-02-regexp-extract-all.md) | 6 | ✅ |
| QE-03 | [`describe_table` shape collapse for keyed siblings](QE-03-describe-shape-collapse.md) | 10 | ✅ |
| QE-04 | [Agent query-session lifetime](QE-04-session-lifetime.md) | 11 | ✅ |
| QE-05 | [Agent result export — the first curated write](QE-05-result-export.md) | 12 | ✅ |
| QE-06 | [Deep-JSON guidance + the upstream ledger](QE-06-guidance-and-ledger.md) | 3, 4, 5, 7, 8, 9 | ✅ |
| QE-07 | [Bound every schema surface: shared collapse + derived depth](QE-07-schema-bound.md) | follow-on from 10 | ⬜ |
| QE-08 | [The catalog pane survives a keyed struct](QE-08-catalog-pane-bound.md) | follow-on from 10 | ⬜ |

QE-03 landed the collapse the "real win" line named: past the byte budget, eight or more
structurally identical sibling *containers* become one `<key>` entry with `keys_total` and
`key_examples`, and `matching` answers one row with `matched_keys` instead of thousands of
paths differing in one segment. Four corrections are in its file — the collapse is a
**cutting** strategy (an answer that fits complete is never collapsed, or sixty `Utf8`
columns lose their names), a leaf never joins a set, the walk root collapses **before** it
pages (or `keys_total` would be a fact about the page), and shape equality is *checked*
(`ColumnInfo` derives `PartialEq`) with the hash only bucketing. It also moved `SCHEMA_DEPTH`
3 → 5 behind a per-rung node cap, which is what puts `eligibilityRule` in the first answer.

QE-07 and QE-08 were planned 2026-08-14, out of QE-03's review discussion plus a probe of
the real 62 MB `config.json` (gitignored; symlinked into worktrees) and a window-freezing
bug: expanding `contentBlocks` (19,311 keys) in the catalog pane hung the app to a
force-kill. The probe's verdict, recorded in QE-07 so it is not re-derived: the keyed
object fragments into **50 shapes, power-law distributed** — the 15 an answer shows cover
93.6% — which vindicates `json_poly`'s Struct inference (a `Map` needs one value type; do
not reopen record-vs-map) and makes the *presentation* bound the invariant: a surface that
renders a schema bounds it. QE-07 promotes QE-03's collapse to a shared `strata-engine`
mechanism, derives the describe ladder's depth instead of pinning it at 5, and counts
elided shapes; QE-08 lands the pane's cap + collapse rows **after DB-05's tree**, whose
task file was deliberately not edited (active session).

Dependencies: QE-01..05 are independent of each other. QE-06 references QE-01's function by
name, so it lands last (or its guidance ships without that line and gains it with QE-01).
QE-07 sits on QE-03's merge; QE-08 sits on DB-05 and (for its collapse half) QE-07.
QE-05's permission model was **decided** (Alex, 2026-08-13) and built that way: always
available, agent-supplied path — read access already hands over the data, so the fence is the
write rules (owned storage refused, no overwrite, no folder creation, plus the three shape
rules that make those answerable), not a consent gate.

## Settled facts the tasks stand on (source-verified 2026-08-13)

- Adding a built-in UDF is **one `register_udf` call** in `build_context`
  (`engine/mod.rs:1884-1995`) — autocomplete, signature detail, the docs panel and the agent's
  `list_functions` all follow automatically because they read `functions::snapshot`
  (docs/reference/ENGINE.md: "Adding a UDF family means one `register_*` call in
  `build_context` and nothing else"). Strata registers **zero** custom UDFs today; the only
  `ScalarUDFImpl` in the workspace is `SqlMacro`. A new built-in inherits the `DROP FUNCTION`
  fence for free (`Functions::created` stays false for it).
- The reported "describe_table pages at 25" is specifically `MATCH_PAGE` (`describe.rs:56`),
  the name-search page; plain column paging is `SCHEMA_PAGE` = 50, under `SCHEMA_BUDGET` =
  16 KB and the depth/width sampling ladder. The fix is not a bigger page — it is QE-03's
  shape collapse, which the feedback itself names as "the real win".
- Agent sessions die five ways; the two that match "expired mid-investigation" are the
  **stateless idle sweep** (`STATELESS_IDLE` — `Caller::Stateless` only; connected clients
  are retracted by `Drop`) and the **20-sessions-per-agent cap** (`agents.rs:65`, oldest
  non-running evicted). `read_page` deliberately does not pin (AGENTS.md), so an expired
  session's result is gone by design; the lever is the TTL and the stated bound, not a pin.
  QE-04 moved that TTL 5 min → 30 min and stated it in the tool description, `system.md` and
  the spec; the 5 was parity with rmcp's `SessionConfig::keep_alive`, which governs the
  *session* lifecycle this sweep does not serve.
- Export from the agent was deliberately absent, and the spec reserved its shape:
  "**Curated writes** … arrive as new, separately permissioned tools; `run` never loosens".
  QE-05 built it and relaxed the permission half: the data is already fully readable through
  `read_page`, so the fence is the **write** rules and `run` still never loosens. The spec's
  reserved paragraph is now a *Curated writes* section recording that. `export_result` is the
  eleventh tool; the assistant keeps its other answer too, because `offer_sql` validates under
  the **editor's** capability and can hand the user a `COPY … TO` card the assistant itself is
  refused — `system.md` says which is for which.
- `datafusion.sql_parser.enable_ident_normalization` already exists in `ENGINE_KEYS`
  (`engine/config.rs:321`, default `true`), is offered in Settings ▸ Engine ▸ Properties, is
  **settable by typed `SET`** (absent from `refuse_reserved_key`'s list), and the language
  service follows it (`sql/resolve.rs:89-95`). Catalog identity deliberately does not follow
  it (`fold_ident`, `engine/mod.rs:1842-1843`) — that is fine: it changes SQL resolution, not
  registration keys. Feedback item 9 is therefore discoverability (QE-06), not capability.

## Upstream ledger — DataFusion 54 behaviour, not ours to build

The pin is structural: DataFusion is held at **54** by `datafusion-table-providers` 0.13 +
`datafusion-federation` 0.5.5 (the four move together — see `docs/CONNECTIONS_SPEC.md`), so
even an upstream fix arrives only when that whole set bumps. Recorded here so nobody
re-diagnoses these from scratch; revisit the list at the next DF bump.

Every **refusal and workaround** below was re-run against this build in QE-06 (2026-08-14), and
three of them had inherited a wrong workaround from the field reports; those corrections are the
entries, not footnotes to them. What was *not* re-run is called out where it sits — item 3's
fd exhaustion is still the field report's, because reproducing it needs the 96-branch query and
the 62 MB source, not a unit-scale fixture.

3. **A UNION ALL branch is its own scan** — measured: three branches over one table plan three
   `DataSourceExec` nodes, so ~96 branches over one JSON source re-parse it 96 times. The
   `EMFILE` that follows is **the field report's, not re-run here** — it needs that query
   against the 62 MB source, and nothing at fixture scale exhausts fds. Upstream: no shared or
   materialised scan for repeated references. **Workaround:** materialise once with an internal
   table (`CREATE TABLE t AS …`) and query that. **Corrected:** it does not reduce the *number*
   of scans — the union still plans one per branch — it makes each one a read of an already
   parsed Arrow spool instead of a re-parse of the source. The spool is one file per CTAS
   output partition (1 and 4 observed, for a one-row and a four-partition create), so it is
   fewer files than the source only when the source is a multi-file listing — which is why this
   is a parse saving first and an fd saving only sometimes.
4. **A `json_get_json` result will not unify against plain text across a recursive CTE's
   branches** — its `arrow.json` extension metadata fails the projection check
   ("field metadata differs"). **Corrected:** the mismatch is *between branches*, not a
   property of the function — a CTE whose seed and recursive term both call it plans fine,
   and one where only one side does fails in whichever direction the metadata sits.
   `x || ''` strips the metadata; `CAST(x AS VARCHAR)` and `arrow_cast(x, 'Utf8')` were both
   re-checked and do **not**, so the spelling has to go on every branch that calls a json
   function. Upstream (`datafusion-functions-json` / DF's unifier). **Mitigated by QE-01:**
   `to_json` returns plain Utf8 with no extension metadata.
5. **A FROM-clause `UNNEST` alias has no addressable fields** — `SELECT r.p FROM t,
   UNNEST(t.arr) AS r` fails with "No field named r.p. Valid fields are …
   r.\"UNNEST(outer_ref(t.arr))\"", with or without an outer reference (a literal
   `FROM UNNEST([…]) AS r` fails the same way; only `SELECT *` gets the column out). The
   report's own wording, "Invalid qualifier r", **does not reproduce** in any shape tried —
   worth saying upstream, because it suggests the report came from a different version or a
   different query. **Corrected: the bracket spelling does not work either**
   ("No field named r"), and a column alias (`AS r(v)`) trips a DataFusion internal-error
   assertion in the federation optimizer rule. The workaround is to **unnest in the select
   list of a subquery** — `SELECT r.p FROM (SELECT unnest(arr) AS r FROM t)` — where both
   `r.p` and `r['p']` resolve.
6. *(ours — QE-02, built: `regexp_extract_all`)*
7. **`string_agg(DISTINCT x, d ORDER BY y)` is refused** — "In an aggregate with DISTINCT,
   ORDER BY expressions must appear in argument list". **Corrected: the error names the
   workaround and it works** — `string_agg(DISTINCT s, ',' ORDER BY s)` runs; only ordering
   by a *different* column is unsupported, for which a pre-deduped subquery carrying its own
   ordering is the dodge. Upstream aggregate limitation.
8. **`UNNEST` in FROM can't reference nested outer columns** ("Nested identifiers are not
   yet supported for OuterReferenceColumn") — upstream. The subquery-projection rewrite is
   the workaround, and it has to land in a **select-list** unnest: projecting the nested
   column out and then unnesting it in FROM clears this error only to hit item 5.
9. *(already a key — see settled facts; guidance lands in QE-06)*
   Re-verified: an unquoted mixed-case struct field is "Field contentvariants not found in
   struct", the quoted spelling resolves, and `SET`/`RESET` of
   `datafusion.sql_parser.enable_ident_normalization` turn the folding off and back on for
   the session.

Feedback items 1, 2, 6, 10, 11, 12 are the six tasks. Nothing here is filed upstream yet; if
any of 3/4/5/7/8 blocks a user again, filing the issue against DataFusion (or
datafusion-functions-json for 4) is the next escalation, and this ledger is the reproduction
note to file from.
