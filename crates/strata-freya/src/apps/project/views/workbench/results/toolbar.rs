use crate::apps::project::state::{Chan, SessionState, TabId};
use crate::components::icon::{Icon, IconName};
use crate::components::typography::InputTypography;
use freya::components::use_theme;
use freya::prelude::*;
use freya::radio::use_radio;
use strata_core::config::Command;

use super::find::FindState;
use super::selection::Selection;

/// The editor query toolbar, built to the comp. The bar itself only needs the editor surface (its
/// background) and the divider colour. The Run control is its own three-state `RunButton`; the rest
/// are outline [`Button`]s wrapping an icon (the rationalised button model — no bespoke IconButton).
///
/// **Search** (P2-09) toggles the find popover — an [`Attached`] panel anchored to the trigger
/// (bottom-end, so it opens down-and-left clear of the window edge), on the [`Menu`] base for its
/// backdrop dismissal (outside-click / its own Esc). Every close path goes through
/// [`FindState::dismiss`], clearing the filter with the popover.
///
/// **Trash** clears the active tab's results (Rz8 / P2-14): it drops the tab's Run trigger,
/// unmounting the grid back to the empty state — the per-run find state unmounts (and so resets)
/// with it. The mid-run guard is structural — this toolbar only renders inside a settled grid body
/// (a running query shows the Running body instead), so the button can't fire while a query
/// executes. Reload / Download stay stubbed until their layers land (re-run P2-15, export in
/// Phase 4).
#[derive(PartialEq)]
pub struct DataGridToolbar {
    /// The tab whose results this grid shows — Trash clears its Run trigger.
    tab: TabId,
    /// The grid's find state — the Search trigger + popover render it (P2-09).
    find: FindState,
}

impl DataGridToolbar {
    pub fn new(tab: TabId, find: FindState) -> Self {
        Self { tab, find }
    }
}

impl Component for DataGridToolbar {
    fn render(&self) -> impl IntoElement {
        let theme = use_theme();
        let (bg, danger, accent, faint) = {
            let t = theme.read();
            (
                t.colors.background,
                t.colors.error,
                t.colors.primary,
                t.colors.text_placeholder,
            )
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
        // Every tool wears its comp `title=` as a tooltip; Find's carries the effective find
        // chord (reactive — a rebind repaints it), the popover's ✕ the effective Esc.
        let tip = |title: String, button: Button| {
            TooltipContainer::new(Tooltip::new(title))
                .position(AttachedPosition::Bottom)
                .child(button)
        };
        let find_title = crate::keymap::use_hint_title(
            "Find in results",
            Command::Find,
        );

        // ── find (Search) ─────────────────────────────────────────────────────────────────
        let find = self.find;
        let open = *find.open.read();

        let trigger = tool(IconName::Search)
            .on_press(move |_| find.toggle())
            // The comp's `on` dress while the popover is open: accent icon over an
            // accent-tinted fill and border (13% / 55% accent mixes).
            .maybe(open, |b| {
                b.background(accent.with_a(33))
                    .border_fill(accent.with_a(140))
                    .color(accent)
            });

        // The popover panel (comp `res-find-panel`, 340×34): the `Menu` chrome *is* the
        // panel — one bordered row holding the magnifier, a chrome-less `Input` that fills
        // it, and the ✕. The ✕ sits *beside* the input, not in its `trailing`: the input's
        // focus-press `prevent_default`s the pointer-down, which suppresses the follow-up
        // press on anything nested inside it.
        let popover = move || {
            // The ✕: a flat 20×20 icon button (the tab-close recipe — its icon inherits the
            // flat-button colour + hover tint, so it reads as interactive). No tooltip.
            let close = Button::new()
                .flat()
                .width(Size::px(20.))
                .height(Size::px(20.))
                .on_press(move |e: Event<PressEventData>| {
                    e.stop_propagation();
                    find.dismiss();
                })
                .child(Icon::new(IconName::Close).size(12.));
            let panel = rect()
                .width(Size::px(340.))
                .height(Size::px(34.))
                .horizontal()
                .content(Content::Flex)
                .cross_align(Alignment::Center)
                .padding((0., 10.))
                .spacing(8.)
                .child(Icon::new(IconName::Search).color(faint).size(14.))
                .child(
                    InputTypography::mono(
                        Input::new(find.query)
                            // Bare, per the comp: the panel wears the border/background,
                            // so the input's own dress goes fully transparent.
                            .background(Color::TRANSPARENT)
                            .focus_background(Color::TRANSPARENT)
                            .border_fill(Color::TRANSPARENT)
                            .focus_border_fill(Color::TRANSPARENT)
                            .placeholder("Find in results")
                            .compact()
                            .auto_focus(true)
                            .width(Size::fill()),
                    )
                    .width(Size::flex(1.)),
                )
                .child(close);
            // The `Menu` base supplies the popup chrome + dismissal (outside-press backdrop
            // and its own Esc — normally consumed first by the grid root's `Cancel`). The
            // padded wrapper floats the panel 4px clear of the trigger (`Attached` itself
            // anchors flush).
            rect()
                .padding(Gaps::new(4., 0., 0., 0.))
                .child(Menu::new().on_close(move |_| find.dismiss()).child(panel))
        };
        let search = Attached::new(tip(find_title, trigger))
            .bottom()
            .align_end()
            .maybe_child(open.then(popover));

        let row = rect()
            .width(Size::fill())
            .height(Size::px(38.))
            .horizontal()
            .cross_align(Alignment::Center)
            .main_align(Alignment::End)
            .spacing(8.)
            .padding((0., 10.))
            .background(bg)
            .child(search)
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
