# P3-08 · Column inspector (facts box)

**Phase:** 3 · **Status:** ✅ `[core ✓]` · **DEV_TASKS:** U9 · **Depends on:** P3-01

## Goal
A right-panel inspector showing a selected column's real, footer-derived metadata.

## As built

`views/inspector/` — `mod.rs` (frame + theme), `model.rs` (the derivation, unit-tested without a
window), `column.rs` (the body), `tests.rs` (the rendered panel).

1. **The facts box** is a dynamic list of key/value rows: `TYPE`, then `ROWS` where the source
   reports one, then whatever `ColumnInfo.stats` actually carries, in a fixed `FACT_ORDER`
   (`DISTINCT · MIN · MAX · MEAN · MEDIAN`). A Parquet column shows four rows, a CSV column shows
   one, and neither shows a blank. Inexact facts render `~value` (a Parquet footer truncates long
   strings, so what it stored is a bound).
2. **The title** carries the kind swatch + name, the dtype badge, a **source-format badge**
   (PARQUET · CSV · JSON · ARROW · VIEW, each its own theme colour; an unrecognised format wears
   the recessive tone), and "from &lt;owner&gt;".
3. **Nested columns** state their whole shape in a NESTED FIELDS box, depth-indented, every level
   expanded (display only — there is nothing to collapse, and profiling never descends).
4. **Selection resolves by path**, so `address.city` is that field and not an unrelated top-level
   `city`. A view's columns resolve the same way, off `ViewInfo::columns`. This closes the
   nested-column gap P3-02 left — P3-07 was rescoped onto registration-failure messages and never
   covered it.
5. **The completeness bar** renders only with a real, *exact* null count and a real row count. The
   engine already drops a `null_count == num_rows` (ambiguous in DataFusion), and the bar refuses
   an inexact count or one past the row count. The percentage never rounds into a claim: a column
   with nulls can't read `100%` (it reads `>99.9%`), one with values can't read `0%`. The count
   itself is **never** a fact row — one number, one rendering.
6. **A selection the catalog moves under** says what happened rather than going blank or holding
   stale facts: `Loading…` mid-re-scan, the engine's own reason for a refused table, and
   "'x' is no longer in the catalog." / "'c' is no longer a column of 'x'." after a drop or a
   schema change.

The panel subscribes to the `ProjChan` its **selection's kind** names, so a table registration
landing never wakes it while a view's column is being inspected.

## Left inert (P3-09 owns the capability)

The STATISTICS zone's scan half: the age / view-as-query / re-scan controls, the distribution
bars, the running state. Its **call-to-action card is rendered in full** — the canvas's accent
`filled` button at the design system's `ACTION_HEIGHT`, simply with **no press handler**
(`column.rs::profile_card`). Not greyed out: the card is the surface's primary call to action, and
a disabled one would misrepresent the canvas. See P3-09 for the wiring notes.

## Acceptance
- [x] Selecting a column (top-level or nested) shows its real metadata; no fabricated stats.
- [x] Completeness bar shows only when a real null count exists.

## Notes for later

- **`fmt_int` moved** to `strata_core::util` (beside `human_bytes`), where the plan view's metrics,
  the results footer and this panel all import it. It was two copies before.
- **Fork change:** `TooltipContainer` gained `ContainerSizeExt` / `ContainerPositionExt`
  (`crates/freya/crates/freya-components/src/tooltip.rs`), matching `OverflowedContent`'s impls —
  a tooltip over a fill-width control (the completeness bar) could not be sized before. **Push the
  fork**, or a fresh clone can't init the submodule.
- One-sided borders **do** work (`BorderWidth` has per-side fields; only its `From` impl is
  `f32`-wide), which the row hairlines here use. A comment in `sidebar/catalog/entry.rs` claimed
  otherwise and has been corrected.

## Freya / references
- Bespoke — hand-rolled (plan §5). Core `ColumnInfo.stats` / `TableMeta.rows`. Design:
  `Strata.dc.html` inspector canvas. DEV_TASKS U9 (the "only real facts" reasoning + honesty
  calls). Previews: `cargo test -p strata-freya inspector_preview -- --ignored`.
