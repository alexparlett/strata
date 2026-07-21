# Phase 6 — Platform + parity

The platform layer and the cutover: command palette, native menu, the OS shim
seam, then run both apps side by side, close gaps, and **delete the Dioxus app**.

## State of play
Much of the old platform complexity **evaporates in Freya** — native key events remove the webview
⌘A/⌘C swallowing and the objc close intercept (the base keymap + OS-close already land in P2-20; the
rebindable keymap page in P4-08). What remains is the palette, the native menu decision, the platform seam, and the final
parity sweep before deleting `crates/strata-dioxus`.

## Tasks

| # | Task | Status | DEV_TASKS | Depends on |
|---|---|---|---|---|
| P6-01 | Command palette (⌘K) + depth | ⬜ | U11 / T3 | P2-20 |
| P6-02 | Native menu bar (decision + menu-follows-opener) | ⬜ | F8 | P4-01 |
| P6-03 | Platform seam for OS shims | ⬜ | F5 | — |
| P6-04 | Side-by-side parity + delete the Dioxus app | ⬜ | — | all |

## Legend
✅ done · 🟢 UI only · 🟡 partial · ⬜ todo.
