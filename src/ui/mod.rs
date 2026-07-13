//! UI components (Dioxus). Each reads the shared `Signal<AppState>` from context
//! and calls controller actions in `crate::app`.

pub mod activity_rail;
pub mod code_editor;
pub mod components;
pub mod drawer;
pub mod errview;
/// Dev-only S28/S29 component gallery (Help → Component Gallery); compiled out of
/// release builds.
#[cfg(debug_assertions)]
pub mod gallery;
pub mod header;
pub mod icons;
pub mod inspector;
pub mod launcher;
pub mod modals;
pub mod settings;
pub mod sidebar;
// `statusbar` retired by S23 (the activity rail carries Events/History; run state
// lives in the results panel). The file is kept but no longer compiled.
pub mod workbench;

use dioxus_code::{Language, Theme};

/// Resolve a `dioxus-code` grammar by slug (e.g. "sql", "json"). Falls back to
/// the always-present Rust grammar if the requested one isn't bundled, so a
/// missing grammar degrades to plain-ish highlighting instead of panicking.
pub fn lang(slug: &str) -> Language {
    Language::from_slug(slug)
        .or_else(|| Language::from_slug("rust"))
        .expect("dioxus-code default (rust) grammar available")
}

/// The dark code theme closest to the Strata palette. GitHub Dark's
/// token colors line up with the app's data-type palette (string `#7ee787`,
/// number `#79c0ff`, etc.); the editor's background/gutter/selection are then
/// retuned to the app in CSS (`.dxc-editor.ps-sql`).
pub fn code_theme() -> Theme {
    Theme::GITHUB_DARK
}
