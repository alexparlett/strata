//! The **Connections** sidebar pane (W7 · U3) — the object stores this project reads from, and
//! whether each one actually resolved.
//!
//! **One row, one status slot — the catalog entry row's**: badge, name, a trailing status glyph, ⋮.
//! A row the engine accepted is clean; a row it refused wears the warning triangle, with the
//! engine's reason on the popover in full. It was a two-line row first, with the status spelled
//! underneath, and that line is gone because at sidebar width a real refusal ellipsizes to about
//! four words while costing the bucket half the row.
//!
//! The glyph reports [`ConnRow::reg`] and nothing else — `engine::store::connect` already resolves
//! the chain and probes the bucket, so a second liveness check here would re-ask a question the
//! pass has answered. `Reg::Loading` reports nothing until the wait outlasts `PROGRESS_HOLD`, and
//! the slot holds its last settled verdict across that gap so a ↻ does not blink a triangle off and
//! back.
//!
//! **The row is not clickable; its actions are the menu.** A connection is a thing you look at, not
//! a thing you open, so Edit and Forget come from the ⋮ or a right-click — both building the same
//! [`connection_menu`], which is what keeps the two triggers from drifting.
//!
//! **Collapsing the pane strands nothing.** This subtree unmounts mid-gesture as a matter of
//! routine, so it holds no local state and **Forget sets the confirm slot and stops** — the dialog
//! at the window root owns the mutation, the persist and the `Engine::disconnect`. A task spawned
//! from a handler here would belong to a scope the collapse tears down before its first poll.
//!
//! **Add and Edit set a slot; the editor window is the project root's.** All three gestures set
//! [`ConnectionRequest`] and `ConnectionLauncher` opens the window, because that is where the
//! app-globals, the engine and the log live. The split Configure makes.

#[cfg(test)]
mod interaction;

use async_io::Timer;
use freya::components::{
    define_theme, get_theme, CircularLoader, MenuItemThemePartial, ScrollView,
};
use freya::prelude::*;
use freya::radio::use_radio;
use strata_model::ConnectionDef;

use crate::apps::connection::ConnectionTarget;
use crate::apps::project::state::{ProjChan, ProjectState, Reg};
use crate::apps::project::views::{ConnectionRequest, DropTarget};
use crate::components::badge::Badge;
use crate::components::divider::Divider;
use crate::components::icon::{Icon, IconName};
use crate::components::metrics::{
    CONTEXT_MENU_WIDTH, MENU_ICON, PANE_BODY_MIN_W, PROGRESS_HOLD, ROW_ACTION, STATUS_DOT,
};
use crate::components::metrics::{R_3, SP_2, SP_3, SP_4, SP_6, SP_7};
use crate::components::sidebar_row::SidebarRow;
use crate::components::tones::{tones, Tones};
use crate::components::typography::{MonoValue, Prose};

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

/// The pane's scroll inset, matching the catalog's.
const BODY_PAD: Gaps = Gaps::new(SP_3, SP_3, SP_4, SP_3);
/// The empty state's inset (canvas `--sp-7 --sp-6`) — generous at the top, because it sits where
/// the first row would rather than in the middle of the panel.
const EMPTY_PAD: Gaps = Gaps::new(SP_7, SP_6, SP_7, SP_6);
/// A connection row's height — the catalog entry row's 30, because it is now the same row.
const ROW_HEIGHT: f32 = 30.;
/// What the spinner says on hover (and to a screen reader).
const CONNECTING: &str = "Connecting…";
/// What the triangle says on hover (and to a screen reader) — **a pointer, not the reason**.
///
/// The engine's own words used to hang here, and a sidebar tooltip is the one place they cannot
/// be read: `object_store` writes a diagnosis worth two clauses ("Received redirect without
/// LOCATION, this normally indicates an incorrectly configured region") and this row clipped it
/// at the first, so the half naming the cause was the half thrown away. A row this narrow can
/// say *that* something is wrong; it cannot say what. Problems can, wraps it, and has a button
/// that copies it.
const REFUSED: &str = "Connection failed. See Problems for the reason.";
const ITEM_GAP: f32 = SP_4;

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
        let tones = tones();
        let actions = use_connection_actions(tones);

        let waiting = self.status == Status::Loading;
        let refused = matches!(self.status, Status::Refused(_));

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

        let held = use_state(|| false);
        use_side_effect_with_deps(&(waiting, refused), move |(waiting, refused)| {
            if !waiting {
                let mut held = held;
                held.set_if_modified(*refused);
            }
        });

        let status = match (waiting && waited(), if waiting { held() } else { refused }) {
            (true, _) => Some(
                tip(CONNECTING).child(CircularLoader::new().size(STATUS_DOT).a11y_alt(CONNECTING)),
            ),
            (false, true) => Some(
                tip(REFUSED).child(
                    rect().a11y_alt(REFUSED).child(
                        Icon::new(IconName::Warning)
                            .color(tones.warning)
                            .size(STATUS_DOT),
                    ),
                ),
            ),
            (false, false) => None,
        };

        let build_menu = {
            let url = self.url.clone();
            move || connection_menu(&actions, url.clone())
        };
        let menu_for_row = build_menu.clone();

        SidebarRow::new()
            .height(ROW_HEIGHT)
            .padding(Gaps::new(0., SP_2, 0., SP_3))
            .on_context_menu(move |_: Event<PressEventData>| {
                ContextMenu::open(menu_for_row());
            })
            .child(
                Badge::tag(self.def.provider.to_string(), self.theme.provider_color)
                    .outlined()
                    .height(16.),
            )
            .child(
                MonoValue::new(self.def.address.clone())
                    .color(self.theme.bucket_color)
                    .width(Size::flex(1.))
                    .text_overflow(TextOverflow::Ellipsis),
            )
            .maybe_child(status.map(IntoElement::into_element))
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
                .width(Size::px(ROW_ACTION))
                .height(Size::px(ROW_ACTION))
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
    /// The editor-window request slot, on the same terms: setting it *is* Edit, and
    /// `ConnectionLauncher` at the project root opens the window.
    editor: ConnectionRequest,
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
        editor: use_consume::<ConnectionRequest>(),
        danger: tones.error,
    }
}

/// One menu row: the glyph over its label, at the catalog menus' gap.
fn menu_row(icon: IconName, label: impl Into<String>) -> impl IntoElement {
    rect()
        .horizontal()
        .cross_align(Alignment::Center)
        .spacing(ITEM_GAP)
        .child(Icon::new(icon).size(MENU_ICON))
        .child(Prose::new(label))
}

/// A **connection** row's menu: edit it · forget it (spec §1, `kind:"conn"`).
///
/// Both items **set a slot and stop** — the editor window and the remove confirm are both the
/// project root's, and a menu built inside an event handler can run no hook to reach what either
/// of them needs.
fn connection_menu(actions: &ConnectionActions, url: String) -> Menu {
    let actions = *actions;
    Menu::new()
        .min_width(Size::px(CONTEXT_MENU_WIDTH))
        .child(
            MenuButton::new()
                .on_press({
                    let url = url.clone();
                    move |_| {
                        let mut slot = actions.editor;
                        slot.set(Some(ConnectionTarget::Edit(url.clone())));
                        ContextMenu::close();
                    }
                })
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
/// user has no other way to learn. Mounted by the sidebar shell beside the `CONNECTIONS`
/// label.
#[derive(PartialEq)]
pub struct ConnectionsHint;

impl Component for ConnectionsHint {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(
            &None::<ConnectionsThemePartial>,
            ConnectionsThemePreference,
            "connections"
        );
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
/// **It folds under panel pressure** (`ToolbarItem::Custom { folded: None }`, the catalog ↻'s
/// terms) and, unlike ↻, has no second entry point on the pane: the empty state's CTA is gone the
/// moment there is one connection. The command palette's *New connection…* is what makes that
/// fold cost nothing.
#[derive(PartialEq)]
pub struct AddConnectionButton;

impl Component for AddConnectionButton {
    fn render(&self) -> impl IntoElement {
        let editor = use_consume::<ConnectionRequest>();
        TooltipContainer::new(Tooltip::new_text("Add connection"))
            .position(AttachedPosition::Bottom)
            .child(
                Button::new()
                    .flat()
                    .width(Size::px(24.))
                    .height(Size::px(24.))
                    .on_press(move |_: Event<PressEventData>| {
                        let mut editor = editor;
                        editor.set(Some(ConnectionTarget::New));
                    })
                    .child(Icon::new(IconName::Plus).size(14.)),
            )
    }
}

/// No connections. Not a fault — most projects read local files and never need one — so the copy
/// says what a connection is for rather than what is missing.
///
/// **Top-aligned, not centred**: a pane's empty state sits where its first row would, so
/// switching panes doesn't move the reader's eye down the panel and back.
#[derive(PartialEq)]
struct Empty {
    theme: ConnectionsTheme,
}

impl Component for Empty {
    fn render(&self) -> impl IntoElement {
        let editor = use_consume::<ConnectionRequest>();
        rect()
            .width(Size::fill())
            .min_width(Size::px(PANE_BODY_MIN_W))
            .vertical()
            .cross_align(Alignment::Center)
            .padding(EMPTY_PAD)
            .spacing(SP_4)
            .child(
                rect()
                    .width(Size::px(40.))
                    .height(Size::px(40.))
                    .corner_radius(R_3)
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
            .child(
                Button::new()
                    .on_press(move |_: Event<PressEventData>| {
                        let mut editor = editor;
                        editor.set(Some(ConnectionTarget::New));
                    })
                    .child(
                        rect()
                            .horizontal()
                            .cross_align(Alignment::Center)
                            .spacing(SP_3)
                            .child(Icon::new(IconName::Plus).size(12.))
                            .child(Prose::new("Add connection")),
                    ),
            )
    }
}
