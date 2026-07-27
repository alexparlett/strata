//! The list a form's rows sit in — the one place the rhythm between them is spelled out.

use freya::prelude::*;

use crate::components::divider::Divider;
use crate::components::form::{form_theme, ROW_GAP, SETTING_GAP};

/// How a list separates its rows — see the module doc's "known divergences".
#[derive(PartialEq, Clone, Copy, Default, Debug)]
enum Variant {
    /// A gap, as the export window and the config modal draw their forms.
    #[default]
    Spaced,
    /// A hairline with breathing room either side, as the Settings panes draw theirs. The
    /// design's Settings consistency pass settled on this ("divider rules separate settings"),
    /// so it is that window's list and not a decoration.
    Divided,
}

/// A form's rows, in order, with the separation between them drawn here rather than by each
/// surface.
#[derive(PartialEq)]
pub struct FormList {
    children: Vec<Element>,
    variant: Variant,
}

impl Default for FormList {
    fn default() -> Self {
        Self::new()
    }
}

impl FormList {
    pub fn new() -> Self {
        Self {
            children: Vec::new(),
            variant: Variant::default(),
        }
    }

    /// Separate the rows with a rule instead of a gap (see [`Variant::Divided`]).
    pub fn divided(mut self) -> Self {
        self.variant = Variant::Divided;
        self
    }
}

impl ChildrenExt for FormList {
    fn get_children(&mut self) -> &mut Vec<Element> {
        &mut self.children
    }
}

impl Component for FormList {
    fn render(&self) -> impl IntoElement {
        let theme = form_theme();

        // Spelled out rather than set as `spacing`, because the divided variant's separator is
        // three children (gap, rule, gap) and the two variants should read as one loop.
        let mut list = rect().width(Size::fill()).vertical();
        for (i, row) in self.children.iter().enumerate() {
            if i > 0 {
                list = match self.variant {
                    Variant::Spaced => list.child(rect().height(Size::px(ROW_GAP))),
                    Variant::Divided => list
                        .child(rect().height(Size::px(SETTING_GAP)))
                        .child(Divider::horizontal().color(theme.divider_fill))
                        .child(rect().height(Size::px(SETTING_GAP))),
                };
            }
            list = list.child(row.clone());
        }
        list
    }
}
