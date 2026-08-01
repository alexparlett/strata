//! The activity rail — the permanent 48px tool-window strip down the left edge (design
//! `ActivityRail.dc.html` / `Strata.dc.html` `data-rg="rail"`, RustRover-style).
//!
//! Two groups of icon toggles over the panel `surface_primary`: the top group selects the
//! sidebar's tool pane (Catalog · Agents · Connections), the bottom group the drawer's tab
//! (Problems · Events · History). **Agents is a tool pane, not a drawer tab** (AA-03b, and the
//! canvas says why): a drawer is an ephemeral log you consult, while Agents is a live,
//! navigable tree of connected things you press into — which is the catalog's job description.
//! Each button is a standard [`ToggleButton`] (reusing the `toggle_button`
//! theme, whose transparent-rest / accent-soft-active dress already matches the rail), sized to
//! the rail's 40×38. Its `on` state is *derived* from the layout — the single source of truth —
//! and a press routes through the layout store's toggle (`onRailPane` / `onOpen*` semantics):
//! open the pane/tab, or collapse it if it's already the active one. The Connections **button**
//! lives here; its sidebar pane content is W7.

use freya::components::use_theme;
use freya::prelude::*;
use freya::radio::use_radio;
use strata_model::{DrawerTab, SidebarPane};

use crate::apps::project::state::{
    AgentsCtx, Chan, FaultsCtx, ProjChan, ProjectState, SessionState,
};
use crate::apps::project::views::drawer::project_error_count;
use crate::components::icon::{Icon, IconName};
use crate::components::toggle_button::{ChangeEventData, ToggleButton};
use crate::components::typography::Meta;

#[derive(PartialEq)]
pub struct ActivityRail;

impl ActivityRail {
    pub fn new() -> Self {
        Self
    }
}

impl Component for ActivityRail {
    fn render(&self) -> impl IntoElement {
        // Subscribe on `Chan::Layout` — a collapse / pane switch re-renders the rail's active
        // dress, but a resize drag (on `Chan::LayoutSize`) does not.
        let radio = use_radio::<SessionState, Chan>(Chan::Layout);
        let layout = radio.read().layout;
        let background = use_theme().read().colors().surface_primary;

        // A rail toggle: 40×38, `on` derived from the layout, a press routing to `toggle`
        // (a fn pointer — `toggle_pane` / `toggle_drawer` for the chosen pane / tab).
        let button =
            move |icon: IconName, title: &str, active: bool, toggle: fn(&mut SessionState)| {
                ToggleButton::new()
                    .width(Size::px(40.))
                    .height(Size::px(38.))
                    .toggle(active)
                    .title(title)
                    .on_change(move |_: Event<ChangeEventData>| {
                        let mut radio = radio;
                        toggle(&mut radio.write_channel(Chan::Layout));
                    })
                    .child(Icon::new(icon).size(18.))
            };

        rect()
            .width(Size::px(48.))
            .height(Size::fill())
            .background(background)
            .cross_align(Alignment::Center)
            .content(Content::Flex)
            .padding((8., 0.))
            .spacing(2.)
            // Top group — the sidebar's tool panes.
            .child(button(
                IconName::Database,
                "Catalog",
                layout.sidebar == Some(SidebarPane::Catalog),
                |s| s.toggle_pane(SidebarPane::Catalog),
            ))
            .child(
                rect()
                    .width(Size::px(40.))
                    .height(Size::px(38.))
                    .child(button(
                        IconName::Agent,
                        "Agents",
                        layout.sidebar == Some(SidebarPane::Agents),
                        |s| s.toggle_pane(SidebarPane::Agents),
                    ))
                    .child(AgentsBadge),
            )
            .child(button(
                IconName::Connections,
                "Connections",
                layout.sidebar == Some(SidebarPane::Connections),
                |s| s.toggle_pane(SidebarPane::Connections),
            ))
            // Flexible spacer pushes the diagnostics group to the bottom.
            .child(rect().width(Size::px(1.)).height(Size::flex(1.)))
            // Bottom group — the drawer's diagnostics tabs. Problems wears the error count.
            .child(
                rect()
                    .width(Size::px(40.))
                    .height(Size::px(38.))
                    .child(button(
                        IconName::Problems,
                        "Problems",
                        layout.drawer == Some(DrawerTab::Problems),
                        |s| s.toggle_drawer(DrawerTab::Problems),
                    ))
                    .child(ProblemsBadge),
            )
            .child(button(
                IconName::Lines,
                "Events",
                layout.drawer == Some(DrawerTab::Events),
                |s| s.toggle_drawer(DrawerTab::Events),
            ))
            .child(button(
                IconName::Clock,
                "History",
                layout.drawer == Some(DrawerTab::History),
                |s| s.toggle_drawer(DrawerTab::History),
            ))
    }
}

/// The Problems button's error count (canvas `Strata.dc.html:355`): how many errors are open
/// across **every** open tab, hidden at zero and capped at `99+`.
///
/// Its own leaf, not a read on [`ActivityRail`]: the rail renders five toggles, and a validation
/// pass settling on any tab has no business re-rendering the other four.
///
/// It totals the **same two counts the drawer header does**, from the same two functions, so the
/// badge and the header can never disagree — the SQL errors across every open tab
/// (`error_count`) plus the project-scope conditions behind the Problems drawer's second tab
/// (`project_error_count`: defs the engine refused, and `.strata` files a failed write left
/// behind). A badge that counted only the first would go quiet while the project underneath was
/// broken, which is the case P4-15 exists for.
///
/// Errors only — a keyword-typo warning lists in the drawer without claiming the query is broken.
#[derive(PartialEq)]
struct ProblemsBadge;

impl Component for ProblemsBadge {
    fn render(&self) -> impl IntoElement {
        let session = use_radio::<SessionState, Chan>(Chan::Diagnostics);
        let tables = use_radio::<ProjectState, ProjChan>(ProjChan::Tables);
        let views = use_radio::<ProjectState, ProjChan>(ProjChan::Views);
        let faults = use_consume::<FaultsCtx>();
        let _ = views.read();
        let errors =
            session.read().error_count() + project_error_count(&tables.read(), &faults.read());
        let theme = use_theme();
        let (background, color, ring) = {
            let t = theme.read();
            let c = t.colors();
            // Semantic: the badge is the app-wide error tone wherever it appears (AGENTS.md §3).
            (c.error, c.text_inverse, c.surface_primary)
        };

        // Nothing to say: no pill, not an empty one (canvas `sc-if hasProblems`).
        if errors == 0 {
            return rect();
        }
        rect()
            // Pinned over the button's top-right corner, the way the grid header's resize grip
            // pins to its cell — an explicit offset, not fill-plus-alignment.
            .position(Position::new_absolute().top(1.).right(1.))
            .min_width(Size::px(15.))
            .height(Size::px(15.))
            .corner_radius(7.5)
            .background(background)
            // The canvas's 2px ring, so the pill reads clear of the glyph beneath it.
            .border(
                Border::new()
                    .width(2.)
                    .fill(ring)
                    .alignment(BorderAlignment::Outer),
            )
            .center()
            .padding((0., 3.))
            .child(
                Meta::new(match errors {
                    n if n > 99 => "99+".to_string(),
                    n => n.to_string(),
                })
                .color(color),
            )
    }
}

/// The Agents button's **live-agent count** (canvas `agentLiveCount`): how many MCP clients are
/// working in this project right now, hidden at zero.
///
/// Its own leaf for [`ProblemsBadge`]'s reason — an agent connecting has no business re-rendering
/// the other five toggles — and it wears the **accent**, not the error tone: this is a count of
/// things happening, not of things wrong. It also needs no `99+` clamp, because the number is
/// bounded by how many MCP clients a person has open, which is one or two.
///
/// The pane shows **only connected agents**, so this count and that list are the same fact: an
/// agent that disconnects takes its query sessions with it.
#[derive(PartialEq)]
struct AgentsBadge;

impl Component for AgentsBadge {
    fn render(&self) -> impl IntoElement {
        let agents = use_consume::<AgentsCtx>();
        let live = agents.read().len();
        let theme = use_theme();
        let (background, color) = {
            let t = theme.read();
            let c = t.colors();
            (c.primary, c.text_inverse)
        };

        if live == 0 {
            return rect();
        }
        rect()
            // The canvas's own offset, and deliberately without the Problems badge's 2px ring:
            // that ring exists to lift a red pill off the glyph beneath it, and the accent pill
            // reads clear without one.
            .position(Position::new_absolute().top(3.).right(3.))
            .min_width(Size::px(14.))
            .height(Size::px(14.))
            .corner_radius(7.)
            .background(background)
            .center()
            .padding((0., 3.))
            .child(Meta::new(live.to_string()).color(color))
    }
}
