# AA-04 · Settings ▸ Agent access

**Workstream:** Agent access · **Status:** ⬜ · **DEV_TASKS:** — · **Depends on:** AA-03

## Goal
The control surface for a capability AA-03 ships dark: enable/disable, the port, the token
(view / copy / regenerate), and enough status to configure a client without reading docs.

## Current state
AA-03 reads `agent_access.*` settings (default off) from a hand-edited config. The Settings
window has five built categories; the pane vocabulary is `components::form`
(`Variant::Preferences`), the index is `apps/settings/search.rs`'s one-table `Anchor` enum, and
new fields ride `settings_merge!` (a field that isn't merged is a build error).

## What to build

### Settings fields (`strata-core::config::Settings`)
`agent_access_enabled: bool` (default false), `agent_access_port: u16` (default: the constant
AA-02/03 named), `agent_access_token: String` (minted on first enable). Through
`settings_merge!`; committed via the standard per-field diff (draft vs seed).

### The pane
A new category (or a group under System — check the nav model's shape and the designer's
breadcrumb conventions; a new category needs a `Route`, a `CATEGORIES` entry and breadcrumbs in
`apps/settings/model.rs`). Rows, all via `Anchor::row()` so search reaches them:

- **Enable agent access** — Switch. Subtext states what it opens: a local server on
  127.0.0.1 for MCP clients; off by default.
- **Port** — `NumberField`, bounded to the valid range; subtext notes a change restarts the
  server (AA-03's live start/stop handles it off `ConfigChan::Settings`).
- **Token** — mono `ValueField` (read-only display) + copy + **Regenerate** (a T2-style confirm
  is unnecessary — regenerating just invalidates clients; say so in the subtext).
- **Client setup** — a `Note` row carrying the one-line `claude mcp add …` incantation with the
  live port/token substituted, copyable. This is the row that makes the feature usable without
  reading the spec.
- **Status** — running / not running (and the reason when enabled-but-failed, e.g. port in
  use). Read from the server handle's state, not derived from the settings — a toggle that
  claims running while the bind failed is a lying control.

### Search index
Every row above gets an `Anchor` variant + keywords ("mcp", "agent", "claude", "token",
"port") in `search.rs`'s table. The category (if new) resolves through `model`'s `category` —
never restated in the index.

## Acceptance
- Toggling enable starts/stops the server live (no app restart); status row tells the truth,
  including bind failure.
- Port edit + Apply restarts the server on the new port; out-of-range values can't be applied
  (the field's bounds, not a post-hoc correction).
- Regenerate mints a new token, persists it, and the old one 401s immediately.
- Settings search finds every row and reveals it (scroll + flash) — the P4-09 machinery, driven
  by the anchors.
- `settings_merge!` covers the new fields (the compiler enforces it); another window's
  concurrent setting commit survives an Apply here (the standard seed-diff behaviour — no new
  work, just don't break it).
- Unit tests where the panes already have them (model/search tables).

## Notes
- Don't build a client-config generator beyond the one Note row — the incantation is the whole
  need.
- If the designer supplies a canvas for this pane, it wins over the sketch above; the rows'
  *existence* is settled, their dress is not.
