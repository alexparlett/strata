use crate::apps::project::state::{Chan, SessionState, TabId};
use crate::components::icon::{Icon, IconName};
use freya::components::use_theme;
use freya::prelude::*;
use freya::radio::use_radio;

use super::selection::Selection;

/// The editor query toolbar, built to the comp. The bar itself only needs the editor surface (its
/// background) and the divider colour. The Run control is its own three-state `RunButton`; the rest
/// are outline [`Button`]s wrapping an icon (the rationalised button model — no bespoke IconButton).
///
/// **Trash** clears the active tab's results (Rz8 / P2-14): it drops the tab's Run trigger,
/// unmounting the grid back to the empty state. The mid-run guard is structural — this toolbar only
/// renders inside a settled grid body (a running query shows the Running body instead), so the
/// button can't fire while a query executes. Search / Reload / Download stay stubbed until their
/// layers land (find P2-09, re-run P2-15, export in Phase 4); the find query will join the Trash reset
/// when P2-09 gives it state to clear.
#[derive(PartialEq)]
pub struct DataGridToolbar {
    /// The tab whose results this grid shows — Trash clears its Run trigger.
    tab: TabId,
}

impl DataGridToolbar {
    pub fn new(tab: TabId) -> Self {
        Self { tab }
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
        let tab = self.tab;
        let mut session = use_radio::<SessionState, Chan>(Chan::Request(tab));

        // An outline icon button — `outline_button` variant with a centred icon (the icon inherits
        // the button's colour, hover included, via `currentColor`).
        let tool = move |icon: IconName| {
            Button::new()
                .height(Size::px(28.))
                .width(Size::px(28.))
                .child(Icon::new(icon).size(15.))
        };
        // Every tool wears its comp `title=` as a tooltip; Find's carries the effective
        // find chord (reactive — a rebind repaints it) even while the button is a stub.
        let tip = |title: String, button: Button| {
            TooltipContainer::new(Tooltip::new(title))
                .position(AttachedPosition::Bottom)
                .child(button)
        };
        let find_title = crate::keymap::use_hint_title(
            "Find in results",
            strata_core::config::Command::Find,
        );

        let row = rect()
            .width(Size::fill())
            .height(Size::px(38.))
            .horizontal()
            .cross_align(Alignment::Center)
            .main_align(Alignment::End)
            .spacing(8.)
            .padding((0., 10.))
            .background(bg)
            .child(tip(find_title, tool(IconName::Search)))
            .child(tip(
                "Re-run the query to refresh the snapshot".into(),
                tool(IconName::Reload),
            ))
            .child(tip(
                "Clear results".into(),
                // Destructive dress on hover, per the comp: red icon over a red-tinted fill and
                // border (the Dioxus `.res-clear` recipe — 15% / 45% red mixes).
                tool(IconName::Trash)
                    .hover_background(danger.with_a(38))
                    .hover_border_fill(danger.with_a(115))
                    .hover_color(danger)
                    .on_press(move |_| {
                        session.write_channel(Chan::Request(tab)).clear_request(tab);
                        sel.set(Selection::None);
                    }),
            ))
            .child(tip("Export results".into(), tool(IconName::Download)));

        rect()
            .width(Size::fill())
            .vertical()
            .child(row)
    }
}
