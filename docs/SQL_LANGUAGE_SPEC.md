# SQL language service — combined design for S7 (autocomplete) + S25 (validator)

Autocomplete and validation are two capabilities of one language service over the SQL buffer. This spec designs the
**shared foundation** we build for both, then each feature as a thin consumer. Feasibility groundwork is in
`docs/EDITOR_LANG_SPIKE.md`; this supersedes the standalone framing of S7 and S25.

> **Prerequisite — DataFusion 43 → 54 upgrade (do first).** This design assumes the
> latest DataFusion (54.0.0). The upgrade is load-bearing here, not incidental:
> - Only 54's `FunctionRegistry` enumerates **all** function categories by name
>   (`udfs`/`udafs`/`udwfs`/`higher_order_function_names`); 43 exposes only scalar
>   names, so "discover every registered function" has no clean path on 43.
> - 54 bumps `sqlparser` 0.51 → 0.62 (the parser we reuse) and ships richer
>   planner **`Diagnostic`s with source spans**, which the semantic validator can
>   surface directly.
>
> It is also a sizeable, independent piece of work (arrow 53 → 58, object_store
> 0.11 → 0.13, sqlparser 0.51 → 0.62, plus DataFusion API churn across 11 majors —
> touching `engine.rs` register/query/explain paths, and it unblocks S21's
> object_store credential work too). Tracked as its own backlog task; **F0** in the
> build order below.

## 0. Principle

One analysis, two outputs. Tokenise the buffer, work out the clause/context at an offset, resolve identifiers against
the catalog, and:

- the **validator** turns unresolved identifiers + structural faults into
  `Diagnostic`s (already rendered by the Problems view; add inline squiggles);
- **autocomplete** turns the caret context + symbol set into ranked completions.

Building them apart means writing the lexer and the symbol resolver twice, and they would drift. So the foundation is
shared; S7 and S25 are consumers.

---

## 1. Foundation

Three pieces, all shared. None is a feature on its own; both features need all three.

### 1a. Vendored editor — `src/ui/code_editor/`

We **vendor** `dioxus-code-editor` 0.1.2 (decision from the spike; the crate exposes no caret/selection/decoration API
and the alternative — scraping its DOM — is version-fragile). It is small and uses only public APIs
(`dioxus_code::advanced`
`Buffer`/`HighlightedSource`/`TokenSpan`/`SourceEdit` + stock dioxus 0.7).

**Copy:** `lib.rs` (the component), `edit_capture.rs` (the `InputEditTracker` +
`use_input_edit_attributes` that wires `onmounted`/`oninput`), `edit_capture/
desktop.rs` (a trivial old-vs-new string diff → `SourceEdit`), and the CSS asset.

**Strip:** `edit_capture/web.rs` and its `#[cfg(target_arch = "wasm32")]` arm — that is the only `web_sys`/
`wasm-bindgen` user. We are desktop-only, so it goes, along with those deps.

**Add** (the reason we vendor):

1. **A caret/selection channel.** `use_input_edit_attributes` already attaches an
   `onmounted` to the `<textarea>` that today no-ops on desktop — that is our hook. We keep the `MountedData`, and
   expose the textarea's `selectionStart`/`selectionEnd`
   plus `scrollLeft/Top` to the parent via an `oncaret: EventHandler<CaretInfo>` prop. Reading selection is a ~5-line
   `document::eval` against the mounted element (dioxus 0.7 `MountedData` gives rects/scroll/focus but not selection —
   **verify** before writing, in case a newer point release adds it). Fire it on `keyup`/`click`/`select`.
2. **Key routing.** `onkeydown: EventHandler<KeyboardEvent>` on the textarea so a consumer can intercept ⌘Space,
   ↑/↓/Enter/Esc/Tab **while the completion popup is open** and `preventDefault` before the textarea acts. When the
   popup is closed the editor behaves exactly as today.
3. **A decorations layer.** A `decorations: Vec<Decoration>` prop rendered as an absolutely-positioned overlay inside
   `.dxc-editor-viewport`, aligned to the same monospace metrics as the text (char-width × col − scrollLeft,
   line-height × line − scrollTop; `wrap:"off"` makes this exact). Used for **squiggles** (underline a range) and
   optional gutter marks. This is what the crate cannot give us.

Props stay a superset of the upstream ones (`value`/`oninput`/…): existing call sites in `editor.rs` keep working; new
props are opt-in with defaults.

### 1b. SQL analysis layer — `crate::sql`

Pure Rust, no UI, no engine. Input: buffer text (+ a caret offset for completion) and a `Catalog` symbol snapshot.
Output: tokens, caret-context, symbol resolution, structural diagnostics, completion candidates. Error-tolerant
throughout — mid-edit SQL is usually invalid, and a full AST parser (sqlparser) bails on the first error, which is
useless for *both* features.

- **Tokeniser — reuse DataFusion's `sqlparser`, don't hand-roll.** DataFusion ships the `sqlparser` fork it actually
  parses with; use its `Tokenizer` for a token stream with byte locations. Depend on it **via DataFusion's re-export**
  (`datafusion::sql::sqlparser`) so the version + dialect can never skew from what the engine runs (0.62 on DF 54). The
  tokenizer is error-tolerant enough for mid-edit text and hands us keyword/ident/quoted-ident/string/number/punct kinds
  with spans — which feed squiggle ranges and token-under-caret. (Earlier drafts proposed a hand lexer; reusing the
  engine's own tokenizer is both less code and guaranteed-faithful to DataFusion's lexical rules.)
- **Full parse (validity + DataFusion-specific syntax).** For a syntactically complete statement, run
  `datafusion::sql::parser::DFParser::parse_sql` — **DataFusion's own**
  parser layer over sqlparser, which understands DataFusion extensions (`COPY … TO`,
  `CREATE EXTERNAL TABLE`, `EXPLAIN ANALYZE`, `SET`/`SHOW`, …) that a vanilla dialect would reject. This is the honest
  "would DataFusion parse this?" check; a `ParserError`
  becomes a structural diagnostic. (The tokenizer still drives completion mid-edit, where a full parse would fail.)
- **Statement split + clause-context.** Split on top-level `;`, then a small state machine walks the **tokens** of the
  statement containing the caret and classifies the caret position: `SelectList | AfterFrom | AfterJoin | AfterDot(alias) | WhereExpr |
  FunctionArgs | Unknown`. Completion keys off this; it does not need a full parse (and usually can't get one mid-edit).
- **Symbol model.** `Catalog { tables: Vec<TableSym>, views: Vec<TableSym> }` where
  `TableSym { name, columns: Vec<ColSym{name, dtype}> }`, built from `state.project`
  (see 1c). Within the current statement, parse the `FROM`/`JOIN` clause into
  `alias → table` bindings so `alias.` resolves to that table's columns and unknown aliases are flagged.
- **Facade.** `analyze(sql, &catalog) -> Analysis { tokens, statements,
  diagnostics }` for validation (no caret), and `complete(sql, caret, &catalog) ->
  Vec<Completion>` for autocomplete (reuses the same lex + symbols). An `Analysis` is cacheable per tab; completion
  computes caret-context on demand.

### 1c. Symbol source

**Tables + columns — already available.** `project::CatalogTable` and `CatalogView`
already carry `columns: Vec<ColumnInfo>` (`{name, dtype, kind, nullable, children}`), populated from engine register
events in `app.rs` and read today by the inspector/ sidebar. So the table/column half of `crate::sql::Catalog` is a
cheap projection of
`state.project.tables` + `.views` on the UI thread — no new push needed. Columns are
`#[serde(skip)]` (runtime, re-populated on registration), so the set is "eventually complete" as registration finishes;
a registration failure contributes the table *name* only.

**Functions — a real (small) engine push.** Completion must offer, and the validator must recognise, the **actual
registered functions**: DataFusion's built-ins *and* any UDFs — a hardcoded list would rot across versions and miss
registrations. On DF 54 the engine enumerates the whole registry in a few calls on `SessionState` (`impl
FunctionRegistry`):

- `udfs()` → scalar names, `udafs()` → aggregate names, `udwfs()` → window names,
  `higher_order_function_names()` → higher-order names. (Optionally each `ScalarUDF`/
  `AggregateUDF`/`WindowUDF` also exposes `.aliases()` and `.signature()` for richer completion detail + so an alias
  isn't mis-flagged.)

The engine sends this once on startup as a new `Event::Functions { scalar, aggregate,
window }` into a UI-side `FunctionCatalog` (per-window, like the schema). Re-emit only if we ever register/deregister a
UDF (we don't today). `crate::sql::Catalog` includes it. This is what makes completion offer real functions and lets the
validator flag an unknown *function-call* token with confidence.

**Keywords** for completion + the typo-lint dictionary come from `sqlparser::keywords`
(the same set DataFusion parses against), plus the handful of DataFusion-specific statement keywords DFParser adds.

---

## 2. Data-model changes

- **`Diagnostic` gains a span.** Today `diagnostics::Diagnostic.loc` is a display string (`"line L:C"`). Squiggles and
  jump-to-token need a real range. Add
  `span: Option<Range<usize>>` (byte offsets into the tab's SQL). `loc` stays for the human label; `span` drives the
  decoration + the S23-deferred token-select on click.
- **`Completion`** (new): `{ label, insert, kind: Table|Column|View|Keyword|Function,
  detail: Option<String>, replace: Range<usize> }`. `replace` is the token under the caret being completed (so accept
  replaces the partial word, not just inserts).
- **`Decoration`** (new, in the vendored editor): `{ range: Range<usize>, kind:
  Squiggle(Severity) | GutterMark(Severity) }`.
- **`CaretInfo`** (new): `{ offset: usize, selection: Range<usize>, scroll: (f64,f64) }`.
- **`FunctionCatalog`** (new, per-window): `{ scalar, aggregate, window: Vec<FnSym> }`
  where `FnSym { name, aliases: Vec<String>, detail: Option<String> }`, filled from the engine `Event::Functions` push.
  Folded into `crate::sql::Catalog` alongside tables.

`crate::diagnostics::validate(sql)` — the current stub — becomes a thin wrapper over
`crate::sql::analyze(sql, &catalog).diagnostics`.

---

## 3. S25 — validator (consumer 1)

Rules in tiers, cheapest first; each pass replaces the tab's validation slice (the authoritative model already in
`crate::diagnostics`).

- **Lexical / structural** (no catalog): unterminated string/quoted-ident, unbalanced parens, empty statement, stray
  trailing tokens, keyword-typo lint (`FORM`→`FROM`, `WEHRE`→`WHERE`, … edit-distance ≤1 against a SQL keyword set, only
  on identifier tokens in keyword position).
- **Semantic** — two options, and we should prefer the engine:
    - **(preferred) engine dry-plan.** Ask the engine to *plan but not execute* the SQL
      (`SessionState::create_logical_plan` / `ctx.sql(...)` stops at the logical plan; execution is only on `collect`).
      Any unknown table/column/function surfaces as a real `DataFusionError` — the same resolver the query uses, so zero
      drift, and on DF 54 the planner attaches a `Diagnostic` with a **source span** we map straight to a squiggle. Runs
      on the existing engine thread, debounced, on a background (non- blocking) task like queries already do;
      supersede-abort stale ones. Downside:
      async + one error at a time (the planner bails at the first).
    - **(fallback/instant) client-side** against `Catalog`: unknown table after
      `FROM`/`JOIN`, unknown `alias.` prefix, unknown column / function-call. Cheaper and synchronous but re-implements
      resolution, so keep it heuristic and conservative (only flag when confident) — mainly to light up squiggles
      instantly before the dry-plan returns.

  Net: instant lexical/structural + conservative client-side semantic for immediate feedback, reconciled by the
  authoritative engine dry-plan. (Completion still needs the client-side symbol lists from 1c — it can't round-trip per
  keystroke.)

Rendering: each `Diagnostic` with a `span` becomes a `Squiggle(severity)` decoration in the vendored editor; the
Problems view + rail badge already consume the diagnostics store, and clicking a Problems row (S23) can now use `span`
to select the token, not just switch tabs. Validation runs on the existing per-tab debounced `use_revalidate`
(no caret needed).

## 4. S7 — autocomplete (consumer 2)

> **Superseded by [`COMPLETION_SPEC.md`](COMPLETION_SPEC.md)** — the as-built P2-04
> design (clause×role position model, ladder-derived continuations, fuzzy tiers,
> synchronous pipeline, ⌃/⌘Space). This section is the pre-build plan, kept for
> history; where they disagree, `COMPLETION_SPEC.md` is the truth.

- **Trigger.** ⌘Space anywhere; auto after typing an identifier char or `.`
  (debounced ~120ms); never inside a string/comment token.
- **Candidates by context** (from clause-context + symbols): `AfterFrom/AfterJoin` → table + view names;
  `AfterDot(alias)` → that table's columns; `SelectList/WhereExpr`
  → columns of in-scope tables + functions + keywords; fallthrough → a pooled set (all table/column/keyword symbols).
  Rank: prefix match > substring; in-scope columns
  > out-of-scope; then alphabetical.
- **Popup UI.** A `Popup`-container dropdown (reuse `ui::components`) anchored at the caret via `CaretInfo` + the
  editor's client rect (flip-up when near the bottom). Rows: kind icon + label + dim `detail` (type / table). ↑/↓ move,
  Enter/Tab accept, Esc dismiss, keystrokes filter live. Accept replaces `Completion.replace` and moves the caret to the
  insert end.
- **State.** A per-tab-or-window `completion` overlay signal (open, items, selected, anchor). Local to the workbench,
  not `AppState` (matches the overlay conventions).

## 5. Editor integration details

- **Caret read path.** `oncaret` fires from the vendored editor on keyup/click/select; the workbench stores the latest
  `CaretInfo` for the active tab. Completion reads it synchronously; validation ignores it.
- **Key interception order.** The vendored `onkeydown` fires first; when the popup is open the consumer handles nav keys
  and `preventDefault`s so the textarea does not also move the caret / insert a tab. Closed popup → pass through
  unchanged.
- **Decoration positioning.** Overlay layer uses the same font metrics as the text layer; recompute on scroll + resize
  (a `scroll` listener on the viewport, cheap). Squiggles are CSS wavy underlines coloured by severity.
- **Desktop JS.** The only `eval` is the selection read (and possibly a scroll listener). Everything else is native
  dioxus rsx. Keep the JS snippet inside the vendored module, keyed to the mounted element.

---

## 6. Build order

Foundation first; both features then land in parallelisable slices.

0. **F0 — DataFusion 43 → 54 upgrade.** Its own task (see the prerequisite banner). Unblocks the tokenizer/DFParser
   reuse, the function-registry enumeration, and the planner-diagnostics semantic path — and S21.
1. **F1 — Vendor the editor.** Copy in, strip `web.rs`, confirm `editor.rs` still renders and compiles unchanged (props
   superset). No behaviour change yet.
2. **F2 — Caret + key surface.** Add `oncaret` / `onkeydown` (+ the selection `eval`); prove caret offset reaches the
   workbench. Verify the dioxus-0.7 selection API first.
3. **F3 — Analysis layer.** `crate::sql`: reuse the `sqlparser` tokenizer + `DFParser`
   (via DataFusion's re-export) → statement split → clause-context → symbol resolver +
   `analyze`/`complete`. Unit-tested, no UI. Add `Diagnostic.span`.
4. **F4 — Decorations.** `decorations` prop + overlay layer in the vendored editor.
5. **F5 — Engine function push.** Enumerate the registry on the engine thread (`udfs`/`udafs`/`udwfs`/
   `higher_order_function_names`) → `Event::Functions` →
   `FunctionCatalog`; fold into `Catalog`.
6. **S25a — Structural validator.** Wire `validate` → `sql::analyze` structural rules; render squiggles (F4); Problems
   already consumes them. Ship keyword-typo lints.
7. **S25b — Semantic validator.** Engine dry-plan path (real planner diagnostics + spans) with the conservative
   client-side fallback.
8. **S7a — Completion provider.** `complete()` + the per-tab completion overlay state.
9. **S7b — Completion UI.** Dropdown, caret anchor (F2), keyboard nav, accept/replace.
10. **Polish.** Ranking, flip-up, click-to-select from Problems via `span`, function signature detail/help.

Dependencies: **F0 → everything**; F1→F2→ (F4, S7b); F3→ (S25a, S7a); F4→S25a; F5→ (S7a, S25b); F2→S7b. Once F0–F5 land,
the S25 and S7 tracks are independent.

## 7. Open questions (resolve at build time)

- Does dioxus 0.7 `MountedData` expose textarea `selectionStart`/`selectionEnd`
  natively? If yes, F2 needs no `eval` at all.
- Confirm the DF-54 re-export paths (`datafusion::sql::sqlparser`,
  `datafusion::sql::parser::DFParser`) and that the `sql` feature stays enabled (it is in default features; `ctx.sql`
  already depends on it).
- Confirm DF-54 exposes planner `Diagnostic`s with spans on the returned error (the
  `datafusion_common::Diagnostic` surface) — decides how precise engine-driven squiggles are vs. falling back to the
  client-side span.
- Engine dry-plan cost/cadence: debounce interval + supersede-abort so a burst of keystrokes doesn't queue plans; skip
  while a real query is running on that tab.
- Multi-statement scope: resolve aliases only within the statement under the caret (simplest, correct for our use) —
  confirm no cross-statement CTE needs in v1.
- Reuse the existing `use_revalidate` debounce for validation, add a separate lighter debounce for completion (different
  cadence + it needs the caret).

## 8. Risks

- **Selection via eval** is async; the caret the popup uses could momentarily lag a very fast keystroke. Mitigate by
  also deriving caret from the `oninput` diff (`SourceEdit.new_end_byte` is the caret after an edit) so typing has a
  synchronous caret and `eval` only corrects on click/arrow navigation.
- **Decoration/caret metric drift** on font/zoom/theme change — recompute on resize; keep the overlay reading the live
  computed font.
- **Vendored-editor maintenance** — we own it now, but it is ~350 lines over a public, semver `dioxus_code` surface; pin
  and only chase upstream deliberately.

---

## Appendix A — DataFusion 54 SQL surface (grounds F3 / F5)

Distilled from the DataFusion 54 SQL reference. Function **names** are *not* listed here — they come from the engine
registry at runtime (F5). This captures **statement shapes, clause keywords, operators, and DataFusion-specific
extensions** the analysis layer must recognise for parsing/clause-context and keyword completion. (The `sqlparser`
tokenizer + `DFParser` already implement the grammar; this is the surface completion + the typo dictionary target.)

**A.1 Statements** (leading keyword → statement-start completion; DFParser handles the DF-specific ones):

- *Queries:* `SELECT`, `WITH … SELECT`, `VALUES`, set-ops `UNION [ALL|DISTINCT]` /
  `INTERSECT` / `EXCEPT`.
- *DDL:* `CREATE [EXTERNAL] TABLE`, `CREATE TABLE AS`, `CREATE VIEW`, `CREATE SCHEMA`,
  `CREATE DATABASE`, `DROP TABLE`, `DROP VIEW`, `DESCRIBE`.
- *DML:* `INSERT INTO …`, `COPY … TO '<path>' [STORED AS fmt] [OPTIONS (…)]` (our export path; format options in the
  Format-Options page).
- *Session / introspection:* `SET <cfg> = …`, `RESET`, `SHOW TABLES|COLUMNS|ALL|
  FUNCTIONS`, `information_schema.*` (S17 already intercepts a lone `SET`/`RESET`/`SHOW`).
- `EXPLAIN [ANALYZE] [VERBOSE] <query>` (S12/S20); `PREPARE`/`EXECUTE` with `$1` params.

**A.2 SELECT grammar** (drives the clause-context state machine):
`WITH` → `SELECT [ALL|DISTINCT]` → `FROM` → `JOIN`s → `WHERE` → `GROUP BY`
[`ROLLUP`/`CUBE`/`GROUPING SETS`] → `HAVING` → `QUALIFY` → set-ops → `ORDER BY`
[`ASC|DESC`] [`NULLS FIRST|LAST`] → `LIMIT [OFFSET]`.

- *DF-specific projection:* `SELECT * EXCLUDE(cols)` / `* EXCEPT(cols)`.
- *JOIN keyword set* (completion after FROM/join): `INNER`, `LEFT|RIGHT|FULL [OUTER]`,
  `NATURAL`, `CROSS`, `LEFT|RIGHT SEMI`, `LEFT|RIGHT ANTI`, `LATERAL`,
  `[LEFT] JOIN LATERAL`, `ON`, `USING`.
- *Pipe operators* `|>` (BigQuery dialect, **off by default**) — out of scope v1.

**A.3 Operators & literals** (tokenizer set + validation):

- Arithmetic `+ - * / %`; bitwise `& | # ^ << >>`; concat `||`; array `@> <@`.
- Comparison `= != <> < <= > >= <=>`; `IS [NOT] DISTINCT FROM`; `IS [NOT] NULL`;
  `IS [NOT] TRUE|FALSE`.
- Pattern: `LIKE`/`ILIKE`/`SIMILAR TO` + operator forms `~ ~* !~ !~*` (regex) and
  `~~ ~~* !~~ !~~*` (like).
- Logical `AND OR NOT`; `BETWEEN`, `IN (…)`, `EXISTS`.
- Literals: `'…'` (escape `''`, **not** C-escapes), `E'…\n…'` escape strings, numbers,
  `TRUE/FALSE/NULL`, typed `DATE '…'`/`TIMESTAMP '…'`/`INTERVAL '…'`, `"quoted ident"`.
- **Identifiers lowercase unless double-quoted** → resolve case-insensitively unless the token is quoted (matters for
  the validator's column/table matching).

**A.4 Expression special forms** (parsing + completion contexts):

- `CASE WHEN … THEN … ELSE … END`; `CAST(x AS <type>)` / `x::<type>`;
  `EXTRACT(field FROM x)`; `SUBSTRING(x FROM a FOR b)`.
- Aggregates: `fn(DISTINCT x)`, `fn(x ORDER BY y)` (e.g. `array_agg`),
  `fn(…) FILTER (WHERE …)`, `WITHIN GROUP (ORDER BY …)` (ordered-set).
- Windows: `fn(…) OVER (PARTITION BY … ORDER BY … [ROWS|RANGE|GROUPS frame])` or
  `OVER named_window`; `WINDOW w AS (…)`.
- `unnest(array)` / `unnest(struct)` expansion; subqueries scalar `(…)`, `IN (…)`,
  `EXISTS (…)`, `ANY`/`ALL`, correlated.

**A.5 Type keywords** (CAST targets + typo dict): `INT/INTEGER`, `BIGINT`, `SMALLINT`,
`TINYINT`, `FLOAT/REAL`, `DOUBLE`, `DECIMAL/NUMERIC`, `VARCHAR/CHAR/TEXT/STRING`,
`BOOLEAN`, `DATE`, `TIME`, `TIMESTAMP [WITH TIME ZONE]`, `INTERVAL`, `BINARY/BYTEA`.

**A.6 Implications for the language service:**

- *Completion contexts:* statement-start keywords · after `FROM`/`JOIN` → tables + views (+ subquery) · after `alias.` →
  that table's columns · `SELECT`/`WHERE`/`HAVING`/
  `QUALIFY`/`ORDER BY` → columns + functions + keywords · inside `fn(` → args (+
  `DISTINCT`/`ORDER BY`) · after `CAST(x AS` → type keywords · after `OVER` → windows.
- *Tokenizer* must handle multi-char operators (`<=>`, `!~*`, `~~*`, `::`, `@>`, `<@`,
  `>>`, `<<`) and `E''` strings — a reason to reuse sqlparser's tokenizer, not re-derive.
- *Validator conservatism:* the keyword-typo dict = SQL keywords ∪ type keywords ∪ join keywords. Many bare identifiers
  are legitimately quoted/aliased/CTE/`information_schema`
  names, so only flag with **high confidence** and let the engine dry-plan (§3) be the authority for unknown
  table/column/function.
- *Out of scope v1:* pipe `|>` operators (dialect-gated off), Spark-compat functions, prepared-statement placeholders.
