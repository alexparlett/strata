# Workstream — Chart view (Rz2)

The results **Chart** surface: chart types, an encoder strip, client-side aggregation, and guardrails.
A whole feature surface (not drift), switched into by the results Table/Chart segment (P2-07). Spec:
`docs/CHART_SPEC.md`.

## State of play
Not built in Freya. The results pane has no Chart body; P2-07 adds the switcher, this builds the body.
Charts render **client-side** over the current result / snapshot (P2-01), with guardrails on size/types.

## Tasks

| # | Task | Status | DEV_TASKS | Depends on |
|---|---|---|---|---|
| 01 | Chart body + type selection | ⬜ | Rz2 | P2-07, P2-03 |
| 02 | Encoder strip (x / y / series / agg) | ⬜ | Rz2 | 01 |
| 03 | Client aggregate + guardrails | ⬜ | Rz2 | 01 |

## Legend
✅ done · 🟢 UI only · 🟡 partial · ⬜ todo.
