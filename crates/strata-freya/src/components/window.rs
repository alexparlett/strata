//! **Window chrome** — the tones every one of the app's windows is built out of: the body it
//! floats on, the recessed insets inside it, its rules, and the two status blocks.
//!
//! One theme for *all* windows, not one per window. A window is not a component in the sense
//! that a button is, but its dress is shared vocabulary in exactly the way a button's is, and a
//! per-window block of the same fifteen fields is four blocks to keep in step for one reskin.
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
        /// A rule *inside* a panel, one step quieter than the panel's own edge (`line`): one row
        /// of a list from the next.
        divider_fill: Color,
        /// A control's edge — a text field, a bordered list, an outlined button.
        control_border_fill: Color,
        /// A selected row within a panel.
        row_selected_background: Color,
        /// The window's mark, and the accent-tinted tile behind it.
        icon_color: Color,
        icon_background: Color,
        /// Recessive prose a window writes about itself — an empty state, a line explaining why
        /// an action is unavailable. Brighter than a form's label, which is an eyebrow pitched to
        /// recede under a control; this is a sentence meant to be read.
        muted_color: Color,
        /// A work-in-flight strip.
        busy_background: Color,
        busy_color: Color,
        /// A failure block. Its glyph and text take the sheet's `error` directly — that is one of
        /// the four semantic slots and must follow the app-wide ramp wherever it appears — so
        /// only the tinted box is named here.
        error_background: Color,
        error_border_fill: Color,
    }
);

/// Read the window chrome. Every window's views resolve their surfaces through here.
pub fn window_theme() -> WindowTheme {
    get_theme!(&None::<WindowThemePartial>, WindowThemePreference, "window")
}
