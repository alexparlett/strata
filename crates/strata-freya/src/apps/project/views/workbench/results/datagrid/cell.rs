//! One grid cell — a hoverable, selectable fixed-width slot. Used for body data cells, the row-number
//! gutter, and the `#` corner (not the trailing filler). It owns its hover state **locally** and reads
//! the selection **reactively**, so only the cell under the cursor / in the selection repaints — no
//! grid-wide re-render, and it updates even inside the memoized virtual scroller. The trailing 1px
//! column rule lives *inside* the slot width, so header + body slots with the same width stay aligned.

use freya::prelude::*;

use crate::apps::project::views::workbench::results::selection::{
    cell_sel_style, CellRole, SelCtl, SelStyle,
};
use crate::components::divider::Divider;
use crate::components::typography::{Meta, MonoValue};

#[derive(PartialEq)]
pub struct Cell {
    pub width: Size,
    pub text: String,
    pub color: Color,
    /// `MonoValue` for data cells, `Meta` for the gutter / `#`.
    pub mono: bool,
    /// Horizontal alignment of the text (Start for data, Center for the gutter).
    pub cross: Alignment,
    pub pad: Gaps,
    pub hover_bg: Color,
    pub divider: Color,
    /// Which selection interaction this cell drives on mousedown.
    pub role: CellRole,
    /// Shared selection controller — read reactively for this cell's styling, and mutated on interaction.
    pub sel: SelCtl,
    /// The 2px edge-ring colour (`selection_border_fill`).
    pub sel_border: Color,
    /// Text colour when active (the gutter swaps to `gutter_active_color`); `None` keeps `color`.
    pub active_color: Option<Color>,
    /// Fill colour when active (the gutter's `gutter_active_background`); `None` = no fill.
    pub active_background: Option<Color>,
    /// Double-click → the nested-cell view (P2-12). `Some` only for a data cell whose column
    /// is nested (`struct`/`list`/`map`) and whose value is non-null; the grid builds the
    /// handler (it owns the batch + the open state). The standard optional event-prop shape
    /// (`Button::on_press`), fed the double-click's pointer event.
    pub on_nested_open: Option<EventHandler<Event<PointerEventData>>>,
}

impl Component for Cell {
    fn render(&self) -> impl IntoElement {
        let mut hovered = use_state(|| false);
        let sel = self.sel;
        let role = self.role;
        // Read the selection reactively so this cell re-renders on any change — even though it lives
        // inside the memoized virtual scroller (whose builder never re-runs). Compute *this* cell's
        // styling from its role.
        let selection = sel.sel.read();
        let ss = match role {
            CellRole::Data(r, c) => cell_sel_style(
                selection.cell_bounds(),
                selection.rows(),
                selection.cols(),
                r,
                c,
                sel.nrows.saturating_sub(1),
                sel.ncols.saturating_sub(1),
            ),
            _ => SelStyle::default(),
        };
        let active = match role {
            CellRole::Row(r) => selection.rows().contains(&r),
            _ => false,
        };

        let text = self.text.clone();
        let text_color = if active { self.active_color.unwrap_or(self.color) } else { self.color };
        let label: Element = if self.mono {
            MonoValue::new(text).color(text_color).max_lines(1).into()
        } else {
            Meta::new(text).color(text_color).into()
        };

        // 2px accent ring on whichever outer edges this cell sits on (invisible when all zero).
        let border = Border::new()
            .fill(self.sel_border)
            .alignment(BorderAlignment::Inner)
            .width(BorderWidth {
                top: if ss.top { 2. } else { 0. },
                right: if ss.right { 2. } else { 0. },
                bottom: if ss.bot { 2. } else { 0. },
                left: if ss.left { 2. } else { 0. },
            });

        rect()
            .width(self.width.clone())
            .height(Size::fill())
            .direction(Direction::Horizontal)
            .content(Content::Flex)
            .maybe(active, |el| el.background(self.active_background.unwrap_or(Color::TRANSPARENT)))
            .maybe(hovered(), |el| el.background(self.hover_bg))
            .border(border)
            .on_pointer_down({
                let on_nested_open = self.on_nested_open.clone();
                move |e: Event<PointerEventData>| {
                    if !e.data().is_primary() {
                        return;
                    }
                    // Consume so the grid-background handler doesn't treat this as a click-to-deselect.
                    e.stop_propagation();
                    match role {
                        CellRole::Data(r, c) => sel.cell_down(r, c),
                        CellRole::Row(r) => sel.row(r),
                        CellRole::Corner => sel.all(),
                        CellRole::None => {}
                    }
                    // Double-click on a nested cell opens its value view. Detected here — inside
                    // the same handler as the single-click selection (à la the resize grip), so
                    // the first press of the pair still selects and the second just opens.
                    if let Some(open) = &on_nested_open {
                        if EventsCombos::pressed(e.global_location()).is_double() {
                            open.call(e);
                        }
                    }
                }
            })
            .on_pointer_enter(move |_| {
                hovered.set(true);
                if let CellRole::Data(r, c) = role {
                    sel.cell_paint(r, c); // drag-paint (no-op unless a drag is active)
                }
            })
            .on_pointer_leave(move |_| hovered.set(false))
            .child(
                rect()
                    .width(Size::flex(1.))
                    .height(Size::fill())
                    // Vertical centring only; the vertical padding lives in the row height so a single
                    // line can't be clipped by the fixed row box.
                    .main_align(Alignment::Center)
                    .cross_align(self.cross.clone())
                    .padding(self.pad)
                    .overflow(Overflow::Clip)
                    .child(label),
            )
            .child(Divider::vertical().color(self.divider))
    }
}
