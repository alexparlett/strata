//! The bottom **drawer** — the panel under the workbench: **Problems** (P3-12),
//! **Events** (P3-13) and **History** (P3-14). Which one it shows is the **rail's bottom group**
//! (`Layout::drawer`), which is the design's tab switcher — the canvas's drawer-header tab pills
//! were computed and never rendered, so there is no pill row here (P3-11).
//!
//! The header is `title · count`, then the expand / restore toggle (P3-11) and the collapse ×;
//! its top border is the resize handle above it, so the shell draws none. The two pieces P3-11
//! handed to its content tasks land here:
//!
//! - **The count** is resolved by whichever body is mounted, into the [`DrawerCount`] slot this
//!   shell owns — the `running` mirror's pattern (state-arch §6 / AGENTS.md §4): one resolver, one
//!   slot, read by props. The header cannot answer it itself without re-deriving the body's whole
//!   list, and a second derivation of the same facts is how two numbers start disagreeing.
//! - **The Clear rule**: shown on Events / History, **never** on Problems, whose rows self-clear
//!   when the SQL is fixed or the query re-runs — a Clear there would either lie or imply the
//!   problems aren't real (DEV_TASKS U10). The rule is this shell's; the *action* belongs to the
//!   tabs that keep a log — the ephemeral event log (P3-13) and the persisted history satellite
//!   (P3-14), which is why the two arms are different functions and not one.
//!
//! The frame the three bodies share — a scroll container and a centred empty state — is [`frame`].

mod events;
mod frame;
mod history;
mod problems;
/// The project-scope tally, for the rail badge: it must be the same function the drawer's own
/// header totals, or the two numbers disagree.
pub use problems::project_error_count;

use freya::components::{define_theme, get_theme, Tooltip, TooltipContainer};
use freya::prelude::*;
use freya::radio::{use_radio, use_radio_station};
use strata_model::DrawerTab;

use self::events::Events;
use self::history::History;
use self::problems::{Problems, ScopeStrip};
use super::shell::set_drawer_panel_height;
use crate::apps::project::state::{
    clear_history, Chan, HistoryCtx, LogCtx, ProjChan, ProjectState, SessionState,
};
use crate::components::divider::Divider;
use crate::components::icon::{Icon, IconName};
use crate::components::typography::{Caption, Control, Meta};

pub use frame::{DrawerBody, DrawerEmpty};

define_theme!(
    %[component]
    pub Drawer {
        %[fields]
        /// The drawer's own surface.
        background: Color,
        /// The rule under the header.
        border_fill: Color,
        /// The header's title (Problems · Events · History).
        label_color: Color,
        /// A group header's leading glyph.
        group_icon_color: Color,
        /// A group header's name — for Problems, the tab its rows belong to.
        group_color: Color,
        /// The most recessive run on a row — the header's count, a group header's tally, a
        /// `line L:C`, a timestamp, a pill's label. The footnote tone.
        meta_color: Color,
        /// A row's **secondary fact**: figures that are the row's own data rather than its
        /// prose. One step forward from [`meta_color`](Self::meta_color) and one back from
        /// [`message_color`](Self::message_color) — a History row uses all three at once (a
        /// query, what running it cost, and when), which is why the drawer needs a third text
        /// tone at all.
        value_color: Color,
        /// A row's message — its prose, and the brightest run in a list.
        message_color: Color,
        /// The surface a **pressable** row takes on hover.
        ///
        /// Not History's: Problems' rows are pressable too (they switch to the owning tab) and
        /// have no hover feedback at all, which is a gap this names rather than creates.
        ///
        /// It is the app's `surface_hover`, not the canvas's `--c-surface2`: the canvas's value
        /// is one step off *its* drawer surface, but ours is `surface_secondary`, which in
        /// Daylight is pure white — leaving a ~2% step that reads as no hover at all. A History
        /// card carries no pointer cursor and no tooltip, so the fill is its only affordance; it
        /// has to be the slot the rail and the tab strip already hover with.
        row_hover_fill: Color,
        /// The rule under an Events row — the recessive hairline *inside* a list, a step back
        /// from [`border_fill`](Self::border_fill), which separates the header from the body.
        divider_fill: Color,
        /// An empty state's copy. Its glyph wears a sheet colour, like every other semantic
        /// mark in the app.
        empty_color: Color,
    }
);

/// The header's tally, written by the mounted body and read by the header. A count of **errors**
/// for Problems (the canvas's `drawerCountLabel`), of rows for the two logs. `0` hides the label
/// rather than printing a zero, matching the canvas's `sc-if`.
pub type DrawerCount = State<usize>;

/// The header row's height, matching the sidebar's and the inspector's.
const HEADER_HEIGHT: f32 = 36.;

#[derive(PartialEq)]
pub struct Drawer {
    /// The controller for the vertical container this drawer is a panel of — the expand toggle
    /// resizes the live panel through it. Required, not defaulted: a drawer holding its own
    /// throwaway controller would toggle nothing.
    sizing: State<ResizableContext>,
    pub theme: Option<DrawerThemePartial>,
}

impl Drawer {
    pub fn new(sizing: State<ResizableContext>) -> Self {
        Self {
            sizing,
            theme: None,
        }
    }
}

impl Component for Drawer {
    fn render(&self) -> impl IntoElement {
        let radio = use_radio::<SessionState, Chan>(Chan::Layout);
        let layout = radio.read().layout;
        let tab = layout.drawer.unwrap_or(DrawerTab::Problems);
        // The remembered restore height *is* the expanded flag — see
        // `SessionState::toggle_drawer_height`.
        let expanded = layout.drawer_restore_h.is_some();
        // One source for every colour the drawer paints (AGENTS.md §3) — the sheet is reached for
        // only inside the bodies, and only for the semantic severity ramp.
        let theme = get_theme!(&self.theme, DrawerThemePreference, "drawer");
        // The body's tally. Each body resets it on unmount, so switching tabs can never leave the
        // previous one's number under the new one's title.
        let count: DrawerCount = use_state(|| 0usize);
        // The two logs **Clear** empties, one per tab (below), and the project whose
        // `history.jsonl` the History one has to unwrite as well.
        let log = use_consume::<LogCtx>();
        let history = use_consume::<HistoryCtx>();
        let project = use_radio_station::<ProjectState, ProjChan>();

        let (title, body): (&str, Element) = match tab {
            DrawerTab::Problems => (
                "Problems",
                Problems {
                    theme: theme.clone(),
                }
                .into_element(),
            ),
            DrawerTab::Events => (
                "Events",
                Events {
                    theme: theme.clone(),
                    count,
                }
                .into_element(),
            ),
            DrawerTab::History => (
                "History",
                History {
                    theme: theme.clone(),
                    count,
                }
                .into_element(),
            ),
        };

        let sizing = self.sizing;
        let expand = Button::new()
            .flat()
            .width(Size::px(24.))
            .height(Size::px(24.))
            .on_press(move |_| {
                let mut radio = radio;
                // Two halves of one resize: the layout write re-seeds the panel's
                // `initial_size` (its next mount, and the session file), and the controller
                // moves the panel that is on screen now.
                let h = radio.write_channel(Chan::Layout).toggle_drawer_height();
                set_drawer_panel_height(sizing, h);
            })
            .child(
                Icon::new(if expanded {
                    IconName::ChevronsDown
                } else {
                    IconName::ChevronsUp
                })
                .size(14.),
            );

        let shown = *count.read();
        // Clear: the two log tabs only (never Problems — see the module doc). Events empties the
        // window's ephemeral event log; History empties the satellite **and** removes
        // `history.jsonl`, so the rows don't come straight back on the next open.
        //
        // Enabled off the mounted body's **count** rather than a second read of either log: for
        // both tabs the count *is* the list's length, so the number in the header and the button
        // beside it can never disagree.
        let clear = (tab != DrawerTab::Problems).then(|| {
            Button::new()
                .flat()
                .enabled(shown > 0)
                .height(Size::px(24.))
                .on_press(move |_| match tab {
                    DrawerTab::History => clear_history(history, project, log),
                    _ => {
                        let mut log = log;
                        log.write().clear();
                    }
                })
                .child(Control::new("Clear"))
        });

        rect()
            .expanded()
            .background(theme.background)
            .vertical()
            .content(Content::Flex)
            .child(
                rect()
                    .width(Size::fill())
                    .height(Size::px(HEADER_HEIGHT))
                    .horizontal()
                    .cross_align(Alignment::Center)
                    .main_align(Alignment::SpaceBetween)
                    .padding((0., 12.))
                    .child(
                        rect()
                            .horizontal()
                            .cross_align(Alignment::Center)
                            .spacing(8.)
                            .child(Caption::new(title).color(theme.label_color))
                            // Problems puts its **scopes** here instead of a number: each tab
                            // carries its own count, so a total beside the title would be a third
                            // copy of the same two figures (the IntelliJ arrangement — the panel
                            // name and its scopes share one bar). The other two tabs have one
                            // list and so one tally.
                            .maybe_child((tab == DrawerTab::Problems).then_some(ScopeStrip))
                            .maybe_child(
                                (tab != DrawerTab::Problems && shown > 0)
                                    .then(|| Meta::new(shown.to_string()).color(theme.meta_color)),
                            ),
                    )
                    .child(
                        rect()
                            .horizontal()
                            .cross_align(Alignment::Center)
                            .spacing(2.)
                            .maybe_child(clear)
                            .child(
                                TooltipContainer::new(Tooltip::new(if expanded {
                                    "Restore"
                                } else {
                                    "Expand"
                                }))
                                .position(AttachedPosition::Bottom)
                                .child(expand),
                            )
                            .child(
                                Button::new()
                                    .flat()
                                    .width(Size::px(24.))
                                    .height(Size::px(24.))
                                    .on_press(move |_| {
                                        let mut radio = radio;
                                        radio.write_channel(Chan::Layout).close_drawer();
                                    })
                                    .child(Icon::new(IconName::Close).size(13.)),
                            ),
                    ),
            )
            .child(Divider::horizontal().color(theme.border_fill))
            .child(
                rect()
                    .width(Size::fill())
                    .height(Size::flex(1.))
                    .child(body),
            )
    }
}
