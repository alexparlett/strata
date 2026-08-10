//! The Settings window's frame (design `Settings.dc.html`): the title bar, the category
//! rail, the pane the router swaps content into, and the Cancel/Apply footer.
//!
//! All four are mounted by [`SettingsChrome`], the router layout — so navigating a category
//! remounts nothing but the pane's content.
//!
//! A category page is a [`Pane`] wrapping a [`Form::preferences`] of [`Row`]s — the
//! shared form vocabulary in [`crate::components::form`], so the pages carry only their own
//! settings. Two of the five are not that shape, because they are *surfaces* rather than lists of
//! named settings: [`EnginePane`] (P4-07) and [`KeymapPane`] (P4-08) are both a grid on Freya's
//! builtin `Table`, and what they share beyond it — the note that appears between two rows — is
//! [`RowNote`].
//!
//! The [`ai`] group (AS-03) is three more: [`McpPane`] is AA-04's, the ordinary shape of a
//! preferences form; [`ProvidersPane`] and [`ChatPane`] are the assistant's roster and its
//! new-chat defaults.
//!
//! [`Form::preferences`]: crate::components::form::Form::preferences
//! [`Row`]: crate::components::form::Row

mod ai;
mod chrome;
mod data_display;
mod engine;
mod footer;
mod keymap;
mod nav;
mod pane;
mod row_note;
mod system;
mod theme;
mod title_bar;

pub use ai::{commit, ChatPane, McpPane, Probes, ProvidersPane, TypedKeys};
pub use chrome::SettingsChrome;
pub use data_display::DataDisplayPane;
pub use engine::{EnginePane, PropRows};
pub use keymap::KeymapPane;
pub use pane::Pane;
pub use row_note::RowNote;
pub use system::SystemPane;
pub use theme::ThemePane;
