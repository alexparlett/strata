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
//! **The theme is what makes the elevation deliberate.** Every field maps onto a role in the
//! static table (`theme/components.rs`) — the body onto `elevated_surface.background`, because
//! a window *floats above* the app rather than being cut out of it. A view reaching for
//! `use_roles()` and picking the nearest-sounding role instead tends to land the body on
//! `background`, the app's darkest tone — a window wearing it reads as a hole. That mistake was
//! built once, and it is why windows resolve their chrome through this one theme rather than
//! each view picking roles for itself (AGENTS.md §3: a themed surface reads its theme).
//!
//! The export, settings and launcher windows still carry their own copies and should migrate
//! onto this (P5-09); their fields already map onto these very roles.

use freya::prelude::*;

define_theme!(
    %[no_ext]
    %[component]
    pub Window {
        %[fields]
        /// The window body — the floating-chrome tone (`elevated_surface.background`), because
        /// a window floats above the app rather than being cut out of it. The single most
        /// consequential field here: `background` is the app's darkest, and a window wearing
        /// it reads as a hole.
        background: Color,
        /// A **recessed** inset within the body (`surface.background`): a list, a field's box,
        /// a status block. Below the body, not above it.
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
