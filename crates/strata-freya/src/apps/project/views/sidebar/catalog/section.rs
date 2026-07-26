//! A collapsible catalog section — `▾ TABLES · 4` over its rows.
//!
//! Hand-rolled rather than Freya's [`Accordion`]: that component
//! keeps its open flag internal *and* starts closed (`use_state(|| false)`), while every catalog
//! section opens by default and shows a live count in its own header. The collapse flag is
//! section-local — a way of looking, not project data, so it neither persists nor reaches a store.

use freya::prelude::*;

use super::CatalogTheme;
use crate::components::icon::{Icon, IconName};
use crate::components::typography::Eyebrow;

#[derive(PartialEq)]
pub struct CatalogSection {
    label: &'static str,
    count: usize,
    /// Drop the header's leading gap — the first section already sits under the scroll padding.
    first: bool,
    children: Vec<Element>,
    theme: CatalogTheme,
}

impl CatalogSection {
    pub fn new(label: &'static str, count: usize, theme: CatalogTheme) -> Self {
        Self {
            label,
            count,
            first: false,
            children: Vec::new(),
            theme,
        }
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

        let header = rect()
            .width(Size::fill())
            .horizontal()
            .cross_align(Alignment::Center)
            .spacing(8.)
            .padding(Gaps::new(top, 8., 8., 8.))
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
                Eyebrow::new(format!("{} · {}", self.label, self.count))
                    .color(self.theme.label_color),
            );

        rect()
            .width(Size::fill())
            .vertical()
            .child(header)
            .maybe(open(), |el| el.children(self.children.clone()))
    }
}
