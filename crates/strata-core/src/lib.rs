//! Strata's framework-agnostic **logic core** — everything the app reasons *with*, below
//! any UI framework. Depends *down* onto `strata-model` (the data vocabulary) and is shared
//! by both the Dioxus app and the Freya app. See `docs/FREYA_PORT_PLAN.md`.
//!
//! Modules (filled in over the port's phase 0):
//! - [`sql`] — the SQL language service (lex / context / symbols / validate / complete).
//! - [`util`] — small pure helpers (hashing, byte/duration/timezone parsing, names).
//! - [`plan`] — the query-plan (EXPLAIN) model + formatting.
//! - [`config`] — disk app config + settings/keymap definitions.
//! - [`profile`] — the profiling scan logic (aggregate exprs + result decode).
//! - [`engine`] — the DataFusion worker, `Command`/`Event` protocol, and connection handle.

use engine::{plan, profile};

pub mod config;
pub mod engine;
pub mod theme;
pub mod util;
