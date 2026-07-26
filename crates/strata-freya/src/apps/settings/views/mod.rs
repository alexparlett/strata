//! The Settings window's frame (design `Settings.dc.html`): the title bar, the category
//! rail, the pane the router swaps content into, and the Cancel/Apply footer.
//!
//! All four are mounted by [`SettingsChrome`], the router layout — so navigating a category
//! remounts nothing but the pane's content.
//!
//! [`ThemePane`] is the one category page built so far (P4-04); the rest are placeholders that
//! land with P4-05…P4-08.

mod chrome;
mod footer;
mod nav;
mod pane;
mod theme;
mod title_bar;

pub use chrome::SettingsChrome;
pub use pane::Pane;
pub use theme::ThemePane;
