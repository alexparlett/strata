//! Strata's framework-agnostic **app core** — the services the app reasons *with*, below any UI
//! framework and clear of DataFusion. Depends *down* onto `strata-model`, the data vocabulary;
//! the engine is `strata-engine`, which depends *up* onto this crate and never the reverse.
//!
//! - [`util`] — shared helpers, plus the crash-safe write every persisted file goes through.
//! - [`config`] — disk app config + settings/keymap definitions.
//! - [`keymap`] — the command table and chord resolution, settings-driven.
//! - [`theme`] — the theme data model: the role vocabulary, the built-ins, schema generation.
//! - [`project`] — `.strata/` project persistence (the durable catalog defs).
//! - [`secret`] — the OS-keystore secret store: config holds a reference, never the secret.
//! - [`ai`] — the assistant's configuration vocabulary: the persisted tokens only, since the
//!   provider *table* is `strata-agent`'s, next to the `genai` pin it is verified against.
//! - [`models`] — the model-listings satellite, beside [`config`] rather than in it because a
//!   fetched list is a cache of a remote fact rather than something the user edited.
//! - [`update`] — the in-app updater's mechanism. Window-free and blocking, like the listings
//!   fetch.

pub mod ai;
pub mod config;
pub mod keymap;
pub mod models;
pub mod project;
pub mod secret;
pub mod theme;
pub mod update;
pub mod util;
