//! A hairline divider — the 1px rule that separates groups. One place for the pattern that was
//! otherwise re-inlined as a bare `rect` all over (tab strip, menus, toolbars). Horizontal (fills the
//! width) or vertical (fills the height); the cross-axis length, thickness, colour and margin are all
//! overridable, and the colour defaults to the `border` role.

use freya::prelude::*;

use crate::theme::{use_roles, Role as ThemeRole};

/// Which surface a rule belongs to, and so where its default colour comes from: the sheet's
/// `border` for a plain rule between regions, the menu card's own hairline for one inside a menu.
#[derive(PartialEq, Clone, Copy)]
enum Role {
    Plain,
    Menu,
}

/// A 1px separating rule. Build with [`Divider::horizontal`], [`Divider::vertical`] or
/// [`Divider::menu`].
#[derive(PartialEq)]
pub struct Divider {
    vertical: bool,
    length: Size,
    thickness: f32,
    color: Option<Color>,
    role: Role,
    margin: Gaps,
}

impl Divider {
    /// A horizontal rule: `thickness` tall, filling the available width.
    pub fn horizontal() -> Self {
        Self {
            vertical: false,
            length: Size::fill(),
            thickness: 1.,
            color: None,
            role: Role::Plain,
            margin: Gaps::new_all(0.),
        }
    }

    /// A vertical rule: `thickness` wide, filling the available height.
    pub fn vertical() -> Self {
        Self {
            vertical: true,
            length: Size::fill(),
            thickness: 1.,
            color: None,
            role: Role::Plain,
            margin: Gaps::new_all(0.),
        }
    }

    /// A **menu** separator — the rule that divides one group of menu rows from the next.
    ///
    /// A variant rather than a colour argument: the rule belongs to the menu card, so it takes
    /// its colour from that card's own `menu_container` theme (the same hairline as the card's
    /// border) instead of every menu picking one. Its length is `fill_minimum` because a menu
    /// container hugs its children — a plain `fill` would blow it out to the window width —
    /// plus a little vertical breathing room.
    pub fn menu() -> Self {
        Self {
            vertical: false,
            length: Size::fill_minimum(),
            thickness: 1.,
            color: None,
            role: Role::Menu,
            margin: Gaps::new_all(4.),
        }
    }

    /// Override the cross-axis extent (default: `fill`). Use `Size::px(18.)` for a short group rule,
    /// or `Size::fill_minimum()` inside a hug-content parent (e.g. a menu) where `fill` would blow the
    /// container out to its own parent's width.
    pub fn length(mut self, length: impl Into<Size>) -> Self {
        self.length = length.into();
        self
    }

    /// Override the line thickness (default `1.`).
    pub fn thickness(mut self, thickness: f32) -> Self {
        self.thickness = thickness;
        self
    }

    /// Paint the rule in `color` instead of its role's default — for a component painting a
    /// rule from its **own** theme (the datagrid's cell separators, the tab strip's edge).
    pub fn color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }

    /// Breathing room around the rule (e.g. vertical margin for a menu separator).
    pub fn margin(mut self, margin: impl Into<Gaps>) -> Self {
        self.margin = margin.into();
        self
    }
}

impl Component for Divider {
    fn render(&self) -> impl IntoElement {
        // Hooks run unconditionally and the fallback is chosen only *after* — a plain rule is
        // the `border` role's colour, a menu's is the menu card's hairline (so it reads on that
        // elevated surface in every theme, rather than each menu naming a colour).
        let sheet_border = use_roles().get(ThemeRole::Border);
        let menu_border = get_theme!(
            &None::<MenuContainerThemePartial>,
            MenuContainerThemePreference,
            "menu_container"
        )
        .border_fill;
        let color = self.color.unwrap_or(match self.role {
            Role::Plain => sheet_border,
            Role::Menu => menu_border,
        });
        let t = Size::px(self.thickness);
        let base = rect().margin(self.margin).background(color);
        // A fixed `px` thickness holds even in a `Content::Flex` parent, so no min is needed — and a
        // *min* on the thickness is actively wrong: it lets flex distribution grow the line (that's what
        // thickened the menu rules from 1px). The length axis fills (or whatever `length` set).
        if self.vertical {
            base.width(t).height(self.length.clone())
        } else {
            base.height(t).width(self.length.clone())
        }
    }
}
