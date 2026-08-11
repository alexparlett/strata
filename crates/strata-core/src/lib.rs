//! Strata's framework-agnostic **logic core** — everything the app reasons *with*, below
//! any UI framework. Depends *down* onto `strata-model` (the data vocabulary) and is shared
//! by both the Dioxus app and the Freya app. See `docs/FREYA_PORT_PLAN.md`.
//!
//! Modules (filled in over the port's phase 0):
//! - [`sql`] — the SQL language service (lex / context / symbols / validate / complete).
//! - [`util`] — small shared helpers (hashing, byte/duration/timezone parsing, names) plus
//!   the crash-safe file write every persisted file goes through.
//! - [`plan`] — the query-plan (EXPLAIN) model + formatting.
//! - [`config`] — disk app config + settings/keymap definitions.
//! - [`project`] — `.strata/` project persistence (the durable catalog defs).
//! - [`profile`] — the profiling scan logic (aggregate exprs + result decode).
//! - [`engine`] — the DataFusion boundary: the direct-call async [`engine::Engine`] facade
//!   (query / plan / profile / serialize) and its snapshot lifecycle.
//! - [`register`] — the project registration pass: make the engine match a set of defs,
//!   reporting per-def outcomes (shared by the app's catalog passes and headless hosts).
//! - [`secret`] — the OS-keystore secret store: config holds a reference, never the secret.
//! - [`ai`] — the assistant's configuration vocabulary (AS-03): which brains are set up, and
//!   what a new chat starts with. The persisted tokens only — the provider *table* is
//!   `strata-agent`'s, next to the `genai` pin it is verified against.

use engine::profile;

pub mod ai;
pub mod config;
pub mod engine;
pub mod keymap;
pub mod project;
pub mod register;
pub mod secret;
pub mod theme;
pub mod util;
