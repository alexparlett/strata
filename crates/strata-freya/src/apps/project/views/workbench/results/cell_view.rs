//! The nested-cell value view (P2-12 / Dioxus U5): double-clicking a **nested**
//! (`struct` / `list` / `map`) grid cell opens its value as pretty JSON in a centred
//! backdrop modal — the canvas `cellViewOpen` comp. One of the grid's two double-click
//! targets: **cell → nested value** (here), **gutter → whole row** (P2-10).
//!
//! The open state ([`State<Option<CellValue>>`]) lives on the `DataGrid` (it survives
//! page flips like the column widths, and its `Command::Cancel` arm dismisses on Esc);
//! the value is **snapshotted at press time** — the canvas does the same, so a later
//! filter / page shift can't retarget an open modal. Every colour is a `cell_view`
//! component token; the card follows the `CloseConfirm` overlay idiom (overlay layer +
//! global position + backdrop press), plus the canvas's 3px backdrop blur.

use freya::components::{define_theme, get_theme, use_theme, ScrollView};
use freya::prelude::*;

use crate::components::divider::Divider;
use crate::components::icon::{Icon, IconName};
use crate::components::typography::{Meta, MonoValue, Readout};

define_theme!(
    %[component]
    pub CellView {
        %[fields]
        backdrop: Color,
        background: Color,
        border_fill: Color,
        divider_fill: Color,
        name_color: Color,
        badge_color: Color,
        badge_background: Color,
        close_color: Color,
        close_hover_background: Color,
        close_hover_color: Color,
        body_background: Color,
        body_color: Color,
    }
);

/// What the modal shows — the cell's column name, its dtype (the header badge), and the
/// value pretty-printed as JSON. Snapshotted when the double-click opens the view.
#[derive(Clone, PartialEq)]
pub struct CellValue {
    pub name: String,
    pub dtype: String,
    pub json: String,
}

/// Map a (possibly find-filtered) page row index back to its row in the **page batch**:
/// a filtered `GridData` keeps the unfiltered batch, and `row_nums` carries the
/// survivors' absolute 1-based gutter numbers (`row_base` + original position + 1).
pub fn page_batch_row(row_nums: Option<&[usize]>, row_base: usize, index: usize) -> usize {
    row_nums
        .and_then(|nums| nums.get(index).map(|abs| abs.saturating_sub(row_base + 1)))
        .unwrap_or(index)
}

/// The centred backdrop modal: 460px card (name + dtype badge + ghost close over a
/// scrollable mono JSON block). Backdrop press and the ✕ dismiss; Esc is arbitrated by
/// the grid root's `Command::Cancel` chain (the modal's ancestor in document order).
#[derive(PartialEq)]
pub struct CellView {
    value: CellValue,
    /// The grid's open slot — cleared to dismiss.
    open: State<Option<CellValue>>,
    pub(crate) theme: Option<CellViewThemePartial>,
}

impl CellView {
    pub fn new(value: CellValue, open: State<Option<CellValue>>) -> Self {
        Self { value, open, theme: None }
    }
}

impl Component for CellView {
    fn render(&self) -> impl IntoElement {
        let mut close_hover = use_state(|| false);
        let theme = get_theme!(&self.theme, CellViewThemePreference, "cell_view");
        let sheet = use_theme();
        let shadow = sheet.read().colors.shadow;
        let mut open = self.open;
        let close = move |_: Event<PressEventData>| open.set(None);

        // Header: cell name (mono 12.5) + the cyan dtype badge + ghost close.
        let header = rect()
            .width(Size::fill())
            .horizontal()
            .content(Content::Flex)
            .cross_align(Alignment::Center)
            .spacing(12.)
            .padding((12., 16.))
            .child(MonoValue::new(self.value.name.clone()).color(theme.name_color))
            .child(
                rect()
                    .corner_radius(4.)
                    .background(theme.badge_background)
                    .padding((2., 8.))
                    .child(Meta::new(self.value.dtype.clone()).color(theme.badge_color)),
            )
            .child(rect().width(Size::flex(1.)))
            .child(
                rect()
                    .width(Size::px(28.))
                    .height(Size::px(28.))
                    .corner_radius(6.)
                    .center()
                    .maybe(close_hover(), |el| el.background(theme.close_hover_background))
                    .on_pointer_enter(move |_| close_hover.set(true))
                    .on_pointer_leave(move |_| close_hover.set(false))
                    .on_press(close)
                    .child(Icon::new(IconName::Close).size(13.).color(if close_hover() {
                        theme.close_hover_color
                    } else {
                        theme.close_color
                    })),
            );

        // Body: the sunken mono JSON block. Hugs short values; long ones cap at ~2/3 of
        // the window (the canvas's 80vh card minus the header) and scroll. Wrapped rather
        // than panned sideways — the `explain_plan` raw-text idiom (a long string value
        // wraps instead of clipping).
        let body = ScrollView::new()
            .height(Size::auto())
            .max_height(Size::window_percent(66.))
            .child(
                rect()
                    .width(Size::fill())
                    .background(theme.body_background)
                    .padding((12., 16.))
                    .child(Readout::new(self.value.json.clone()).color(theme.body_color).wrap()),
            );

        let card = rect()
            .width(Size::px(460.))
            .max_width(Size::window_percent(92.))
            .corner_radius(14.)
            .background(theme.background)
            .border(Border::new().width(1.).fill(theme.border_fill))
            .shadow(Shadow::new().y(30.).blur(70.).color(shadow))
            .overflow(Overflow::Clip)
            .vertical()
            .child(header)
            .child(Divider::horizontal().color(theme.divider_fill))
            .child(body);

        // The overlay layer + global position lift the modal above the window content
        // (the `CloseConfirm` / `PopupBackground` wrapper), hand-rolled here for the
        // canvas's backdrop blur. The backdrop press closes; presses on the card land on
        // its own nodes and never reach the backdrop.
        rect()
            .layer(Layer::Overlay)
            .position(Position::new_global())
            .child(
                rect()
                    .position(Position::new_global().top(0.).left(0.))
                    .width(Size::window_percent(100.))
                    .height(Size::window_percent(100.))
                    .background(theme.backdrop)
                    .blur(3.)
                    .on_press(close),
            )
            .child(
                rect()
                    .position(Position::new_global().top(0.).left(0.))
                    .width(Size::window_percent(100.))
                    .height(Size::window_percent(100.))
                    .center()
                    .child(card),
            )
    }
}

#[cfg(test)]
mod interaction {
    use freya_testing::TestingRunner;

    use super::*;

    fn value() -> CellValue {
        CellValue {
            name: "attrs".into(),
            dtype: "Struct<plan, seats, regions, trial>".into(),
            json: "{\n  \"plan\": \"pro\",\n  \"seats\": 12,\n  \"regions\": [\n    \"us-east-1\",\n    \"eu-west-1\"\n  ],\n  \"trial\": false\n}".into(),
        }
    }

    fn app() -> impl IntoElement {
        use_init_theme(|| crate::theme::strata_theme(&strata_core::theme::load("midnight")));
        let open = use_consume::<State<Option<CellValue>>>();
        rect()
            .width(Size::fill())
            .height(Size::fill())
            .maybe_child(open.read().clone().map(|v| CellView::new(v, open)))
    }

    /// The two dismissal paths the acceptance names: a backdrop press closes; a press
    /// inside the card must **not** fall through to the backdrop and close.
    #[test]
    fn backdrop_dismisses_and_the_card_does_not() {
        let (mut runner, open) = TestingRunner::new(
            app,
            (900., 700.).into(),
            |r| r.provide_root_context(|| State::create(Some(value()))),
            1.,
        );
        runner.sync_and_update();
        runner.click_cursor((450., 350.)); // centre of the centred card
        runner.sync_and_update();
        assert!(open.peek().is_some(), "a press inside the card must not dismiss");
        runner.click_cursor((30., 30.)); // the backdrop
        runner.sync_and_update();
        assert!(open.peek().is_none(), "a backdrop press dismisses");
        // Reopen and dismiss via the ✕ (top-right of the header).
        let mut open = open;
        open.set(Some(value()));
        runner.sync_and_update();
        runner.click_cursor((650., 252.));
        runner.sync_and_update();
        assert!(open.peek().is_none(), "the close button dismisses");
    }

    /// Headless preview for eyeballing against the canvas `cellViewOpen` comp. Ignored by
    /// default (it writes a file, asserts nothing):
    /// `cargo test -p strata-freya cell_view_preview -- --ignored`.
    #[test]
    #[ignore = "writes target/cell-view-preview.png for eyeballing; run explicitly"]
    fn cell_view_preview() {
        let (mut runner, _) = TestingRunner::new(
            app,
            (900., 700.).into(),
            |r| r.provide_root_context(|| State::create(Some(value()))),
            1.,
        );
        runner.sync_and_update();
        runner.render_to_file(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../target/cell-view-preview.png"
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unfiltered_rows_map_straight_through() {
        assert_eq!(page_batch_row(None, 0, 3), 3);
        assert_eq!(page_batch_row(None, 200, 3), 3); // row_base only matters when filtered
    }

    #[test]
    fn filtered_rows_map_back_through_their_gutter_numbers() {
        // Page 2 of 100/page: survivors kept absolute numbers 101 and 103 → batch rows 0 and 2.
        let nums = vec![101, 103];
        assert_eq!(page_batch_row(Some(&nums), 100, 0), 0);
        assert_eq!(page_batch_row(Some(&nums), 100, 1), 2);
    }

    #[test]
    fn out_of_range_filtered_index_falls_back_to_position() {
        assert_eq!(page_batch_row(Some(&[101]), 100, 5), 5);
    }
}
