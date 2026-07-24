# SQL Completion — design spec (as built, P2-04)

**Status: definitive.** This documents the completion system as implemented across
`strata-core::engine::sql` (the language side), `strata-code-editor` (the popup), and
`strata-freya` (the wiring). It **supersedes `SQL_LANGUAGE_SPEC.md` §4** (which described
the pre-build plan: a debounced async pipeline, substring ranking, ⌘Space-only). Follow-on
work: `P2-22` (docs panel + signature help), `P2-23` (validator fitness — multi-error).

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
   entirely the provider's answer. Completion is a mount-site service beside the language
   (like `use_validation`), not part of `EditorLanguage`: highlight queries are static
   data, completion needs live app state (catalog, registry).
5. **Mid-edit text is a valid prefix, not a mistake.** Guards and suppressions treat the
   draft as something being *composed*: quiet inside strings/comments/dangling decimals,
   no premature unresolved-column squiggles before a FROM exists (see §8).

## 2. The position model — clause × role

`Context` (context.rs) is two orthogonal dimensions, not a flat enum of cases:

```
Context = Dot(resolved_relation)          — after `alias.` / `relation.`
        | At(Clause, Role)
Clause  = Start | Select | From | On | Where | GroupBy | Having | Qualify
        | OrderBy | Limit | Offset | Describe | Unknown
Role    = Operand        — an item is being started
        | Continuation   — the item just written is complete
        | Binding        — a fresh name is being invented (`AS |`) or an
                           unmodeled statement noun typed (`SHOW |`): the empty
                           offer is correct by definition, not a suppression
```

**Clause** comes from the nearest clause keyword scanning back from the caret
(`last_clause` — derived from `clause_of`, so the two can't drift), within the
caret's statement (split on top-level `;`). The *restart* positions override it,
all the same idea — a fresh statement begins: statement start, a derived-table
`FROM (`, the position after a set operation (`UNION [ALL] |`, `EXCEPT |`), and
after an `EXPLAIN [ANALYZE]` prefix.

**Role** is one uniform test (`item_complete`) on the token before the caret:
identifiers, literals, `)`, `END`, and the projection `*` end an item; everything else
starts one. Keyword tokens resolve through the same `is_name_like` predicate used by
every name position (sqlparser's reserved tables) *minus* the `OPERAND_EXPECTING`
connectives — so a column named `status` ends an item exactly like a plain identifier,
while `AND` / `DISTINCT` / `WHEN` never do. The FROM zone alternates on its own tokens
(targets after `FROM`/`JOIN`/a list comma); `DESCRIBE` expects one relation and then
nothing.

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
  (`ALL_KEYWORDS` minus `BLOCKED_KEYWORDS` — the managed-DDL policy), name-position
  reservedness (`lex::is_reserved_in_name_position` over the `RESERVED_FOR_*_ALIAS`
  tables — also the identifier-quoting rule).
- **Declared** (grammar/policy knowledge no parser table encodes, one definition each):
  - `LADDER` — the canonical clause order (`SELECT → FROM → WHERE → GROUP BY → HAVING →
    QUALIFY → ORDER BY → LIMIT → OFFSET`), plus `SET_OPS` appended to every tail.
  - `OPERAND_EXPECTING` / `LITERAL_WORDS` (context.rs) — connectives that start operands
    vs literal/direction words that end items; shared by the role test and the
    projection scraper.
  - `EXPR_OPS`, `JOIN_CONT`, `ORDER_CONT` — clause-internal continuations.
  - `STATEMENT_KEYWORDS` — the policy-shaped statement leads (SELECT/WITH/EXPLAIN/
    SHOW/DESCRIBE forms).
  - `MULTI_WORD` — presentation phrases (`GROUP BY`, `LEFT JOIN`, `IS NOT NULL`).
  - `JOIN_LEADINS` — join modifiers after which `JOIN` itself is next.

## 4. Pools and ranking

Per position (the `complete/` module: `mod.rs` = API + pools + insert shaping,
`vocabulary.rs` = the declared grammar tables, `ranking.rs` = tiers + forces +
the rank pipeline, `tests.rs` = the suite):

| Position | Pool (context tier order) |
|---|---|
| `Start` operand | `STATEMENT_KEYWORDS` (curated order) |
| `From`/`Describe` operand | relations only — CTEs, tables, views (projection-boosted, §5) |
| `Limit`/`Offset` operand | **nothing** (numbers) |
| any `Binding` position | **nothing** (a name is being invented) |
| any expression operand | in-scope columns (0) → select-aliases (1, **only** in GROUP BY/ORDER BY/HAVING/QUALIFY — where SQL allows them) → functions (2) → relations-as-qualifiers + core keywords (3) |
| any continuation | `continuation_keywords(clause)` in curated order (0): clause-internal ops + **the ladder strictly after the clause** — never backwards |
| `Dot(rel)` | that relation's columns only |

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
- **The all-columns fallback filters before allocating** (match first, materialize on
  hit) and caps at `FALLBACK_COLUMN_CAP` (2048) — the only pool that scales with total
  catalog size.
- Everything else is bounded by *scope*: the FROM'd tables' columns, ~400 functions,
  ~1200 keywords, relation names. Typical keystroke ≪ 1ms release; the 120Hz frame
  budget (8.3ms) holds with an order of magnitude to spare.

## 8. Editor integration (strata-code-editor)

The editor owns the **generic** machinery; `CompletionItem/Kind/Request` are its own
types (no strata-core dependency), the provider is a component prop
(`on_completions: Callback<CompletionRequest, Vec<CompletionItem>>`) wired at the mount
(tab.rs) — the same seam as `use_validation`.

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
the provider call onto the P2-18 validation pattern (spawn + cancel-and-rearm +
revision gate) behind the same `on_completions` seam. LSP (process boundary, JSON-RPC)
is categorically out — the provider lives in-process.

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
  and differs from the dialect's `is_word_char` (`$`/`@`/`#` are dialect word chars) —
  a `price$usd` column dismisses the popup at the `$`. A provider-supplied word
  predicate would close this; deferred until it bites.
- The three SELECT-list scrapers (`column_aliases`, `projection_columns`,
  `select_column_refs`) share the grammar tables and now agree on depth/literal
  policy, but remain three walks — a single parameterised scraper is a clean refactor
  awaiting P2-23's resolver work.
- The no-FROM diagnostic suppression in `validate.rs` is the third mid-edit stopgap of
  its class (now depth-0-scoped so CTE drafts keep the grace) — consciously superseded
  by **P2-23** (native multi-error name resolution in front of the planner).
- Type-aware argument narrowing (only numeric columns inside `sum(`) needs registry
  signature metadata — **P2-22**, together with the docs panel and signature help.
