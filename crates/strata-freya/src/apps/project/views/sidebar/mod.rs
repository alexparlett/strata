//! The left **sidebar**: the frame (P3-01) and the data-sources tree that fills it (P3-02 · W7 ·
//! DB-05).
//!
//! One pane, because there is one question: what data do I have. The tree answers it for the
//! project's own catalog and for every data source beside it, so the Data sources pane that used to
//! sit next to this one is gone and the rail has one fewer toggle.
//!
//! The shell owns the header row and the collapse (×); what sits to the left of the × is the
//! pane's, per the design canvas — there is no "CATALOG" label, the filter field *is* the header,
//! and the `+` (a new data source) and ↻ (a re-scan) follow it. The body below the divider is the
//! tree itself.

mod catalog;

use freya::components::{CircularLoader, Input};
use freya::prelude::*;
use freya::radio::use_radio;

use self::catalog::Catalog;
pub use self::catalog::CatalogThemePreference;
/// The catalog's actions, on through to the command palette — see the catalog's own module.
pub use self::catalog::{open_saved_query, use_catalog_actions, view_row, CatalogActions};
use crate::apps::project::state::{
    refresh_catalog, use_catalog, use_catalog_rescan, Chan, SessionState,
};
use crate::apps::project::views::SourceRequest;
use crate::apps::source::SourceTarget;
use crate::components::divider::Divider;
use crate::components::icon::{Icon, IconName};
use crate::components::metrics::{HEADER_CONTROL, SIDEBAR_HEADER_HEIGHT, SP_3, SP_4};
use crate::components::toolbar::{Toolbar, ToolbarItem};
use crate::components::typography::{Eyebrow, InputTypography};
use crate::theme::{use_roles, Role};

/// A pane header that is just its name — what the tree falls back to when the filter field has
/// too little room to be worth showing. `Size::flex` for the shell's reason: the row distributes,
/// so a `fill` label would push the collapse × off the panel.
///
/// The name sits in a flexing, clipping cell of its own rather than straight in the row. Flex
/// sizes the *wrapper*, but a text child still hugs, and `Overflow` defaults to painting outside
/// the box — so at a narrow width the name drew over the controls beside it and then over the
/// collapse × (P5-06). Anything the caller adds after it keeps its room.
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

/// The width of the header's **leading run** below which the filter field is dropped rather than
/// squeezed.
///
/// Measured on the leading run itself, so this is the room the field would actually get — not the
/// gross row width, from which the controls, the pinned × and their gaps still have to come off.
/// It was briefly both: the probe moved onto the flex wrapper when the header became a `Toolbar`
/// while the threshold stayed calibrated for the row, which dropped the filter at panel widths
/// with ~112px going spare.
///
/// Below this the field has too little left to read a table name back in. Filtering stays
/// reachable through the command palette.
const CATALOG_FILTER_MIN: f32 = 80.;

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
        let roles = use_roles();
        let (bg, border, label_color, faint) = (
            roles.get(Role::SurfaceRaised),
            roles.get(Role::Border),
            roles.get(Role::TextPlaceholder),
            roles.get(Role::TextPlaceholder),
        );

        let filter = use_state(String::new);
        let mut leading_w = use_state(|| f32::INFINITY);
        let roomy = *leading_w.read() >= CATALOG_FILTER_MIN;

        let leading = match roomy {
            false => label("DATA", label_color).into_element(),
            true => rect()
                .width(Size::flex(1.))
                .horizontal()
                .content(Content::Flex)
                .overflow(Overflow::Clip)
                .cross_align(Alignment::Center)
                .spacing(SP_3)
                .child(
                    InputTypography::mono(
                        Input::new(filter)
                            .placeholder("Filter data sources…")
                            .compact()
                            .leading(Icon::new(IconName::Search).color(faint).size(13.))
                            .width(Size::flex(1.)),
                    )
                    .width(Size::flex(1.)),
                )
                .into_element(),
        };

        rect()
            .expanded()
            .background(bg)
            .vertical()
            .child(
                Toolbar::new()
                    .header()
                    .height(SIDEBAR_HEADER_HEIGHT)
                    .padding(SP_4)
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
                    .item(ToolbarItem::Custom {
                        width: HEADER_CONTROL,
                        inline: AddSourceButton.into_element(),
                        folded: None,
                    })
                    .item(ToolbarItem::Custom {
                        width: HEADER_CONTROL,
                        inline: RefreshButton.into_element(),
                        folded: None,
                    })
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
            .child(Catalog::new(filter))
    }
}

/// The header's **+** — a new data source, which is now a top-level node of this tree.
///
/// **It folds under panel pressure** (`ToolbarItem::Custom { folded: None }`, the ↻'s terms). Its
/// two other entry points are the tree's own empty state and the command palette's *New
/// data source…*, which is what makes the fold cost nothing.
#[derive(PartialEq)]
struct AddSourceButton;

impl Component for AddSourceButton {
    fn render(&self) -> impl IntoElement {
        let editor = use_consume::<SourceRequest>();
        TooltipContainer::new(Tooltip::new_text("Add data source"))
            .position(AttachedPosition::Bottom)
            .child(
                Button::new()
                    .flat()
                    .width(Size::px(HEADER_CONTROL))
                    .height(Size::px(HEADER_CONTROL))
                    .on_press(move |_: Event<PressEventData>| {
                        let mut editor = editor;
                        editor.set(Some(SourceTarget::New));
                    })
                    .child(Icon::new(IconName::Plus).size(14.)),
            )
    }
}

/// The header's **↻ re-scan** (P3-03): ask for a re-connect of every data source, a re-infer of
/// every table's schema from its def, and a re-create of the views over what that found — see
/// [`refresh_catalog`]. On a database source the re-connect *is* the refresh: its schemas and
/// relations are the connect-time enumeration.
///
/// Its own component so the scan flag's subscription lives here rather than on the sidebar shell,
/// which would re-render the whole pane header twice per scan for a button swap.
///
/// Spins in place and disables for the duration — including the registration pass at project
/// open, which is the *same* scan and would otherwise be raced by a press.
///
/// The press only bumps the window's re-scan counter; the pass itself is spawned by the driver at
/// the window root. A task spawned from this handler would belong to *this* scope, and collapsing
/// the sidebar mid-scan would cancel a pass the whole tree is waiting on.
#[derive(PartialEq)]
struct RefreshButton;

impl Component for RefreshButton {
    fn render(&self) -> impl IntoElement {
        let catalog = use_catalog();
        let rescan = use_catalog_rescan();
        let scanning = catalog.read().is_scanning();

        Button::new()
            .flat()
            .width(Size::px(HEADER_CONTROL))
            .height(Size::px(HEADER_CONTROL))
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
/// These exist because they didn't. The header is a horizontal row of a flexible run (the filter)
/// plus fixed 24px controls (`+`, ↻ re-scan, collapse ×), and the row was hugging its content
/// instead of distributing it — a `Size::fill()` filter takes the whole parent width regardless of
/// its siblings, so the trailing controls were pushed past the panel edge and clipped. The refresh
/// button shipped with P3-02 and was invisible until P3-03 went looking for it.
///
/// Asserting on *laid-out geometry* rather than on which element is which is deliberate: the bug
/// was never "the button isn't in the tree" — it was there the whole time, just off-screen.
#[cfg(test)]
mod tests {
    use crate::apps::project::state::{CatalogState, Chats, Log, PersistFaults, Pick};
    use std::path::PathBuf;
    use strata_engine::Registrations;

    use freya::radio::RadioStation;
    use freya_testing::TestingRunner;
    use strata_core::config::AppConfig;
    use strata_core::project::ProjectDefs;
    use strata_core::theme::load;
    use strata_model::{ColRef, RemoteRef, SourceFormat, TableDef, TableOrigin};

    use super::*;
    use crate::apps::configure::ConfigureTarget;
    use crate::apps::project::contexts::EngineCtx;
    use crate::apps::project::query::{ProfileTarget, ScanId};
    use crate::apps::project::state::{ProjChan, ProjectState, ScanRequest, ScanScope};
    use crate::apps::project::views::{DropTarget, SchemasRequest};
    use crate::state::ConfigStation;
    use crate::theme::strata_theme;

    /// The panel width these tests lay out at — wide enough that nothing is clipped for want of
    /// room, so an out-of-bounds control is a layout fault and not a squeeze.
    const PANEL_WIDTH: f32 = 260.;
    /// The header's fixed controls are 24×24, in a 48px row (design canvas).
    const CONTROL: f32 = 24.;
    const SIDEBAR_HEADER_HEIGHT: f32 = 48.;
    /// The header's horizontal padding, per side.
    const HEADER_PAD: f32 = SP_4;

    fn defs() -> ProjectDefs {
        ProjectDefs {
            name: "test".into(),
            tables: vec![TableDef {
                name: "orders".into(),
                format: SourceFormat::Parquet,
                source: None,
                paths: vec!["orders.parquet".into()],
                partition_cols: vec![],
                origin: TableOrigin::External,
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

    /// The sidebar mounted over the contexts its shell + tree consume, plus the re-scan counter
    /// handed back so a test can see what ↻ did. The engine is real but never asked anything:
    /// there is no scan *driver* here — that lives at the window root — so pressing ↻ raises a
    /// request and nothing else, which is exactly the button's whole contract.
    fn runner() -> (TestingRunner, State<ScanRequest>) {
        TestingRunner::new(
            app,
            (PANEL_WIDTH, 700.).into(),
            move |r| {
                r.provide_root_context(EngineCtx::default);
                r.provide_root_context(|| State::create(CatalogState::Cold));
                r.provide_root_context(|| State::create(Registrations::default()));
                let rescan = r.provide_root_context(|| State::create(ScanRequest::default()));
                r.provide_root_context(|| State::create(None::<ColRef>));
                r.provide_root_context(|| ConfigStation::create(AppConfig::default()));
                r.provide_root_context(|| State::create(None::<DropTarget>));
                r.provide_root_context(|| State::create(None::<ProfileTarget>));
                r.provide_root_context(|| {
                    State::create(std::collections::BTreeMap::<RemoteRef, ScanId>::new())
                });
                r.provide_root_context(|| State::create(None::<ConfigureTarget>));
                r.provide_root_context(|| State::create(None::<SourceTarget>));
                r.provide_root_context(|| State::create(None::<String>) as SchemasRequest);
                r.provide_root_context(|| State::create(Log::default()));
                r.provide_root_context(|| State::create(PersistFaults::default()));
                r.provide_root_context(|| State::create(Chats::new(Pick::default())));
                r.provide_root_context(move || {
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

    /// The header's three fixed 24×24 controls, left to right: `+`, ↻, then the collapse ×.
    fn header_controls(runner: &TestingRunner) -> Vec<Box2> {
        let mut controls: Vec<_> = header_content(runner)
            .into_iter()
            .filter(|b| (b.width - CONTROL).abs() < 0.5 && (b.height - CONTROL).abs() < 0.5)
            .collect();
        controls.sort_by(|a, b| a.min_x.total_cmp(&b.min_x));
        controls.dedup_by(|a, b| (a.min_x - b.min_x).abs() < 0.5);
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
            .filter(|b| b.max_y <= SIDEBAR_HEADER_HEIGHT + 0.5 && b.width < PANEL_WIDTH)
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

    /// All three of the header's fixed 24×24 controls — `+`, ↻ re-scan and the collapse × — are
    /// on screen, side by side in the 48px header. Counting them is what catches the case the
    /// bounds test can't: a squeezed-to-nothing control has no area to overflow with.
    #[test]
    fn every_header_control_is_present_and_on_screen() {
        let (mut runner, _) = runner();
        runner.sync_and_update();
        runner.sync_and_update();

        let controls = header_controls(&runner);

        assert_eq!(
            controls.len(),
            3,
            "expected the +, ↻ and × controls at their full 24×24: {controls:?}"
        );
        for b in &controls {
            assert!(
                b.min_x >= 0. && b.max_x <= PANEL_WIDTH,
                "control at {}..{} is outside 0..{PANEL_WIDTH}",
                b.min_x,
                b.max_x
            );
            assert!(
                b.max_x > PANEL_WIDTH / 2.,
                "the controls trail the filter: {b:?}"
            );
        }
    }

    /// The filter takes the slack rather than the whole row: it must leave room for the controls
    /// beside it. This is the `Size::flex` vs `Size::fill` distinction the header got wrong — with
    /// `fill` the field spans the full content box and the buttons are pushed off the end.
    #[test]
    fn the_filter_leaves_room_for_the_header_controls() {
        let (mut runner, _) = runner();
        runner.sync_and_update();
        runner.sync_and_update();

        let content = PANEL_WIDTH - 2. * HEADER_PAD;
        let widest = header_content(&runner)
            .into_iter()
            .map(|b| b.width)
            .fold(0., f32::max);

        assert!(widest > 0., "the header laid out nothing");
        assert!(
            widest <= content - CONTROL,
            "the filter run ({widest}px) must leave the {CONTROL}px controls room inside the \
             {content}px content box"
        );
    }

    /// Pressing ↻ raises a re-scan **request** — and that is all it does. The pass belongs to the
    /// window root's driver (`use_init_project`), because a task spawned from this handler would
    /// be cancelled the moment the sidebar collapses, leaving every tree row unanswered.
    /// So the button's whole contract is this counter, and the test drives it the way the user
    /// does rather than calling `refresh_catalog` directly.
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

        let refresh = header_controls(&runner)[1];
        let point = (
            ((refresh.min_x + refresh.max_x) / 2.) as f64,
            (SIDEBAR_HEADER_HEIGHT / 2.) as f64,
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
