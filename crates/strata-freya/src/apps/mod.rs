//! One self-contained folder per OS window — a Freya window root (`App`). Each window
//! owns its root shell + its own `views/` + per-window `state/` (Radio station),
//! symmetrically; only genuinely global state, DS widgets, and the engine bridge sit at
//! the crate top level. See `docs/FREYA_PORT_PLAN.md` §3.
//!
//! Phase 4 adds the `launcher`, `settings`, `export` and `configure`; the `data source` editor
//! lands with the Data sources workstream (W7 · 03). Spawning and focusing between them is
//! `crate::platform::windows`.

pub mod configure;
pub mod export;
pub mod launcher;
pub mod project;
pub mod settings;
pub mod source;
