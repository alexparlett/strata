//! The Settings window's frame (design `Settings.dc.html`): the title bar, the category
//! rail, the pane the router swaps content into, and the Cancel/Apply footer.
//!
//! All four are mounted by [`SettingsChrome`], the router layout — so navigating a category
//! remounts nothing but the pane's content.
//!
//! A category page is a [`Pane`] wrapping a [`SettingList`](field::SettingList) of
//! [`Setting`](field::Setting) rows — the shell in [`field`], so the pages carry only their own
//! settings. [`ThemePane`] (P4-04) and [`DataDisplayPane`] (P4-05) are built; System, Engine
//! and Keymap are placeholders until P4-06…P4-08.

mod chrome;
mod data_display;
pub mod field;
mod footer;
mod nav;
mod pane;
mod theme;
mod title_bar;

pub use chrome::SettingsChrome;
pub use data_display::DataDisplayPane;
pub use pane::Pane;
pub use theme::ThemePane;
