//! Strata's framework-agnostic **logic core** — everything the app reasons *with*, below any UI
//! framework. Depends *down* onto `strata-model`, the data vocabulary.
//!
//! - [`sql`] — the SQL language service (lex / context / symbols / validate / complete).
//! - [`util`] — shared helpers, plus the crash-safe write every persisted file goes through.
//! - [`plan`] — the query-plan (EXPLAIN) model + formatting.
//! - [`config`] — disk app config + settings/keymap definitions.
//! - [`project`] — `.strata/` project persistence (the durable catalog defs).
//! - [`profile`] — the profiling scan logic (aggregate exprs + result decode).
//! - [`engine`] — the DataFusion boundary: the direct-call async [`engine::Engine`] facade and its
//!   snapshot lifecycle.
//! - [`register`] — the project registration pass, shared by the app's catalog passes and by
//!   headless hosts.
//! - [`secret`] — the OS-keystore secret store: config holds a reference, never the secret.
//! - [`ai`] — the assistant's configuration vocabulary: the persisted tokens only, since the
//!   provider *table* is `strata-agent`'s, next to the `genai` pin it is verified against.
//! - [`models`] — the model-listings satellite, beside [`config`] rather than in it because a
//!   fetched list is a cache of a remote fact rather than something the user edited.
//! - [`update`] — the in-app updater's mechanism. Window-free and blocking, like the listings
//!   fetch.

use engine::profile;

pub mod ai;
pub mod config;
pub mod engine;
pub mod keymap;
pub mod models;
pub mod project;
pub mod register;
pub mod secret;
pub mod theme;
pub mod update;
pub mod util;
