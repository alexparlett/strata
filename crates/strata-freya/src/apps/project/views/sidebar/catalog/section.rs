//! A collapsible catalog section — `▾ TABLES · 4` over its rows.
//!
//! Hand-rolled rather than Freya's [`Accordion`]: that component
//! keeps its open flag internal *and* starts closed (`use_state(|| false)`), while every catalog
//! section opens by default and shows a live count in its own header. The collapse flag is
//! section-local — a way of looking, not project data, so it neither persists nor reaches a store.

use freya::prelude::*;

use super::CatalogTheme;
use crate::components::icon::{Icon, IconName};
use crate::components::metrics::SP_3;
use crate::components::typography::Eyebrow;

#[derive(PartialEq)]
pub struct CatalogSection {
    label: &'static str,
    count: usize,
    /// Drop the header's leading gap — the first section already sits under the scroll padding.
    first: bool,
    /// A control at the header's trailing edge — TABLES' `+`. A **sibling** of the collapse
    /// press, never inside it: a built-in's press does not stop propagation, so a button nested
    /// in the pressable header would collapse the section on its way through.
    action: Option<Element>,
    children: Vec<Element>,
    theme: CatalogTheme,
}

impl CatalogSection {
    pub fn new(label: &'static str, count: usize, theme: CatalogTheme) -> Self {
        Self {
            label,
            count,
            first: false,
            action: None,
            children: Vec::new(),
            theme,
        }
    }

    /// A control at the header's trailing edge — see [`action`](Self::action).
    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.action = Some(action.into_element());
        self
    }

    pub fn first(mut self) -> Self {
        self.first = true;
        self
    }
}

impl ChildrenExt for CatalogSection {
    fn get_children(&mut self) -> &mut Vec<Element> {
        &mut self.children
    }
}

impl Component for CatalogSection {
    fn render(&self) -> impl IntoElement {
        let mut open = use_state(|| true);
        let top = if self.first { 4. } else { 12. };

        let title = rect()
            .width(Size::flex(1.))
            .horizontal()
            .content(Content::Flex)
            .overflow(Overflow::Clip)
            .cross_align(Alignment::Center)
            .spacing(SP_3)
            .on_press(move |_| {
                let now = *open.peek();
                open.set(!now);
            })
            .child(
                Icon::new(if open() {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                })
                .color(self.theme.chevron_color)
                .size(11.),
            )
            .child(
                rect().width(Size::flex(1.)).overflow(Overflow::Clip).child(
                    Eyebrow::new(format!("{} · {}", self.label, self.count))
                        .color(self.theme.label_color)
                        .text_overflow(TextOverflow::Ellipsis),
                ),
            );

        let header = rect()
            .width(Size::fill())
            .horizontal()
            .content(Content::Flex)
            .cross_align(Alignment::Center)
            .spacing(SP_3)
            .padding(Gaps::new(top, SP_3, SP_3, SP_3))
            .child(title)
            .maybe_child(self.action.clone());

        rect()
            .width(Size::fill())
            .vertical()
            .child(header)
            .maybe(open(), |el| el.children(self.children.clone()))
    }
}
