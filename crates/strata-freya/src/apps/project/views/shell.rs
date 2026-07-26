//! The project window **body shell** (P3-01) — the rail · sidebar · workbench · inspector · drawer
//! frame (design `Strata.dc.html` `data-rg` regions). Mounted between the header and the workbench
//! in `project.rs`, it composes:
//!
//! ```text
//! [ rail | ── right area (vertical resizable) ─────────────── ]
//!        |  [ sidebar | workbench | inspector ]  (horizontal resizable, panes-row)
//!        |  [ ─────────── drawer ─────────── ]   (collapsible bottom panel)
//! ```
//!
//! The rail is fixed (48px, always visible); the sidebar / inspector / drawer are collapsible
//! `ResizableContainer` panels — present only when the layout has them open, so collapsing a panel
//! removes it *and* its handle. `ResizableContainer` owns live resizing; each collapsible region
//! reports its dragged size back to the layout (`Chan::LayoutSize`) so a reopen or a restart
//! restores it (the shell seeds each panel's `initial_size` from the remembered size). The panels
//! are **keyed** so the `Workbench` subtree (editor buffer, in-flight query) survives a sibling
//! collapsing and shifting its position.
//!
//! The vertical container is driven through an explicit [`ResizableContext`] controller, because
//! one size change doesn't come from a drag: the drawer's expand toggle. Seeding `initial_size`
//! can't move a mounted panel — `ResizablePanel` reads it once, in a `use_hook` — so programmatic
//! sizing goes through the controller, which is what the fork's own
//! `component_resizable_panel_controller` example does.

use freya::components::use_theme;
use freya::prelude::*;
use freya::radio::use_radio;

use super::drawer::Drawer;
use super::inspector::Inspector;
use super::rail::ActivityRail;
use super::sidebar::Sidebar;
use super::Workbench;
use crate::apps::project::state::{Chan, SessionState};
use crate::components::divider::Divider;

#[derive(PartialEq)]
pub struct Shell;

impl Shell {
    pub fn new() -> Self {
        Self
    }
}

impl Component for Shell {
    fn render(&self) -> impl IntoElement {
        // Subscribe on `Chan::Layout`: a collapse / toggle re-renders the shell (and re-seeds the
        // panels from the remembered sizes), but a resize drag — which writes `Chan::LayoutSize`,
        // deriving only `Persist` — never wakes it, so the drag runs churn-free.
        let radio = use_radio::<SessionState, Chan>(Chan::Layout);
        let layout = radio.read().layout;
        let border = use_theme().read().colors.border;

        // The vertical container's controller (see the module note). Supplying one means its
        // `direction` / `handle_size` come from here rather than the builder — the builder's
        // `.handle_size` is documented as ignored when a controller is given.
        let drawer_sizing = use_state(|| ResizableContext {
            direction: Direction::Vertical,
            handle_size: 1.,
            ..ResizableContext::default()
        });

        // Sidebar / inspector panels: present only when open. Each wraps its shell in a sizing
        // probe (`on_sized`) that remembers the dragged width on `Chan::LayoutSize`.
        let sidebar_panel = layout.sidebar.map(|_| {
            ResizablePanel::new(PanelSize::px(layout.sidebar_w))
                .min_size(210.)
                .key("sidebar")
                .order(0usize)
                .child(
                    rect()
                        .expanded()
                        .on_sized(move |e: Event<SizedEventData>| {
                            let w = e.area.width();
                            let mut radio = radio;
                            // Only a real resize writes — the mount-time probe reports the seeded
                            // size, and rewriting it would wake autosave (and rewrite session.json)
                            // on every launch for no change.
                            if radio.read().layout.sidebar_w != w {
                                radio.write_channel(Chan::LayoutSize).set_sidebar_w(w);
                            }
                        })
                        .child(Sidebar::new()),
                )
        });
        let inspector_panel = layout.inspector_open.then(|| {
            ResizablePanel::new(PanelSize::px(layout.inspector_w))
                .min_size(220.)
                .key("inspector")
                .order(2usize)
                .child(
                    rect()
                        .expanded()
                        .on_sized(move |e: Event<SizedEventData>| {
                            let w = e.area.width();
                            let mut radio = radio;
                            if radio.read().layout.inspector_w != w {
                                radio.write_channel(Chan::LayoutSize).set_inspector_w(w);
                            }
                        })
                        .child(Inspector::new()),
                )
        });

        // panes-row: [ sidebar? | workbench (fills) | inspector? ]. The workbench panel is keyed
        // so it isn't remounted when a sibling collapses.
        let panes_row = ResizableContainer::new()
            .direction(Direction::Horizontal)
            .handle_size(1.)
            .panel(sidebar_panel)
            .panel(
                ResizablePanel::new(PanelSize::percent(100.))
                    .key("main")
                    .order(1usize)
                    .child(Workbench),
            )
            .panel(inspector_panel);

        // Drawer panel: present only when open; remembers its dragged height.
        let drawer_panel = layout.drawer.map(|_| {
            ResizablePanel::new(PanelSize::px(layout.drawer_h))
                .min_size(140.)
                .key("drawer")
                .order(1usize)
                .child(
                    rect()
                        .expanded()
                        .on_sized(move |e: Event<SizedEventData>| {
                            let h = e.area.height();
                            let mut radio = radio;
                            if radio.read().layout.drawer_h != h {
                                radio.write_channel(Chan::LayoutSize).set_drawer_h(h);
                            }
                        })
                        .child(Drawer::new(drawer_sizing)),
                )
        });

        // Right area: [ panes-row (fills) | drawer? ] stacked vertically.
        let right_area = ResizableContainer::new()
            .direction(Direction::Vertical)
            .controller(drawer_sizing)
            .panel(
                ResizablePanel::new(PanelSize::percent(100.))
                    .key("panes")
                    .order(0usize)
                    .child(panes_row),
            )
            .panel(drawer_panel);

        // Body row: the fixed rail, a 1px rule, then the resizable right area.
        rect()
            .expanded()
            .horizontal()
            .child(ActivityRail::new())
            .child(Divider::vertical().color(border))
            .child(right_area)
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
    }
}
