//! The nested-cell value view (P2-12 / Dioxus U5): double-clicking a **nested**
//! (`struct` / `list` / `map`) grid cell opens it in a centred backdrop modal — the canvas
//! `cellViewOpen` comp. One of the grid's two double-click targets: **cell → nested value**
//! (here), **gutter → whole row** (P2-10).
//!
//! The body is a **lazy tree** (P2-25), not text. It was pretty JSON, which is a dead end at the
//! size this surface exists for: `config.json`'s `contentBlocks` is 19,311 keys under one
//! top-level key, and any bounded rendering of that names the shape and gives you no way into it.
//! The tree opens what you ask for and nothing else — see `super::value_tree` for the model and
//! `strata_core::engine::value_tree` for the Arrow reads.
//!
//! The open state ([`State<Option<CellValue>>`]) lives on the `DataGrid` (it survives page flips
//! like the column widths, and its `Command::Cancel` arm dismisses on Esc), and now carries the
//! **batch** rather than rendered text — which keeps the snapshot rule rather than breaking it,
//! since the arrays the modal reads are the ones it opened with, so a later filter or page shift
//! still cannot retarget it. Every colour is a `cell_view` component token; the card follows the
//! `CloseConfirm` overlay idiom (overlay layer + global position + backdrop press), plus the
//! canvas's 3px backdrop blur.

use std::rc::Rc;

use freya::components::{define_theme, get_theme, Disclosure, Tree, TreeItem};
use freya::prelude::*;

use super::value_tree::{leaf_text, RowKind, TreeModel, TreeRow};
use crate::components::badge::Badge;
use crate::components::divider::Divider;
use crate::components::icon::{Icon, IconName};
use crate::components::metrics::{R_1, R_4, SP_3, SP_4, SP_5};
use crate::components::type_palette::{kind_color, type_palette};
use crate::components::typography::{Meta, MonoValue};
use crate::theme::{use_roles, Role};

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

/// What the modal shows — the cell's column name, its dtype (the header badge), and the model the
/// tree reads.
///
/// The **batch** rides here rather than the rendered text, which is P2-25's change and keeps
/// P2-12's rule rather than breaking it: the modal is still a snapshot, because the arrays it reads
/// are the ones it was opened with, so a later filter or page flip cannot retarget it. A
/// `RecordBatch` clone is an `Arc` bump per column.
#[derive(Clone, PartialEq)]
pub struct CellValue {
    pub name: String,
    pub dtype: String,
    pub tree: TreeModel,
}

/// Map a (possibly find-filtered) page row index back to its row in the **page batch**:
/// a filtered `GridData` keeps the unfiltered batch, and `row_nums` carries the
/// survivors' absolute 1-based gutter numbers (`row_base` + original position + 1).
pub fn page_batch_row(row_nums: Option<&[usize]>, row_base: usize, index: usize) -> usize {
    row_nums
        .and_then(|nums| nums.get(index).map(|abs| abs.saturating_sub(row_base + 1)))
        .unwrap_or(index)
}

/// The tree itself: the model's visible rows over Freya's `Tree`.
///
/// **Expansion is written back through the open slot** rather than kept in a state of its own.
/// The slot already holds the model, so a second copy would be a second answer to "what is open" —
/// and this way closing the modal disposes of it, which is what closing it means.
#[derive(PartialEq)]
struct ValueTreeBody {
    model: TreeModel,
    open: State<Option<CellValue>>,
}

impl Component for ValueTreeBody {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(
            &None::<CellViewThemePartial>,
            CellViewThemePreference,
            "cell_view"
        );
        let palette = type_palette();
        let mut open = self.open;
        // Derived on every render rather than memoized. `use_memo` settles *asynchronously*, so a
        // toggle would paint the previous rows for a frame — the trap `record_view`'s `PreviewMemo`
        // and `find`'s `PageMemo` both exist to avoid. The walk visits only what is open, so it is
        // proportional to what is on screen; a synchronous cache is the way to trim it if the
        // re-walk on an unrelated render (the ✕'s hover lives on `CellView`) ever shows up.
        let rows = Rc::new(self.model.rows());

        Tree::new_with_data(rows.clone(), move |index, rows: &Rc<Vec<TreeRow>>| {
            let Some(row) = rows.get(index.index).cloned() else {
                return rect().into();
            };
            match row.kind {
                RowKind::Node { node, disclosure } => {
                    let path = row.path.clone();
                    TreeItem::new()
                        .depth(row.depth)
                        .disclosure(disclosure)
                        .on_toggle(move |_| {
                            if let Some(value) = open.write().as_mut() {
                                value.tree.toggle(&path);
                            }
                        })
                        .child(
                            rect()
                                .horizontal()
                                .cross_align(Alignment::Center)
                                .spacing(SP_3)
                                // A list item has no name, so it is titled by its index — the
                                // subscript a reader would write to reach it.
                                .child(MonoValue::new(match &node.key {
                                    Some(key) => key.clone(),
                                    None => format!("[{}]", node.index),
                                }))
                                .child(
                                    Meta::new(node.dtype.clone())
                                        .color(kind_color(node.kind, &palette)),
                                )
                                .maybe_child(leaf_text(&node.value).map(|(text, null)| {
                                    MonoValue::new(text.to_string())
                                        .color(if null {
                                            theme.name_color
                                        } else {
                                            theme.body_color
                                        })
                                        .into_element()
                                })),
                        )
                        .into()
                }
                // The tail of a paged container. A leaf as far as the tree is concerned — it opens
                // nothing, it reveals more of its owner.
                RowKind::More { label, .. } => {
                    // The tail sits at its container's path, so that is what widens.
                    let owner = row.path.clone();
                    TreeItem::new()
                        .depth(row.depth)
                        .disclosure(Disclosure::Leaf)
                        .on_press(move |_| {
                            if let Some(value) = open.write().as_mut() {
                                value.tree.reveal_more(&owner);
                            }
                        })
                        .child(Meta::new(label).color(theme.name_color))
                        .into()
                }
            }
        })
        .length(rows.len())
        .height(Size::fill())
    }
}

/// The card's size. Wider than the 460px the canvas drew for a JSON blob, because a tree row
/// carries a key, a type and a value side by side.
const CARD_SIZE: (f32, f32) = (720., 520.);

/// The centred backdrop modal: the card (name + dtype badge + ghost close over the value tree).
/// Backdrop press and the ✕ dismiss; Esc is arbitrated by the grid root's `Command::Cancel` chain
/// (the modal's ancestor in document order).
#[derive(PartialEq)]
pub struct CellView {
    value: CellValue,
    /// The grid's open slot — cleared to dismiss.
    open: State<Option<CellValue>>,
    pub(crate) theme: Option<CellViewThemePartial>,
}

impl CellView {
    pub fn new(value: CellValue, open: State<Option<CellValue>>) -> Self {
        Self {
            value,
            open,
            theme: None,
        }
    }
}

impl Component for CellView {
    fn render(&self) -> impl IntoElement {
        let mut close_hover = use_state(|| false);
        let theme = get_theme!(&self.theme, CellViewThemePreference, "cell_view");
        let shadow = use_roles().get(Role::Shadow);
        let mut open = self.open;
        let close = move |_: Event<PressEventData>| open.set(None);

        // Header: cell name (mono 12.5) + the cyan dtype badge + ghost close.
        let header = rect()
            .width(Size::fill())
            .horizontal()
            .content(Content::Flex)
            .cross_align(Alignment::Center)
            .spacing(SP_4)
            .padding((SP_4, SP_5))
            .child(MonoValue::new(self.value.name.clone()).color(theme.name_color))
            .child(
                Badge::value(self.value.dtype.clone(), theme.badge_color)
                    .background(theme.badge_background),
            )
            .child(rect().width(Size::flex(1.)))
            .child(
                rect()
                    .width(Size::px(28.))
                    .height(Size::px(28.))
                    .corner_radius(R_1)
                    .center()
                    .maybe(close_hover(), |el| {
                        el.background(theme.close_hover_background)
                    })
                    .on_pointer_enter(move |_| close_hover.set(true))
                    .on_pointer_leave(move |_| close_hover.set(false))
                    .on_press(close)
                    .child(
                        Icon::new(IconName::Close)
                            .size(13.)
                            .color(if close_hover() {
                                theme.close_hover_color
                            } else {
                                theme.close_color
                            }),
                    ),
            );

        // Body: the value tree. Its height is stated rather than hugged — a `VirtualScrollView`
        // needs a viewport to virtualize against, and this is the surface that has to stay cheap
        // with 19,311 siblings in one node.
        let (card_w, card_h) = CARD_SIZE;
        let body = rect()
            .width(Size::fill())
            .height(Size::flex(1.))
            .background(theme.body_background)
            .padding((SP_3, SP_3))
            .child(ValueTreeBody {
                model: self.value.tree.clone(),
                open: self.open,
            });

        let card = rect()
            .width(Size::px(card_w))
            .height(Size::px(card_h))
            .max_width(Size::window_percent(96.))
            .max_height(Size::window_percent(92.))
            .corner_radius(R_4)
            .background(theme.background)
            .border(Border::new().width(1.).fill(theme.border_fill))
            .shadow(Shadow::new().y(30.).blur(70.).color(shadow))
            .overflow(Overflow::Clip)
            .vertical()
            .content(Content::Flex)
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
    use std::sync::Arc;

    use datafusion::arrow::array::{ArrayRef, Int32Array, StringArray, StructArray};
    use datafusion::arrow::datatypes::{DataType, Field, Fields};
    use freya_testing::TestingRunner;
    use strata_core::engine::{RecordBatch, Schema};
    use strata_core::theme::load;

    use super::*;
    use crate::theme::strata_theme;

    /// `attrs: { plan: "pro", seats: 12 }` — a real batch, since the modal now reads Arrow rather
    /// than a rendered string.
    fn value() -> CellValue {
        let fields = Fields::from(vec![
            Field::new("plan", DataType::Utf8, true),
            Field::new("seats", DataType::Int32, true),
        ]);
        let attrs = StructArray::new(
            fields.clone(),
            vec![
                Arc::new(StringArray::from(vec!["pro"])) as ArrayRef,
                Arc::new(Int32Array::from(vec![12])) as ArrayRef,
            ],
            None,
        );
        let schema = Schema::new(vec![Field::new("attrs", DataType::Struct(fields), true)]);
        let batch = RecordBatch::try_new(Arc::new(schema), vec![Arc::new(attrs)]).unwrap();
        CellValue {
            name: "attrs".into(),
            dtype: "Struct".into(),
            tree: TreeModel::new(batch, 0, 0),
        }
    }

    /// The harness window. Named so the coordinates below can be derived from it.
    const WINDOW: (f32, f32) = (900., 700.);

    fn app() -> impl IntoElement {
        use_init_theme(|| strata_theme(&load("midnight")));
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
            WINDOW.into(),
            |r| r.provide_root_context(|| State::create(Some(value()))),
            1.,
        );
        runner.sync_and_update();
        runner.click_cursor((450., 350.)); // centre of the centred card
        runner.sync_and_update();
        assert!(
            open.peek().is_some(),
            "a press inside the card must not dismiss"
        );
        runner.click_cursor((30., 30.)); // the backdrop
        runner.sync_and_update();
        assert!(open.peek().is_none(), "a backdrop press dismisses");
        // Reopen and dismiss via the ✕ (top-right of the header).
        let mut open = open;
        open.set(Some(value()));
        runner.sync_and_update();
        // The ✕, top-right of the header — derived from the card's size rather than written down,
        // because the card is resizable now and a hardcoded corner has broken twice already.
        // Header: 12px padding over a 28px button, so its middle is 26px below the card's top.
        let (w, h) = CARD_SIZE;
        let (left, top) = ((WINDOW.0 - w) / 2., (WINDOW.1 - h) / 2.);
        runner.click_cursor(((left + w - 16. - 14.) as f64, (top + 26.) as f64));
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
            WINDOW.into(),
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
