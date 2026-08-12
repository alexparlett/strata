# Phase 5 — Design polish

Apply the rebuilt design system across every surface: the spacing/radius token scale, hover/focus/
active states, animations, and theme dial-in against the canvases.

## State of play
Cross-cutting, and partly continuous — each surface built in phases 2–4 already targets its `.dc.html`
canvas, so this phase is the **consistency + finish pass**, not a first build. In Freya the design
system is the **theme** (`define_theme!` + the JSON themes) — polish is mostly theme/token work, not
per-widget CSS. Every open task's "current state" was re-verified against the tree on **2026-08-12**;
the scopes below are the audited ones, not the ones the tasks were first filed with.

**Surfaces added since these tasks were first written** — all in scope for this phase: the chat
pane (`views/chat/`), the connection editor window (`apps/connection/`), the Configure window
(`apps/configure/`), the connections sidebar pane (`views/sidebar/connections/`), and
Settings ▸ AI (`apps/settings/views/ai/`).

**Where the canvases live** (design handoff refreshed 2026-08-12): the chat pane and the inspector
are **sections of `Strata.dc.html`** — there is no standalone `Chat.dc.html`/`Inspector.dc.html`;
the connection editor and Configure window have `Connections.dc.html` / `Configure.dc.html`.

**Canvas vs settled decisions — the settled decision wins.** The handoff's changelog still
describes two things the app has since deliberately walked away from: the **Agents pane** and the
header's agent-access dot (removed 2026-08-12 — "no surface lists agents", AGENTS.md §2), and a
Settings ▸ AI roster of eight named provider rows (the app settled **one OpenAI-compatible row**,
`56da328`). Neither is drift to fix.

## Tasks

| # | Task | Status | DEV_TASKS | Depends on |
|---|---|---|---|---|
| P5-01 | Spacing & radius token scale across surfaces | ✅ | F3 | — |
| P5-02 | Hover / focus / active interaction states | ⬜ | — | — |
| P5-03 | Animations & transitions | ⬜ | — | P5-01 (shared timing consts) |
| P5-04 | Theme dial-in (Midnight / Daylight) | ⬜ | W5 | — |
| P5-05 | Per-surface design audit (Freya drift pass) | ⬜ | Part 1 | P5-01 (a scale to snap to) |
| P5-06 | Panel overflow & small-size behaviour (scroll / fold / hide) | ✅ | — | P3-01 + content |
| P5-07 | One `Search` control for the app's eight filter boxes | ⬜ | — | — |
| P5-08 | Scroll acceleration for long lists (fork: `scrollviews::shared`) | ✅ | — | — |
| P5-09 | Window-theme unification (settings / export / launcher → `window`) | ⬜ | — | theme v2 (landed) |
| P5-10 | Role-read re-homing (component-themed surfaces off the direct reads) | ⬜ | — | **P5-09** (coupled — see both files) |

## Recommended order

1. **P5-09 → P5-10** — a coupled, mechanical pair: P5-09 grows `window`'s field set, which is
   what P5-10 re-homes the connection/configure text reads onto. Doing P5-10 first would re-home
   against a field set about to change.
2. **P5-07** — mechanical; one component, eight call sites.
3. ~~**P5-01**~~ — done: the scale and the shared sizes are `components::metrics`.
4. **P5-02**, then **P5-03** — both carry a fork half; P5-03 puts its durations in P5-01's
   **Timing** section, beside `PROGRESS_HOLD`.
5. **P5-04**, then **P5-05 last** — the dial-in and the audit judge the app *after* it matches
   the tokens and themes it is judged against.

## Legend
✅ done · 🟢 UI only · 🟡 partial · ⬜ todo.
