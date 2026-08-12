# P5-04 · Theme dial-in (Midnight / Daylight)

**Phase:** 5 · **Status:** ⬜ · **DEV_TASKS:** W5 · **Depends on:** —

## Goal
Tune the Midnight/Daylight themes to match the canvases across every surface.

## Current state (verified 2026-08-12)
No dial-in pass has ever run — the last colour-tuning commit (`4865a54`) predates this backlog.
The theme files live at **repo-root `themes/`** (daylight.json / midnight.json), not under the
crate. Of the items flagged for visual judgment, the ledger now reads:

**Resolved in code (verify visually, then strike):**
- The confirm dialog's card is already on `elevated_surface.background`
  (`components/dialog.rs:230` reads `Role::ElevatedSurface`). **But** `dialog.rs:7`'s module doc
  still describes the card as on `surface_tertiary` — a role that no longer exists. Fix the doc
  line as part of this task.
- `elevated_element.hover` is translucent in both themes (`rgba(15,23,42,.06)` /
  `rgba(255,255,255,.09)`), as the reconciliation wanted.
- `inspector.emphasis_color` → `role(Role::TextAccent)` (`theme/components.rs:640`) and the cell
  view's badge → `AccentBadge`/`Accent` (:529-530) are wired as intended — judge the resolved
  colours, don't re-wire.
- `launcher.remove_hover_background` → `role(Role::ErrorBackground)` (:305).

**Still open for judgment:**
- `accent.selection` and `accent.badge` are **value-identical** in both themes (both `.12` alpha
  on the accent) — if the merge was meant to keep them distinguishable, it isn't; decide whether
  one moves.
- `ghost_element.hover` is opaque in both themes (`#eef0f4` / `#262d37`) while
  `elevated_element.hover` is translucent — judge whether the split is intended.
- `data_type.timestamp` on tan (`#9a6700` / `#e2b98c`) — the flagged tan question, unjudged.
- `flat_button.disabled_color` is **silently inherited**: the retune at `theme/components.rs:89-94`
  never names it, so it falls through the fork default (`Preference::reference("disabled")`) which
  the bridge resolves to `Role::TextDisabled` (`theme/mod.rs:175`). The colour is right; decide
  whether to state it in the mapping table or keep the documented inheritance — then document
  whichever wins.
- The entity hues agreed between catalog and completion, and the per-surface tier accuracy
  (Midnight = JetBrains-style tiers; Daylight = comfort zone) — the original W5 pass, untouched.

## Build
- Preview with the Freya component gallery / `FreyaThemeGallery.dc.html`; adjust the **roles** in
  both theme files (and, where a role is genuinely over-shared, split it in `roles!` + the mapping
  table) so each surface matches its canvas. The chat pane and inspector are judged against their
  sections of **`Strata.dc.html`** (no standalone canvas); the connection editor and Configure
  window against `Connections.dc.html` / `Configure.dc.html`.
- **Skip the canvas-vs-settled conflicts**: the Agents pane (removed on purpose) and the
  Settings ▸ AI eight-provider roster (settled as one OpenAI-compatible row) are not targets.
- **After any theme change**, regenerate + verify the schema:
  `UPDATE_SCHEMA=1 cargo test -p strata-freya schema_in_sync`.

## Acceptance
- [ ] Both themes match the canvases across surfaces; `schema_in_sync` passes.
- [ ] The still-open list above is judged and either changed or struck with a reason.
- [ ] `dialog.rs:7`'s stale role name fixed.

## Freya / references
- `themes/*.json` (repo root) + `strata-core`'s `roles!` + `crates/strata-freya/src/theme/components.rs`,
  `FreyaThemeGallery.dc.html`, the `schema_in_sync` test, `docs/FREYA_THEME_SPEC.md`. DEV_TASKS W5.
