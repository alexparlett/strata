use crate::apps::project::query::QuerySpec;
use crate::components::icon::{Icon, IconName};
use freya::components::use_theme;
use freya::prelude::*;

use super::selection::Selection;

/// The editor query toolbar, built to the comp. The bar itself only needs the editor surface (its
/// background) and the divider colour. The Run control is its own three-state `RunButton`; the rest
/// are outline [`Button`]s wrapping an icon (the rationalised button model — no bespoke IconButton).
///
/// **Trash** clears the active tab's results (Rz8 / P2-14): it drops the workbench's Run trigger,
/// unmounting the grid back to the empty state. The mid-run guard is structural — this toolbar only
/// renders inside a settled grid body (a running query shows the Running body instead), so the
/// button can't fire while a query executes. Search / Reload / Download stay stubbed until their
/// layers land (find P2-09, re-run P2-15, export in Phase 4); the find query will join the Trash reset
/// when P2-09 gives it state to clear.
#[derive(PartialEq)]
pub struct DataGridToolbar {
    /// The workbench's Run trigger — the active press whose results this grid shows.
    request: State<Option<QuerySpec>>,
}

impl DataGridToolbar {
    pub fn new(request: State<Option<QuerySpec>>) -> Self {
        Self { request }
    }
}

impl Component for DataGridToolbar {
    fn render(&self) -> impl IntoElement {
        let theme = use_theme();
        let (bg, danger) = {
            let t = theme.read();
            (t.colors.background, t.colors.error)
        };
        // The grid's shared selection (provided by the results pane) — cleared with the results so
        // a later run doesn't wake up wearing the old grid's selection.
        let mut sel = use_consume::<State<Selection>>();
        let mut request = self.request;

        // An outline icon button — `outline_button` variant with a centred icon (the icon inherits
        // the button's colour, hover included, via `currentColor`).
        let tool = move |icon: IconName| {
            Button::new()
                .height(Size::px(28.))
                .width(Size::px(28.))
                .child(Icon::new(icon).size(15.))
        };

        let row = rect()
            .width(Size::fill())
            .height(Size::px(38.))
            .horizontal()
            .cross_align(Alignment::Center)
            .main_align(Alignment::End)
            .spacing(8.)
            .padding((0., 10.))
            .background(bg)
            .child(tool(IconName::Search))
            .child(tool(IconName::Reload))
            .child(
                // Destructive dress on hover, per the comp: red icon over a red-tinted fill and
                // border (the Dioxus `.res-clear` recipe — 15% / 45% red mixes).
                tool(IconName::Trash)
                    .hover_background(danger.with_a(38))
                    .hover_border_fill(danger.with_a(115))
                    .hover_color(danger)
                    .on_press(move |_| {
                        request.set(None);
                        sel.set(Selection::None);
                    }),
            )
            .child(tool(IconName::Download));

        rect()
            .width(Size::fill())
            .vertical()
            .child(row)
    }
}
