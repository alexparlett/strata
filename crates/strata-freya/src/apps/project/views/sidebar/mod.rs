//! The left **sidebar**: the frame (P3-01), the catalog pane that fills it (P3-02) and the
//! Agents pane beside it (AA-03b).
//!
//! The shell owns the header row and the collapse (×); what sits to the left of the × is the
//! active pane's, per the design canvas — the catalog puts its **filter + refresh** there (there
//! is no "CATALOG" label; the filter field is the header), while Connections (W7) and Agents
//! keep a plain section label. The body below the divider is the pane itself.

mod agents;
mod catalog;

use freya::components::{CircularLoader, Input};
use freya::prelude::*;
use freya::radio::use_radio;
use strata_model::SidebarPane;

pub use self::agents::AgentsThemePreference;
use self::agents::{Agents, AgentsHint};
use self::catalog::Catalog;
pub use self::catalog::CatalogThemePreference;
/// The catalog's actions, on through to the command palette — see the catalog's own module.
pub use self::catalog::{open_saved_query, use_catalog_actions, view_row, CatalogActions};
use crate::apps::project::state::{
    refresh_catalog, use_catalog, use_catalog_rescan, Chan, SessionState,
};
use crate::components::divider::Divider;
use crate::components::icon::{Icon, IconName};
use crate::components::toolbar::{Toolbar, ToolbarItem};
use crate::components::typography::{Eyebrow, InputTypography};
use crate::theme::{use_roles, Role};

/// A pane header that is just its name — every pane but the catalog, whose filter field *is*
/// its header. `Size::flex` for the shell's reason: the row distributes, so a `fill` label
/// would push the collapse × off the panel.
///
/// The name sits in a flexing, clipping cell of its own rather than straight in the row. Flex
/// sizes the *wrapper*, but a text child still hugs, and `Overflow` defaults to painting outside
/// the box — so at a narrow width "AGENTS" drew over the ⓘ beside it and then over the collapse ×
/// (P5-06). Anything the caller adds after it keeps its room.
fn label(text: &'static str, color: Color) -> Rect {
    rect()
        .width(Size::flex(1.))
        .horizontal()
        .content(Content::Flex)
        .overflow(Overflow::Clip)
        .cross_align(Alignment::Center)
        .child(
            rect().width(Size::flex(1.)).overflow(Overflow::Clip).child(
                Eyebrow::new(text)
                    .color(color)
                    .text_overflow(TextOverflow::Ellipsis),
            ),
        )
}

/// The width of the header's **leading run** below which the catalog's filter field is dropped
/// rather than squeezed.
///
/// Measured on the leading run itself, so this is the room the field would actually get — not the
/// gross row width, from which the ↻, the pinned × and their gaps still have to come off. It was
/// briefly both: the probe moved onto the flex wrapper when the header became a `Toolbar` while
/// the threshold stayed calibrated for the row, which dropped the filter at panel widths with
/// ~112px going spare.
///
/// Below this the field has too little left to read a table name back in. Filtering stays
/// reachable through the command palette.
const CATALOG_FILTER_MIN: f32 = 80.;

/// The header row's height and the box of the flat controls in it (the canvas's 48 / 24).
const HEADER_HEIGHT: f32 = 48.;
const HEADER_CONTROL: f32 = 24.;

#[derive(PartialEq)]
pub struct Sidebar;

impl Sidebar {
    pub fn new() -> Self {
        Self
    }
}

impl Component for Sidebar {
    fn render(&self) -> impl IntoElement {
        let radio = use_radio::<SessionState, Chan>(Chan::Layout);
        let pane = radio.read().layout.sidebar.unwrap_or(SidebarPane::Catalog);
        let roles = use_roles();
        let (bg, border, label_color, faint) = (
            roles.get(Role::SurfaceRaised),
            roles.get(Role::Border),
            roles.get(Role::TextPlaceholder),
            roles.get(Role::TextPlaceholder),
        );

        // The catalog filter lives in the header beside the refresh button, but its consumer is
        // the tree below — so the shell owns the signal and hands it down.
        let filter = use_state(String::new);
        // The leading run's measured width, so the filter can get out of the way before it is too
        // narrow to type in (P5-06 rule 3: shrink, then hide). Local and per-mount — a fold
        // verdict is derived state, like `components::toolbar`'s.
        //
        // The fixed run either side of it (↻ and the pinned ×) is the same whichever branch wins,
        // so the measurement cannot oscillate between them.
        let mut leading_w = use_state(|| f32::INFINITY);
        let roomy = *leading_w.read() >= CATALOG_FILTER_MIN;

        let leading = match pane {
            // `Content::Flex` + a `Size::flex` field, *not* `Size::fill()`: fill takes the whole
            // parent width regardless of its siblings, so the filter ate the row and pushed ↻
            // out of the panel (the same trap `SidebarRow` documents). Flex distributes what is
            // left after the button's fixed 24px.
            // Below `CATALOG_FILTER_MIN` the field is dropped for the pane's name: an input too
            // narrow to read a word in is worse than none, and its magnifier was drawing over the
            // ↻ beside it. ↻ and the collapse × keep their room either way.
            SidebarPane::Catalog if !roomy => label("CATALOG", label_color).into_element(),
            SidebarPane::Catalog => rect()
                .width(Size::flex(1.))
                .horizontal()
                .content(Content::Flex)
                .overflow(Overflow::Clip)
                .cross_align(Alignment::Center)
                .spacing(8.)
                .child(
                    InputTypography::mono(
                        Input::new(filter)
                            .placeholder("Filter catalog…")
                            .compact()
                            .leading(Icon::new(IconName::Search).color(faint).size(13.))
                            .width(Size::flex(1.)),
                    )
                    .width(Size::flex(1.)),
                )
                .into_element(),
            SidebarPane::Connections => label("CONNECTIONS", label_color).into_element(),
            // The one pane header with something beside its name: the query-session model is
            // the single concept here a user has no other way to learn, so the canvas puts it
            // behind an ⓘ rather than in a line of pane copy nobody reads twice.
            SidebarPane::Agents => label("AGENTS", label_color)
                .spacing(6.)
                .child(AgentsHint)
                .into_element(),
        };

        let body = match pane {
            SidebarPane::Catalog => Catalog::new(filter).into_element(),
            // The connections pane is W7's; the frame is here so the rail's toggle has somewhere
            // to land.
            SidebarPane::Connections => rect().expanded().into_element(),
            SidebarPane::Agents => Agents::new().into_element(),
        };

        rect()
            .expanded()
            .background(bg)
            .vertical()
            .child(
                Toolbar::new()
                    .header()
                    .height(HEADER_HEIGHT)
                    .padding(12.)
                    // The pane's own run flexes, so the row distributes rather than hugs — else
                    // the collapse × is the thing that gets pushed out.
                    .leading(
                        rect()
                            .width(Size::flex(1.))
                            .horizontal()
                            .content(Content::Flex)
                            .overflow(Overflow::Clip)
                            .cross_align(Alignment::Center)
                            .on_sized(move |e: Event<SizedEventData>| {
                                leading_w.set_if_modified(e.area.width());
                            })
                            .child(leading),
                        0.,
                    )
                    // Re-scan folds into the `⋯` before the collapse × does, because × is pinned.
                    // The palette offers the same scan, so a folded row loses nothing.
                    .maybe(pane == SidebarPane::Catalog, |bar| {
                        bar.item(ToolbarItem::Custom {
                            width: HEADER_CONTROL,
                            inline: RefreshButton.into_element(),
                            folded: None,
                        })
                    })
                    // Pinned: it is how the user gets out of a squeezed panel, so it outranks
                    // everything the header could otherwise show.
                    .pinned(
                        Button::new()
                            .flat()
                            .width(Size::px(HEADER_CONTROL))
                            .height(Size::px(HEADER_CONTROL))
                            .on_press(move |_| {
                                let mut radio = radio;
                                radio.write_channel(Chan::Layout).close_sidebar();
                            })
                            .child(Icon::new(IconName::Close).size(13.)),
                        HEADER_CONTROL,
                    ),
            )
            .child(Divider::horizontal().color(border))
            .child(body)
    }
}

/// The catalog header's **↻ re-scan** (P3-03): ask for a re-infer of every table's schema from
/// its def and a re-create of the views over what that found — see
/// [`refresh_catalog`].
///
/// Its own component so the scan flag's subscription lives here rather than on the sidebar shell,
/// which would re-render the whole pane header twice per scan for a button swap.
///
/// Spins in place and disables for the duration — including the registration pass at project
/// open, which is the *same* scan and would otherwise be raced by a press.
///
/// The press only bumps the window's re-scan counter; the pass itself is spawned by the driver at
/// the window root. A task spawned from this handler would belong to *this* scope, and collapsing
/// the sidebar mid-scan would cancel a pass the whole catalog is waiting on.
#[derive(PartialEq)]
struct RefreshButton;

impl Component for RefreshButton {
    fn render(&self) -> impl IntoElement {
        let catalog = use_catalog();
        let rescan = use_catalog_rescan();
        let scanning = catalog.read().is_scanning();

        Button::new()
            .flat()
            .width(Size::px(24.))
            .height(Size::px(24.))
            .enabled(!scanning)
            .on_press(move |_| refresh_catalog(rescan))
            .child(if scanning {
                CircularLoader::new().size(13.).into_element()
            } else {
                Icon::new(IconName::Reload).size(14.).into_element()
            })
    }
}

/// Header **layout** tests: the pane header's controls must lay out *inside* the panel.
///
/// These exist because they didn't. The header is a horizontal row of a flexible run (the catalog
/// filter) plus fixed 24px controls (↻ re-scan, collapse ×), and the row was hugging its content
/// instead of distributing it — a `Size::fill()` filter takes the whole parent width regardless of
/// its siblings, so the trailing controls were pushed past the panel edge and clipped. The refresh
/// button shipped with P3-02 and was invisible until P3-03 went looking for it.
///
/// Asserting on *laid-out geometry* rather than on which element is which is deliberate: the bug
/// was never "the button isn't in the tree" — it was there the whole time, just off-screen.
#[cfg(test)]
mod tests {
    use crate::apps::project::state::{CatalogState, Log, PersistFaults};
    use std::path::PathBuf;

    use freya::radio::RadioStation;
    use freya_testing::TestingRunner;
    use strata_core::config::AppConfig;
    use strata_core::project::ProjectDefs;
    use strata_core::theme::load;
    use strata_model::{ColRef, SourceFormat, TableDef};

    use super::*;
    use crate::apps::configure::ConfigureTarget;
    use crate::apps::project::contexts::EngineCtx;
    use crate::apps::project::state::{ProjChan, ProjectState, ScanRequest, ScanScope};
    use crate::apps::project::views::{DropTarget, ProfileTarget};
    use crate::state::ConfigStation;
    use crate::theme::strata_theme;

    /// The panel width these tests lay out at — wide enough that nothing is clipped for want of
    /// room, so an out-of-bounds control is a layout fault and not a squeeze.
    const PANEL_WIDTH: f32 = 260.;
    /// The header's fixed controls are 24×24, in a 48px row (design canvas).
    const CONTROL: f32 = 24.;
    const HEADER_HEIGHT: f32 = 48.;
    /// The header's horizontal padding, per side.
    const HEADER_PAD: f32 = 12.;

    fn defs() -> ProjectDefs {
        ProjectDefs {
            name: "test".into(),
            tables: vec![TableDef {
                name: "orders".into(),
                format: SourceFormat::Parquet,
                sources: vec!["orders.parquet".into()],
                partition_cols: vec![],
            }],
            views: Vec::new(),
            saved_queries: Vec::new(),
            ..Default::default()
        }
    }

    fn app() -> impl IntoElement {
        use_init_theme(|| strata_theme(&load("midnight")));
        Sidebar::new()
    }

    /// The sidebar mounted over the contexts its shell + catalog pane consume, plus the re-scan
    /// counter handed back so a test can see what ↻ did. The engine is real but never asked
    /// anything: there is no scan *driver* here — that lives at the window root — so pressing ↻
    /// raises a request and nothing else, which is exactly the button's whole contract.
    fn runner() -> (TestingRunner, State<ScanRequest>) {
        TestingRunner::new(
            app,
            (PANEL_WIDTH, 700.).into(),
            |r| {
                r.provide_root_context(EngineCtx::default);
                // Catalog · CatalogRescan · CatalogSelection — the three context signals
                // the pane's header and rows consume (`state/catalog.rs`).
                r.provide_root_context(|| State::create(CatalogState::Settled(0)));
                let rescan = r.provide_root_context(|| State::create(ScanRequest::default()));
                r.provide_root_context(|| State::create(None::<ColRef>));
                // The catalog rows' menu handles (P3-06): the app config behind "View table"'s
                // LIMIT, and the drop- / profile-confirm slots. Nothing here opens a menu — they
                // only have to be reachable, since every row gathers them on render.
                r.provide_root_context(|| ConfigStation::create(AppConfig::default()));
                r.provide_root_context(|| State::create(None::<DropTarget>));
                r.provide_root_context(|| State::create(None::<ProfileTarget>));
                // The Configure-window request slot (P4-11): the TABLES `+` and the row menus
                // set it, and the project root's launcher — not mounted here — acts on it.
                r.provide_root_context(|| State::create(None::<ConfigureTarget>));
                // Where the catalog's row menus report the one action that writes
                // `project.json` inline (the saved-query rename, P4-15).
                r.provide_root_context(|| State::create(Log::default()));
                r.provide_root_context(|| State::create(PersistFaults::default()));
                r.provide_root_context(|| {
                    RadioStation::<SessionState, Chan>::create(SessionState::default())
                });
                r.provide_root_context(|| {
                    RadioStation::<ProjectState, ProjChan>::create(ProjectState::from_defs(
                        defs(),
                        PathBuf::from("/tmp/strata-sidebar-test"),
                    ))
                });
                rescan
            },
            1.,
        )
    }

    /// The header's two fixed 24×24 controls, left to right: ↻ then the collapse ×.
    fn header_controls(runner: &TestingRunner) -> Vec<Box2> {
        let mut controls: Vec<_> = header_content(runner)
            .into_iter()
            .filter(|b| (b.width - CONTROL).abs() < 0.5 && (b.height - CONTROL).abs() < 0.5)
            .collect();
        controls.sort_by(|a, b| a.min_x.total_cmp(&b.min_x));
        controls
    }

    /// One laid-out box, in the terms these tests reason about.
    #[derive(Debug, Clone, Copy)]
    struct Box2 {
        min_x: f32,
        max_x: f32,
        max_y: f32,
        width: f32,
        height: f32,
    }

    /// Every laid-out box in the pane.
    fn areas(runner: &TestingRunner) -> Vec<Box2> {
        runner.find_many(|node, _| {
            let a = node.layout().area;
            Some(Box2 {
                min_x: a.min_x(),
                max_x: a.max_x(),
                max_y: a.max_y(),
                width: a.width(),
                height: a.height(),
            })
        })
    }

    /// The boxes inside the 48px header row, excluding the row (and panel) itself — i.e. the
    /// header's actual content.
    fn header_content(runner: &TestingRunner) -> Vec<Box2> {
        areas(runner)
            .into_iter()
            .filter(|b| b.max_y <= HEADER_HEIGHT + 0.5 && b.width < PANEL_WIDTH)
            .collect()
    }

    /// The headline regression: **nothing** in the sidebar extends past the panel's right edge. A
    /// control pushed out is invisible to the user however correct the element tree is.
    #[test]
    fn nothing_in_the_pane_is_laid_out_past_the_panel_edge() {
        let (mut runner, _) = runner();
        runner.sync_and_update();
        runner.sync_and_update();

        let overflowing: Vec<_> = areas(&runner)
            .into_iter()
            .filter(|b| b.width > 0. && b.max_x > PANEL_WIDTH + 0.5)
            .collect();

        assert!(
            overflowing.is_empty(),
            "laid out past the {PANEL_WIDTH}px panel edge: {overflowing:?}"
        );
    }

    /// Both of the header's fixed 24×24 controls — ↻ re-scan and the collapse × — are on screen,
    /// side by side in the 48px header. Counting them is what catches the case the bounds test
    /// can't: a squeezed-to-nothing control has no area to overflow with.
    #[test]
    fn both_header_controls_are_present_and_on_screen() {
        let (mut runner, _) = runner();
        runner.sync_and_update();
        runner.sync_and_update();

        let controls = header_controls(&runner);

        assert_eq!(
            controls.len(),
            2,
            "expected the ↻ and × controls at their full 24×24: {controls:?}"
        );
        for b in &controls {
            assert!(
                b.min_x >= 0. && b.max_x <= PANEL_WIDTH,
                "control at {}..{} is outside 0..{PANEL_WIDTH}",
                b.min_x,
                b.max_x
            );
            // Both trail the filter, at the right-hand end of the header.
            assert!(
                b.max_x > PANEL_WIDTH / 2.,
                "the controls trail the filter: {b:?}"
            );
        }
    }

    /// The filter takes the slack rather than the whole row: it must leave room for ↻ beside it.
    /// This is the `Size::flex` vs `Size::fill` distinction the header got wrong — with `fill` the
    /// field spans the full content box and the button is pushed off the end.
    #[test]
    fn the_filter_leaves_room_for_the_refresh_button() {
        let (mut runner, _) = runner();
        runner.sync_and_update();
        runner.sync_and_update();

        // The header's content box: the panel less its horizontal padding.
        let content = PANEL_WIDTH - 2. * HEADER_PAD;
        // The widest run in the header that isn't the header row itself — the filter and the
        // wrapper it flexes inside. With `fill` this is the whole content box; with `flex` it
        // stops short of the ↻ beside it.
        let widest = header_content(&runner)
            .into_iter()
            .map(|b| b.width)
            .fold(0., f32::max);

        assert!(widest > 0., "the header laid out nothing");
        assert!(
            widest <= content - CONTROL,
            "the filter run ({widest}px) must leave the {CONTROL}px ↻ room inside the {content}px \
             content box"
        );
    }

    /// Pressing ↻ raises a re-scan **request** — and that is all it does. The pass belongs to the
    /// window root's driver (`use_init_project`), because a task spawned from this handler would
    /// be cancelled the moment the sidebar collapses, stranding every catalog row in
    /// `Reg::Loading`. So the button's whole contract is this counter, and the test drives it the
    /// way the user does rather than calling `refresh_catalog` directly.
    #[test]
    fn pressing_refresh_raises_a_rescan_request() {
        let (mut runner, rescan) = runner();
        runner.sync_and_update();
        runner.sync_and_update();
        assert_eq!(
            *rescan.peek(),
            ScanRequest::default(),
            "nothing asked for yet"
        );

        let refresh = header_controls(&runner)[0];
        let point = (
            ((refresh.min_x + refresh.max_x) / 2.) as f64,
            (HEADER_HEIGHT / 2.) as f64,
        );
        runner.move_cursor(point);
        runner.click_cursor(point);
        runner.sync_and_update();
        runner.sync_and_update();

        assert_eq!(
            *rescan.peek(),
            ScanRequest {
                seq: 1,
                scope: ScanScope::All
            },
            "↻ asked for a re-scan"
        );
    }
}
