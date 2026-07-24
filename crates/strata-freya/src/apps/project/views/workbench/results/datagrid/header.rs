//! The sticky header row ([`HeaderRow`]): the `#` corner + one [`HeaderCell`] per column —
//! name + dtype label + sort chevron, with column-select on mousedown and the accent-name
//! active state — each with its right-edge drag-to-resize grip ([`ColGrip`]).

use std::rc::Rc;

use freya::prelude::*;

use super::cell::Cell;
use super::model::KindColors;
use super::{
    DataGridTheme, GridData, DEFAULT_COL_W, EDGE_MARGIN, EDGE_STEP, GRIP_W, GUTTER_W, HEADER_H,
    MAX_COL_W, MIN_COL_W, TRAIL_W,
};
use crate::apps::project::views::workbench::results::selection::{CellRole, SelCtl};
use crate::apps::project::views::workbench::results::sort::SortState;
use crate::components::divider::Divider;
use crate::components::icon::{Icon, IconName};
use crate::components::typography::{Meta, MonoValue};

// Content auto-fit (double-click a resize grip): a char-count estimate, à la the Dioxus
// `col_autofit`.
const AUTOFIT_CHAR_W: f32 = 7.6; // mono char-width estimate
const AUTOFIT_PAD: f32 = 28.; // cell horizontal padding + affordance

/// Per-column content auto-fit width — `max(header name + 3, widest cell) × char-width +
/// padding`, clamped to the resize bounds. Recomputed per page (a grip double-click fits the
/// *visible* cells).
fn autofit_widths(data: &GridData) -> Vec<f32> {
    (0..data.columns.len())
        .map(|ci| {
            let mut max_len = data.columns[ci].name.chars().count() + 3;
            for row in &data.rows {
                if let Some(cell) = row.get(ci) {
                    max_len = max_len.max(cell.text.chars().count());
                }
            }
            (max_len as f32 * AUTOFIT_CHAR_W + AUTOFIT_PAD).clamp(MIN_COL_W, MAX_COL_W)
        })
        .collect()
}

/// The sticky header row: `#` gutter (select-all corner) + per-column [`HeaderCell`]s +
/// the trailing dead zone that keeps the last grip reachable. Pans horizontally with the
/// body (shared horizontal scroll) but not vertically (it sits above the scroll region).
#[derive(PartialEq)]
pub struct HeaderRow {
    /// The resolved page — column names/types, and the cells the auto-fit measures.
    pub data: Rc<GridData>,
    pub widths: State<Vec<f32>>,
    pub controller: ScrollController,
    pub viewport: State<Area>,
    pub hold_w: State<f32>,
    pub sel: SelCtl,
    pub sort: SortState,
    pub theme: DataGridTheme,
}

impl Component for HeaderRow {
    fn render(&self) -> impl IntoElement {
        let theme = &self.theme;
        // Per-column content auto-fit widths (grip double-click), from this page's cells.
        let autofit = autofit_widths(&self.data);
        let mut header = rect()
            .width(Size::fill())
            .height(Size::px(HEADER_H))
            .direction(Direction::Horizontal)
            .content(Content::Flex)
            .background(theme.header_background)
            .child(Cell {
                width: Size::px(GUTTER_W),
                text: "#".to_string(),
                color: theme.header_label_color,
                mono: false,
                cross: Alignment::Center,
                pad: Gaps::default(),
                hover_bg: theme.header_hover_background,
                divider: theme.header_divider_fill,
                role: CellRole::Corner,
                sel: self.sel,
                sel_border: theme.selection_border_fill,
                active_color: None,
                active_background: None,
                on_open: None,
                on_secondary: None,
            });
        for (ci, col) in self.data.columns.iter().enumerate() {
            let w = self.widths.read().get(ci).copied().unwrap_or(DEFAULT_COL_W);
            header = header.child(HeaderCell {
                index: ci,
                name: col.name.clone(),
                dtype: col.dtype.clone(),
                w,
                widths: self.widths,
                controller: self.controller,
                viewport: self.viewport,
                hold_w: self.hold_w,
                sel: self.sel,
                sort: self.sort,
                name_color: theme.header_color,
                active_color: theme.header_active_color,
                type_color: col.kind.type_color(theme),
                arrow: theme.arrow_fill,
                divider: theme.header_divider_fill,
                grip: theme.selection_border_fill,
                hover_bg: theme.header_hover_background,
                active_bg: theme.header_active_background,
                autofit_w: autofit.get(ci).copied().unwrap_or(DEFAULT_COL_W),
            });
        }
        // Trailing dead space: keeps the last column's resize grip clear of the content's
        // right edge so it stays reachable, and gives somewhere to drag when widening the
        // last column.
        header
            .child(rect().width(Size::flex(1.)).min_width(Size::px(TRAIL_W)).height(Size::fill()))
    }
}

/// One header column: name + sort chevron on top, dtype label below, a trailing column rule, and an
/// absolutely-positioned [`ColGrip`] on the right edge for drag-to-resize. Owns its hover locally, like
/// the body [`Cell`](super::cell::Cell)s.
#[derive(PartialEq)]
pub struct HeaderCell {
    pub index: usize,
    pub name: String,
    pub dtype: String,
    pub w: f32,
    pub widths: State<Vec<f32>>,
    pub controller: ScrollController,
    pub viewport: State<Area>,
    pub hold_w: State<f32>,
    pub sel: SelCtl,
    pub sort: SortState,
    pub name_color: Color,
    pub active_color: Color,
    pub type_color: Color,
    pub arrow: Color,
    pub divider: Color,
    pub grip: Color,
    pub hover_bg: Color,
    pub active_bg: Color,
    /// Width to snap to on a grip double-click (content auto-fit).
    pub autofit_w: f32,
}

impl Component for HeaderCell {
    fn render(&self) -> impl IntoElement {
        let mut hovered = use_state(|| false);
        let sel = self.sel;
        let index = self.index;
        // Read the selection reactively so this header re-renders when any column selection changes
        // (activating this one *and* deactivating the previously-selected one).
        let active = sel.sel.read().cols().contains(&index);
        let name_color = if active { self.active_color } else { self.name_color };
        // The sort chevron (Rz6): up = asc, down = desc / unsorted. Unsorted is invisible
        // until the header is hovered (the comp's faint-on-hover reveal — the button stays
        // mounted so the name row's layout never shifts); the sorted column's stays lit in
        // the accent. A press cycles asc → desc → clear; `stop_propagation` on the down so
        // grabbing it never selects the column (the Dioxus `col-sort` contract).
        let sort = self.sort;
        let dir = sort.dir(index);
        let sort_icon = if dir == Some(true) { IconName::ChevronUp } else { IconName::ChevronDown };
        let sort_color = if dir.is_some() {
            self.active_color
        } else if hovered() {
            self.arrow
        } else {
            Color::TRANSPARENT
        };
        rect()
            .width(Size::px(self.w))
            .height(Size::px(HEADER_H))
            .direction(Direction::Horizontal)
            .content(Content::Flex)
            // Selected column → accent name + active background; the column's *body* cells carry the
            // selection fill. Hover still shows on an unselected column.
            .maybe(active, |el| el.background(self.active_bg))
            .maybe(hovered(), |el| el.background(self.hover_bg))
            .on_pointer_down(move |e: Event<PointerEventData>| {
                if !e.data().is_primary() {
                    return;
                }
                e.stop_propagation();
                sel.col(index);
            })
            .on_pointer_enter(move |_| hovered.set(true))
            .on_pointer_leave(move |_| hovered.set(false))
            .child(
                rect()
                    .width(Size::flex(1.))
                    .height(Size::fill())
                    .main_align(Alignment::Center)
                    .cross_align(Alignment::Start)
                    .padding(Gaps::new(8., 12., 8., 12.))
                    .spacing(2.)
                    .overflow(Overflow::Clip)
                    .child(
                        rect()
                            .width(Size::fill())
                            .direction(Direction::Horizontal)
                            .main_align(Alignment::SpaceBetween)
                            .cross_align(Alignment::Center)
                            .child(MonoValue::new(self.name.clone()).color(name_color).max_lines(1))
                            .child(
                                TooltipContainer::new(Tooltip::new("Sort by this column"))
                                    .position(AttachedPosition::Bottom)
                                    .child(
                                        Button::new()
                                            .flat()
                                            .width(Size::px(16.))
                                            .height(Size::px(16.))
                                            .on_pointer_down(|e: Event<PointerEventData>| {
                                                e.stop_propagation();
                                            })
                                            .on_press(move |_| sort.cycle(index))
                                            .child(Icon::new(sort_icon).size(11.).color(sort_color)),
                                    ),
                            ),
                    )
                    .child(Meta::new(self.dtype.clone()).color(self.type_color)),
            )
            .child(Divider::vertical().color(self.divider))
            .child(ColGrip {
                index: self.index,
                widths: self.widths,
                controller: self.controller,
                viewport: self.viewport,
                hold_w: self.hold_w,
                accent: self.grip,
                autofit_w: self.autofit_w,
            })
    }
}

/// The right-edge column-resize grip — a 6px hot-zone, absolutely positioned over the column's right
/// rule, showing a 2px accent line while hovered/dragging. Owns its drag in local state and writes the
/// shared `widths` on move (Freya's global-pointer-capture pattern: down → track, global-move →
/// apply, global-press → release), so every cell in the column reflows in lockstep. No shared drag
/// state, mirroring the Dioxus per-grip `ColGrip`.
#[derive(PartialEq)]
struct ColGrip {
    index: usize,
    widths: State<Vec<f32>>,
    /// The outer horizontal scroll — auto-scrolled while the drag nears a viewport edge so a column
    /// can grow past the visible right edge.
    controller: ScrollController,
    /// The grid viewport in screen coords, for edge detection.
    viewport: State<Area>,
    /// Shared content-width hold: pinned to the drag high-water mark while resizing so the scroll
    /// extent can't shrink mid-drag (0 = not resizing).
    hold_w: State<f32>,
    accent: Color,
    /// Width to snap to on a double-click (content auto-fit), computed once from the data.
    autofit_w: f32,
}

impl Component for ColGrip {
    fn render(&self) -> impl IntoElement {
        let index = self.index;
        let mut widths = self.widths;
        let accent = self.accent;
        let controller = self.controller;
        let viewport = self.viewport;
        let mut hold_w = self.hold_w;
        let autofit_w = self.autofit_w;

        let mut clicking = use_state(|| false);
        let mut hovering = use_state(|| false);
        let mut origin_x = use_state(|| 0.0f32);
        let mut start_w = use_state(|| 0.0f32);
        // Auto-scroll we've *intentionally* applied during this drag (px) — tracked here rather than
        // read live from the controller, so shrinking (which clamps the scroll offset) can't feed back
        // into the width and make the drag jerky.
        let mut scroll_accum = use_state(|| 0.0f32);

        let on_pointer_enter = move |_| {
            hovering.set(true);
            Cursor::set(CursorIcon::ColResize);
        };
        let on_pointer_leave = move |_| {
            hovering.set(false);
            if !clicking() {
                Cursor::set(CursorIcon::default());
            }
        };
        let on_pointer_down = move |e: Event<PointerEventData>| {
            if !e.data().is_primary() {
                return;
            }
            e.stop_propagation();
            e.prevent_default();
            // Double-click → auto-fit the column to its content, and don't start a resize drag. (Checked
            // here, not via `on_press`, because the `prevent_default` above suppresses the press event.)
            if EventsCombos::pressed(e.global_location()).is_double() {
                if let Some(slot) = widths.write().get_mut(index) {
                    *slot = autofit_w;
                }
                return;
            }
            origin_x.set(e.global_location().x as f32);
            start_w.set(widths.read().get(index).copied().unwrap_or(DEFAULT_COL_W));
            scroll_accum.set(0.0);
            // Freeze the content width at the current natural span for the duration of the drag.
            hold_w.set(GUTTER_W + widths.read().iter().sum::<f32>() + TRAIL_W);
            clicking.set(true);
        };
        let on_capture_global_pointer_move = move |e: Event<PointerEventData>| {
            if !clicking() {
                return;
            }
            e.prevent_default();
            let x = e.global_location().x as f32;
            // Width follows the cursor plus the scroll we've intentionally auto-scrolled this drag
            // (`scroll_accum`) — never the live controller offset. Reading the live offset made dragging
            // left jerky: narrowing shrinks the content, the scroll view clamps its offset, and that
            // clamp fed back into the width. The accumulator only moves when *we* edge-scroll below.
            let new = (start_w() + (x - origin_x()) + scroll_accum()).clamp(MIN_COL_W, MAX_COL_W);
            if let Some(slot) = widths.write().get_mut(index) {
                *slot = new;
            }
            // Raise the held width to the drag's high-water mark, so shrinking a column doesn't shrink
            // the scroll extent mid-drag (the trailing flex grows to fill the difference instead).
            let nat = GUTTER_W + widths.read().iter().sum::<f32>() + TRAIL_W;
            if nat > hold_w() {
                hold_w.set(nat);
            }
            // Auto-scroll when the cursor nears a viewport edge (Freya's scroll-x grows to reveal
            // *earlier* content, so nudge it down at the right edge). Each nudge shifts the content by
            // `EDGE_STEP`, so fold that back into the width via the accumulator (`accum -= step`).
            let vp = *viewport.peek();
            let step = if x > vp.max_x() as f32 - EDGE_MARGIN {
                -EDGE_STEP
            } else if x < vp.min_x() as f32 + EDGE_MARGIN {
                EDGE_STEP
            } else {
                0.
            };
            if step != 0. {
                let (sx, _): (i32, i32) = controller.into();
                let mut c = controller;
                c.scroll_to_x(sx + step as i32);
                scroll_accum.set(scroll_accum() - step);
            }
        };
        let on_global_pointer_press = move |_: Event<PointerEventData>| {
            if clicking() {
                clicking.set(false);
                // Release the hold: the content settles from the frozen high-water back to the live span.
                hold_w.set(0.0);
                if !hovering() {
                    Cursor::set(CursorIcon::default());
                }
            }
        };
        let lit = hovering() || clicking();
        rect()
            .position(Position::new_absolute().top(0.).right(0.))
            .width(Size::px(GRIP_W))
            .height(Size::px(HEADER_H))
            .cross_align(Alignment::End)
            .on_pointer_enter(on_pointer_enter)
            .on_pointer_leave(on_pointer_leave)
            .on_pointer_down(on_pointer_down)
            .on_capture_global_pointer_move(on_capture_global_pointer_move)
            .on_global_pointer_press(on_global_pointer_press)
            .maybe(lit, |el| {
                el.child(
                    rect()
                        .width(Size::px(2.))
                        .height(Size::fill())
                        .background(accent),
                )
            })
    }
}
