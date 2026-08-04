//! One result row, and the heading above a group of them.
//!
//! A row is a fixed shape whatever it points at (canvas: 42px, `gap: var(--sp-4)`): a glyph, the
//! name, then whatever the row has to say for itself at the far end — a mono detail, a shortcut
//! hint, and the `↵` that marks the one Enter will run. What varies is only the glyph and the
//! tone.
//!
//! **The active row is the one the pointer is over**, because hover and ↑↓ write the same slot
//! (the canvas's `onCmdkHover`). That is what makes a palette answer to the mouse and the keyboard
//! at once instead of carrying two selections that disagree.

use freya::prelude::*;
use strata_model::CatalogKind;

use super::model::{Entry, Group};
use super::palette_theme;
use crate::components::dot::Dot;
use crate::components::icon::{Icon, IconName};
use crate::components::keycap::KeyCap;
use crate::components::type_palette::{kind_color, type_palette};
use crate::components::typography::{Eyebrow, MonoValue, Prose};
use crate::theme::{use_roles, Role};

/// The canvas's row box: 42px tall, `--sp-4` inside and between, `--r-2` corners.
const ROW_HEIGHT: f32 = 42.;
const ROW_INSET: f32 = 12.;
const ROW_GAP: f32 = 12.;
const ROW_RADIUS: f32 = 8.;
/// The glyph slot — one width whether it holds an icon or a column's type swatch, so every row's
/// name starts at the same x.
const GLYPH_SLOT: f32 = 15.;
const GLYPH_SIZE: f32 = 15.;
/// A column's type swatch, matching the catalog's own column rows.
const SWATCH: f32 = 6.;
/// The mono detail's ceiling (canvas `max-width: 260px`) — it is the row's second fact, so it
/// gives way to the name rather than competing with it.
const SUB_MAX_WIDTH: f32 = 260.;
/// A group heading's box (canvas `padding: var(--sp-4) var(--sp-4) var(--sp-2)`).
const HEAD_TOP: f32 = 12.;
const HEAD_BOTTOM: f32 = 4.;

/// A section heading — mono small-caps, and nothing else. It is not a row: ↑↓ step over it.
#[derive(PartialEq)]
pub struct GroupHead {
    pub group: Group,
}

impl Component for GroupHead {
    fn render(&self) -> impl IntoElement {
        let theme = palette_theme();
        rect()
            .padding(Gaps::new(HEAD_TOP, ROW_INSET, HEAD_BOTTOM, ROW_INSET))
            .child(Eyebrow::new(self.group.title()).color(theme.label_color))
    }
}

#[derive(PartialEq)]
pub struct PaletteRow {
    pub entry: Entry,
    /// This row's place in the flat list — what ↑↓ move along, and what a press reports back.
    pub index: usize,
    pub active: bool,
    /// The chord the entry answers to, already resolved against the live settings. Passed in
    /// rather than read here so one subscription serves every row (and so a row stays a pure
    /// function of what it was handed).
    pub hint: Option<String>,
    /// Where hover writes the active row.
    pub set_active: EventHandler<usize>,
    /// Run this row.
    pub run: EventHandler<usize>,
}

impl Component for PaletteRow {
    fn render(&self) -> impl IntoElement {
        let theme = palette_theme();
        let accent = use_roles().get(Role::Accent);
        let types = type_palette();

        let (label_color, icon_color) = match self.active {
            true => (theme.row_active_color, accent),
            false => (theme.row_color, theme.icon_color),
        };

        // A column's glyph is its **type** swatch, in the type ramp whether or not the row is
        // active — the colour is a fact about the column, not a selection state (the catalog's
        // own column rows carry the same dot). Everything else takes the row's tone, and its
        // mark from the catalog: the palette lists what the sidebar lists, so it marks them the
        // same way.
        let icon = |name| {
            Icon::new(name)
                .size(GLYPH_SIZE)
                .color(icon_color)
                .into_element()
        };
        let glyph = match &self.entry {
            Entry::Column { kind, .. } => Dot::new(kind_color(*kind, &types))
                .size(SWATCH)
                .square()
                .into_element(),
            Entry::Action(action) => icon(action.route().icon),
            Entry::Table { .. } => icon(IconName::for_catalog(CatalogKind::Table)),
            Entry::View { .. } => icon(IconName::for_catalog(CatalogKind::View)),
            Entry::Query { .. } => icon(IconName::for_catalog(CatalogKind::Query)),
        };

        let sub = self.entry.sub();
        let index = self.index;
        let (set_active, run) = (self.set_active.clone(), self.run.clone());

        rect()
            .height(Size::px(ROW_HEIGHT))
            .width(Size::fill())
            .horizontal()
            .cross_align(Alignment::Center)
            .spacing(ROW_GAP)
            .padding(Gaps::new(0., ROW_INSET, 0., ROW_INSET))
            .corner_radius(ROW_RADIUS)
            .background(match self.active {
                true => theme.row_active_background,
                false => Color::TRANSPARENT,
            })
            .on_pointer_enter(move |_| set_active.call(index))
            .on_press(move |_| run.call(index))
            .child(
                rect()
                    .width(Size::px(GLYPH_SLOT))
                    .cross_align(Alignment::Center)
                    .main_align(Alignment::Center)
                    .child(glyph),
            )
            // The name takes the slack and truncates; everything after it keeps its own width,
            // so a long table name never pushes the shortcut hint off the card.
            .child(
                Prose::new(self.entry.label())
                    .color(label_color)
                    .width(Size::flex(1.))
                    .text_overflow(TextOverflow::Ellipsis),
            )
            .maybe_child((!sub.is_empty()).then(|| {
                MonoValue::new(sub)
                    .color(theme.sub_color)
                    .max_width(Size::px(SUB_MAX_WIDTH))
                    .text_overflow(TextOverflow::Ellipsis)
                    .into_element()
            }))
            .maybe_child(
                self.hint
                    .as_ref()
                    .filter(|hint| !hint.is_empty())
                    .map(|hint| KeyCap::chip(hint.clone()).into_element()),
            )
            // The `↵` marks what Enter will do, so it belongs to the active row alone.
            .maybe_child(
                self.active
                    .then(|| MonoValue::new("\u{21b5}").color(accent).into_element()),
            )
    }
}
