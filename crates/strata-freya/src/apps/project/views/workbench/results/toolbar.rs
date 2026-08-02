use strata_model::{ResultsView, TabId};

use crate::apps::export::ExportLaunch;
use crate::apps::project::state::{Chan, SessionState};
use crate::components::icon::{Icon, IconName};
use crate::components::segmented_toggle::{SegmentedToggle, ToggleSegment, TOOLBAR_TWO_ICON_WIDTH};
use crate::components::tool_button::TOOL_SIZE;
use crate::components::toolbar::{Toolbar, ToolbarAction, ToolbarItem};
use crate::components::typography::InputTypography;
use crate::keymap::use_hint_title;
use crate::platform::open_export;
use freya::components::use_theme;
use freya::prelude::*;
use freya::radio::use_radio;
use strata_core::config::Command;

use super::find::FindState;
use super::selection::Selection;

/// The results toolbar, built to the comp — shared by the grid and chart bodies. The
/// **Table/Chart segmented toggle** sits at the left (P2-07): it reads the tab's per-tab view
/// mode off `Chan::View(id)` and a press flips it, swapping the body under this bar. The right
/// cluster are outline [`Button`]s wrapping an icon (the rationalised button model — no bespoke
/// IconButton); **Find is grid-only** (CHART_SPEC §1), Reload and Download show in both modes.
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
pub struct ResultsToolbar {
    /// The tab whose results this pane shows — Trash clears its Run trigger, the toggle
    /// flips its view mode.
    tab: TabId,
    /// The grid's find state — the Search trigger + popover render it (P2-09).
    find: FindState,
    /// What a press of Download would open the Export window on (P4-10). `None` when the run
    /// hasn't settled rows — there is nothing to export, so the button is disabled rather than
    /// opening a window onto nothing.
    export: Option<ExportLaunch>,
}

impl ResultsToolbar {
    pub fn new(tab: TabId, find: FindState, export: Option<ExportLaunch>) -> Self {
        Self { tab, find, export }
    }
}

impl Component for ResultsToolbar {
    fn render(&self) -> impl IntoElement {
        let theme = use_theme();
        // The destructive tone is no longer read here: `ToolbarAction::danger` owns that dress, so
        // Clear's red hover is one variant rather than three overrides at this call site.
        let (bg, accent, faint) = {
            let t = theme.read();
            (
                t.colors().background,
                t.colors().primary,
                t.colors().text_placeholder,
            )
        };
        // The grid's shared selection (provided by the results pane) — cleared with the results so
        // a later run doesn't wake up wearing the old grid's selection.
        let mut sel = use_consume::<State<Selection>>();
        let tab = self.tab;
        let mut session = use_radio::<SessionState, Chan>(Chan::Request(tab));
        // The Table/Chart view mode — its own channel, so a flip wakes only the results pane.
        let mut view_radio = use_radio::<SessionState, Chan>(Chan::View(tab));
        let view = view_radio.read().view(tab);

        // Find keeps its own button and tooltip: it is an [`Attached`] anchor, not just a control,
        // so it cannot come from `Toolbar`'s action shape. Its title carries the effective find
        // chord (reactive — a rebind repaints it), the popover's ✕ the effective Esc.
        let tool = move |icon: IconName| {
            Button::new()
                .height(Size::px(TOOL_SIZE))
                .width(Size::px(TOOL_SIZE))
                .child(Icon::new(icon).size(15.))
        };
        let tip = |title: String, button: Button| {
            TooltipContainer::new(Tooltip::new_text(title))
                .position(AttachedPosition::Bottom)
                .child(button)
        };
        let find_title = use_hint_title("Find in results", Command::Find);

        // The Export window's launch inputs arrive as a prop (see `ExportLaunch`); only the
        // `Platform` is taken here, because a handler has no scope to read one from.
        let export = self.export.clone();
        let platform = use_hook(Platform::get);

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

        // ── the Table/Chart segmented toggle (left cluster, P2-07) ────────────────────────
        // A press writes the tab's view mode; leaving the grid dismisses the find popover
        // (and so its filter) with it — Find is grid-only.
        let toggle = SegmentedToggle::new()
            .child(
                ToggleSegment::new(IconName::Grid)
                    .title("Table")
                    .selected(view == ResultsView::Grid)
                    .on_press(move |_| {
                        view_radio
                            .write_channel(Chan::View(tab))
                            .set_view(tab, ResultsView::Grid);
                    }),
            )
            .child(
                ToggleSegment::new(IconName::Chart)
                    .title("Chart")
                    .selected(view == ResultsView::Chart)
                    .on_press(move |_| {
                        find.dismiss();
                        view_radio
                            .write_channel(Chan::View(tab))
                            .set_view(tab, ResultsView::Chart);
                    }),
            );

        // The row folds tail-first once the pane is too narrow to hold it (P5-06,
        // `components::toolbar`): Export goes first, then Clear, then Reload.
        //
        // **The Table/Chart toggle is the leading run**, so it never folds — it decides what the
        // whole body below is, and it flexes to push the tool cluster to the far end exactly as
        // the comp draws it.
        //
        // **Find is deliberately first**, so it is the last thing to fold. It is a `Custom` rather
        // than an ordinary action because it is an [`Attached`] anchor as well as a button: the
        // popover measures itself against this node, so the toolbar cannot rebuild it as a menu
        // row. See the note below on what that costs.
        let row = Toolbar::new()
            .background(bg)
            // Charged the toggle's real width, not zero: the wrapper flexes but the pill inside it
            // does not, so telling the fold arithmetic this run can vanish kept the tool cluster
            // inline past the point it fitted, and the pill then painted out over it.
            .leading(
                rect()
                    .width(Size::flex(1.))
                    .overflow(Overflow::Clip)
                    .child(toggle),
                TOOLBAR_TWO_ICON_WIDTH,
            )
            .item(ToolbarItem::Custom {
                // Find is grid-only, so in Chart mode the slot draws nothing and is charged
                // nothing — otherwise the tools folded 36px earlier than they needed to.
                width: match view {
                    ResultsView::Grid => TOOL_SIZE,
                    _ => 0.,
                },
                // Find is grid-only (CHART_SPEC §1) — in Chart mode the slot is an empty box.
                inline: match view {
                    ResultsView::Grid => search.into_element(),
                    _ => rect().into_element(),
                },
                // No folded form, and this is the one seam P5-06 leaves: the popover is anchored
                // to the trigger, so with the trigger gone ⌘F (handled in the datagrid) toggles
                // state nothing renders. It only bites once the pane is narrower than the toggle
                // plus two controls, where the grid shows nothing anyway. The fix is to host the
                // popover on the results pane root rather than on this button — a change to the
                // popover stack, which is high-risk ground (AGENTS.md §8) and wants its own pass.
                folded: None,
            })
            .item(
                ToolbarAction::new(IconName::Reload, "Re-run the query to refresh the snapshot")
                    .enabled(false),
            )
            .item(
                // Destructive dress on hover, per the comp: red icon over a red-tinted fill and
                // border (the Dioxus `.res-clear` recipe — 15% / 45% red mixes).
                ToolbarAction::new(IconName::Trash, "Clear results")
                    .danger()
                    .on_press(move |_| {
                        session.write_channel(Chan::Request(tab)).clear_request(tab);
                        sel.set(Selection::None);
                    }),
            )
            .item(
                // Opens a window **on this run**, carrying its snapshot handle: the window
                // pins that snapshot for its life, so re-running here afterwards doesn't
                // change what it writes (SNAPSHOT_SPEC §4).
                ToolbarAction::new(IconName::Download, "Export results")
                    .enabled(export.is_some())
                    .on_press(move |_| {
                        if let Some(launch) = export.clone() {
                            open_export(platform.clone(), launch);
                        }
                    }),
            );

        rect().width(Size::fill()).vertical().child(row)
    }
}
