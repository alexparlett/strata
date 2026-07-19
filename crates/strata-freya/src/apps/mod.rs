//! One self-contained folder per OS window — a Freya window root (`App`). Each window
//! owns its root shell + its own `views/` + per-window `state/` (Radio station),
//! symmetrically; only genuinely global state, DS widgets, and the engine bridge sit at
//! the crate top level. See `docs/FREYA_PORT_PLAN.md` §3.
//!
//! Phase 1 ships only the project window; `launcher` / `settings` / `export` / `configure`
//! / `connections` land with their phases (§6).

pub mod project;
