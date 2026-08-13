//! The activity rail — the permanent 48px tool-window strip down the left edge (design
//! `ActivityRail.dc.html` / `Strata.dc.html` `data-rg="rail"`, RustRover-style).
//!
//! Two groups of icon toggles over the panel `surface_primary`: the top group selects the
//! sidebar's tool pane (Catalog · Connections), the bottom group the drawer's tab
//! (Problems · Events · History). Each button is a standard [`ToggleButton`] (reusing the
//! `toggle_button` theme, whose transparent-rest / accent-soft-active dress already matches the
//! rail), sized to the rail's 40×38. Its `on` state is *derived* from the layout — the single source of truth —
//! and a press routes through the layout store's toggle (`onRailPane` / `onOpen*` semantics):
//! open the pane/tab, or collapse it if it's already the active one. The Connections **button**
//! lives here; its sidebar pane content is W7.

use freya::prelude::*;
use freya::radio::use_radio;
use strata_model::{DrawerTab, SidebarPane};

use crate::apps::project::state::{Chan, FaultsCtx, ProjChan, ProjectState, SessionState};
use crate::apps::project::views::drawer::project_error_count;
use crate::components::icon::{Icon, IconName};
use crate::components::metrics::{pill, SP_1, SP_2, SP_3};
use crate::components::toggle_button::{ChangeEventData, ToggleButton};
use crate::components::tones::tones;
use crate::components::typography::Meta;
use crate::theme::{use_roles, Role};

/// The problem-count pill's diameter at its narrowest — it grows with a two- or three-figure
/// count, and is a circle until it does.
const BADGE: f32 = 15.;

#[derive(PartialEq)]
pub struct ActivityRail;

impl ActivityRail {
    pub fn new() -> Self {
        Self
    }
}

impl Component for ActivityRail {
    fn render(&self) -> impl IntoElement {
        let radio = use_radio::<SessionState, Chan>(Chan::Layout);
        let layout = radio.read().layout;
        let background = use_roles().get(Role::SurfaceBackground);

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
            .padding((SP_3, 0.))
            .spacing(SP_1)
            .child(button(
                IconName::Database,
                "Catalog",
                layout.sidebar == Some(SidebarPane::Catalog),
                |s| s.toggle_pane(SidebarPane::Catalog),
            ))
            .child(button(
                IconName::Connections,
                "Connections",
                layout.sidebar == Some(SidebarPane::Connections),
                |s| s.toggle_pane(SidebarPane::Connections),
            ))
            .child(rect().width(Size::px(1.)).height(Size::flex(1.)))
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
/// (`project_error_count`: connections and defs the engine refused, and `.strata` files a failed
/// write left behind). A badge that counted only the first would go quiet while the project
/// underneath was broken, which is the case P4-15 exists for.
///
/// Errors only — a keyword-typo warning lists in the drawer without claiming the query is broken.
#[derive(PartialEq)]
struct ProblemsBadge;

impl Component for ProblemsBadge {
    fn render(&self) -> impl IntoElement {
        let session = use_radio::<SessionState, Chan>(Chan::Diagnostics);
        let connections = use_radio::<ProjectState, ProjChan>(ProjChan::Connections);
        let tables = use_radio::<ProjectState, ProjChan>(ProjChan::Tables);
        let views = use_radio::<ProjectState, ProjChan>(ProjChan::Views);
        let faults = use_consume::<FaultsCtx>();
        let _ = connections.read();
        let _ = views.read();
        let errors =
            session.read().error_count() + project_error_count(&tables.read(), &faults.read());
        let roles = use_roles();
        let (background, color, ring) = (
            tones().error,
            roles.get(Role::TextOnAccent),
            roles.get(Role::SurfaceBackground),
        );

        if errors == 0 {
            return rect();
        }
        rect()
            .position(Position::new_absolute().top(1.).right(1.))
            .min_width(Size::px(BADGE))
            .height(Size::px(BADGE))
            .corner_radius(pill(BADGE))
            .background(background)
            .border(
                Border::new()
                    .width(2.)
                    .fill(ring)
                    .alignment(BorderAlignment::Outer),
            )
            .center()
            .padding((0., SP_2))
            .child(
                Meta::new(match errors {
                    n if n > 99 => "99+".to_string(),
                    n => n.to_string(),
                })
                .color(color),
            )
    }
}
