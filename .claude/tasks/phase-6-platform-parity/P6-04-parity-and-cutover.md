# P6-04 · Side-by-side parity + delete the Dioxus app

**Phase:** 6 · **Status:** ⬜ · **Depends on:** all phases

## Goal
Reach parity with the Dioxus app, then remove it.

## Current state
Both apps share the core and can run side by side (plan §1 coexistence). The Dioxus app
(`crates/strata-dioxus`) is the mature reference.

## Build
- Run both apps side by side; walk the DEV_TASKS parity list + the known bugs; close remaining gaps.
- Confirm the two **known bugs** don't survive the port (re-open-in-place path corruption; ⌘S on a
  view saving a new saved-query instead of updating the view).
- When parity holds: **delete `crates/strata-dioxus`** (and its workspace-exclude, the `links`
  workaround, the transitional shims); update `Cargo.toml` + docs; make Freya the sole app.

## Acceptance
- [ ] Parity checklist clear; known bugs absent; `crates/strata-dioxus` removed; root builds Freya only.

## Freya / references
- Plan §1/§6 (coexistence → cutover), §7 (survives vs rewrite). DEV_TASKS Part 3 + Known bugs.
