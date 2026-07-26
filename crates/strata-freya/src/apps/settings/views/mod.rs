//! The Settings window's frame (design `Settings.dc.html`): the title bar, the category
//! rail, the pane the router swaps content into, and the Cancel/Apply footer.
//!
//! All four are mounted by [`SettingsChrome`], the router layout — so navigating a category
//! remounts nothing but the pane's content.

mod chrome;
mod footer;
mod nav;
mod pane;
mod title_bar;

pub use chrome::SettingsChrome;
pub use pane::Pane;
