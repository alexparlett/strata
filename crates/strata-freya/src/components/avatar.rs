//! The initials tile that leads a project row — the header's project switcher today, the
//! launcher's project lists next (design `Strata.dc.html` project-switcher rows /
//! `Launcher.dc.html` `PINNED` + `RECENT`).
//!
//! The caller passes the **name**, not the letters: deriving the initials is the component's job,
//! so every list spells a project the same way. The dress is the theme's `avatar` component — a
//! resting tile and an `active` one (the accent dress a project with a window open wears),
//! mirroring [`ToggleButton`](crate::components::toggle_button::ToggleButton)'s shape.

use freya::prelude::*;

define_theme!(
    %[component]
    pub Avatar {
        %[fields]
        background: Color,
        color: Color,
        /// The dress for a project that is currently open.
        active_background: Color,
        active_color: Color,
        corner_radius: CornerRadius,
    }
);

/// A rounded initials tile.
#[derive(PartialEq)]
pub struct Avatar {
    name: String,
    active: bool,
    size: f32,
    theme: Option<AvatarThemePartial>,
}

impl Avatar {
    /// A tile for `name` — its initials, in the resting dress.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            active: false,
            size: 28.,
            theme: None,
        }
    }

    /// Wear the active dress (the accent tile — a project that has a window open).
    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    /// The tile's edge length (default 28px — the dropdown row's; the launcher's rows are 32).
    pub fn size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }
}

impl Component for Avatar {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(&self.theme, AvatarThemePreference, "avatar");
        let (background, color) = if self.active {
            (theme.active_background, theme.active_color)
        } else {
            (theme.background, theme.color)
        };
        rect()
            .width(Size::px(self.size))
            .height(Size::px(self.size))
            .corner_radius(theme.corner_radius)
            .background(background)
            .center()
            .child(
                crate::components::typography::Caption::new(initials_of(&self.name)).color(color),
            )
    }
}

/// Two-letter initials for a name, splitting on `_ - space` (the Dioxus `initials_of`): a project
/// called `sales_daily` reads `SD`, a one-word one its first letter, and an empty one `?`.
fn initials_of(name: &str) -> String {
    let mut parts = name.split(['_', '-', ' ']).filter(|s| !s.is_empty());
    let a = parts.next().and_then(|s| s.chars().next());
    let b = parts.next().and_then(|s| s.chars().next());
    match (a, b) {
        (Some(a), Some(b)) => format!("{}{}", a.to_ascii_uppercase(), b.to_ascii_uppercase()),
        (Some(a), None) => a.to_ascii_uppercase().to_string(),
        _ => "?".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::initials_of;

    #[test]
    fn initials_split_on_word_separators() {
        assert_eq!(initials_of("sales_daily"), "SD");
        assert_eq!(initials_of("sales-daily"), "SD");
        assert_eq!(initials_of("sales daily"), "SD");
        assert_eq!(initials_of("a_b_c"), "AB");
        assert_eq!(initials_of("sample"), "S");
        assert_eq!(initials_of(""), "?");
        assert_eq!(initials_of("__"), "?");
    }
}
