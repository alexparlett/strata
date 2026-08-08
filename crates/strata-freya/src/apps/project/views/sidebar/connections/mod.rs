//! The **Connections** sidebar pane (W7 · U3) — the object stores this project reads from, and
//! whether each one actually resolved.
//!
//! Built to the canvas (`Strata.dc.html`, `data-pane="connections"`) and
//! `docs/CONNECTIONS_SPEC.md` §1.
//!
//! ## One row, one status slot — the catalog entry row's
//!
//! A connection row is a catalog row: badge, name, a trailing status glyph, ⋮. A row the engine
//! accepted is **clean**; a row it refused wears the warning triangle, and the engine's reason
//! is on the triangle's popover in full.
//!
//! It was a two-line row first, with a green/amber dot and the status spelled underneath. The
//! second line is gone because it could not do the one job it existed for: at sidebar width
//! "The AWS profile 'analytics' resolved no credentials: the credential provider was not
//! enabled" ellipsizes to about four words, which tells the user nothing and costs the bucket
//! half the row. The whole sentence on hover is worth more than a quarter of it in place — the
//! same trade the catalog row made, reached the same way.
//!
//! What the glyph reports is [`ConnRow::reg`] and nothing else: `engine::store::connect` already
//! resolves the credential chain once *before* registering and throws the answer away, and the
//! whole reason it does so is to make this one slot mean something. A second liveness check here
//! would be a request to the bucket answering a question the pass has already answered.
//!
//! `Reg::Loading` reports nothing until the wait outlasts `PROGRESS_HOLD`, then spins — and the
//! slot holds its last settled verdict across that gap, so a ↻ does not blink a triangle off and
//! back. That hold earns its keep here more than on a table: a table registers by reading local
//! metadata, while a connection may resolve a chain that reaches SSO, ECS or IMDS over the
//! network.
//!
//! ## The row is not clickable; its actions are the menu
//!
//! Per the spec: a connection is a thing you look at, not a thing you open. Edit and Forget come
//! from the ⋮ (or a right-click), exactly as a catalog row's do — which is also what keeps the
//! two triggers from drifting, since both build the same [`connection_menu`].
//!
//! ## Collapsing the pane strands nothing
//!
//! The shell mounts `Sidebar` only while `layout.sidebar` is `Some`, and the rail's
//! `toggle_pane` collapses on a press of the pane already showing (the VS Code model the spec
//! asks for) — so this whole subtree unmounts mid-gesture as a matter of routine, and comes
//! back subscribed to the same store rather than to a copy of it.
//!
//! Nothing here owns work that would die with it. The pane holds no local state at all (a row's
//! status is the store's, not a cached verdict), and **Forget sets the confirm slot and stops**:
//! the dialog at the window root owns the store mutation, the persist and the
//! `Engine::disconnect` behind it. That is the same reason the catalog's ↻ raises a request
//! instead of spawning its own pass — a task spawned from a handler in here belongs to a scope
//! the collapse tears down before the future is ever polled.
//!
//! ## Add and Edit are inert here (AGENTS.md §5)
//!
//! The editor forms are **Connections 03**'s, so the two gestures that open one are rendered and
//! disabled rather than wired to a local one-off. Nothing at these call sites changes when that
//! task lands but the handler behind them.

#[cfg(test)]
mod interaction;

use async_io::Timer;
use freya::components::{
    define_theme, get_theme, CircularLoader, MenuItemThemePartial, ScrollView,
};
use freya::prelude::*;
use freya::radio::use_radio;
use strata_model::ConnectionDef;

use crate::apps::project::state::{ProjChan, ProjectState, Reg};
use crate::apps::project::views::DropTarget;
use crate::components::badge::Badge;
use crate::components::divider::Divider;
use crate::components::icon::{Icon, IconName};
use crate::components::sidebar_row::SidebarRow;
use crate::components::tones::{tones, Tones};
use crate::components::typography::{MonoValue, Prose};
use crate::components::{PANE_BODY_MIN_W, PROGRESS_HOLD};

define_theme!(
    %[component]
    pub Connections {
        %[fields]
        /// The provider badge's label and outline (canvas: `--accent`).
        provider_color: Color,
        /// The bucket — the row's own name, and its brightest run.
        bucket_color: Color,
        /// Every recessive glyph the pane paints itself: the header's ⓘ and the empty
        /// state's cloud.
        hint_color: Color,
        /// The empty state's tile, its edge, and its copy.
        empty_background: Color,
        empty_border_fill: Color,
        empty_color: Color,
    }
);

/// The pane's scroll inset, matching the catalog's and the Agents pane's.
const BODY_PAD: Gaps = Gaps::new(8., 8., 12., 8.);
/// The empty state's inset (canvas `--sp-7 --sp-6`) — generous at the top, because it sits where
/// the first row would rather than in the middle of the panel.
const EMPTY_PAD: Gaps = Gaps::new(32., 24., 32., 24.);
/// A connection row's height — the catalog entry row's 30, because it is now the same row.
const ROW_HEIGHT: f32 = 30.;
/// The trailing ⋮ actions button — the canvas's 22×22, the catalog row's size.
const ACTIONS_SIZE: f32 = 22.;
/// The trailing **status glyph** — spinner or warning triangle, one slot, one size. The catalog
/// entry row's, so the two report a refused def identically.
const STATUS_SIZE: f32 = 12.;
/// What the spinner says on hover (and to a screen reader).
const CONNECTING: &str = "Connecting…";
/// The menu card's width — the catalog menus' 210, so the two read as one vocabulary.
const MENU_WIDTH: f32 = 210.;
/// The glyph beside each menu label, and the gap to it.
const ITEM_ICON: f32 = 15.;
const ITEM_GAP: f32 = 12.;

/// What the registration pass answered for one row, in the terms the pane renders.
///
/// A projection of [`Reg<()>`], not the value itself: `Reg` is deliberately not `Clone` (see
/// [`ProjectState::remove_view`]), and the pane reads its rows out of the store's guard before
/// building any element — so what crosses that boundary is this.
#[derive(Clone, PartialEq)]
enum Status {
    /// The pass is out and has not answered for this bucket yet.
    Loading,
    Connected,
    /// The engine refused it, carrying what to fix — the triangle's tooltip.
    Refused(String),
}

impl Status {
    fn of(reg: &Reg<()>) -> Self {
        match reg {
            Reg::Loading => Self::Loading,
            Reg::Ready(()) => Self::Connected,
            Reg::Failed(why) => Self::Refused(why.clone()),
        }
    }
}

/// The connections list — the sidebar body under the pane header.
#[derive(PartialEq)]
pub struct Connections {
    pub theme: Option<ConnectionsThemePartial>,
}

impl Connections {
    pub fn new() -> Self {
        Self { theme: None }
    }
}

impl Component for Connections {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(&self.theme, ConnectionsThemePreference, "connections");
        let radio = use_radio::<ProjectState, ProjChan>(ProjChan::Connections);

        // Cloned out, so the store's read guard drops before any element is built. The `url` is
        // the row's identity everywhere — the menu's Forget target, and what the engine's answer
        // was addressed by — while the def carries what the row renders.
        let rows: Vec<(String, ConnectionDef, Status)> = radio
            .read()
            .connections
            .iter()
            .map(|c| (c.def.url(), c.def.clone(), Status::of(&c.reg)))
            .collect();

        if rows.is_empty() {
            return Empty { theme }.into_element();
        }

        rect()
            .expanded()
            .child(
                ScrollView::new().child(
                    // Floored at `PANE_BODY_MIN_W`, with the panel clipping the remainder — the
                    // catalog's and Agents' rule (P5-06).
                    rect()
                        .width(Size::fill())
                        .min_width(Size::px(PANE_BODY_MIN_W))
                        .vertical()
                        .padding(BODY_PAD)
                        .children(rows.into_iter().map(|(url, def, status)| {
                            ConnectionRow {
                                url: url.clone(),
                                def,
                                status,
                                theme: theme.clone(),
                                key: DiffKey::None,
                            }
                            .key(url)
                        })),
                ),
            )
            .into_element()
    }
}

/// One connection: its provider badge, its bucket, and what registering it answered.
#[derive(PartialEq)]
struct ConnectionRow {
    /// `ConnectionDef::url()` — the row's identity, and the key Forget deregisters by.
    url: String,
    def: ConnectionDef,
    status: Status,
    theme: ConnectionsTheme,
    key: DiffKey,
}

impl KeyExt for ConnectionRow {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for ConnectionRow {
    /// Reconcile rows by **connection**, not by position. Without this the default is a
    /// per-type constant, so `KeyExt::key` writes a field nothing reads and forgetting a row
    /// mid-list hands its scope — and `SideBarItem`'s hover and focus state with it — to the
    /// row that shuffled up into its slot.
    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }

    fn render(&self) -> impl IntoElement {
        // One `tones()` read per render, its own documented contract.
        let tones = tones();
        let actions = use_connection_actions(tones);

        // **The status slot holds still**, the catalog entry row's rule and mechanism. A settled
        // answer applies at once; the gap while the pass is out keeps whatever the slot last
        // showed, for `PROGRESS_HOLD`. Without it a ↻ blinks a refused row's triangle off and
        // back, and the empty slot in between reads as "connected" — a claim the row cannot make
        // while it has no answer.
        //
        // The hold matters more here than on a table. A table registers by reading local
        // metadata; a connection resolves a credential chain that may reach SSO, ECS or IMDS
        // over the network, so this is the slowest answer in the pass and the one most worth
        // spinning about once it outlasts the hold.
        let waiting = self.status == Status::Loading;
        let refused = match &self.status {
            Status::Refused(why) => Some(why.clone()),
            _ => None,
        };

        // Whether the wait has outlasted the hold. Re-armed on every entry into and exit from
        // the wait, so a re-scan of a row that was already waiting does not blink its spinner.
        let waited = use_state(|| false);
        let pending = use_state(|| None::<TaskHandle>);
        use_side_effect_with_deps(&waiting, move |waiting| {
            let mut waited = waited;
            let mut pending = pending;
            if let Some(task) = pending.write().take() {
                task.cancel();
            }
            waited.set_if_modified(false);
            if *waiting {
                pending.set(Some(spawn(async move {
                    Timer::after(PROGRESS_HOLD).await;
                    waited.set_if_modified(true);
                })));
            }
        });

        // The verdict to keep showing through the gap: the last one that actually settled.
        let held = use_state(|| None::<String>);
        use_side_effect_with_deps(&(waiting, refused.clone()), move |(waiting, refused)| {
            if !waiting {
                let mut held = held;
                held.set_if_modified(refused.clone());
            }
        });

        // One slot, at most one glyph in it, with the words only on hover — and a settled,
        // connected row is clean. The status *text* is deliberately gone: it was the engine's
        // reason ellipsized to nothing in a sidebar-width row, which said strictly less than the
        // triangle does and cost the bucket half its width. Each glyph declares its message as
        // an **a11y label** too, so the explanation is not mouse-only.
        let status = match (
            waiting && waited(),
            if waiting {
                held.read().clone()
            } else {
                refused
            },
        ) {
            (true, _) => Some(
                tip(CONNECTING).child(CircularLoader::new().size(STATUS_SIZE).a11y_alt(CONNECTING)),
            ),
            (false, Some(why)) => Some(
                tip(why.clone()).child(
                    rect().a11y_alt(why).child(
                        Icon::new(IconName::Warning)
                            .color(tones.warning)
                            .size(STATUS_SIZE),
                    ),
                ),
            ),
            (false, None) => None,
        };

        // One menu, two triggers (right-click the row, or press its ⋮) — a fresh snapshot each
        // time it opens, like the catalog's.
        let build_menu = {
            let url = self.url.clone();
            move || connection_menu(&actions, url.clone())
        };
        let menu_for_row = build_menu.clone();

        SidebarRow::new()
            .height(ROW_HEIGHT)
            .padding(Gaps::new(0., 4., 0., 8.))
            // No `on_press`: the row is not clickable (spec §1 — Edit is menu-only), which is
            // also what leaves the whole row free as the context-menu surface.
            .on_context_menu(move |_: Event<PressEventData>| {
                ContextMenu::open(menu_for_row());
            })
            .child(
                // `Display`, not a label function here: the Configure window's connection
                // picker (W7 · 04) has to name providers the same way, and the badge is only
                // the first surface to ask. `S3` / `GCS` / `HTTP` — the product's name, never
                // `Provider::scheme`'s URL word.
                Badge::tag(self.def.provider.to_string(), self.theme.provider_color)
                    .outlined()
                    .height(16.),
            )
            // The bucket absorbs the slack and truncates, so the status run and the ⋮ stay
            // visible however long it is named — the catalog entry row's arrangement exactly.
            .child(
                MonoValue::new(self.def.bucket.clone())
                    .color(self.theme.bucket_color)
                    .width(Size::flex(1.))
                    .text_overflow(TextOverflow::Ellipsis),
            )
            .maybe_child(status.map(|s| s.into_element()))
            .child(actions_button(build_menu))
    }
}

/// A status glyph wearing its message as a tooltip. Dropped below, like the rest of the app's
/// overlays, so it cannot cover the row above it in a dense list. The catalog entry row's helper,
/// same shape and same placement.
fn tip(message: impl Into<std::borrow::Cow<'static, str>>) -> TooltipContainer {
    TooltipContainer::new(Tooltip::new_text(message)).position(AttachedPosition::Bottom)
}

/// The row's **⋮ trigger** — the catalog row's affordance, at the same size and with the same
/// lazily-built menu, because the two rows are one vocabulary.
///
/// No `stop_propagation` is needed here as it is on a catalog row: this row has no press of its
/// own for the event to reach.
fn actions_button(menu: impl Fn() -> Menu + 'static) -> impl IntoElement {
    TooltipContainer::new(Tooltip::new_text("Actions"))
        .position(AttachedPosition::Bottom)
        .child(
            Button::new()
                .flat()
                .width(Size::px(ACTIONS_SIZE))
                .height(Size::px(ACTIONS_SIZE))
                .on_press(move |_: Event<PressEventData>| ContextMenu::open(menu()))
                .child(Icon::new(IconName::Dots).size(15.)),
        )
}

/// The handles a connection row's menu acts through, gathered once per row — the catalog's
/// [`CatalogActions`] shape, with only what this pane's two items need.
///
/// [`CatalogActions`]: super::CatalogActions
#[derive(Clone, Copy)]
struct ConnectionActions {
    /// The remove-confirm slot provided at the window root. Setting it *is* Forget: the dialog
    /// owns the store mutation, the persist and the `Engine::disconnect` behind it (P3-05's
    /// rule, and W7's fourth target).
    drop_target: State<Option<DropTarget>>,
    /// The destructive tone, resolved here because a menu is built from an event handler, where
    /// no hook — `use_theme` included — may run.
    danger: Color,
}

/// Gather this row's action handles.
///
/// Takes the row's **already-resolved** [`Tones`] rather than reading them itself, unlike
/// [`use_catalog_actions`](super::use_catalog_actions): `tones()` is a theme read whose contract
/// is one call per render, and this row needs the same four for its status dot. Resolving them
/// in both places read the theme twice per row for one `Color`.
fn use_connection_actions(tones: Tones) -> ConnectionActions {
    ConnectionActions {
        drop_target: use_consume::<State<Option<DropTarget>>>(),
        danger: tones.error,
    }
}

/// One menu row: the glyph over its label, at the catalog menus' gap.
fn menu_row(icon: IconName, label: impl Into<String>) -> impl IntoElement {
    rect()
        .horizontal()
        .cross_align(Alignment::Center)
        .spacing(ITEM_GAP)
        .child(Icon::new(icon).size(ITEM_ICON))
        .child(Prose::new(label))
}

/// A **connection** row's menu: edit it · forget it (spec §1, `kind:"conn"`).
///
/// **Edit is parked** — rendered, disabled — because the editor forms are Connections 03's and
/// AGENTS.md §5 forbids folding a local one-off in front of a capability another task owns. A
/// menu is a list of things you can do right now, which is why this one is greyed rather than
/// dressed live: pressing it would have to do nothing.
fn connection_menu(actions: &ConnectionActions, url: String) -> Menu {
    let actions = *actions;
    Menu::new()
        .min_width(Size::px(MENU_WIDTH))
        .child(
            MenuButton::new()
                .enabled(false)
                .child(menu_row(IconName::Pencil, "Edit connection")),
        )
        .child(Divider::menu())
        .child(
            MenuButton::new()
                .theme(MenuItemThemePartial::default().color(actions.danger))
                .on_press(move |_| {
                    let mut slot = actions.drop_target;
                    slot.set(Some(DropTarget::Connection(url.clone())));
                    ContextMenu::close();
                })
                .child(menu_row(IconName::Trash, "Forget connection")),
        )
}

/// The pane header's ⓘ, which is what a connection *is* — the one thing about this surface a
/// user has no other way to learn. Mounted by the sidebar shell beside the `CONNECTIONS` label,
/// like the Agents pane's.
#[derive(PartialEq)]
pub struct ConnectionsHint;

impl Component for ConnectionsHint {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(
            &None::<ConnectionsThemePartial>,
            ConnectionsThemePreference,
            "connections"
        );
        // What a connection is, and the one thing about it a user is most likely to get wrong.
        // Deliberately **not** "the Configure window picks one": that control is Connections
        // 04's and is not in the build, so the sentence would send the reader to a surface that
        // does not exist. Add it back with the control.
        TooltipContainer::new(Tooltip::new_text(
            "Object stores this project reads from - s3://, gs://, http(s)://. Credentials \
             resolve from this machine's own configuration; Strata never stores a key.",
        ))
        .position(AttachedPosition::Bottom)
        .child(
            rect()
                .width(Size::px(14.))
                .height(Size::px(14.))
                .center()
                .child(Icon::new(IconName::Info).color(theme.hint_color).size(13.)),
        )
    }
}

/// The pane header's **+**, which opens the editor on a new connection.
///
/// **Inert until Connections 03**, and disabled rather than live for [`connection_menu`]'s
/// reason: a control the user can press to no effect is worse than one that says it is not
/// available yet. Its tooltip is the same either way, so nothing here changes when the editor
/// lands but the handler.
#[derive(PartialEq)]
pub struct AddConnectionButton;

impl Component for AddConnectionButton {
    fn render(&self) -> impl IntoElement {
        TooltipContainer::new(Tooltip::new_text("Add connection"))
            .position(AttachedPosition::Bottom)
            .child(
                Button::new()
                    .flat()
                    .width(Size::px(24.))
                    .height(Size::px(24.))
                    .enabled(false)
                    .child(Icon::new(IconName::Plus).size(14.)),
            )
    }
}

/// No connections. Not a fault — most projects read local files and never need one — so the copy
/// says what a connection is for rather than what is missing.
///
/// **Top-aligned, not centred**, the Agents pane's rule: a pane's empty state sits where its
/// first row would, so switching panes doesn't move the reader's eye down the panel and back.
#[derive(PartialEq)]
struct Empty {
    theme: ConnectionsTheme,
}

impl Component for Empty {
    fn render(&self) -> impl IntoElement {
        rect()
            .width(Size::fill())
            // The pane returns this *instead of* its scrolling body, so it carries the same
            // floor itself (P5-06).
            .min_width(Size::px(PANE_BODY_MIN_W))
            .vertical()
            .cross_align(Alignment::Center)
            .padding(EMPTY_PAD)
            .spacing(12.)
            .child(
                rect()
                    .width(Size::px(40.))
                    .height(Size::px(40.))
                    .corner_radius(10.)
                    .background(self.theme.empty_background)
                    .border(Border::new().width(1.).fill(self.theme.empty_border_fill))
                    .center()
                    .child(
                        Icon::new(IconName::Connections)
                            .color(self.theme.hint_color)
                            .size(19.),
                    ),
            )
            .child(
                Prose::new(
                    "No connections yet. Add one to read tables from S3, GCS, or an HTTP(S) \
                     endpoint.",
                )
                .color(self.theme.empty_color)
                .max_width(Size::px(210.))
                .wrap()
                .align(TextAlign::Center),
            )
            // The canvas's primary call to action, and **inert until Connections 03** — see
            // `AddConnectionButton`. Disabled on the same terms: an empty state whose one button
            // silently does nothing is a dead end that reads as a bug.
            .child(
                Button::new().enabled(false).child(
                    rect()
                        .horizontal()
                        .cross_align(Alignment::Center)
                        .spacing(6.)
                        .child(Icon::new(IconName::Plus).size(12.))
                        .child(Prose::new("Add connection")),
                ),
            )
    }
}
