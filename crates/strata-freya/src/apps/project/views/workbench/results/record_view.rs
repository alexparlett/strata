//! The **record view** (P2-10 / Dioxus Rz5): double-clicking the row-number gutter opens the
//! **entire row** in a centred backdrop modal — the canvas `rowViewOpen` comp. The grid's other
//! double-click target (cell → nested value) is the sibling `cell_view` (P2-12), whose overlay
//! idiom (overlay layer + global position + backdrop press + 3px blur) this card shares.
//!
//! The open state ([`State<Option<usize>>`], a **page row index**) lives on the `DataGrid` —
//! beside the nested-cell slot, so it survives page flips and its `Command::Cancel` arm
//! dismisses on Esc. Unlike the nested-cell view the record view is **live**, not a snapshot:
//! prev/next re-point the index within the page (clamped at its edges, per the task), and the
//! body always renders the current page's row — the canvas's `rowAt(gi)` semantics.
//!
//! Header per the canvas: `Row n of total` + **Copy row as JSON** / **Copy row as CSV** +
//! divider + prev / next + ghost close. The copy buttons are **inert until P2-11** lands the
//! shared results-copy path (selection → core serializers → clipboard); they then route
//! through it (`serialize::row_pretty_json` for the row object, `serialize::write_selection`
//! for the header+row CSV) rather than growing a local clipboard wiring here.

use std::rc::Rc;

use freya::components::{define_theme, get_theme, use_theme, Button, ScrollView};
use freya::prelude::*;

use strata_core::engine::serialize::cell_pretty_json;
use strata_model::Kind;

use super::cell_view::page_batch_row;
use super::datagrid::GridData;
use super::status_bar::fmt_int;
use crate::components::divider::Divider;
use crate::components::icon::{Icon, IconName};
use crate::components::typography::{Meta, MonoValue, Path, Readout};

define_theme!(
    %[component]
    pub RecordView {
        %[fields]
        backdrop: Color,
        background: Color,
        border_fill: Color,
        divider_fill: Color,
        row_divider_fill: Color,
        label_color: Color,
        name_color: Color,
        value_color: Color,
        null_color: Color,
        nested_background: Color,
        nested_color: Color,
        type_str_color: Color,
        type_num_color: Color,
        type_bool_color: Color,
        type_ts_color: Color,
        type_struct_color: Color,
        type_list_color: Color,
        type_map_color: Color,
    }
);

/// The dtype label colour for a field's left column — the same categorical palette the
/// datagrid header wears, carried as this component's own `type_*_color` tokens (the
/// `explain_plan` precedent: no cross-component theme reads).
fn kind_color(kind: Kind, t: &RecordViewTheme) -> Color {
    match kind {
        Kind::Str => t.type_str_color,
        Kind::Num => t.type_num_color,
        Kind::Bool => t.type_bool_color,
        Kind::Ts => t.type_ts_color,
        Kind::Struct => t.type_struct_color,
        Kind::List => t.type_list_color,
        Kind::Map => t.type_map_color,
    }
}

/// The centred backdrop modal: 540px card — header (label + copy JSON/CSV + prev/next + ghost
/// close) over a scrollable field list, one row per column (150px name + dtype gutter, then a
/// nested pretty-JSON block or a scalar value run). Backdrop press and the ✕ dismiss; Esc is
/// arbitrated by the grid root's `Command::Cancel` chain.
#[derive(PartialEq)]
pub struct RecordView {
    /// The row shown, as a **page row index** (display order — a find-filtered page indexes
    /// its survivors). The caller clamps it into the page.
    row: usize,
    /// The grid's open slot — prev/next re-point it, dismissal clears it.
    open: State<Option<usize>>,
    /// The resolved current page (schema + formatted cells + the typed batch).
    data: Rc<GridData>,
    /// The find filter's absolute gutter numbers, when the page is filtered (see `DataGrid`).
    row_nums: Option<Rc<Vec<usize>>>,
    /// Absolute index of the page's first row (0-based) — gutter numbering + batch mapping.
    row_base: usize,
    /// The snapshot's total row count — the label's `of total`.
    total: usize,
    pub(crate) theme: Option<RecordViewThemePartial>,
}

impl RecordView {
    pub fn new(
        row: usize,
        open: State<Option<usize>>,
        data: Rc<GridData>,
        row_nums: Option<Rc<Vec<usize>>>,
        row_base: usize,
        total: usize,
    ) -> Self {
        Self { row, open, data, row_nums, row_base, total, theme: None }
    }
}

impl Component for RecordView {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(&self.theme, RecordViewThemePreference, "record_view");
        let sheet = use_theme();
        let shadow = sheet.read().colors.shadow;
        let mut open = self.open;
        let row = self.row;
        let len = self.data.rows.len();

        // The label's absolute row number — the same numbering the gutter shows (a filtered
        // page keeps the survivors' original positions).
        let abs_n = self
            .row_nums
            .as_ref()
            .and_then(|nums| nums.get(row).copied())
            .unwrap_or(self.row_base + row + 1);

        // ── header: label · copy JSON/CSV · divider · prev/next · ghost close ────────────
        // A copy button: outline dress (the theme's ghost-button recipe), copy glyph + label.
        // No-op for now — the clipboard wiring is P2-11's shared copy path (see the module
        // doc); these buttons pick it up when it lands rather than one-off plumbing here.
        let copy_button = |label: &'static str, title: &'static str| {
            TooltipContainer::new(Tooltip::new(title))
                .position(AttachedPosition::Bottom)
                .child(
                    Button::new().height(Size::px(28.)).child(
                        rect()
                            .horizontal()
                            .cross_align(Alignment::Center)
                            .spacing(6.)
                            .child(Icon::new(IconName::Copy).size(12.))
                            .child(Path::new(label)),
                    ),
                )
        };
        // Prev/next re-point the open slot within the page (clamped — the standard outline
        // button's disabled dress covers the canvas's faint/no-cursor edge states).
        let step = |icon: IconName, title: &'static str, target: Option<usize>| {
            TooltipContainer::new(Tooltip::new(title))
                .position(AttachedPosition::Bottom)
                .child(
                    Button::new()
                        .width(Size::px(28.))
                        .height(Size::px(28.))
                        .enabled(target.is_some())
                        .on_press(move |_| {
                            if let Some(target) = target {
                                open.set(Some(target));
                            }
                        })
                        .child(Icon::new(icon).size(13.)),
                )
        };
        let close = Button::new()
            .flat()
            .width(Size::px(28.))
            .height(Size::px(28.))
            .on_press(move |_: Event<PressEventData>| open.set(None))
            .child(Icon::new(IconName::Close).size(13.));

        let header = rect()
            .width(Size::fill())
            .horizontal()
            .content(Content::Flex)
            .cross_align(Alignment::Center)
            .spacing(8.)
            .padding((12., 16.))
            .child(
                MonoValue::new(format!("Row {} of {}", fmt_int(abs_n), fmt_int(self.total)))
                    .color(theme.label_color),
            )
            .child(rect().width(Size::flex(1.)))
            .child(copy_button("JSON", "Copy row as JSON"))
            .child(copy_button("CSV", "Copy row as CSV"))
            .child(rect().height(Size::px(20.)).child(Divider::vertical().color(theme.divider_fill)))
            .child(step(IconName::ChevronUp, "Previous row", row.checked_sub(1)))
            .child(step(IconName::ChevronDown, "Next row", (row + 1 < len).then_some(row + 1)))
            .child(close);

        // ── body: one field row per column ───────────────────────────────────────────────
        let batch_row =
            page_batch_row(self.row_nums.as_deref().map(Vec::as_slice), self.row_base, row);
        let mut fields = rect().width(Size::fill()).vertical();
        for (ci, col) in self.data.columns.iter().enumerate() {
            let cell = &self.data.rows[row][ci];
            let nested = matches!(col.kind, Kind::Struct | Kind::List | Kind::Map) && !cell.null;
            // The 150px left column: field name over its dtype in the type colour.
            let left = rect()
                .width(Size::px(150.))
                .vertical()
                .spacing(2.)
                .padding(Gaps::new(2., 0., 0., 0.))
                .child(MonoValue::new(col.name.clone()).color(theme.name_color).wrap())
                .child(Meta::new(col.dtype.clone()).color(kind_color(col.kind, &theme)).wrap());
            // The value: a nested cell renders its pretty JSON in a sunken scroll block
            // (capped at 190px, per the canvas); a scalar renders as one wrapped mono run —
            // nulls in the dimmed tone, everything else in the value colour.
            let value: Element = if nested {
                let json = cell_pretty_json(&self.data.batch, ci, batch_row)
                    .unwrap_or_else(|| cell.text.clone());
                rect()
                    .width(Size::flex(1.))
                    .corner_radius(6.)
                    .background(theme.nested_background)
                    .overflow(Overflow::Clip)
                    .child(
                        ScrollView::new()
                            .height(Size::auto())
                            .max_height(Size::px(190.))
                            // The block sits inside the scrolling field list: a wheel gesture
                            // that starts over it (and can move it) stays latched to it — no
                            // mid-gesture spill to the body (double scroll) — while a gesture
                            // starting at its end, or over a block too short to scroll, passes
                            // through to the body (no hover trap).
                            .latch_wheel()
                            .child(
                                rect()
                                    .width(Size::fill())
                                    .padding((8., 12.))
                                    .child(Readout::new(json).color(theme.nested_color).wrap()),
                            ),
                    )
                    .into()
            } else {
                rect()
                    .width(Size::flex(1.))
                    .main_align(Alignment::Center)
                    .child(
                        MonoValue::new(cell.text.clone())
                            .color(if cell.null { theme.null_color } else { theme.value_color })
                            .wrap(),
                    )
                    .into()
            };
            fields = fields
                .child(
                    rect()
                        .width(Size::fill())
                        .horizontal()
                        .content(Content::Flex)
                        .spacing(12.)
                        .padding((12., 16.))
                        .child(left)
                        .child(value),
                )
                .child(Divider::horizontal().color(theme.row_divider_fill));
        }
        // Hugs a short record; a long one caps at ~the canvas's 82vh card (minus the header)
        // and scrolls — the `cell_view` body idiom.
        let body = ScrollView::new()
            .height(Size::auto())
            .max_height(Size::window_percent(72.))
            .child(fields);

        let card = rect()
            .width(Size::px(540.))
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

        // The overlay layer + global position (the `cell_view` wrapper, verbatim): backdrop
        // press closes; presses on the card land on its own nodes and never reach the backdrop.
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
                    .on_press(move |_: Event<PressEventData>| open.set(None)),
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
    use std::sync::Arc;

    use freya_testing::TestingRunner;
    use strata_core::engine::{RecordBatch, Schema};
    use strata_model::{Cell, ColumnInfo, Kind};

    use super::*;

    /// A 3-row page: a scalar column, a nested column (empty batch — the pretty-JSON read
    /// falls back to the display text, which is all the interaction test needs), a null.
    fn page() -> Rc<GridData> {
        let col = |name: &str, dtype: &str, kind: Kind| ColumnInfo {
            name: name.into(),
            dtype: dtype.into(),
            kind,
            nullable: true,
            children: Vec::new(),
            stats: Vec::new(),
        };
        let cell = |text: &str, null: bool| Cell { text: text.into(), null };
        Rc::new(GridData {
            columns: vec![col("id", "Int64", Kind::Num), col("attrs", "Struct", Kind::Struct)],
            rows: vec![
                vec![cell("1", false), cell("{plan: pro}", false)],
                vec![cell("2", false), cell("NULL", true)],
                vec![cell("3", false), cell("{plan: free}", false)],
            ],
            batch: RecordBatch::new_empty(Arc::new(Schema::empty())),
        })
    }

    fn app() -> impl IntoElement {
        use_init_theme(|| crate::theme::strata_theme(&strata_core::theme::load("midnight")));
        let open = use_consume::<State<Option<usize>>>();
        rect().width(Size::fill()).height(Size::fill()).maybe_child(
            (*open.read()).map(|row| RecordView::new(row, open, page(), None, 0, 3)),
        )
    }

    /// The dismissal paths the acceptance names: a backdrop press closes; a press inside the
    /// card must **not** fall through to the backdrop and close.
    #[test]
    fn backdrop_dismisses_and_the_card_does_not() {
        let (mut runner, open) = TestingRunner::new(
            app,
            (900., 700.).into(),
            |r| r.provide_root_context(|| State::create(Some(0usize))),
            1.,
        );
        runner.sync_and_update();
        runner.click_cursor((450., 350.)); // centre of the centred card
        runner.sync_and_update();
        assert!(open.peek().is_some(), "a press inside the card must not dismiss");
        runner.click_cursor((30., 30.)); // the backdrop
        runner.sync_and_update();
        assert!(open.peek().is_none(), "a backdrop press dismisses");
    }

    /// Headless preview for eyeballing against the canvas `rowViewOpen` comp. Ignored by
    /// default (it writes a file, asserts nothing):
    /// `cargo test -p strata-freya record_view_preview -- --ignored`.
    #[test]
    #[ignore = "writes target/record-view-preview.png for eyeballing; run explicitly"]
    fn record_view_preview() {
        let (mut runner, _) = TestingRunner::new(
            app,
            (900., 700.).into(),
            |r| r.provide_root_context(|| State::create(Some(1usize))),
            1.,
        );
        runner.sync_and_update();
        runner.render_to_file(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../target/record-view-preview.png"
        ));
    }
}
