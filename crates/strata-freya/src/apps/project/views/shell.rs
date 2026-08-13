//! The project window **body shell** (P3-01) — the two rails · sidebar · workbench · right pane ·
//! drawer frame (design `Strata.dc.html` `data-rg` regions). Mounted between the header and the
//! workbench in `project.rs`, it composes:
//!
//! ```text
//! [ rail | ── right area (vertical resizable) ─────────────── | right rail ]
//!        |  [ sidebar | workbench | right pane ]  (horizontal resizable, panes-row)
//!        |  [ ──────────── drawer ──────────── ]  (collapsible bottom panel)
//! ```
//!
//! The **right pane is one slot** (AS-04): the right rail picks the inspector or the chat, the
//! way the left rail picks a sidebar pane. Both rails are full height, so the drawer sits between
//! them.
//!
//! Both rails are fixed (48px, always visible); the sidebar / right pane / drawer are collapsible
//! `ResizableContainer` panels, present only when the layout has them open. Each reports its
//! dragged size back to the layout (`Chan::LayoutSize`) so a reopen or a restart restores it, and
//! the panels are **keyed** so the `Workbench` subtree survives a sibling collapsing and shifting
//! its position.
//!
//! The vertical container is driven through an explicit [`ResizableContext`] controller, because
//! one size change does not come from a drag: the drawer's expand toggle. `ResizablePanel` reads
//! `initial_size` once in a `use_hook`, so programmatic sizing has to go through the controller.
//!
//! **Behaviour when it runs out of room** follows RustRover rather than the design canvas, which
//! declares `min-width: 1180px` and has no narrow states to port. Five rules, in order:
//!
//! 1. **Nothing has a usability floor; everything has a stub floor.** [`PANEL_STUB_W`] and
//!    [`WORKBENCH_STUB_H`] exist so a panel cannot become a sliver too thin to grab, not to keep it
//!    useful. The rail is exempt.
//! 2. **Space is given up in a stated order**: the main pane is the container's *proportional*
//!    panel, so it gives first and gives everything; the pixel side panels start giving only once
//!    it is on `min_pixels`, and then equally. That is the sizing model, not a policy written here.
//! 3. **Chrome shrinks its flexible run, then folds into an overflow menu** — `components::toolbar`.
//! 4. **Nothing a drag or a squeeze does ever closes a panel.** Both stop at the stub, with the
//!    handle still there to pull back out. `ResizablePanel::on_collapse` was tried and rejected: a
//!    panel that vanishes mid-drag reads as having been lost.
//! 5. **A body scrolls; chrome never does.** Vertically only — a long identifier ellipsizes.
//!
//! Nothing overlaps at any width because each panel rect is `Overflow::Clip`, a flex panel is never
//! measured negative, and each chrome row folds or ellipsizes inside its own box.

use freya::prelude::*;
use freya::radio::use_radio;
use strata_model::RightPane;

use super::chat::ChatPane;
use super::drawer::Drawer;
use super::inspector::Inspector;
use super::rail::ActivityRail;
use super::right_rail::RightRail;
use super::sidebar::Sidebar;
use super::workbench::WORKBENCH_STUB_H;
use super::Workbench;
use crate::apps::project::state::{Chan, SessionState};
use crate::components::divider::Divider;
use crate::theme::{use_roles, Role};

/// The narrowest a side panel may become: enough for its collapse × plus the header's padding,
/// and enough that the resize handle beside it is still there to drag back out with.
///
/// Deliberately **not** a usability floor. A panel this narrow is unreadable, and that is the
/// point: the app follows RustRover in having no minimum worth the name, so what a floor is for
/// is keeping the panel grabbable. Dragging into it **pins** here rather than closing the panel —
/// IntelliJ keeps a very small minimum and leaves the splitter working, and a drag that quietly
/// closed a panel turned out to read as losing it. Closing is the rail button's job, and the
/// header ×'s.
const PANEL_STUB_W: f32 = 48.;

/// The shortest the drawer may become: its 36px header over a single row.
const DRAWER_STUB_H: f32 = 72.;

/// The widths the design canvas clamps each side panel to (`Strata.dc.html` `onResize*`). Freya
/// grew `ResizablePanel::max_size` for these; before that they had nowhere to live.
const SIDEBAR_MAX_W: f32 = 520.;
const INSPECTOR_MAX_W: f32 = 560.;
/// The chat's own ceiling (`Strata.dc.html` `onResizeChat`), wider than the inspector's floor
/// allows for because a transcript is prose and the inspector is a fact list.
const CHAT_MAX_W: f32 = 620.;
const DRAWER_MAX_H: f32 = 680.;

#[derive(PartialEq)]
pub struct Shell;

impl Shell {
    pub fn new() -> Self {
        Self
    }
}

impl Component for Shell {
    fn render(&self) -> impl IntoElement {
        let radio = use_radio::<SessionState, Chan>(Chan::Layout);
        let layout = radio.read().layout;
        let border = use_roles().get(Role::Border);

        let drawer_sizing = use_state(|| ResizableContext {
            direction: Direction::Vertical,
            handle_size: 1.,
            ..ResizableContext::default()
        });

        let sidebar_panel = layout.sidebar.map(|_| {
            ResizablePanel::new(PanelSize::px(layout.sidebar_w))
                .min_size(PANEL_STUB_W)
                .max_size(SIDEBAR_MAX_W)
                .key("sidebar")
                .order(0usize)
                .on_resized(move |w: f32| {
                    let mut radio = radio;
                    if radio.read().layout.sidebar_w != w {
                        radio.write_channel(Chan::LayoutSize).set_sidebar_w(w);
                    }
                })
                .child(rect().expanded().child(Sidebar::new()))
        });
        let right_panel = layout.right.map(|pane| {
            let (width, key, max, body): (f32, &str, f32, Element) = match pane {
                RightPane::Inspector => (
                    layout.inspector_w,
                    "inspector",
                    INSPECTOR_MAX_W,
                    Inspector::new().into_element(),
                ),
                RightPane::Chat => (layout.chat_w, "chat", CHAT_MAX_W, ChatPane.into_element()),
            };
            ResizablePanel::new(PanelSize::px(width))
                .min_size(PANEL_STUB_W)
                .max_size(max)
                .key(key)
                .order(2usize)
                .on_resized(move |w: f32| {
                    let mut radio = radio;
                    let layout = radio.read().layout;
                    match pane {
                        RightPane::Inspector if layout.inspector_w != w => {
                            radio.write_channel(Chan::LayoutSize).set_inspector_w(w);
                        }
                        RightPane::Chat if layout.chat_w != w => {
                            radio.write_channel(Chan::LayoutSize).set_chat_w(w);
                        }
                        _ => {}
                    }
                })
                .child(rect().expanded().child(body))
        });

        let panes_row = ResizableContainer::new()
            .direction(Direction::Horizontal)
            .handle_size(1.)
            .panel(sidebar_panel)
            .panel(
                ResizablePanel::new(PanelSize::percent(100.))
                    .min_pixels(PANEL_STUB_W)
                    .min_size(0.)
                    .key("main")
                    .order(1usize)
                    .child(Workbench),
            )
            .panel(right_panel);

        let drawer_panel = layout.drawer.map(|_| {
            ResizablePanel::new(PanelSize::px(layout.drawer_h))
                .min_size(DRAWER_STUB_H)
                .max_size(DRAWER_MAX_H)
                .key("drawer")
                .order(1usize)
                .on_resized(move |h: f32| {
                    let mut radio = radio;
                    if radio.read().layout.drawer_h != h {
                        radio.write_channel(Chan::LayoutSize).set_drawer_h(h);
                    }
                })
                .child(rect().expanded().child(Drawer::new(drawer_sizing)))
        });

        let right_area = ResizableContainer::new()
            .direction(Direction::Vertical)
            .controller(drawer_sizing)
            .panel(
                ResizablePanel::new(PanelSize::percent(100.))
                    .min_pixels(WORKBENCH_STUB_H)
                    .min_size(0.)
                    .key("panes")
                    .order(0usize)
                    .child(panes_row),
            )
            .panel(drawer_panel);

        rect()
            .expanded()
            .horizontal()
            .content(Content::Flex)
            .child(ActivityRail::new())
            .child(Divider::vertical().color(border))
            .child(
                rect()
                    .width(Size::flex(1.))
                    .height(Size::fill())
                    .child(right_area),
            )
            .child(Divider::vertical().color(border))
            .child(RightRail::new())
    }
}

/// Move the **mounted** drawer panel to `h`, in the vertical container `sizing` drives.
///
/// The drawer's expand toggle writes the new height to the layout, which re-seeds
/// `initial_size` for the panel's *next* mount; this is the other half, moving the panel that
/// is on screen now.
///
/// The drawer is that container's only pixel-sized panel — the panes row above it is
/// `PanelSize::percent(100.)`, which is what makes it take the slack when the drawer grows.
/// A collapsed drawer has no panel registered at all, and nothing to move.
pub fn set_drawer_panel_height(mut sizing: State<ResizableContext>, h: f32) {
    let mut sizing = sizing.write();
    if let Some(panel) = sizing
        .panels()
        .iter_mut()
        .find(|p| matches!(p.sizing, PanelSize::Pixels(_)))
    {
        panel.size = h;
        panel.desired = h;
    }
}
