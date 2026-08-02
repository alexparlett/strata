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
//!
//! # How the shell behaves when it runs out of room (P5-06)
//!
//! The reference is RustRover, not the design canvas: `Strata.dc.html` declares
//! `min-width: 1180px` on the app root and scrolls the page below it, so it has no narrow states
//! to port. JetBrains' answer is the opposite of a minimum — *"it is not possible to enforce
//! minimal tool window size, and it is up to users to resize it to their needs"* — and a
//! RustRover window squeezed to ~680px keeps both tool windows **open**, wrapping their text,
//! while the editor between them is reduced to a stub. Five rules, in order:
//!
//! 1. **Nothing has a usability floor; everything has a stub floor.** [`PANEL_STUB_W`] and the
//!    workbench's own [`WORKBENCH_STUB_H`] exist so a panel cannot become a sliver too thin to
//!    grab, not to keep it useful. The rail is exempt: fixed width, never compressed.
//! 2. **Space is given up in a stated order.** The main pane is the container's *proportional*
//!    panel, which is what makes it give first and give everything; the side panels are pixel
//!    panels, so they only start giving once it is on its `min_pixels`, and then they give in
//!    equal measure. That order is the sizing model rather than a policy written here.
//! 3. **Chrome shrinks its flexible run, then folds its actions into an overflow menu.** Owned by
//!    `components::toolbar`, not by this module.
//! 4. **Nothing a drag or a squeeze does ever closes a panel.** Both stop at the stub, which is
//!    small enough to be out of the way and wide enough that the handle beside it is still there
//!    to pull back out with. Closing stays an explicit act: the rail button, or the header ×.
//!    (The fork grew `ResizablePanel::on_collapse` for a drag-past-the-floor close, and it was
//!    tried and rejected here — a panel that vanishes mid-drag reads as having been lost, and
//!    IntelliJ does not do it either.)
//! 5. **A body scrolls; chrome never does.** Vertically only — a long identifier ellipsizes, which
//!    is the canvas's answer everywhere it has one.
//!
//! Nothing overlaps at any width because each panel rect is `Overflow::Clip`, a flex panel is
//! never measured negative (a torin fix this task carried), and each chrome row folds or
//! ellipsizes inside its own box.

use freya::components::use_theme;
use freya::prelude::*;
use freya::radio::use_radio;

use super::drawer::Drawer;
use super::inspector::Inspector;
use super::rail::ActivityRail;
use super::sidebar::Sidebar;
use super::workbench::WORKBENCH_STUB_H;
use super::Workbench;
use crate::apps::project::state::{Chan, SessionState};
use crate::components::divider::Divider;

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
        // Subscribe on `Chan::Layout`: a collapse / toggle re-renders the shell (and re-seeds the
        // panels from the remembered sizes), but a resize drag — which writes `Chan::LayoutSize`,
        // deriving only `Persist` — never wakes it, so the drag runs churn-free.
        let radio = use_radio::<SessionState, Chan>(Chan::Layout);
        let layout = radio.read().layout;
        let border = use_theme().read().colors().border;

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
                .min_size(PANEL_STUB_W)
                .max_size(SIDEBAR_MAX_W)
                .key("sidebar")
                .order(0usize)
                // Only a **drag** reports a width (`on_resized`, not `on_sized`): a window squeeze
                // moves the panel too, and recording that would overwrite the remembered width
                // with whatever the narrow window allowed, so widening back would not restore it.
                .on_resized(move |w: f32| {
                    let mut radio = radio;
                    if radio.read().layout.sidebar_w != w {
                        radio.write_channel(Chan::LayoutSize).set_sidebar_w(w);
                    }
                })
                .child(rect().expanded().child(Sidebar::new()))
        });
        let inspector_panel = layout.inspector_open.then(|| {
            ResizablePanel::new(PanelSize::px(layout.inspector_w))
                .min_size(PANEL_STUB_W)
                .max_size(INSPECTOR_MAX_W)
                .key("inspector")
                .order(2usize)
                .on_resized(move |w: f32| {
                    let mut radio = radio;
                    if radio.read().layout.inspector_w != w {
                        radio.write_channel(Chan::LayoutSize).set_inspector_w(w);
                    }
                })
                .child(rect().expanded().child(Inspector::new()))
        });

        // panes-row: [ sidebar? | workbench (fills) | inspector? ]. The workbench panel is keyed
        // so it isn't remounted when a sibling collapses.
        let panes_row = ResizableContainer::new()
            .direction(Direction::Horizontal)
            .handle_size(1.)
            .panel(sidebar_panel)
            .panel(
                ResizablePanel::new(PanelSize::percent(100.))
                    // The main pane is proportional, which is what makes it the **first** to give
                    // when the window narrows; `min_pixels` is where it stops and the side panels
                    // start giving instead, in equal measure (the RustRover order).
                    .min_pixels(PANEL_STUB_W)
                    // ...and `min_pixels` is only the *whole* floor if the flex-weight one gets
                    // out of its way. Left unstated it defaults to a quarter of the initial
                    // weight, which is 25 of 100 — about 154px here, three times the stub, and
                    // silently the thing that actually stops the drag.
                    .min_size(0.)
                    .key("main")
                    .order(1usize)
                    .child(Workbench),
            )
            .panel(inspector_panel);

        // Drawer panel: present only when open; remembers its dragged height.
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

        // Right area: [ panes-row (fills) | drawer? ] stacked vertically.
        let right_area = ResizableContainer::new()
            .direction(Direction::Vertical)
            .controller(drawer_sizing)
            .panel(
                ResizablePanel::new(PanelSize::percent(100.))
                    // The point at which the drawer stops taking from the workbench. Comes from
                    // the workbench's own bars rather than being restated here.
                    .min_pixels(WORKBENCH_STUB_H)
                    // See the panes row's main panel: the defaulted flex-weight minimum would
                    // otherwise outrank this and stop the drag well above the stub.
                    .min_size(0.)
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
        // The toggle is a stated intent, so it moves `desired` as a drag would. Setting only
        // `size` would have the container's next reflow put the drawer straight back.
        panel.desired = h;
    }
}
