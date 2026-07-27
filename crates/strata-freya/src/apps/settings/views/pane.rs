//! The **category pane**: the scrolling frame every category's content sits in, and the
//! breadcrumb over it.
//!
//! The breadcrumb is drawn here rather than by each page because it is pure route metadata —
//! group label, then category label, straight off [`category`]. A page that spelled its own
//! out would be a second copy of the nav tree, free to drift from the row that navigated to
//! it.
//!
//! [`Pane::not_built`] is what a category renders until its own task lands (P4-04…P4-08). It
//! is deliberately plain: the shell is what P4-03 delivers, and dressing an empty page up
//! would misrepresent it.

use freya::prelude::*;
use freya::router::*;

use crate::apps::settings::{category, Route, SettingsThemePartial, SettingsThemePreference};
use crate::components::typography::{Control, Prose};

/// The pane's inset (canvas `padding: var(--sp-6)`).
const PANE_PADDING: Gaps = Gaps::new(24., 24., 24., 24.);

/// The gap under the breadcrumb (canvas `margin-bottom: var(--sp-6)`).
const BREADCRUMB_GAP: f32 = 24.;

/// The frame around a category's content — the breadcrumb, then whatever the page renders,
/// scrolling together.
///
/// Two opt-outs, both taken by the Engine pane (P4-07) and by nothing else. It is the one
/// category that is a *surface* rather than a list of settings: an action on the breadcrumb line
/// ([`trailing`](Self::trailing)) and a table that takes whatever height is left rather than a
/// column that grows past the window ([`filled`](Self::filled)). They live here rather than being
/// bypassed so the other four keep one frame between them, and so a fifth page wanting either
/// finds it already named.
#[derive(PartialEq)]
pub struct Pane {
    content: Element,
    /// An action at the breadcrumb's end, on its line.
    trailing: Option<Element>,
    /// Fill the pane instead of scrolling in it — for content that manages its own height.
    filled: bool,
}

impl Pane {
    /// A built category's content, in the frame.
    pub fn new(content: impl IntoElement) -> Self {
        Self {
            content: content.into_element(),
            trailing: None,
            filled: false,
        }
    }

    /// A category whose page belongs to a task that hasn't landed. `what` names the content,
    /// `owner` the task that brings it.
    pub fn not_built(what: &str, owner: &str) -> Self {
        Self::new(Prose::new(format!("{what} is not built yet ({owner}).")))
    }

    /// Put an action at the end of the breadcrumb line.
    pub fn maybe_trailing(mut self, trailing: Option<impl IntoElement>) -> Self {
        self.trailing = trailing.map(IntoElement::into_element);
        self
    }

    /// Give the content the pane's full height instead of scrolling it. The content is then
    /// responsible for its own overflow — which is the point: a table with a pinned header
    /// scrolls its body, not itself.
    pub fn filled(mut self) -> Self {
        self.filled = true;
        self
    }
}

impl Component for Pane {
    fn render(&self) -> impl IntoElement {
        let route = use_route::<Route>();

        let body = rect()
            .width(Size::fill())
            .vertical()
            .padding(PANE_PADDING)
            .maybe(self.filled, |el| {
                el.height(Size::fill()).content(Content::Flex)
            })
            .child(
                rect()
                    .width(Size::fill())
                    .horizontal()
                    .content(Content::Flex)
                    .cross_align(Alignment::Center)
                    .child(rect().width(Size::flex(1.)).child(Breadcrumb { route }))
                    .maybe_child(self.trailing.clone()),
            )
            .child(rect().height(Size::px(BREADCRUMB_GAP)))
            .child(self.content.clone());

        match self.filled {
            true => rect()
                .width(Size::flex(1.))
                .height(Size::fill())
                .child(body)
                .into_element(),
            false => ScrollView::new()
                .width(Size::flex(1.))
                .height(Size::fill())
                .child(body)
                .into_element(),
        }
    }
}

/// `Appearance & behaviour › Theme` — or just the label, for a category with no group.
#[derive(PartialEq)]
struct Breadcrumb {
    route: Route,
}

impl Component for Breadcrumb {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(
            &None::<SettingsThemePartial>,
            SettingsThemePreference,
            "settings"
        );
        // Every route has a category (`model`'s test pins that); a page with none would be
        // unreachable, so there is nothing sensible to draw for it.
        let Some(category) = category(&self.route) else {
            return rect();
        };
        let (group, label) = category.breadcrumb();

        rect()
            .horizontal()
            .cross_align(Alignment::Center)
            .spacing(6.)
            .map(group, |el, group| {
                el.child(Control::new(group).color(theme.hint_color))
                    .child(Control::new("\u{203a}").color(theme.chevron_color))
            })
            .child(Control::new(label).color(theme.item_active_color))
    }
}
