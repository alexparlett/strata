# Chart 04 · Guardrails

**Workstream:** Chart (Rz2) · **Status:** ✅ · **Depends on:** 02, 03

## Goal
The refusal surfaces: guardrail overlays in place of the canvas, and the high-cardinality
banner over it. Spec: `docs/CHART_SPEC.md` §7.

**Scope changed during the task.** It was written as "guardrails + the SQL scaffold", and the
scaffold — an *Aggregate in SQL* press composing a `GROUP BY` and opening it unrun in a new tab
— was built, reviewed, and then **cut** on Alex's call before merge. Spec §8 and
`docs/reference/INVARIANTS.md` (the chart entry) carry the reasoning; the short version is in
"What the scaffold cut settled" below. Do not re-add it to the strip.

## Current state (before this task)
01 answers the refusals as data (`OverCap`, `Duplicates`); 02/03 render and configure.

02 **stated** every refusal already, as a centred title + body `Notice` in place of the canvas
(`results/chart/mod.rs`: `notice` for the engine's two and for every shape that would otherwise
paint nothing, `encode`'s `Err` for the encodings a schema cannot satisfy). What it did not have
was the design's dress or the banner.

## Built
- **Overlays** — the design's guardrail empty state (canvas `Strata.dc.html`): a 46px glyph tile
  over the title and body, centred in the pane the plot would have filled, wrapping at 380px.
  Every condition in spec §7 renders through the one `Notice`, driven by `ChartData`'s refusal
  variants and the config — never re-derived in the UI. No CTA, no button (§8).
- **Banner** — non-blocking, across the canvas top, past 60 categories (`axis.labels.len()`, no
  extra query). The chart renders beneath it, unaltered. Wears the Export window's banner: the
  `chart` theme's new `warning_background` / `warning_border_fill` pair for the box, the sheet's
  semantic `warning` for glyph and text, so there is one warning tone app-wide.
- **`ChartRole::Temporal` split into `Instant` + `Clock`** — see below.
- **A collapsed pane clips the notice rather than reflowing it.** Found by eyeballing a narrow
  window: the pane gives its width away entirely, and the refusal copy — a block that took the
  pane's width verbatim — reflowed into **one character per line**, a column of letters down the
  pane (measured 10.4px per text run). Fixed by sizing the *notice*: it is a fixed
  `COPY_WIDTH + 2 x NOTICE_PAD` block, centred, and `canvas_pane` is `Overflow::Clip`, so the
  copy wraps where it always wraps and the pane cuts it off.

  **Not** by flooring the pane on `PANE_BODY_MIN_W`, which was tried first and is wrong: that is
  the *side* panels' rule, where a floor keeps the resize handle grabbable. The middle pane
  collapses to nothing and clips, and nothing in it needs grabbing. A floor also pushes the pane
  out past the row it sits in, which the clip then has to undo.

  The preview harness calls `canvas_pane` rather than keeping a second copy of the layout, and
  shoots the notice at both widths (`chart-notice-narrow.png`, `chart-notice-wide.png`) — a
  collapse is a visual claim, so it gets a visual check.

There is **no materialize cap, no sampling, and no aggregation fallback** (settled — spec §1.2,
§7), and V1 adds no control behind the refusals.

## What the scaffold cut settled
Recorded because the capability will come up again, and because a chunk of it survives:

- **The mechanism was sound; the placement was not.** A `GROUP BY` composed from the resolved
  encoding over the *run's* SQL (`QuerySpec::sql`, never the editor buffer), opened through
  `session.open_named` and never run. It worked — the generated SQL was verified against a real
  DataFusion fixture. What killed it: it sat in the control strip as the one control that *left*
  the chart, and no comparable tool puts it there (DBeaver's Grouping panel is a surface of its
  own; Metabase / Superset / Looker eject to SQL from a menu).
- **It was standing in for chart-side aggregation**, which is the thing worth revisiting (spec
  §10). A shortcut that makes that gap tolerable is a reason not to close it.
- **The role split stays.** `Instant` (Date32/Date64/Timestamp) and `Clock` (Time32/Time64) are
  identical on an axis — same default X, same default mark, read together by `config::is_time` —
  and differ wherever a stride does. Measured: DataFusion refuses a day-wide `date_bin` over a
  `Time` column, and `date_bin` takes a `Timestamp` so `Date32` coerces and `Date64` does not.
  Nothing in V1 reads the distinction; it is kept because recovering it later means reading a
  type's *spelling*, which the role invariant rules out.
- **Reverted with it:** `scaffold.rs` entire, the `sql` prop on `ChartView`, `ControlStrip`'s
  scaffold section, and the promotion of `quote_col` out of `engine/export.rs` (it is private
  there again, since export is once more its only caller).

## Acceptance
- [x] Each guardrail condition shows its overlay; the banner appears past 60 categories and
      blocks nothing; no silent truncation or aggregation anywhere.
- [x] Nothing in the chart writes or runs SQL.

## References
`docs/CHART_SPEC.md` §7–§8. Banner dress: `apps/export/views/partition.rs`. Guardrail copy per
AGENTS.md §3 (IDE register, single-quoted identifiers).
