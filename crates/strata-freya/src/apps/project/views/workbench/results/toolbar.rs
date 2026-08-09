use strata_model::{ResultsView, TabId};

use crate::apps::export::ExportLaunch;
use crate::apps::project::state::{Chan, SessionState};
use crate::components::icon::{Icon, IconName};
use crate::components::segmented_toggle::{SegmentedToggle, ToggleSegment, TOOLBAR_TWO_ICON_WIDTH};
use crate::components::tool_button::TOOL_SIZE;
use crate::components::toolbar::{Toolbar, ToolbarAction};
use crate::components::typography::InputTypography;
use crate::platform::open_export;
use crate::theme::{use_roles, Role};
use freya::prelude::*;
use freya::radio::use_radio;
use strata_core::config::Command;

use super::chart::ChartCapture;
use super::find::FindState;
use super::selection::Selection;

/// The results toolbar, built to the comp — shared by the grid and chart bodies. The
/// **Table/Chart segmented toggle** sits at the left (P2-07): it reads the tab's per-tab view
/// mode off `Chan::View(id)` and a press flips it, swapping the body under this bar. The right
/// cluster are outline [`Button`]s wrapping an icon (the rationalised button model — no bespoke
/// IconButton); **Find is grid-only** (CHART_SPEC §2), Reload and Download show in both modes.
///
/// **Search** (P2-09) toggles the find popover — an [`Attached`] panel on the [`Menu`] base for
/// its backdrop dismissal (outside-click / its own Esc). Every close path goes through
/// [`FindState::dismiss`], clearing the filter with the popover.
///
/// The panel is anchored to a **zero-width pinned slot**, not to the Search button, so the button
/// is free to fold into the overflow menu at narrow widths (P5-06). Anchored to the button, folding
/// it took the panel's anchor with it and ⌘F — which the datagrid handles, not this row — went
/// silently dead exactly when the pane was too narrow to press the button instead.
///
/// **Trash** clears the active tab's results (Rz8 / P2-14): it drops the tab's Run trigger,
/// unmounting the grid back to the empty state — the per-run find state unmounts (and so resets)
/// with it. The mid-run guard is structural — this toolbar only renders inside a settled grid body
/// (a running query shows the Running body instead), so the button can't fire while a query
/// executes. Reload / Download stay stubbed until their layers land (re-run P2-15, export in
/// Phase 4).
///
/// **Copy Image** is the Chart body's (Chart 08): it renders the settled frame offscreen and puts
/// the pixels on the system clipboard (`chart::capture`). It is here rather than in the strip
/// because it acts on the same settled run Download does, and it is absent — not disabled —
/// wherever the chart drew a notice instead of a plot.
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
    /// The chart a press of **Copy Image** would capture (Chart 08). `Some` only in Chart mode
    /// over a plot that actually drew: it is set by the chart body's drawable branch, so a
    /// notice state has no item at all rather than a dead one — there is no chart to copy, and
    /// a greyed control would suggest there is one that is merely unavailable.
    copy_image: Option<ChartCapture>,
}

impl ResultsToolbar {
    pub fn new(tab: TabId, find: FindState, export: Option<ExportLaunch>) -> Self {
        Self {
            tab,
            find,
            export,
            copy_image: None,
        }
    }

    /// The settled chart Copy Image acts on — the chart body's to supply, since only it knows
    /// whether a plot was drawn.
    pub fn copy_image(mut self, capture: Option<ChartCapture>) -> Self {
        self.copy_image = capture;
        self
    }
}

impl Component for ResultsToolbar {
    fn render(&self) -> impl IntoElement {
        let roles = use_roles();
        // Neither the destructive tone nor the accent is read here any more: `ToolbarAction`'s
        // `danger` and `active` own those dresses, so Clear's red hover and Find's on-state are
        // variants rather than five overrides at this call site.
        let (bg, faint) = (
            roles.get(Role::Background),
            roles.get(Role::TextPlaceholder),
        );
        // The grid's shared selection (provided by the results pane) — cleared with the results so
        // a later run doesn't wake up wearing the old grid's selection.
        let mut sel = use_consume::<State<Selection>>();
        let tab = self.tab;
        let mut session = use_radio::<SessionState, Chan>(Chan::Request(tab));
        // The Table/Chart view mode — its own channel, so a flip wakes only the results pane.
        let mut view_radio = use_radio::<SessionState, Chan>(Chan::View(tab));
        let view = view_radio.read().view(tab);

        // The Export window's launch inputs arrive as a prop (see `ExportLaunch`); only the
        // `Platform` is taken here, because a handler has no scope to read one from.
        let export = self.export.clone();
        let platform = use_hook(Platform::get);

        // ── find (Search) ─────────────────────────────────────────────────────────────────
        let find = self.find;
        let open = *find.open.read();

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
        // **The popover's anchor is not the button.** It sits in the row's pinned slot at zero
        // width, so it exists at every width the pane can take — which is what lets the Find
        // *button* fold into the overflow menu like any other action.
        //
        // With the popover anchored to the button, folding the button took the panel's anchor with
        // it: ⌘F (handled in the datagrid, not here) would flip `find.open` and nothing would
        // render, so the chord went quietly dead exactly when the pane was too narrow to press the
        // button instead. An anchor that cannot fold is the whole fix.
        // Zero **width** so it costs the row nothing, but the height of the button it replaced:
        // `AttachedPosition::Bottom` offsets the panel by `inner_height`, so a zero-height anchor
        // would open it at the row's vertical centre, straight over the toolbar strip, instead of
        // below the row. At `TOOL_SIZE` tall it is centred in the row exactly as the Search button
        // was, and the panel lands where it always did.
        let search_anchor = Attached::new(rect().width(Size::px(0.)).height(Size::px(TOOL_SIZE)))
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
        // `components::toolbar`): Copy Image where there is one, then Export, then Clear, then
        // Reload.
        //
        // **The Table/Chart toggle is the leading run**, so it never folds — it decides what the
        // whole body below is, and it flexes to push the tool cluster to the far end exactly as
        // the comp draws it.
        //
        // **Find is an ordinary action**, folding into the menu with its chord like the rest, and
        // it is deliberately first so it is the last to go. Its popover hangs off the pinned
        // anchor above rather than off this button, which is what lets it fold at all.
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
            // Find is grid-only (CHART_SPEC §2), so in Chart mode there is no item at all rather
            // than an empty slot still charged for its width.
            .maybe(view == ResultsView::Grid, |bar| {
                bar.item(
                    // A **bare** label: `Toolbar` appends the chord to the tooltip itself, and the
                    // folded menu row renders it as a `KeyHint`. Passing a pre-composed
                    // "Find in results (⌘F)" here would print the chord twice in the menu.
                    ToolbarAction::new(IconName::Search, "Find in results")
                        .hint(Command::Find)
                        .active(open)
                        .on_press(move |_| find.toggle()),
                )
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
            )
            // Chart mode only, and only over a plot that drew — the body hands one down exactly
            // then, so there is no state in which this is present and does nothing.
            .map(self.copy_image.clone(), |bar, capture| {
                bar.item(
                    ToolbarAction::new(IconName::Copy, "Copy chart as image")
                        .on_press(move |_| capture.copy()),
                )
            })
            // Zero width, so it costs the fold arithmetic nothing and never folds.
            .pinned(search_anchor, 0.);

        rect().width(Size::fill()).vertical().child(row)
    }
}
