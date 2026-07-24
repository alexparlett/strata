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
                        .child(Drawer::new()),
                )
        });

        // Right area: [ panes-row (fills) | drawer? ] stacked vertically.
        let right_area = ResizableContainer::new()
            .direction(Direction::Vertical)
            .handle_size(1.)
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
