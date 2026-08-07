# Chart 07 · Tier A templates — palette commands that write the SQL

**Workstream:** Chart (Rz2) · **Status:** ⬜ · **Depends on:** 01–04 · Independent of 06, 08–11.

## Goal
The constructive answer to "aggregate it in SQL": three command-palette entries that compose
a query from the current tab's run + resolved chart encoding and open it **unrun** in a new
tab the user owns. Settled in planning (2026-08-07, Alex): Tier A templates land in the
**command palette** — not the strip (the cut press's mistake, 04), not the Shape panel (09,
the interactive composer; these are the fixed templates).

## Current state
- The palette is a registry of offers: `apps/project/commands.rs`, `#[command_router]` on
  `PaletteCommands` — one method + doc comment + `#[command(...)]` = one row; generated
  tests already assert id/label uniqueness and subtext presence. `PaletteCtx` carries
  `session`, `engine`, catalog handles; `active_tab()` peeks.
- The open-a-tab-with-SQL funnel exists: `actions::open_sql(session, &sql) -> TabId`
  (`views/workbench/editor/actions.rs` — built for the Agents pane promotion; new focused
  scratch tab, undoable, unrun).
- The settled result's `Vec<ColumnInfo>` lives only in the freya-query cache (`RunQuery`
  keyed by `QuerySpec`, held by the tab's keeper); the press-time SQL is `QuerySpec::sql`
  via `session.request(tab)`.
- A `WITH … SELECT` passes the editor policy (`sql/validate.rs` blocks by statement kind).

## Build
1. **`chart/templates.rs`** (beside `config.rs`; pure functions over strings + `Encoding`,
   no UI types):
   - `top_n_other(sql, x, y)` — rank + CASE fold: `row_number() OVER (ORDER BY <y> DESC)`,
     `CASE WHEN _rank <= 10 THEN CAST(<x> AS VARCHAR) ELSE 'Other' END` (the CAST is
     load-bearing — the CASE arms must agree in type), `GROUP BY 1`,
     `ORDER BY sum(<y>) DESC`.
   - `share_of_total(sql, x, y, series)` — `y * 100.0 / sum(y) OVER (PARTITION BY x)` when
     a series splits (100%-stacked), `OVER ()` otherwise (honest pie percentages).
   - `filter_split(sql, x, y)` — `sum(<y>) FILTER (WHERE <predicate>)` /
     `FILTER (WHERE NOT (<predicate>))` over `GROUP BY <x>`; `<predicate>` is always a
     placeholder — only the user knows the split.
   - Every template emits its own `ORDER BY` (a `GROUP BY` has no output order — the
     workstream's standing lesson). Role slots come from the resolved encoding; where the
     encoding can't answer, emit angle-bracket placeholders (`<measure>`, `<category>`) —
     an honest "you choose", never a silent no-op.
   - **Check first:** if a press can carry multiple statements, wrapping the whole text is
     wrong — reuse the Run press's statement extraction (`docs/STATEMENTS_SPEC.md`,
     `engine/sql/lex.rs`), never a hand-rolled semicolon strip.
2. **Tab context**: `PaletteCtx` gains `chart: Option<ChartContext { sql, encoding }>`,
   built in `use_palette_ctx` by subscribing the active tab's run. `QuerySpec::query`
   (`query/run_query.rs`) grows an `enabled: bool` — the `enable(false)` placeholder
   pattern `ChartSpec` already uses — so the palette can watch without triggering; it stays
   the **single** `Query` construction site, and both existing call sites pass `true`
   (audit them). On `Settled(Ok(Rows))`: `Roles::of(&columns)` +
   `resolve(&session.chart(tab), &roles)`; anything else → `None` (the command still
   offers, with the current editor text as the source when non-blank and placeholders for
   the roles).
3. **Three `#[command]` methods** — `chart_top_n` ("Chart: Top-N with Other"),
   `chart_share_of_total` ("Chart: Share of total"), `chart_filter_split` ("Chart: Split
   series by filter") — each body is one call into `actions::open_sql`. Subtexts from doc
   comments; keywords cover rank/percent/stacked/filter/template.
4. **Tests**: golden-SQL unit tests in `templates.rs`; an integration test running each
   composed template through `EngineCtx::default()` over a `VALUES` fixture (dates, NULL
   groups, reserved-word and uppercase column names) proving it parses, runs, and yields
   the role columns; a `commands.rs` test that with a `ChartContext` the opened tab's text
   contains the encoded column names, and without one the placeholders.

## Acceptance
- [ ] Each command opens an editable, unrun tab whose SQL runs against the fixture and
      charts cleanly (Top-N as bar, share-of-total as 100%-stacked/pie, filter-split as two
      series).
- [ ] No template ever runs automatically; nothing lands in the strip; the palette rows
      pass the registry's generated tests.

## References
`docs/CHART_FUNCTIONS.md` §3 Tier A. `docs/CHART_SPEC.md` §8 (why the strip is ruled out).
`05-analytical-presets.md` (the remaining tiers). AGENTS.md §2 (the palette is a registry of
offers; bodies call existing funnels).
