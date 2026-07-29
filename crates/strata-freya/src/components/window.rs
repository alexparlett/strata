//! **Window chrome** — the tones every one of the app's windows is built out of: the body it
//! floats on, the recessed insets inside it, its rules, and the two status blocks.
//!
//! Only what a window's chrome actually paints: a field is added here when a surface reads it,
//! not in anticipation (AGENTS.md §1) — an unread slot still has to be authored in every theme
//! file and pinned by the committed schema, for no rendered pixel.
//!
//! One theme for *all* windows, not one per window. A window is not a component in the sense
//! that a button is, but its dress is shared vocabulary in exactly the way a button's is, and a
//! per-window block of the same fields is four blocks to keep in step for one reskin.
//!
//! **This is also the only way to reach the tones at all.** A window's surfaces live in the
//! theme's **palette** — `surface_overlay`, `border_control`, `line`, `accent_badge`,
//! `accent_selection`, `text_muted` — not in the 27-slot sheet, and a `reference` in a component
//! theme is the only thing that resolves a palette name. Reading `use_theme().colors()` and
//! picking the nearest sheet slot instead does not merely approximate them: the body lands on
//! `background`, the app's *darkest* tone, when a floating window wants `surface_overlay`, which
//! is several steps lighter. That is what a window built without this looks like, and it is why
//! this exists rather than each window reaching for the sheet.
//!
//! The export, settings and launcher windows still carry their own copies and should migrate
//! onto this; their fields already reference these very slots.

use freya::prelude::*;

// `%[no_ext]`: window chrome is read by a window's *views* rather than by one component, so
// there is no type for the generated `…ThemePartialExt` builder to hang off.
define_theme!(
    %[no_ext]
    %[component]
    pub Window {
        %[fields]
        /// The window body — a **raised** tone (`surface_overlay`), because a window floats
        /// above the app rather than being cut out of it. The single most consequential field
        /// here: the sheet's `background` is the app's darkest, and a window wearing it reads as
        /// a hole.
        background: Color,
        /// A **recessed** inset within the body (`surface_primary`): a list, a field's box, a
        /// status block. Below the body, not above it.
        panel_background: Color,
        /// The window's own rules — a title bar's underline, a footer's overline, a panel's edge.
        border_fill: Color,
        /// A selected row within a panel.
        row_selected_background: Color,
        /// The window's mark, and the accent-tinted tile behind it.
        icon_color: Color,
        icon_background: Color,
    }
);

/// Read the window chrome. Every window's views resolve their surfaces through here.
pub fn window_theme() -> WindowTheme {
    get_theme!(&None::<WindowThemePartial>, WindowThemePreference, "window")
}
