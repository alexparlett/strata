# Chart 03 · Client aggregate + guardrails

**Workstream:** Chart (Rz2) · **Status:** ⬜ · **Depends on:** 01

## Goal
Client-side aggregation for charts, with guardrails so a chart can't blow up on huge/unsuitable data.

## Current state
Not built.

## Build (to `CHART_SPEC.md`)
- **Client aggregate** over the current result / snapshot (group + agg per the encoding).
- **Guardrails**: row/category caps, type checks (e.g. no aggregate on unsuitable types), and a clear
  message when the data doesn't fit the chart — don't silently mislead.

## Acceptance
- [ ] Aggregated charts render correctly; oversized/unsuitable data hits a guardrail with a clear message.

## Freya / references
- `docs/CHART_SPEC.md` (aggregate + guardrails). The result/snapshot (P2-01/03).
