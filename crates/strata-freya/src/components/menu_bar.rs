//! **The menubar, where there is nowhere to install one** — the App / File / Edit / Window tree
//! behind a single trigger in the title bar we already own off macOS.
//!
//! On macOS this adds nothing: `menu::app_menu` builds a real muda menubar and the fork installs it
//! into `NSApp`, which is the platform's own surface and always the better one. The trigger's label
//! is rendered plainly there, so the project header keeps its brand and nothing moves.
//!
//! **There is no native menubar to install elsewhere, and that is not a fork gap.** muda can attach
//! a menubar to a GTK window (`init_for_gtk_window`) or a Win32 one (`init_for_hwnd`); Freya's
//! windows are winit's, which are neither, so there is no handle to hand it. The menu therefore has
//! to be *drawn* rather than installed, and once the title bar is ours ([`super::chrome`]) there is
//! somewhere to draw it.
//!
//! **One button, not a row of them** — the shape Vivaldi, Firefox and Chrome all use off macOS, and
//! the one this app's header already had room for: the brand mark *is* the trigger, and the menus
//! open as submenus inside the one dropdown it drops. A row of four separate triggers spends the
//! header's width on menu titles that a desktop outside macOS does not expect to see there.
//!
//! **One structure, two presentations.** The items come from [`menu::MENUS`] and act through
//! [`menu::dispatch_from_window`], so this surface and the muda one carry the same items in the
//! same order and do the same things. [`Placement`] is the only thing that differs, and it differs
//! because the App menu has nowhere to go here: off macOS the application's name is not a menu, so
//! its items sit flat at the foot of the dropdown.
//!
//! **Only the windows that can use it.** A [`Gate`] with nothing set is a menu of greyed rows, so
//! this belongs in the launcher and the project header — the two *workspace* windows — and not in
//! Settings, Configure, Export or the source editor, which are panels. Those windows still get
//! window controls; they have no menu because they have no menu items.
//!
//! **Open Recent is a section, not a nested submenu.** It is already inside one flyout, and a
//! second level of nesting for a flat list of projects is a level nobody navigates. The recents are
//! listed under a label instead, which is how the header's project switcher shows the same data.

use freya::prelude::*;
use strata_core::config::{RecentProject, Settings};
use strata_core::keymap::hint;

use super::divider::Divider;
use super::icon::{Icon, IconName};
use super::metrics::{SP_1, SP_3, SP_4};
use super::typography::{Eyebrow, Meta, Prose};
use crate::menu::{self, Gate, Item, MenuCmd, MenuScope, Placement, Section};
use crate::state::{use_config, AppCtx, ConfigChan};
use crate::theme::{use_roles, Role, RoleColors};

/// The width of a row, and so of the menu around it. Wide enough that the longest label and its
/// chord hint — "Check for Updates…" against `Ctrl+Shift+Z` — sit on one line with the gap between
/// them still reading as a gap.
///
/// **Stated on the rows, not as the container's `min_width`.** A `MenuContainer` sizes to its
/// content (`Content::fit`), so a row asking to `fill` has nothing bounded to fill and resolves
/// against the window instead — which is a menu the width of the app. Giving the rows a width makes
/// the panel's width theirs plus its padding, and there is nothing left to resolve.
const MENU_WIDTH: f32 = 268.;

/// How many recents the **Open Recent** section lists. The same window the muda menu's submenu
/// uses, so the two surfaces show the same projects.
const RECENT_LIMIT: usize = 10;

/// The menu trigger for one window.
///
/// `scope` is the same [`MenuScope`] the window hands `use_register_window`, so the rows grey by
/// exactly the rule the macOS menubar greys by — [`Gate::allows`] is the mapping both read.
///
/// `label` is what the trigger shows: the project header passes its brand, so the mark it already
/// draws becomes the button rather than gaining one beside it. Left unset, the trigger draws the
/// app mark on its own, which is what the launcher's bare strip wants. On macOS the label is
/// rendered as-is with no trigger around it, and an unset one renders nothing at all.
#[derive(PartialEq)]
pub struct MenuBar {
    pub scope: MenuScope,
    pub label: Option<Element>,
}

impl MenuBar {
    pub fn new(scope: MenuScope) -> Self {
        Self { scope, label: None }
    }

    /// Use `label` as the trigger's face — the brand the header already draws.
    pub fn label(mut self, label: impl IntoElement) -> Self {
        self.label = Some(label.into_element());
        self
    }
}

impl Component for MenuBar {
    fn render(&self) -> impl IntoElement {
        // macOS has the real menubar, so there is no trigger to build — just whatever the caller
        // wanted shown. An early return rather than a `#[cfg]` at the call sites, so a title bar
        // places this the same way on every platform.
        if cfg!(target_os = "macos") {
            return rect().maybe_child(self.label.clone()).into_element();
        }

        let app = use_consume::<AppCtx>();
        let settings = use_config(ConfigChan::Settings);
        let recents = use_config(ConfigChan::Recents);
        let roles = use_roles();
        let mut open = use_state(|| false);

        let gate = self.scope.gate(&app.windows.read());
        let config = settings.read();
        let recent_rows: Vec<RecentProject> = recents
            .read()
            .recent_projects
            .iter()
            .take(RECENT_LIMIT)
            .cloned()
            .collect();

        // **Submenus first, the root items after them** — which is not `MENUS` order, and is the
        // one place the two presentations lay the same structure out differently. A macOS menubar
        // leads with the App menu because that is where the platform puts it; a single dropdown
        // puts Settings… and Quit at the *foot*, under a rule, the way Vivaldi, Firefox and Chrome
        // all do. Ordering by placement rather than reordering `MENUS` keeps the macOS menubar
        // right, since it reads the list as written.
        let ordered = menu::MENUS
            .iter()
            .filter(|section| section.placement == Placement::Submenu)
            .chain(
                menu::MENUS
                    .iter()
                    .filter(|section| section.placement == Placement::Root),
            );
        let dropdown = ordered.fold(
            Menu::new().on_close(move |()| open.set(false)),
            |dropdown, section| {
                section_into(
                    dropdown,
                    section,
                    gate,
                    &config.settings,
                    &recent_rows,
                    &app,
                    roles,
                    open,
                )
            },
        );

        let face = self
            .label
            .clone()
            .unwrap_or_else(|| Icon::new(IconName::StrataLogo).size(20.).into_element());
        let trigger = Button::new().flat().on_press(move |_| open.toggle()).child(
            rect()
                .horizontal()
                .cross_align(Alignment::Center)
                .spacing(SP_1)
                .child(face)
                .child(
                    Icon::new(IconName::ChevronDown)
                        .size(12.)
                        .color(roles.get(Role::TextPlaceholder)),
                ),
        );

        Attached::new(trigger)
            .bottom()
            .align_start()
            .offset(4.)
            .maybe_child(open().then_some(dropdown))
            .into_element()
    }
}

/// Fold one [`Section`] into the dropdown, as a submenu or as flat rows — see [`Placement`].
///
/// A plain function rather than a component, so it uses no hooks: it runs in a fold over
/// [`menu::MENUS`], and a hook inside that loop would change count with the menu list.
#[allow(clippy::too_many_arguments)]
fn section_into(
    dropdown: Menu,
    section: &Section,
    gate: Gate,
    settings: &Settings,
    recents: &[RecentProject],
    app: &AppCtx,
    roles: RoleColors,
    open: State<bool>,
) -> Menu {
    let items = |into: Vec<Element>| {
        section.items.iter().fold(into, |mut acc, item| {
            match item {
                Item::Separator => acc.push(Divider::menu().into_element()),
                Item::Cmd(cmd) => {
                    acc.push(command_row(*cmd, gate, settings, open, app, roles));
                }
                Item::Recent => recent_rows(&mut acc, recents, gate, open, app, roles),
            }
            acc
        })
    };

    match section.placement {
        Placement::Submenu => dropdown.child(
            SubMenu::new()
                .label(
                    rect()
                        .width(Size::px(MENU_WIDTH))
                        .child(Prose::new(section.title).color(roles.get(Role::Text))),
                )
                .children(items(Vec::new())),
        ),
        // The app-level items sit under a rule, at the foot, with no title over them — there is no
        // App menu off macOS to be their heading.
        Placement::Root => items(vec![Divider::menu().into_element()])
            .into_iter()
            .fold(dropdown, Menu::child),
    }
}

/// One command row: its label, its live chord hint, and whether this window can carry it out.
fn command_row(
    cmd: MenuCmd,
    gate: Gate,
    settings: &Settings,
    mut open: State<bool>,
    app: &AppCtx,
    roles: RoleColors,
) -> Element {
    let app = app.clone();
    row(
        cmd.label().to_string(),
        // The *live* chord, not the one the item was built with: Settings ▸ Keymap can rebind at
        // any moment, and a stale hint is worse than none — it names a key that does nothing.
        cmd.key_command()
            .map(|command| hint(settings, command))
            .unwrap_or_default(),
        gate.allows(cmd),
        roles,
        move || {
            open.set(false);
            menu::dispatch_from_window(cmd, app.clone());
        },
    )
}

/// Push the **Open Recent** rows under a section label — or nothing at all when there are none,
/// since a label over an empty list says less than leaving it out.
fn recent_rows(
    into: &mut Vec<Element>,
    recents: &[RecentProject],
    gate: Gate,
    mut open: State<bool>,
    app: &AppCtx,
    roles: RoleColors,
) {
    if recents.is_empty() || !gate.allows_recent() {
        return;
    }
    into.push(
        rect()
            .padding(Gaps::new(SP_3, SP_4, SP_3, SP_4))
            .child(Eyebrow::new("OPEN RECENT").color(roles.get(Role::TextPlaceholder)))
            .into_element(),
    );
    for recent in recents {
        let app = app.clone();
        let path = recent.path.clone();
        into.push(row(
            recent.name.clone(),
            String::new(),
            true,
            roles,
            move || {
                open.set(false);
                menu::open_recent_from_window(path.clone(), app.clone());
            },
        ));
    }
}

/// A menu row: label on the left, chord hint on the right, greyed when the window cannot act on it.
///
/// A disabled row is **drawn, not omitted**. A menu whose items come and go is one nobody can
/// learn, and greying is what every platform's menubar does with an item that is momentarily out of
/// reach — which is also what `MenuHandles::take_gate` does to the muda items.
fn row(
    label: String,
    chord: String,
    enabled: bool,
    roles: RoleColors,
    on_press: impl FnMut() + 'static,
) -> Element {
    let mut on_press = on_press;
    MenuButton::new()
        .enabled(enabled)
        .on_press(move |_| on_press())
        .child(
            rect()
                .width(Size::px(MENU_WIDTH))
                .horizontal()
                .cross_align(Alignment::Center)
                .content(Content::Flex)
                .spacing(SP_4)
                .child(Prose::new(label))
                .child(rect().width(Size::flex(1.)).height(Size::px(1.)))
                .maybe_child(
                    (!chord.is_empty())
                        .then(|| Meta::new(chord).color(roles.get(Role::TextPlaceholder))),
                ),
        )
        .into_element()
}
