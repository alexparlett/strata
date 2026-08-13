use strata_model::{ResultsView, TabId};

use crate::apps::export::ExportLaunch;
use crate::apps::project::state::{Chan, ChatsCtx, SessionState};
use crate::apps::project::views::{ask_about, result_anchor};
use crate::components::icon::{Icon, IconName};
use crate::components::metrics::TOOL_SIZE;
use crate::components::segmented_toggle::{SegmentedToggle, ToggleSegment, TOOLBAR_TWO_ICON_WIDTH};
use crate::components::toolbar::{Toolbar, ToolbarAction};
use crate::components::typography::InputTypography;
use crate::platform::open_export;
use crate::theme::{use_roles, Role};
use freya::prelude::*;
use freya::radio::{use_radio, use_radio_station};
use strata_core::config::Command;

use super::chart::ChartCapture;
use super::find::FindState;
use super::selection::Selection;
use super::shape::ShapeTarget;
use crate::components::metrics::{SP_2, SP_3, SP_4};

/// The results toolbar, built to the comp — shared by the grid and chart bodies. The
/// **Table/Chart segmented toggle** sits at the left (P2-07): it reads the tab's per-tab view
/// mode off `Chan::View(id)` and a press flips it, swapping the body under this bar. The right
/// cluster are outline [`Button`]s wrapping an icon (the rationalised button model — no bespoke
/// `IconButton`); **Find is grid-only** (`CHART_SPEC` §2), Reload and Download show in both modes.
///
/// **Search** toggles the find popover, an [`Attached`] panel on the [`Menu`] base for its backdrop
/// dismissal; every close path goes through [`FindState::dismiss`], clearing the filter with the
/// popover. The panel is anchored to a **zero-width pinned slot** rather than to the Search button,
/// so the button is free to fold into the overflow menu at narrow widths — anchored to the button,
/// folding it took the anchor with it and ⌘F went silently dead exactly when the pane was too
/// narrow to press the button instead.
///
/// **Trash** clears the active tab's results by dropping its Run trigger, unmounting the grid back
/// to the empty state and resetting the per-run find state with it. The mid-run guard is
/// structural: this toolbar only renders inside a settled grid body.
///
/// **Copy Image** is the Chart body's: it renders the settled frame offscreen onto the system
/// clipboard (`chart::capture`). Here rather than in the strip because it acts on the same settled
/// run Download does, and it is absent — not disabled — wherever the chart drew a notice.
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
    /// What a press of **Shape** opens the composer over (Chart 09) — the settled run's SQL
    /// and schema, seeded from the chart encoding when the press comes from the Chart body.
    /// `None` while the run hasn't settled rows, which disables the button on `export`'s
    /// terms.
    shape: Option<ShapeTarget>,
}

impl ResultsToolbar {
    pub fn new(tab: TabId, find: FindState, export: Option<ExportLaunch>) -> Self {
        Self {
            tab,
            find,
            export,
            copy_image: None,
            shape: None,
        }
    }

    /// The settled chart Copy Image acts on — the chart body's to supply, since only it knows
    /// whether a plot was drawn.
    pub fn copy_image(mut self, capture: Option<ChartCapture>) -> Self {
        self.copy_image = capture;
        self
    }

    /// The settled run a Shape press composes over (Chart 09).
    pub fn shape(mut self, shape: Option<ShapeTarget>) -> Self {
        self.shape = shape;
        self
    }
}

impl Component for ResultsToolbar {
    fn render(&self) -> impl IntoElement {
        let roles = use_roles();
        let (bg, faint) = (
            roles.get(Role::Background),
            roles.get(Role::TextPlaceholder),
        );
        let mut sel = use_consume::<State<Selection>>();
        let tab = self.tab;
        let mut session = use_radio::<SessionState, Chan>(Chan::Request(tab));
        let chats = use_consume::<ChatsCtx>();
        let station = use_radio_station::<SessionState, Chan>();
        let mut view_radio = use_radio::<SessionState, Chan>(Chan::View(tab));
        let view = view_radio.read().view(tab);

        let export = self.export.clone();
        let platform = use_hook(Platform::get);
        let shape_slot = use_consume::<State<Option<ShapeTarget>>>();

        let find = self.find;
        let open = *find.open.read();

        let popover = move || {
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
                .padding((0., SP_4))
                .spacing(SP_3)
                .child(Icon::new(IconName::Search).color(faint).size(14.))
                .child(
                    InputTypography::mono(
                        Input::new(find.query)
                            .background(Color::TRANSPARENT)
                            .hover_background(Color::TRANSPARENT)
                            .focus_background(Color::TRANSPARENT)
                            .border_fill(Color::TRANSPARENT)
                            .hover_border_fill(Color::TRANSPARENT)
                            .focus_border_fill(Color::TRANSPARENT)
                            .focus_ring_fill(Color::TRANSPARENT)
                            .placeholder("Find in results")
                            .compact()
                            .auto_focus(true)
                            .width(Size::fill()),
                    )
                    .width(Size::flex(1.)),
                )
                .child(close);
            rect()
                .padding(Gaps::new(SP_2, 0., 0., 0.))
                .child(Menu::new().on_close(move |()| find.dismiss()).child(panel))
        };
        let search_anchor = Attached::new(rect().width(Size::px(0.)).height(Size::px(TOOL_SIZE)))
            .bottom()
            .align_end()
            .maybe_child(open.then(popover));

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

        let row = Toolbar::new()
            .background(bg)
            .leading(
                rect()
                    .width(Size::flex(1.))
                    .overflow(Overflow::Clip)
                    .child(toggle),
                TOOLBAR_TWO_ICON_WIDTH,
            )
            .maybe(view == ResultsView::Grid, |bar| {
                bar.item(
                    ToolbarAction::new(IconName::Search, "Find in results")
                        .hint(Command::Find)
                        .active(open)
                        .on_press(move |_| find.toggle()),
                )
            })
            .item({
                let shape = self.shape.clone();
                let slot = shape_slot;
                ToolbarAction::new(IconName::Rows, "Shape into a grouped query")
                    .enabled(shape.is_some())
                    .on_press(move |_| {
                        if let Some(target) = shape.clone() {
                            let mut slot = slot;
                            slot.set(Some(target));
                        }
                    })
            })
            .item(
                ToolbarAction::new(IconName::Reload, "Re-run the query to refresh the snapshot")
                    .enabled(false),
            )
            .item(
                ToolbarAction::new(IconName::Trash, "Clear results")
                    .danger()
                    .on_press(move |_| {
                        session.write_channel(Chan::Request(tab)).clear_request(tab);
                        sel.set(Selection::None);
                    }),
            )
            .map(self.export.clone(), |bar, launch| {
                bar.item(
                    ToolbarAction::new(IconName::Chat, "Ask the assistant about this result")
                        .on_press(move |_| {
                            ask_about(station, chats, result_anchor(&launch.target));
                        }),
                )
            })
            .item(
                ToolbarAction::new(IconName::Download, "Export results")
                    .enabled(export.is_some())
                    .on_press(move |_| {
                        if let Some(launch) = export.clone() {
                            open_export(platform.clone(), launch);
                        }
                    }),
            )
            .map(self.copy_image.clone(), |bar, capture| {
                bar.item(
                    ToolbarAction::new(IconName::Copy, "Copy chart as image")
                        .on_press(move |_| capture.copy()),
                )
            })
            .pinned(search_anchor, 0.);

        rect().width(Size::fill()).vertical().child(row)
    }
}
