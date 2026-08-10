# ED-11 · Completion for the statements the editor now runs

**Workstream:** Editor statements · **Status:** ⬜ · **DEV_TASKS:** — · **Depends on:** ED-08,
ED-09, ED-10 (all ✅ — the lead list and the position model land once, over the finished surface)

## Goal

The editor has spent this workstream learning to **run** statements. Completion has not moved with
it: the offer is still the query/inspection vocabulary S7 shipped, so a user who types `SET ` or
`CREATE TABLE ` gets column names and clause keywords for a statement that has neither. Close that
gap in one pass, across every statement the router intercepts — rather than a table entry per ED
task, which is how the two encodings would drift.

This file is the settled contract: the design decisions below were made against the as-built
code with ED-09 and ED-10 landed, and supersede the earlier draft of this task where they differ
(each supersession is called out where it applies).

## Current state

`engine/sql/complete/` (+ `context.rs`, `vocabulary.rs`) resolves against a `Catalog` snapshot the
editor tab rebuilds on catalog change (`tab.rs:124-150`). The model is **clause × role**:
`context::analyze_caret` names the governing `Clause` and whether the caret wants an `Operand`, a
`Continuation` or a `Binding`, and `complete` offers per that pair. `vocabulary.rs` holds the
declared tables — `STATEMENT_KEYWORDS` (the leads offered first at a blank statement), `LADDER`,
`CORE_KEYWORDS`, `BLOCKED_KEYWORDS` — each a named policy.

What has landed of statement completion:

- `Clause::Execute` (`EXECUTE` / `DEALLOCATE`) offers the session's prepared statements from
  `Catalog::prepared`, off `Engine::prepared` (ED-08), guarded by `context::leads_statement_only`
  so a column named `execute` cannot govern a SELECT list. Nothing else.
- `BLOCKED_KEYWORDS` is already honest against the router — `policy_and_completion_agree_on_statement_leads`
  (`complete/tests.rs:729`) asserts that a word leading something the editor runs is never
  filtered out.

What has **not**:

- `STATEMENT_KEYWORDS` is still `SELECT · WITH · EXPLAIN · EXPLAIN ANALYZE · SHOW · SHOW TABLES ·
  DESCRIBE`. ED-04 through ED-10 each shipped a statement without adding its lead.
- No statement operand position is modelled beyond `Execute`. `SET |`, `CREATE TABLE |`,
  `DROP TABLE |`, `COPY |`, `INSERT INTO |`, `STORED AS |`, `DROP FUNCTION |` all fall through to
  `Clause::Unknown` or `Start` and offer expression vocabulary.

## What to build

### 1. The caret model — `engine/sql/context.rs`

**New `Clause` variants**, flat (each is one row of the pool table in §4): `Create` (object word
not yet written), `CreateTable`, `CreateView`, `CreateExternal`, `CreateFunction`, `Drop`,
`DropTable`, `DropView`, `DropFunction`, `Insert`, `Copy`, `SetOption` (`SET` and `RESET` share
it), `Prepare`, `Restart`.

- `clause_of` (:179) gains `CREATE`, `DROP`, `INSERT`, `COPY`, `SET` | `RESET`, `PREPARE` — and
  every one of them joins `leads_statement_only` (:209), so a column named `set`, `copy`, `drop`,
  `insert`, `create` or `prepare` never governs a SELECT clause; position 0 is the whole test,
  exactly as ED-08 built it for `EXECUTE`. **Supersedes** the "`PREPARE` is deliberately absent"
  comment at :195: with the position-0 guard, `DEALLOCATE PREPARE |` still resolves to
  `Clause::Execute` (the scan lands on `DEALLOCATE` at index 0), so `PREPARE` can join safely.
  Rewrite the comment; do not preserve it.
- **Head refinement.** A new `refine_statement_clause(stmt, clause)` reads the statement's first
  few keyword tokens — `CREATE [OR REPLACE] TABLE|VIEW|FUNCTION`, `CREATE EXTERNAL TABLE`,
  `DROP TABLE|VIEW|FUNCTION` — and refines `Create`/`Drop` into the specific variant. Unrefined
  (`CREATE |`, `DROP |`) stays `Create`/`Drop`, whose role is always `Continuation` (the object
  word comes next). The refined value replaces both the context's clause and `ca.governing`.
- **`Start` vs `Restart`.** A true blank statement (`prev.is_none()`, :830) keeps `Clause::Start`
  and offers query leads **and** statement leads. The restart branches — `EXPLAIN [ANALYZE] |`,
  the set-ops, `FROM (|` (:845-862) — switch to `Clause::Restart`, which offers query leads only:
  offering `DROP TABLE` after `EXPLAIN` promises something Run refuses. Two new restarts join
  them: `COPY (|` (add `Clause::Copy` beside `Clause::From` in the derived-paren branch) and the
  `AS |` of CTAS / `CREATE VIEW` / `PREPARE` (next bullet). `role_at` and `continuation_keywords`
  treat `Restart` exactly as `Start`; only the pool differs.
- **The `AS` rule becomes governing-aware** (:863). Today every `… AS |` is `Binding`. Carve out:
  `governing ∈ {CreateTable, CreateView, Prepare}` with prev `AS` → `At(Restart, Operand)` — the
  query ladder restarts (`CREATE TABLE t AS |`, `CREATE OR REPLACE VIEW v AS |`, `PREPARE p AS |`).
  `governing == CreateExternal` with prev `AS`, prev2 `STORED` → `At(CreateExternal, Operand)` —
  the format-word position. Everything else keeps `Binding` (`expr AS |`, `FROM t AS |`, `SHOW`).
  Deeper positions inside a query tail (`INSERT INTO t SELECT … FROM |`,
  `CREATE TABLE t AS … WHERE |`, `COPY (SELECT … WHERE |`) already resolve to their own clauses
  via the nearest-clause scan — pin with tests, no code.
- **The `SET` dotted-key rule**, read **before** the `.`-prev `Dot` branch (:832) whenever
  `governing == SetOption`. A config key is one dotted name, and the `Dot` rule would read
  `SET datafusion.|` as the columns of relation `datafusion`. Key vs value is decided by scanning
  for an `=` between the lead keyword and the caret:
  - **Key position** (no `=`): absorb the dotted chain backwards — while the token before the
    partial is `.` and the one before that is name-like, extend — into **one `partial` with one
    `replace` span**, so accepting `datafusion.execution.batch_size` at `SET datafusion.|`
    replaces the whole chain, never appends a second namespace. Its own tests over `SET |`,
    `SET dat|`, `SET datafusion.|`, `SET datafusion.execution.b|`, with exact byte spans.
  - **Value position** (`=` found): a new `CaretAnalysis` field, in the shape `comparand` already
    has — `set_key: Option<String>`, the dotted key text left of the `=`. `None` everywhere else.
  - After a complete value (`SET k = v |`) the ordinary `item_complete` test yields
    `Continuation`, whose arm offers nothing (§3).
- **`role_at` arms** for the new clauses:

  | Clause | Operand when prev is | Binding when prev is | Otherwise |
  |---|---|---|---|
  | `DropTable` / `DropView` / `DropFunction` | the object keyword, `EXISTS`, or `,` | — | Continuation |
  | `Insert` | `INTO` | `(` or `,` (column list / VALUES tuple) | Continuation |
  | `Copy` | `COPY` | — | Continuation |
  | `CreateTable` / `CreateView` | — | the object keyword (a name being invented), `(` or `,` | Continuation |
  | `CreateFunction` | once a `RETURN` token lies between the statement head and the caret, the body is an expression: alternate on `item_complete` like the default arm (`RETURN |` and `RETURN price * |` are Operand, `RETURN price |` is Continuation) — this is what reaches §4's `At(CreateFunction, Operand)` pool | before `RETURN`: `FUNCTION` (a name being invented), `(` or `,` | Continuation |
  | `CreateExternal` | — (the `STORED AS |` operand comes from the AS carve-out) | `TABLE`, `(` or `,` | Continuation |
  | `Prepare` | — | `PREPARE` | Continuation |
  | `Create` / `Drop` (unrefined) | — | — | Continuation always |
  | `SetOption` | handled in `analyze_caret` (above) | — | Continuation |
  | `Restart` | always Operand, like `Start` | — | — |

### 2. The `Catalog` snapshot and the policy seams

- **`TableSym` gains `internal: bool`** (`engine/sql/symbols.rs:17`); `Catalog::build` takes it
  per table (views stay pairs — a view is never internal). `tab.rs` passes
  `t.def.origin.is_internal()`. State in the `symbols.rs` module doc: **the store's `TableOrigin`
  is the internal-set authority for the offer; `Engine::is_internal` stays the dispatch gate** —
  same fact, read from the store because the snapshot is store-built (the store *is* the catalog).
  This is the "decide once and say which" the earlier draft asked for: the store, not a second
  engine enumeration.
- **`FunctionSym` gains `created: bool`** (`engine/sql/mod.rs`). `functions::snapshot` takes the
  created-name set — `snapshot(ctx, created: &BTreeSet<String>)` — and marks each sym;
  `Functions::new` passes the empty set, `Functions::settle` computes the post-change set before
  the walk, then swaps under the lock. One authority (the `Functions` registry), zero new tab.rs
  plumbing, and `DROP FUNCTION |` filters on the flag.
- **`ddl::session::refuse_reserved_key` (session.rs:221) becomes `pub(crate)`.** The `SET` key
  pool **calls it** to filter `config::ENGINE_KEYS` — zero drift by construction — and the
  agreement test exercises the function itself, never a copy of its list. The fourth class
  (`DIALECT_KEY`) is the reason: it is a plain `datafusion.sql_parser.*` key with no predicate of
  its own, so a filter written from the three predicates alone would offer it — and a session
  dialect is WJ-04 exactly (the language service carries the dialect on the `Catalog` snapshot
  while the planner reads it live).

### 3. The vocabularies — `complete/vocabulary.rs` + `ddl/external.rs`

- **Split the lead table.** `STATEMENT_KEYWORDS` becomes two tables: `QUERY_LEADS` (the unchanged
  seven, offered at `Start` **and** `Restart`) and `STATEMENT_LEADS` (offered at `Start` only,
  after the query leads — a blank tab is usually a query):

  `SET · CREATE TABLE · CREATE VIEW · CREATE EXTERNAL TABLE · CREATE FUNCTION ·
  CREATE OR REPLACE VIEW · CREATE OR REPLACE FUNCTION · INSERT INTO · COPY · DROP TABLE ·
  DROP VIEW · DROP FUNCTION · PREPARE · EXECUTE · DEALLOCATE · RESET`

  **Supersedes** the earlier draft's `CREATE TABLE AS`: the name sits between `TABLE` and `AS`
  (`CREATE TABLE t AS …`), so the phrase is unusable as a lead — CTAS is reached via
  `CREATE TABLE` → name → `AS` continuation → `Restart`. `EXECUTE` and `DEALLOCATE` join now;
  ED-08 never added them. `MULTI_WORD` is **untouched** — it rides ungated at every expression
  operand position, so statement phrases must not enter it. `BLOCKED_KEYWORDS` is untouched.
- **`continuation_keywords` arms** (curated order; the generic gated keyword tail still appends):

  | Clause | List |
  |---|---|
  | `Create` | `TABLE`, `EXTERNAL TABLE`, `VIEW`, `FUNCTION`, `OR REPLACE` |
  | `Drop` | `TABLE`, `VIEW`, `FUNCTION` |
  | `CreateTable` / `CreateView` / `Prepare` | `AS` |
  | `CreateExternal` | `STORED AS`, `LOCATION`, `PARTITIONED BY`, `OPTIONS` — one list serves both after-name and after-format; a re-offered `STORED AS` is the same accepted noise as `LEFT |` (COMPLETION_SPEC §10) |
  | `CreateFunction` | `RETURNS`, `RETURN` — no `LANGUAGE`: SQL is the only accepted value and the default |
  | `DropTable` / `DropView` / `DropFunction` | nothing — `DROP TABLE t` is complete |
  | `Insert` | `SELECT`, `VALUES` (after the target; `INSERT |` wanting `INTO` is served by the `INSERT INTO` lead phrase — documented trade-off) |
  | `Copy` | `TO`, `STORED AS`, `PARTITIONED BY`, `OPTIONS` — `TO` first |
  | `SetOption` | nothing — `=` is punctuation, `SET k = v` is complete |
  | `Restart` | same as `Start` |

- **`STORED_AS_FORMATS`** — a `pub(crate)` const in `ddl/external.rs` next to `read_format`
  (:210): `["PARQUET", "CSV", "JSON", "NDJSON", "ARROW"]`, kept honest by a test in external.rs
  that every entry parses through `read_format` and a non-member (`AVRO`) does not. One table,
  owned by the module whose match arms it mirrors.
- **The `OPTIONS` key tables.** Lift the `format.` key sets out of `external::apply`'s match arms
  (:276) into per-format data — `CSV_OPTION_KEYS` and `JSON_OPTION_KEYS`, entries
  `{ key, kind, what }`:
  - `kind` mirrors the `SET` value design: `Bool` (offers `true` / `false`), `Enum` for
    `format.compression` (`uncompressed · gzip · bzip2 · xz · zstd` — the words `compression()`
    already parses), `Char` / `Int` for the rest (no value offer). `what` is the short detail
    column ("delimiter character", "header row", …).
  - **Rewrite `apply` to consume the tables** so the table *is* the arm set — one vocabulary, not
    a copy kept honest by test. The per-key value coercions (`boolean`, `character`, `count`,
    `compression`) stay; the table names which coercion and which def field each key lands on.
  - CSV: `format.has_header`, `format.delimiter`, `format.quote`, `format.escape`,
    `format.comment`, `format.newlines_in_values`, `format.truncated_rows`,
    `format.schema_infer_max_rec`, `format.compression`. JSON: `format.newline_delimited`,
    `format.schema_infer_max_rec`, `format.compression`. Parquet/Arrow have no options — the
    offer there is empty, matching the arm's refusal by name.

### 4. The pool arms — `complete/mod.rs`

Closed pools (no `push_keywords`), following the `Execute` arm's pattern:

| Position | Pool |
|---|---|
| `At(Start, Operand)` | `QUERY_LEADS` then `STATEMENT_LEADS` (curated ord continues across the two), then gated keywords as today |
| `At(Restart, Operand)` | `QUERY_LEADS` only, then gated keywords |
| `At(SetOption, Operand)`, key | `ENGINE_KEYS` filtered by `refuse_reserved_key(k.key).is_ok()`; label/insert the key verbatim (never quoted, never uppercased); **detail = the key's `default`** (short and non-empty for every offerable key — the empty-default keys are all `runtime.*`, which the filter removes; `desc` is a sentence and too long); ordered by `ENGINE_KEYS` index; kind `Column` (the kind is a glyph, not a taxonomy — precedent `prepared_item`) |
| `At(SetOption, Operand)`, value (`set_key`) | `key_def(key)` → `Kind::Bool` ⇒ `true` / `false`; `Kind::Enum` ⇒ the options; every other kind ⇒ the correct empty offer. Inserted verbatim lowercase, no trailing space |
| `At(DropTable, Operand)` | tables and **not** views (`DROP VIEW` is the other statement, and `ddl::tables` says so by name) |
| `At(DropView, Operand)` | views only, for the mirror reason |
| `At(Insert, Operand)` | `internal && !is_view` only — the same answer `Engine::is_internal` gives dispatch, read from the store |
| `At(Copy, Operand)` | fold into the existing `From \| Describe` relation arm — relations, like a FROM target (CTEs vacuously absent, the projection boost a no-op) |
| `At(CreateExternal, Operand)` | `STORED_AS_FORMATS` as keyword items (uppercase + trailing space is right here) |
| `At(DropFunction, Operand)` | `catalog.functions.all().filter(created)` — bare-name insert (**no** trailing `(`; a DROP takes the name), detail `session function` |
| `At(CreateFunction, Operand)` (the body, after `RETURN`) | **the declared argument names**, scraped from the token stream (the identifiers of the first paren group after the function name), at the primary tier with detail `argument` — plus functions behind them. **Never catalog columns or relations**: the body may reference only its arguments (`ddl/functions.rs`), and offering scope columns would offer exactly what `Definition::check` refuses |
| `At(Execute, Operand)` | unchanged — prepared names |
| `At(_, Binding)` | unchanged — the empty offer (`CREATE TABLE |` name, `PREPARE |` name, column-def lists, VALUES tuples) |

**The `OPTIONS`-key carve-out** — the one exception to the string guard, scoped to exactly one
position: the caret inside a single-quoted literal in **key position** inside the `OPTIONS (…)`
group of a statement whose head refines to `CreateExternal`. Key vs value inside the group: a
literal whose predecessor (within the group) is `(` or `,` is a key; one whose predecessor is
another string is a value (DataFusion's `'key' 'value'` pairs, comma between pairs). Value offers
ride the same carve-out, with the preceding key looked up in the option table (`Bool` / `Enum` as
in §3; everything else silent — the values are user data).

- **Two lexing cases, both required.** (a) Terminated literal — caret between quotes, ordinary
  token stream: partial = content up to caret, replace = the content span between the quotes,
  insert = the bare key (the quotes are already there). (b) **Unterminated literal** — typing
  `OPTIONS ('format.h|` errors the tokenizer, and today the `lex_err` guard silences completion.
  Recovery, bounded to this position only: when the lex error is an unterminated string **and**
  the prefix before the opening quote lexes clean and classifies as the CET-OPTIONS-key position,
  treat the text after the quote as the partial. Any other lex error stays a guard.
- **Format-aware offer.** Scan the statement's tokens for `STORED AS <word>`: CSV →
  `CSV_OPTION_KEYS`; JSON → `JSON_OPTION_KEYS`; NDJSON → `JSON_OPTION_KEYS` **minus**
  `format.newline_delimited` (which `read_format` refuses toward `STORED AS JSON`);
  Parquet / Arrow / format unwritten → empty. Offering a key the arm refuses by name breaks the
  honesty rule.
- **Store-namespace keys (`aws.` …) and `CLIENT_KEYS` are never offered** — the arm refuses them
  toward Connections; absence from the offer is the same policy, stated once.

**Deliberately silent**, recorded as COMPLETION_SPEC §10 trade-offs when the docs land:
`LOCATION '|'` and `COPY … TO '|'` (paths — the right answer is the user's filesystem); `COPY`'s
own `OPTIONS` (DataFusion's open key namespace, not ours); `RESET` sharing `SET`'s key pool (the
session overlay is not on the snapshot; the settable superset is the honest offer); `INSERT |`
relying on the `INSERT INTO` lead phrase; INSERT column lists and VALUES tuples as `Binding`.

## Acceptance

- Every statement the editor runs is offered as a lead at a blank statement; `Restart` positions
  (`EXPLAIN |`, after a set-op, `FROM (|`, `COPY (|`, `CREATE TABLE t AS |`) offer the query leads
  and **no** statement leads. The lead list is kept honest by extending
  `policy_and_completion_agree_on_statement_leads`: a `lead → canonical tail` table (e.g.
  `COPY` → `COPY t TO 'x.parquet'`), every entry parsed and asserted
  `classify(_, Capability::Editor) ∈ {Intercept, Query}` — a lead with no tail entry panics, so
  adding a lead without extending the test fails the suite.
- `SET datafusion.exec|` completes to the full key and **replaces the whole dotted chain** (assert
  the winning item's `replace` span); the key pool agrees **bidirectionally with
  `refuse_reserved_key` itself** — for every `k` in `ENGINE_KEYS`, offered ⇔
  `refuse_reserved_key(k.key).is_ok()` — plus the named absences, `DIALECT_KEY` explicitly;
  `SET <bool key> = |` offers `true` / `false`, `SET <enum key> = |` that key's options, an `Int`
  key nothing; key detail equals the key's `default`.
- `DROP TABLE |` offers tables and no views; `DROP VIEW |` the reverse; `INSERT INTO |` offers
  only tables built with `internal: true`; `DROP FUNCTION |` only syms with `created: true`;
  `CREATE TABLE |` and `PREPARE |` offer nothing; `DEALLOCATE PREPARE |` still offers prepared
  names (the `clause_of` regression test).
- `CREATE EXTERNAL TABLE t STORED AS |` offers exactly the five formats, and external.rs's own
  test holds `STORED_AS_FORMATS` against `read_format`. `OPTIONS ('|` and `OPTIONS ('format.h|`
  (unterminated) both offer the format's keys; JSON vs CSV keys switch on the written format;
  NDJSON drops `format.newline_delimited`; no offer when the format is Parquet/Arrow/unwritten; a
  `Bool` option's value position offers `true` / `false` and `format.compression`'s offers the
  five compression words.
- `CREATE FUNCTION f(price DOUBLE, qty BIGINT) RETURNS DOUBLE RETURN |` offers `price`, `qty`
  (detail `argument`) and functions — and no catalog columns or relations.
- Lead-named columns never govern: `SELECT set, |`, `SELECT copy, |`, `SELECT drop, |`,
  `SELECT insert, |` mirror `a_column_named_execute_does_not_govern_its_clause`.
- Query tails inside statements keep full query completion, pinned by test:
  `INSERT INTO t SELECT a FROM |` is a FROM operand, `CREATE TABLE t AS … WHERE |` a WHERE
  operand, `COPY (SELECT … |` the query's own clauses.
- The existing completion suite is unchanged — every ranking scalpel and the torture sweep still
  pass (query leads stay first at a blank statement). A statement clause must not leak vocabulary
  into a query position: `SELECT * FROM events WHERE |` never offers `datafusion.*` keys,
  `STORED AS`, or `LOCATION`.
- Torture corpus additions swept by `torture_sweep_every_caret_position`: a full
  `CREATE EXTERNAL TABLE … STORED AS CSV LOCATION '…' OPTIONS ('format.has_header' 'true')`,
  `INSERT INTO … SELECT`, `COPY (SELECT …) TO '…' STORED AS CSV PARTITIONED BY (a)`,
  `CREATE FUNCTION f(x BIGINT) RETURNS BIGINT RETURN x + 1`, a `SET k = v` beside a query, and a
  dangling `SET datafusion.exec` mid-edit tail.

## Verification

`cargo test -p strata-core`; run the app and type each statement lead through to its operand —
including both OPTIONS lexing cases. Update `docs/COMPLETION_SPEC.md` (§2 Clause list + the AS
carve-outs, §3 the new tables, §4 the position rows, §10 the trade-offs above) and
`docs/STATEMENTS_SPEC.md` (a completion note per §6.x) **in the same change** — and this file's
status when it lands.

## Why this is one task and not five

Split per statement, the lead table and the "offer what Run accepts" agreement would be edited
five times, and the caret-model change `SET` needs would land under whichever task happened to go
first. One pass, one table, one test that the offer and the router agree — which is the shape
`BLOCKED_KEYWORDS` already has and the reason it has not drifted.
