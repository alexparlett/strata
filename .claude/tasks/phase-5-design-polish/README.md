# Phase 5 — Design polish

Apply the rebuilt design system across every surface: the spacing/radius token scale, hover/focus/
active states, animations, and theme dial-in against the canvases.

## State of play
Cross-cutting, and partly continuous — each surface built in phases 2–4 already targets its `.dc.html`
canvas, so this phase is the **consistency + finish pass**, not a first build. In Freya the design
system is the **theme** (`define_theme!` + the JSON themes) — polish is mostly theme/token work, not
per-widget CSS. Do these once the surfaces exist; several can run in parallel with late phase-4 work.

## Tasks

| # | Task | Status | DEV_TASKS | Depends on |
|---|---|---|---|---|
| P5-01 | Spacing & radius token scale across surfaces | ⬜ | F3 | surfaces exist |
| P5-02 | Hover / focus / active interaction states | ⬜ | — | surfaces exist |
| P5-03 | Animations & transitions | ⬜ | — | surfaces exist |
| P5-04 | Theme dial-in (Midnight / Daylight) | ⬜ | W5 | — |
| P5-05 | Per-surface design audit (Freya drift pass) | ⬜ | Part 1 | phases 2–4 |

## Legend
✅ done · 🟢 UI only · 🟡 partial · ⬜ todo.
