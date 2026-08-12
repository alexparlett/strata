# SQL Completion

The completion system, across `strata-core::engine::sql` (the language side),
`strata-code-editor` (the popup), and `strata-freya` (the wiring). Functions complete with
their real signatures in the row's detail, from the engine's `FunctionSym` catalog; a docs
panel and a signature-help popup were prototyped and dropped — the signature in the
completion detail is the surviving surface.

## 1. Principles

1. **Engine-authoritative vocabulary.** Every symbol source is the engine's own: keywords
   from DataFusion's `sqlparser` (`ALL_KEYWORDS`), reserved-word semantics from its
   `RESERVED_FOR_*_ALIAS` tables, functions from the live registry, tables/views/columns
   from the project catalog. Nothing SQL-shaped is hand-listed except *relevance policy*
   (which no grammar encodes) — and those live in named, documented tables (§3).
2. **Synchronous by construction.** The provider is a pure in-process function called
   inside the key handler, same frame as the edit. No debounce, no spawn, no epoch guards —
   stale results, flicker, and popup lag are *impossible*, not defended against. (§7 for
   the perf model that keeps this honest at scale; §9 for the escalation path if it ever
   stops being true.)
3. **The grammar decides the pool; heuristics only decide the order.** A heuristic may
   float candidates (projection-match boosting, §5) but never subtract them — incomplete
   knowledge (loading registrations, scraped CTEs, typos) must degrade to *worse ordering*,
   never to a mysteriously empty list.
4. **The editor makes zero grammar judgments.** `strata-code-editor` owns keys, placement,
   and the accept edit, generically; *what a position offers* — including nothing — is
   entirely the provider's answer. Completion is a mount-site service beside the language,
   not part of `EditorLanguage`: highlight queries are static data, completion needs live
   app state (catalog, registry).
5. **Mid-edit text is a valid prefix, not a mistake.** Guards and suppressions treat the
   draft as something being *composed*: quiet inside strings/comments/dangling decimals,
   no premature unresolved-column squiggles before a FROM exists (see §8).

## 2. The position model — clause × role

`Context` (context.rs) is two orthogonal dimensions, not a flat enum of cases:

```
Context = Dot(resolved_relation)          — after `alias.` / `relation.`
        | At(Clause, Role)
Clause  = Start | Restart
        | Select | From | On | Where | GroupBy | Having | Qualify
        | OrderBy | Limit | Offset | Describe | Execute
        | Create | CreateTable | CreateView | CreateExternal | CreateFunction
        | Drop | DropTable | DropView | DropFunction
        | Insert | Copy | SetOption | Prepare | Unknown
Role    = Operand        — an item is being started
        | Continuation   — the item just written is complete
        | Binding        — a fresh name is being invented (`AS |`, `CREATE TABLE |`,
                           `PREPARE |`, a column-def list, a VALUES tuple) or an
                           unmodeled statement noun typed (`SHOW |`): the empty
                           offer is correct by definition, not a suppression
```

**Clause** comes from the nearest clause keyword scanning back from the caret
(`last_clause` — derived from `clause_of`, so the two can't drift), within the
caret's statement (split on top-level `;`). The statement leads (`CREATE`, `DROP`,
`INSERT`, `COPY`, `SET`/`RESET`, `PREPARE`, `EXECUTE`/`DEALLOCATE`) only govern from
**position 0** (`leads_statement_only`) — sqlparser classes every dictionary word as
a keyword, so without the guard a column named `set` or `copy` would govern its own
SELECT list. `Create`/`Drop` are refined by the statement's head keywords
(`refine_statement_clause`: `CREATE [OR REPLACE] TABLE|VIEW|FUNCTION`,
`CREATE [OR REPLACE] EXTERNAL TABLE`, `DROP TABLE|VIEW|FUNCTION`); unrefined they
stay `Create`/`Drop`, whose role is always Continuation — the object word comes next.

**Start vs Restart**: a truly blank statement is `Start` and offers query leads
**and** statement leads. The *restart* positions are `Restart` — a fresh **query**
begins, so the statement leads would promise something Run refuses: a derived-table
`FROM (`, the position after a set operation (`UNION [ALL] |`, `EXCEPT |`), an
`EXPLAIN [ANALYZE]` prefix, `COPY (` (the source paren and only that one — a later
`PARTITIONED BY (` or `OPTIONS (` group is the statement's own), and the `AS |` of
CTAS / `CREATE VIEW` / `PREPARE` — a parenthesized body (`CREATE TABLE t AS (|`)
included, which must not read as a column-definition Binding. Role and continuations
treat `Restart` exactly as `Start`; only the lead pool differs.

**The `AS` rule is governing-aware**: `… AS |` is a Binding (a name invented)
*except* when the governing clause is `CreateTable`/`CreateView`/`Prepare` (the query
body restarts) or `CreateExternal` with `STORED` before the `AS` (the format-word
operand). Deeper positions inside a statement's query tail (`INSERT INTO t SELECT …
FROM |`, `CREATE TABLE t AS … WHERE |`, `COPY (SELECT … WHERE |`) resolve to their
own clauses via the nearest-clause scan — pinned by test, no code.

**The `SET` dotted-key rule** runs before the `Dot` rule whenever the governing
clause is `SetOption` (a config key is one dotted name — `SET datafusion.|` must not
read as the columns of a relation named `datafusion`). Key vs value is the presence
of an `=` between the lead and the caret. In key position the dotted chain is
absorbed backwards into **one** partial with **one** replace span, so an accept
replaces the whole chain; in value position `CaretAnalysis::set_key` carries the key
text (the shape `comparand` already has). After a complete value, the ordinary item
test yields Continuation, whose arm offers nothing.

**Role** is one uniform test (`item_complete`) on the token before the caret:
identifiers, literals, `)`, `END`, and the projection `*` end an item; everything else
starts one. Keyword tokens resolve through the same `is_name_like` predicate used by
every name position (sqlparser's reserved tables) *minus* the `OPERAND_EXPECTING`
connectives — so a column named `status` ends an item exactly like a plain identifier,
while `AND` / `DISTINCT` / `WHEN` never do. The FROM zone alternates on its own tokens
(targets after `FROM`/`JOIN`/a list comma); `DESCRIBE` expects one relation and then
nothing. The statement clauses alternate on their own head tokens instead (a
statement's grammar is positional): `DROP TABLE |` / `IF EXISTS |` / `a, |` are
operands and the statement is complete after its name; `INSERT INTO |` is the target
operand and its column list an operand too (the list names *existing* columns of the
target), while a VALUES tuple is a Binding — the content is the user's own data;
`COPY |` the source operand, its `PARTITIONED BY (…)` group an operand (columns of
that source) and any other group (`OPTIONS`) a Binding; a `CREATE FUNCTION` body
becomes an expression once a `RETURN` lies between the head and the caret
(`RETURN |` and `RETURN price * |` are operands, `RETURN price |` a continuation).

**Dot resolution** order: FROM/JOIN alias → inline relation (CTE, then a
**derived-table alias** — `FROM (subquery) t` captures `t` + its scraped projection
exactly like an inline CTE, resolvable but never offered as a FROM target) → catalog
table/view. Unknown qualifier ⇒ empty — precision over noise. The analysis also
carries the **governing clause for dot positions** (an `ON e.|` wants join-key
ranking; a `SELECT e.|` doesn't) and the **comparand** (the column ref across a
trailing comparison operator) for §5's affinity forces.

## 3. Grammar tables

Two kinds, deliberately distinguished:

- **Parser-derived** (track the engine automatically): the keyword universe
  (`ALL_KEYWORDS` minus `BLOCKED_KEYWORDS` — the statement router's refused forms,
  kept honest against `validate::classify` by test; `docs/STATEMENTS_SPEC.md`),
  name-position reservedness (`lex::is_reserved_in_name_position` over the
  `RESERVED_FOR_*_ALIAS` tables — also the identifier-quoting rule).
- **Declared** (grammar/policy knowledge no parser table encodes, one definition each):
  - `LADDER` — the canonical clause order (`SELECT → FROM → WHERE → GROUP BY → HAVING →
    QUALIFY → ORDER BY → LIMIT → OFFSET`), plus `SET_OPS` appended to every tail.
  - `OPERAND_EXPECTING` / `LITERAL_WORDS` (context.rs) — connectives that start operands
    vs literal/direction words that end items; shared by the role test and the
    projection scraper.
  - `EXPR_OPS`, `JOIN_CONT`, `ORDER_CONT` — clause-internal continuations.
  - `QUERY_LEADS` — the query/inspection leads (SELECT/WITH/EXPLAIN/SHOW/DESCRIBE
    forms), offered at `Start` **and** `Restart`.
  - `STATEMENT_LEADS` — every statement the router intercepts (`SET`, the CREATE and
    DROP families, `INSERT INTO`, `COPY`, `PREPARE`/`EXECUTE`/`DEALLOCATE`,
    `RESET`), offered at `Start` only, after the query leads. Kept honest by the
    lead → canonical-tail table in `policy_and_completion_agree_on_statement_leads`:
    every lead's tail must classify `Intercept`/`Query` for the editor, and a lead
    with no tail entry panics the test.
  - `MULTI_WORD` — presentation phrases (`GROUP BY`, `LEFT JOIN`, `IS NOT NULL`).
    Query-only, deliberately: it rides ungated at every expression operand position,
    so statement phrases must not enter it.
  - `JOIN_LEADINS` — join modifiers after which `JOIN` itself is next.
  - Statement vocabularies owned by the modules whose dispatch they mirror:
    `ddl::external`'s `STORED_AS_FORMATS` (each entry must parse through
    `read_format`, held by its own test) and its `CSV_OPTION_KEYS` /
    `JSON_OPTION_KEYS` tables (`{key, kind, what, set}` — the table **is**
    `apply`'s arm set, so the offer and the arm cannot drift); `config::ENGINE_KEYS`
    filtered through `ddl::session::refuse_reserved_key` for the `SET` key pool.

## 4. Pools and ranking

Per position (the `complete/` module: `mod.rs` = API + pools + insert shaping,
`vocabulary.rs` = the declared grammar tables, `ranking.rs` = tiers + forces +
the rank pipeline, `tests.rs` = the suite):

| Position | Pool (context tier order) |
|---|---|
| `Start` operand | `QUERY_LEADS` then `STATEMENT_LEADS` (curated ord continues across the two), then gated keywords |
| `Restart` operand | `QUERY_LEADS` only, then gated keywords |
| `From`/`Describe`/`Copy` operand | relations only — CTEs, tables, views (projection-boosted, §5; for `COPY` the boost is a no-op) |
| `SetOption` operand, key | `ENGINE_KEYS` filtered by `refuse_reserved_key(k).is_ok()` — verbatim insert, detail = the key's `default`, `ENGINE_KEYS` order, kind `Column` (a glyph, not a taxonomy) |
| `SetOption` operand, value (`set_key`) | the key's kind vocabulary: `Bool` ⇒ `true`/`false`, `Enum` ⇒ its options, else nothing — verbatim lowercase, no trailing space |
| `DropTable` operand | tables and **not** views (`DROP VIEW` is the other statement) |
| `DropView` operand | views only, for the mirror reason |
| `Insert` operand | at the target: tables with `internal: true` only — the same answer `Engine::is_internal` gives dispatch, read from the store; in the column list: the target's own columns (see the column-list rule below), offered only when the target is one an INSERT may reach |
| `Copy` operand, in `PARTITIONED BY (…)` | the source's columns — the catalog's for a named table, the scraped projection for a `COPY (SELECT …)` source (the column-list rule below) |
| `CreateExternal` operand (`STORED AS \|`) | `STORED_AS_FORMATS` as keyword items |
| `DropFunction` operand | function syms with `created: true` — bare-name insert, detail `session function` |
| `CreateFunction` operand (the body, after `RETURN`) | the declared argument names (scraped from the token stream, detail `argument`), then functions — **never** catalog columns or relations (the body may reference only its arguments) |
| `Execute` operand | the session's prepared names |
| `Limit`/`Offset` operand | **nothing** (numbers) |
| any `Binding` position | **nothing** (a name is being invented) |
| any expression operand | in-scope columns (0) → select-aliases (1, **only** in GROUP BY/ORDER BY/HAVING/QUALIFY — where SQL allows them) → functions (2) → relations-as-qualifiers + core keywords (3) |
| any continuation | `continuation_keywords(clause)` in curated order (0): clause-internal ops + **the ladder strictly after the clause** — never backwards; the statement clauses carry their own short lists (`CREATE \|` the object words, `CREATE EXTERNAL TABLE t \|` its clauses, `COPY t \|` `TO` first, drop statements nothing) |
| `Dot(rel)` | that relation's columns only |

**The column-list rule** — one capability, not per-statement code, and **one
decision**: `analyze_caret` resolves the list once onto `CaretAnalysis::column_list`,
and both the role (`role_at`) and the pool (`push_list_columns`) read that answer —
never two token scans that must agree. A statement position whose operand is a
**column of one known relation** (an INSERT's column list, a COPY's
`PARTITIONED BY` group) resolves the way a `Dot` position does. That relation's
columns and nothing else (a dotted relation answers its last segment, the
single-namespace rule); an unresolvable relation is the empty offer (Dot's own
"precision over noise"); and the group's already-listed names written-demote through
the same `column_ord` composition a clause region's refs use — rank only, never
filter, exactly as a SELECT list demotes what it already projects. (A CET's
`PARTITIONED BY` deliberately stays a Binding: its schema is inferred from files at
registration, so there is no relation to resolve while typing — see §10.)

**The `OPTIONS`-key carve-out** — the one exception to the string guard, scoped to
exactly one position: the caret inside a single-quoted literal in **key position**
(predecessor `(` or `,`) or **value position** (predecessor another string) inside the
`OPTIONS (…)` group of a statement whose head refines to `CreateExternal`. Two lexing
cases, both required: a terminated literal rides the ordinary token stream (replace =
the content span between the quotes); an unterminated one (`OPTIONS ('format.h|`)
errors the tokenizer, and the recovery — bounded to this position — lexes the prefix
before the opening quote, which must be clean; any other lex error stays a guard.
The offer is format-aware (`STORED AS <word>` scanned from the statement): CSV →
`CSV_OPTION_KEYS`, JSON → `JSON_OPTION_KEYS`, NDJSON → the JSON set minus
`format.newline_delimited` (refused there toward `STORED AS JSON`), Parquet / Arrow /
unwritten → empty. Value offers ride the same carve-out with the preceding key looked
up in the table (`Bool`/`Enum` only). Store-namespace keys and `CLIENT_KEYS` are
never offered — the arm refuses them toward Connections, and absence from the offer
is the same policy stated once.

**Match tiers** (fuzzy.rs, case-insensitive): exact (0) → prefix (1) → word-boundary
subsequence, `ui`→`user_id` (2) → contiguous substring (3) → gap subsequence,
`usrid`→`user_id` (4); non-subsequence is filtered out. Empty partial ⇒ tier 0 for all.

**Composite sort**: `(match_tier, context_tier, curated_ord, label_len, alpha)`, dedupe
by (kind, label), truncate 50.

**Keyword gating**: at operand positions the `CORE_KEYWORDS` vocabulary rides at the
keyword tier and only the obscure tail needs a ≥2-char prefix; at continuation positions
the curated set *is* the expected-token set, so **all** other keywords are tail-gated —
`FROM` can never trail a `WHERE` clause uninvited, yet `TABLESAMPLE` remains reachable
by typing at it.

**Scope columns**: in-scope = the statement's FROM/JOIN relations (aliases bound,
CTEs resolved). When the scope resolves to zero columns (no FROM yet, unregistered
name), *all* catalog columns offer at the secondary tier with the owning table in the
detail — the SELECT-before-FROM affordance.

## 5. Ranking under incomplete knowledge

Beyond match and context tiers, the `ord` sub-tier carries the **composition
heuristics** — every one a demotion/boost over the grammar-determined pool, never a
filter (self-joins, `upper(user_id)` reuse, and cross-type casts are all legal; an
unknown ref shifts every candidate uniformly and the list never empties).

**Reference regions** (context.rs): all scraped by one collector (`refs_in`) over
clause regions — bounded by the caret's **set-op branch** (UNION branches repeat each
other's shapes by design, so refs never cross one) and its **paren scope** (a
subquery's list is its own region; the scope-aware governing scan also means a
subquery tail like `… (SELECT x FROM t) AND |` is governed by the outer WHERE, not
the inner FROM).

- **Coverage boost** (projection → relations, symmetric): FROM targets rank by how
  many written select-list refs they contain; fallback columns rank by their owning
  table's coverage (`ord += deficit × 2`), best-covered tables feeding the cap first.
- **Written-demotion** (uniform): a candidate already referenced in the region it
  would join sinks one step — a projected column in the SELECT list, a grouped key in
  GROUP BY, a tested column (mildly) in WHERE, an already-joined relation at a JOIN
  target. The region is always *the caret's own clause list*, so select-list refs
  never demote in WHERE, where reuse is idiomatic.
- **Join-key affinity** (ON positions): a column whose name exists on the *other*
  side of the join is the probable equi-key — floats at `ON |` and `ON e.|`.
- **Comparison type affinity** (any comparison side, WHERE included): when the caret
  follows `= < > …` with a resolvable column ref on the other side (`comparand`),
  same-type-family candidates (the `Kind` vocabulary — Num/Str/Ts/…) float;
  `a.int = b.string` sinks without vanishing.

Column forces compose in one helper — `column_ord`: affinity-miss ×4, cross-key-miss
×2, written ×1 (a declared strength order, strongest signal first).

## 6. Insert semantics

Per kind, uniformly:

- **Identifiers** (tables/views/columns/CTEs): the name exactly; double-quoted only when
  not a plain lowercase ident **or** when colliding with a *reserved* word (`order` →
  `"order"`; merely-known keywords like `name`, `status`, `plain` stay bare).
- **Keywords**: canonical UPPER + **trailing space** (a keyword is always followed by
  something) — skipped when the buffer already has whitespace after the span.
- **Functions**: `name(` — caret inside the parens.
- **Accept** replaces the partial-word span (byte span from the service, converted at the
  editor seam), lands the caret at the insert's end, and is **one undo step**
  (`replace_range`: seal → remove → insert → seal).
- **Chaining**: accept always re-asks the provider; the popup reopens **only when the
  answer reports a fresh position** (an empty replace span — after `FROM `, inside
  `sum(`). A plain identifier accept (caret at a word end) or a nothing-offer position
  (`LIMIT `, `AS `) stays closed. The gate is the provider's own answer — the editor
  never inspects the inserted text.

## 7. Guards and performance

Guards short-circuit `complete()` before any analysis: caret inside a string literal,
line/block comment (including regions unterminated at EOF — the tokenizer can't answer
this, a dedicated linear scanner does), a dangling decimal (`1.` — the dot absorbed
into the number token; `lex::caret_extends_numeric_literal`), and **any tokenizer
error** — an un-tokenizable buffer (unterminated `"ident`) empties the token stream,
so every position would masquerade as a blank statement; quiet everywhere beats
mis-offering anywhere. A **manual** trigger (⌃/⌘Space) lifts the obscure-keyword tail
gate — an explicit ask deserves the full vocabulary; nothing else widens.

Performance model, sized against a 100-tables × 1000-columns catalog:

- **The Catalog snapshot is memoized** (tab.rs): rebuilt only when the project store
  changes (registration lands, view saved) — never per keystroke. The provider peeks it.
- **A candidate is matched before it is built** (`ranking::Pool`). Every pool used to be
  materialized whole and filtered afterwards, so a keystroke built 1600-2700 `Completion`s
  — three or four string allocations each — however few the partial could match, and the
  demoted `ALL_KEYWORDS` tail (~1200) was built in full at every operand position only to
  be dropped by the tail gate. `Pool` takes the label first and calls the builder only on a
  hit, so the rule the all-columns fallback already followed is now structural and no pool
  can forget it; the tier it computes is the one `rank` sorts on, so nothing matches twice.
  A `debug_assert` holds the gate label to the completion's own label at every push site —
  the equivalence with filtering afterwards rests on exactly that.
- **The match is allocation-free, and it rejects before it ranks** (`fuzzy::match_tier`).
  The filter itself used to allocate two lowercased copies per candidate, so the fallback's
  100k-candidate sweep paid 200k allocations to answer *no* — 36ms per keystroke, four
  dropped frames, for an offer that was usually empty. Being a subsequence is a
  **necessary** condition for every tier, so testing it first is exact and rejects almost
  everything in one scan. Ranking is unchanged (verified equal to the original on 6.6M
  pairs).
- **A per-candidate membership test is a set, not a scan** — `ranking::folded_set`, for the
  written-demotion's clause refs and the ON-position join keys alike. Neither list is
  bounded: a clause region grows with the query and a join's other side with the relation's
  width, so both scans were quadratic. The written-demotion over a long `WHERE` was the
  larger of the two (~120 refs × 2000 columns per keystroke) and is why a long query cost
  more than a short one — **not** the analysis layer, which measures ~340µs of a 7KB
  buffer's total.
- **A coverage boost that would walk the catalog is skipped when there is nothing to
  cover.** The projection→relation ranking counts a table's matching columns per candidate
  relation; with an empty projection that is a uniform zero bought at the price of the
  whole catalog's width.
- **A sort comparator never allocates.** The alphabetical tie-break compares lowercased
  bytes lazily; building those keys inside the comparator cost two allocations per
  *comparison*, paid O(n log n) times whenever an empty partial left the pool unfiltered.
- Everything else is bounded by *scope*: the FROM'd tables' columns, ~400 functions,
  ~1200 keywords, relation names.

Measured (release, min of 100 reps, quiet machine); the 120Hz frame budget is 8.3ms:

| Position | before | after |
|---|---|---|
| `SELECT <prefix>\|`, no FROM — fallback, 100 × 1000 | 35.97ms | 1.62ms |
| `JOIN … ON \|`, 2 × 1000 cols | 4.95ms | 1.13ms |
| `JOIN … ON t.\|`, 2 × 1000 cols | 2.24ms | 0.39ms |
| `SELECT <prefix>\| FROM t`, 1000 cols | 1.93ms | 0.27ms |
| `SELECT \| FROM t`, 1000 cols | 0.74ms | 0.55ms |
| `FROM \|` (relations), 100 tables | 0.04ms | 0.03ms |
| ~250-line query, typed prefix, 2 × 1000 cols | 2.98ms † | 0.79ms |

† measured after the `match_tier` fix, before the pool and set fixes — the original was
worse still.

What is left is genuinely proportional to the offer:

- **An empty partial cannot be filtered** — every candidate is offerable, so the in-scope
  columns really are built (0.55ms at 1000 columns). Bounding it means a 50-element
  heap instead of a full sort, which trades the simple sort key for a bounded one; not
  worth it while the numbers look like this.
- **The all-columns fallback is O(catalog) by construction** — 100k columns must each be
  tested to know none match (1.6ms). A prefix index is the only thing that changes that
  shape, and it is the one place where the indexing half of an IntelliJ-style design would
  genuinely earn its keep.

## 8. Editor integration (strata-code-editor)

The editor owns the **generic** machinery; `CompletionItem/Kind/Request` are its own
types (no strata-core dependency), the provider is a component prop
(`on_completions: Callback<CompletionRequest, Vec<CompletionItem>>`) wired at the mount
(tab.rs).

- **Key claim**: while open, unmodified ↑/↓ (wrap), Enter/Tab (accept), Esc (close) are
  consumed *before* the app's pre-key gate and the editor's own `process_key`, with
  `prevent_default()` — which also cancels the derived global events, so Esc never
  cancels a running query and Enter never inserts a newline. Manual trigger: physical
  `Code::Space` + ⌃ or ⌘.
- **Trigger table** (`trigger_after_edit`, pure + tested): ident chars and `.` recompute;
  digits filter an open popup but never open one; Backspace/Delete refilter only while
  open; word boundaries close; modified chars never trigger. Caret-only moves refilter
  within the anchor word and close on leaving it.
- **Placement**: anchored at the **word start** (never slides while typing), window-space
  via the editor's measured origin, `Layer::Overlay` (escapes the editor pane, paints
  over results), flip-up when the window bottom is short (`flip_and_clamp`, pure +
  tested), horizontal clamp. 300×≤224px, 30px rows, kind chip + label + dim detail —
  the design-canvas dress, themed via the `code_editor` `completion_*` fields.
- **Dismissal**: Esc, zero matches, word exit, outside press (popup-rect hit test),
  any editor scroll. The diagnostics hover panel is suppressed while open.

## 9. Testing strategy + escalation

Two tiers in `complete/tests.rs`/`context.rs`/`lex.rs`: **scalpels** (one rule per test —
ranking claims like "`status` beats `SET`", role detection per position, guard cases,
insert forms) and the **torture corpus** — realistic analyst SQL (window functions +
QUALIFY, derived tables + scalar subqueries, CTE-of-CTE, unions with interleaved
comments, CASE-heavy projections, dangling multi-statements) swept by an
**every-caret invariant test** (no panics, spans in-bounds, cap respected at every byte
of every query) plus targeted probes at the nasty positions. The sweep is what caught
the set-op ladder restart; the probes are where known degradations are *documented as
tests* (a derived-table alias dot-completes to silence — subquery scopes are deferred).

If a future catalog outgrows the sync budget: keep the popup synchronous and move only
the provider call off-frame (spawned work behind a revision gate — the diagnostics
driver's pattern) behind the same `on_completions` seam. LSP (process boundary,
JSON-RPC) is categorically out — the provider lives in-process.

**Async is the structural answer, and it is deliberately *not* the first one.** A sync
provider makes the frame budget a hard ceiling on work the render thread may do, and
nothing bounds a catalog's width or a buffer's length — so the tail is real, and one day
this escalates. But the 36ms above was not the sync design failing to hold a load: it was
200k needless allocations per keystroke, and a worker thread would have *hidden* it —
trading a visible 4-frame stall for a 36ms-stale popup, which is the harder bug to see and
the worse one to type against. The order therefore matters: make the work small, then
decide whether what remains needs a thread. Going async first buys the revision gate, the
cancellation of superseded requests, a `Send` catalog snapshot, and the loss of §1's
"stale results and flicker are *impossible*, not defended against" — to hide a cost that
should not have existed. The waste is now spent: worst measured position 1.6ms against an
8.3ms budget, and what remains is proportional to the offer rather than to the catalog.

So the escalation trigger is a **measured** position over budget — and when one arrives, read
which shape it has first, because they want different answers. Work proportional to *what is
offered* (an empty partial over wide relations) is what a thread genuinely moves. Work
proportional to the *catalog* (the all-columns fallback testing 100k names to find none) is
an index problem, and threading it only relocates it — that half of an IntelliJ-style design
is the half doing the real work. Neither is reached by rewriting the popup.

## 10. Known trade-offs (chosen, not hidden)

- `LEFT |` with an empty partial lists `WHERE` before `JOIN` (From-continuation order is
  one curated list; a "mid-join-phrase" micro-position isn't worth a fourth role).
- Caret-x is `col × char_width` (monospace product) — wide glyphs drift by a few px,
  the same estimate class the diagnostics panel accepts.
- The comparand scan is a fixed token window looking **left** of the operator only
  (`x = |` ranks by x's type; `| = x` has no other side yet); inline-relation columns
  carry no dtypes and count as affinity misses (uniform within their list, so
  relative order is unharmed).
- `column_ord`'s 4/2/1 force weights are a declared priority, not derived — one
  documented constant, revisited only with evidence.
- `SHOW`'s nouns (`TABLES`, `COLUMNS FROM …`) are unmodeled — the Binding role keeps
  those positions silent rather than offering the ladder; the `SHOW TABLES` statement
  phrase still completes from `Start`.
- The editor's `is_ident_char` (trigger/anchor word test) is its own generic definition
  and differs from the parser dialect's identifier characters (`$`/`@`/`#` are word chars
  under `generic`) — a `price$usd` column dismisses the popup at the `$`. A
  provider-supplied word predicate would close this, reaching the configured dialect the
  way `lex` does (`lex::dialect`); deferred until it bites.
- The SELECT-list scrapers (`column_aliases`, `projection_columns`) and the reference
  collector (`refs_in`) share the grammar tables and agree on depth/literal policy, but
  remain separate walks — a single parameterised scraper is a clean refactor deferred
  until it earns its keep.
- The no-FROM grace in `validate.rs` keeps column references quiet before a FROM
  exists (depth-0-scoped, so CTE drafts keep it) — mid-edit text is composition, not
  error (§1), even though the native resolver reports every unknown name once a scope
  exists.
- Type-aware argument narrowing (only numeric columns inside `sum(`) needs registry
  signature metadata, and that metadata exists (`FunctionSym.signatures`, from the
  DataFusion registry) — today it renders only the completion detail; narrowing the
  argument offer against it is unbuilt (the docs-panel + signature-help UX that would
  also have consumed it was dropped).
- Statement completion's deliberate silences (ED-11): `LOCATION '|'` and
  `COPY … TO '|'` stay quiet — they are paths, and the right answer is the user's
  filesystem, not a list; `COPY`'s own `OPTIONS` stays quiet — DataFusion's open
  key namespace, not ours (its `STORED AS |` is likewise a plain Binding — only
  `CREATE EXTERNAL TABLE` has a format vocabulary behind that position); `RESET`
  shares `SET`'s key pool — the session overlay is not on the snapshot, so the
  settable superset is the honest offer; `INSERT |` wanting `INTO` is served by the
  `INSERT INTO` lead phrase rather than a continuation; VALUES tuples are Bindings
  (the content is the user's own data — unlike an INSERT **column list** or a COPY
  **partition list**, which name existing columns and are offered, see §4).
- `CREATE EXTERNAL TABLE`'s `PARTITIONED BY (…)` stays a Binding even though COPY's
  is an operand: a CET's schema is inferred from files at registration, so the
  columns are simply not known while the statement is being typed — there is nothing
  honest to offer.
