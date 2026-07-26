//! The bottom **drawer** — the diagnostics panel under the workbench: **Problems** (P3-12),
//! Events (P3-13) and History (P3-14). Which one it shows is the **rail's bottom group**
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
//!   tabs that keep a log. Events' is wired (P3-13); History's stays parked until P3-14 gives it a
//!   truncate.
//!
//! The frame the three bodies share — a scroll container and a centred empty state — is [`frame`].

mod events;
mod frame;
mod problems;

use freya::components::{define_theme, get_theme, Tooltip, TooltipContainer};
use freya::prelude::*;
use freya::radio::use_radio;
use strata_model::DrawerTab;

use self::events::Events;
use self::problems::Problems;
use super::shell::set_drawer_panel_height;
use crate::apps::project::state::{Chan, LogCtx, SessionState};
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
        /// The header's count, a group header's tally, and a row's `line L:C`.
        meta_color: Color,
        /// A row's message.
        message_color: Color,
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
        // The window's event log — what **Clear** empties on the Events tab (below).
        let log = use_consume::<LogCtx>();

        let (title, body): (&str, Element) = match tab {
            DrawerTab::Problems => (
                "Problems",
                Problems {
                    theme: theme.clone(),
                    count,
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
            // History (P3-14) fills this same frame, and writes the same count.
            DrawerTab::History => ("History", rect().expanded().into_element()),
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
        // event log; History has no truncate yet, so its button stays parked (P3-14) rather than
        // promising something it can't do.
        //
        // Enabled off the mounted body's **count** rather than a second read of the log: for
        // Events the count *is* the log's length, so this way the number in the header and the
        // button beside it can never disagree.
        let clear = (tab != DrawerTab::Problems).then(|| {
            Button::new()
                .flat()
                .enabled(tab == DrawerTab::Events && shown > 0)
                .height(Size::px(24.))
                .maybe(tab == DrawerTab::Events, |button| {
                    button.on_press(move |_| {
                        let mut log = log;
                        log.write().clear();
                    })
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
                            .maybe_child(
                                (shown > 0)
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
