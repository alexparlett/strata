//! The application menubar (macOS): an App menu — About · Check for Updates… · Settings… ·
//! Hide/Show · Quit — a File menu — New Query · Open… · Open Recent · Save Query · Close
//! Project — a standard Edit menu — Undo · Redo · Cut · Copy · Paste · Select All — and a
//! Window menu — Minimize · Zoom · Cycle Windows — built with muda through the fork's `menu`
//! feature.
//!
//! **Quit and Close Project are different things, and both route through the close veto**,
//! never Cocoa's `terminate:` (muda's `PredefinedMenuItem::quit()` sends exactly that, the
//! thing that bypassed the T2 confirm). Quit asks *every* window to close and marks the app
//! as quitting, so the open projects stay in the persisted open-set and the next launch
//! reopens them; Close Project asks only the focused window, which drops it from that set
//! and puts the launcher up if it was the last — the same thing its red button does.
//!
//! **The Edit menu is custom items too**, not muda's predefined set, whose items send Cocoa
//! first-responder selectors a Skia view never receives. Each item's event instead **synthesizes
//! the command's effective chord into the focused window's keyboard pipeline**
//! ([`NativeEventExt::send_key_press`]), so menu clicks and accelerator presses flow through the
//! same path as typed keys and the focused element decides. First-responder semantics, without
//! Cocoa.
//!
//! **That is also why the menubar is scoped to the focused window** ([`MenuScope`], resolved into a
//! [`Gate`]). An item reaching its window through the pipeline works only where that window has a
//! listener, and *where the listener is* differs per item — so the gate is four independent flags
//! rather than a rank. Without it those items look live and do nothing, and Close Project, which
//! routes at the focused window rather than through the pipeline, would close a panel while naming
//! the project.
//!
//! **Predefined items carry their own platform accelerators** (Hide ⌘H, Minimize ⌘M) which muda
//! offers no way to change, so those chords are effectively reserved: the keymap can be told to
//! bind one and the OS still resolves the menu item first, [`MenuHandles::suspend_accelerators`]
//! included. Left as it is, since no app lets you rebind ⌘H.
//!
//! Accelerators derive from the keymap (`effective_chord`), and
//! [`MenuHandles::sync_chords`] re-applies every one off `ConfigChan::Settings`. That is not
//! tidiness: a stale accelerator is the OS *consuming* the old chord before the window sees it, so
//! the item would keep firing on a shortcut the user rebound away.

use freya::menu::accelerator::Accelerator;
use freya::menu::{
    AboutMetadata, IsMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu,
};
use freya::prelude::{
    use_drop, use_hook, use_side_effect, Code, Key, Modifiers, ModifiersExt, NamedKey,
    NativeEventExt, Platform, RendererContext, State, WinitPlatformExt, WritableUtils,
};
use strata_core::config::{Command, KeyChord, RecentProject, Settings};
use strata_core::keymap::effective_chord;

use crate::apps::project::{window_geometry_blocking, ProjectApp};
use crate::platform::{self, OpenCtx, OpenTarget, Windows};
use crate::state::{install_site, use_config, AppCtx, ConfigChan};
use crate::updater::{raise, AskSlot};

/// A custom menubar item — the typed vocabulary the builder and the event handler
/// share, so dispatch is an exhaustive `match`, not string comparison (the Dioxus
/// menu's `MenuCmd` pattern). Grows a variant per item as the menu fills out (P6-02).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MenuCmd {
    /// Quit Strata — every window, routed through the close veto rather than Cocoa
    /// `terminate:`.
    Quit,
    /// Open the Settings window, pinned above the focused window.
    OpenSettings,
    /// Ask what the update situation is, and report whatever the answer turns out to be in the
    /// focused window's dialog (`updater::raise`) — "nothing to install" included.
    CheckUpdates,
    /// Pick a project folder and open it.
    OpenProject,
    /// Close the focused window (and open the launcher if it was the last).
    CloseProject,
    /// Open a query tab in the focused project window.
    NewQuery,
    /// Save the focused project window's active query.
    SaveQuery,
    /// Move focus to the next workspace window.
    CycleWindow,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
}

impl MenuCmd {
    const ALL: [Self; 14] = [
        Self::Quit,
        Self::OpenSettings,
        Self::CheckUpdates,
        Self::OpenProject,
        Self::CloseProject,
        Self::NewQuery,
        Self::SaveQuery,
        Self::CycleWindow,
        Self::Undo,
        Self::Redo,
        Self::Cut,
        Self::Copy,
        Self::Paste,
        Self::SelectAll,
    ];

    /// The stable string id muda carries (namespaced clear of predefined items).
    fn id(self) -> &'static str {
        match self {
            Self::Quit => "strata.quit",
            Self::OpenSettings => "strata.app.settings",
            Self::CheckUpdates => "strata.app.check-updates",
            Self::OpenProject => "strata.file.open-project",
            Self::CloseProject => "strata.file.close-project",
            Self::NewQuery => "strata.file.new-query",
            Self::SaveQuery => "strata.file.save-query",
            Self::CycleWindow => "strata.window.cycle",
            Self::Undo => "strata.edit.undo",
            Self::Redo => "strata.edit.redo",
            Self::Cut => "strata.edit.cut",
            Self::Copy => "strata.edit.copy",
            Self::Paste => "strata.edit.paste",
            Self::SelectAll => "strata.edit.select-all",
        }
    }

    /// The command a [`MenuEvent`]'s id names, if it is one of ours (tray menus and
    /// predefined items share muda's event stream — foreign ids are simply not ours).
    fn parse(id: &MenuId) -> Option<Self> {
        Self::ALL.into_iter().find(|cmd| id.0 == cmd.id())
    }

    /// The keymap command this item dispatches through the focused window's keyboard
    /// pipeline — the Edit set, plus **Open…**, whose folder picker belongs to whichever
    /// window is focused (the launcher stands down after opening; a project window doesn't),
    /// **Settings…**, which pins itself above whichever window asked, and the two that act on
    /// the focused project window (New Query · Save Query). `None` for the window-lifecycle
    /// items, which the handler routes through the close path instead — and for **Check for
    /// Updates…**, which has no chord to synthesize (UP-03 binds none) and so acts on the
    /// focused window's parked slot, the way Open Recent acts on its parked open path.
    pub fn key_command(self) -> Option<Command> {
        match self {
            Self::Quit | Self::CloseProject | Self::CheckUpdates => None,
            Self::OpenSettings => Some(Command::OpenSettings),
            Self::OpenProject => Some(Command::OpenProject),
            Self::NewQuery => Some(Command::NewTab),
            Self::SaveQuery => Some(Command::SaveQuery),
            Self::CycleWindow => Some(Command::CycleWindow),
            Self::Undo => Some(Command::Undo),
            Self::Redo => Some(Command::Redo),
            Self::Cut => Some(Command::Cut),
            Self::Copy => Some(Command::Copy),
            Self::Paste => Some(Command::Paste),
            Self::SelectAll => Some(Command::SelectAll),
        }
    }
}

impl MenuCmd {
    /// The item's text. Hoisted off the muda builder because the in-app menubar
    /// ([`crate::components::menu_bar`]) renders the same items and must name them identically —
    /// two spellings of "Close Project" is two features as far as a user is concerned.
    ///
    /// The ellipses are load-bearing: an item that opens a dialog rather than acting says so, and
    /// that is a platform convention on every desktop, not a macOS one.
    pub fn label(self) -> &'static str {
        match self {
            Self::Quit => "Quit Strata",
            Self::OpenSettings => "Settings…",
            Self::CheckUpdates => "Check for Updates…",
            Self::OpenProject => "Open…",
            Self::CloseProject => "Close Project",
            Self::NewQuery => "New Query",
            Self::SaveQuery => "Save Query",
            Self::CycleWindow => "Cycle Windows",
            Self::Undo => "Undo",
            Self::Redo => "Redo",
            Self::Cut => "Cut",
            Self::Copy => "Copy",
            Self::Paste => "Paste",
            Self::SelectAll => "Select All",
        }
    }
}

/// One entry in a menu's item list.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Item {
    Cmd(MenuCmd),
    Separator,
    /// **Open Recent** — the one entry that is a submenu rather than a command, because each of its
    /// rows names a different project path ([`RECENT_ID_PREFIX`]).
    Recent,
}

/// Where a section's items sit in the **in-app** menu. macOS ignores this: a menubar is a row of
/// submenus and every section is one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Placement {
    /// A submenu behind its title — `File ›`, opening a flyout.
    Submenu,
    /// Flat at the foot of the one dropdown, with no title above them. The app-level items go here
    /// because there is no App menu to put them in: off macOS the application's name is not a menu,
    /// so Settings… and Quit sit at the bottom of the menu the way Vivaldi, Firefox and Chrome all
    /// place them.
    Root,
}

/// One menu, and the items under it.
pub struct Section {
    pub title: &'static str,
    pub items: &'static [Item],
    pub placement: Placement,
}

/// **The menus, and what is in them** — the structure of record, shared by the two surfaces that
/// present it.
///
/// macOS gets a real menubar built with muda ([`app_menu`]); every other platform has no such
/// thing to install into, so it gets [`crate::components::menu_bar::MenuBar`] drawn into the title
/// bar we already own there. Both read this, so an item cannot exist on one platform only — which
/// is what `every_command_is_in_exactly_one_menu` below pins.
///
/// The App menu's predefined Cocoa items — About, Hide, Hide Others, Show All — are deliberately
/// **not** here. They are `PredefinedMenuItem`s with no [`MenuCmd`] behind them and no meaning off
/// macOS (nothing hides an app on a tiling WM), so they stay inside [`app_menu`], which is the
/// macOS-only builder and the right place for macOS-only items.
pub const MENUS: &[Section] = &[
    Section {
        title: "Strata",
        placement: Placement::Root,
        items: &[
            Item::Cmd(MenuCmd::CheckUpdates),
            Item::Separator,
            Item::Cmd(MenuCmd::OpenSettings),
            Item::Separator,
            Item::Cmd(MenuCmd::Quit),
        ],
    },
    Section {
        title: "File",
        placement: Placement::Submenu,
        items: &[
            Item::Cmd(MenuCmd::NewQuery),
            Item::Cmd(MenuCmd::OpenProject),
            Item::Recent,
            Item::Cmd(MenuCmd::SaveQuery),
            Item::Separator,
            Item::Cmd(MenuCmd::CloseProject),
        ],
    },
    Section {
        title: "Edit",
        placement: Placement::Submenu,
        items: &[
            Item::Cmd(MenuCmd::Undo),
            Item::Cmd(MenuCmd::Redo),
            Item::Separator,
            Item::Cmd(MenuCmd::Cut),
            Item::Cmd(MenuCmd::Copy),
            Item::Cmd(MenuCmd::Paste),
            Item::Separator,
            Item::Cmd(MenuCmd::SelectAll),
        ],
    },
    Section {
        title: "Window",
        placement: Placement::Submenu,
        items: &[Item::Cmd(MenuCmd::CycleWindow)],
    },
];

impl From<MenuCmd> for MenuId {
    fn from(cmd: MenuCmd) -> Self {
        MenuId::new(cmd.id())
    }
}

/// A [`KeyChord`] as a muda accelerator (`CmdOrCtrl+Q`). `None` when the chord has no
/// muda-parsable key — the item then simply ships without an accelerator.
fn accelerator(chord: &KeyChord) -> Option<Accelerator> {
    let mut spec = String::new();
    if chord.primary {
        spec.push_str("CmdOrCtrl+");
    }
    if chord.shift {
        spec.push_str("Shift+");
    }
    if chord.alt {
        spec.push_str("Alt+");
    }
    spec.push_str(&chord.key);
    spec.parse().ok()
}

/// A [`KeyChord`] as a synthesizable key event: the chord's key plus its modifier
/// flags (`primary` folds to the platform primary modifier). `None` when the key name
/// is neither a single character nor a keyboard-types named key.
fn synthetic_key(chord: &KeyChord) -> Option<(Key, Modifiers)> {
    let mut chars = chord.key.chars();
    let key = match (chars.next(), chars.next()) {
        (Some(_), None) => Key::Character(chord.key.clone()),
        _ => Key::Named(chord.key.parse::<NamedKey>().ok()?),
    };
    let mut modifiers = Modifiers::empty();
    if chord.primary {
        modifiers |= Modifiers::ctrl_or_meta();
    }
    if chord.shift {
        modifiers |= Modifiers::SHIFT;
    }
    if chord.alt {
        modifiers |= Modifiers::ALT;
    }
    Some((key, modifiers))
}

/// The id prefix a **recent-project** item carries, with its project folder appended —
/// the one piece of menu vocabulary that can't be a fixed [`MenuCmd`] variant, because
/// each item names a different path. Namespaced like the rest, and the path is the id's
/// whole remainder, so a folder containing the separator can't split wrong.
const RECENT_ID_PREFIX: &str = "strata.file.recent:";

/// What the focused window can actually carry out, as four independent facts.
///
/// The plain-data half of [`MenuScope`]: a window's `OpenCtx` must **not** be parked in an
/// app-global that outlives it — that is what [`use_file_menu`]'s drop guard is for — and the
/// items need only the answers, not the handles behind them.
///
/// **Four flags rather than a rank**, because they do not nest. A project window whose load
/// failed can close, open and reach Settings but has no workbench to put a query tab in; a
/// window with nowhere to cycle to is otherwise complete. Ordering these as "how much of a
/// project window is this" was the first shape and it was wrong — every such scale ends up with
/// an item on the wrong side of a threshold, and the flag it wanted was the one the scale
/// smoothed over. Each field names the items it gates, so an item's gate is a lookup rather
/// than a judgement about where it falls.
///
/// [`Default`] is every flag off — a panel, and also what the menubar carries between launch
/// and the first window taking focus.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct Gate {
    /// The focused window is one the user *works* in — the launcher or a project window, the
    /// same split [`WindowKind::is_workspace`](crate::platform::WindowKind) draws for the
    /// registry. Gates **Open…**, **Open Recent** and **Settings…**, whose listeners every
    /// such window has and no panel does.
    workspace: bool,
    /// …and it has a project. Gates **Close Project**, whose listener is on the window root
    /// and so is mounted in every arm — including the one showing a load error, which is
    /// precisely the window a user most wants to close.
    project: bool,
    /// …whose subtree is up. Gates **New Query** and **Save Query**, whose listeners live in
    /// the *workbench*: on the loading and load-failed arms there is no such listener, so
    /// without this the items would sit enabled over a window that cannot answer them.
    workbench: bool,
    /// There is a second workspace window to move focus to. Gates **Cycle Windows**, which is
    /// the one item whose availability is about the app rather than about this window.
    cyclable: bool,
    /// This build has somewhere to install an update into — `install_site().bundle().is_some()`.
    /// Gates **Check for Updates…**, which can find one but never install it in a `cargo run`
    /// build, and so is offered nowhere it cannot be carried out.
    ///
    /// The one flag here that is about the *build* rather than about a window or the window set.
    /// It lived outside `Gate` until it did not have to: the muda menubar applied it inline while
    /// [`allows`](Self::allows) said Check for Updates… was always live, so the two surfaces
    /// disagreed for exactly this item in exactly the build a developer runs.
    installed: bool,
}

/// Whether this build has somewhere to install an update into.
///
/// A function rather than the expression inline at both `Gate` construction sites, so "what counts
/// as installed" is one answer: `install_site()` is cached, and a dev build's `Site::Unbundled` is
/// exactly the case this is here to catch.
fn installed() -> bool {
    install_site().bundle().is_some()
}

impl Gate {
    /// **Can this window carry `cmd` out?** — the flag-to-item mapping the field docs above
    /// describe, stated once as code.
    ///
    /// It exists because there are now two menus to gate. `MenuHandles::take_gate` greys muda's
    /// items from the same four flags, and the in-app menubar has to grey exactly the same ones or
    /// the two surfaces disagree about what the app can do. Reading the mapping off one function is
    /// what keeps that from being a matter of remembering.
    ///
    /// Quit is always live: it acts on the app rather than on a window. Check for Updates… needs a
    /// window that can raise its dialog *and* a build with somewhere to install into — see
    /// [`installed`](Self::installed).
    pub fn allows(self, cmd: MenuCmd) -> bool {
        match cmd {
            MenuCmd::Quit => true,
            MenuCmd::CheckUpdates => self.workspace && self.installed,
            MenuCmd::OpenProject | MenuCmd::OpenSettings => self.workspace,
            MenuCmd::CloseProject => self.project,
            MenuCmd::NewQuery | MenuCmd::SaveQuery => self.workbench,
            MenuCmd::CycleWindow => self.cyclable,
            // The Edit set reaches the focused *element* through the keyboard pipeline, so it is
            // live wherever a window is — a panel's form fields undo and paste like any other.
            MenuCmd::Undo
            | MenuCmd::Redo
            | MenuCmd::Cut
            | MenuCmd::Copy
            | MenuCmd::Paste
            | MenuCmd::SelectAll => true,
        }
    }

    /// Whether **Open Recent** has anything to open into — the submenu's own gate, which is
    /// `workspace` for the same reason [`MenuCmd::OpenProject`] is.
    pub fn allows_recent(self) -> bool {
        self.workspace
    }
}

/// The mutable half of the menubar: the File and Window menus' parts and every item that
/// carries an accelerator, kept after construction so the menus can follow the app.
///
/// Four things make them dynamic. **Open Recent** mirrors the config store's recents, which
/// move on every open, pin and remove. **Close Project** applies only to a window that has
/// one, so it (and the separator above it) is pulled from the menu rather than sitting there
/// greyed — a menubar is app-global, but this half of it is about the focused window. The
/// other **scoped** items grey instead of leaving, because unlike Close Project they belong to
/// a menu that keeps its shape ([`Gate`], [`sync_gate`](Self::sync_gate)). And the
/// **accelerators** follow the keymap, which Settings ▸ Keymap can rebind at any moment
/// ([`sync_chords`](Self::sync_chords)).
pub struct MenuHandles {
    file: Submenu,
    recent: Submenu,
    separator: PredefinedMenuItem,
    /// Every item that carries an accelerator, in the order [`MenuChords`] names them.
    quit: MenuItem,
    settings: MenuItem,
    /// Carries no chord, so it is not in [`MenuChords`] — only a gate.
    check_updates: MenuItem,
    open_project: MenuItem,
    close_project: MenuItem,
    new_query: MenuItem,
    save_query: MenuItem,
    cycle_window: MenuItem,
    undo: MenuItem,
    redo: MenuItem,
    cut: MenuItem,
    copy: MenuItem,
    paste: MenuItem,
    select_all: MenuItem,
    /// Whether the separator + Close Project pair is currently in the File menu.
    closable: bool,
    /// What the focused window is, for the items that grey rather than leave.
    gate: Gate,
    /// The recents the submenu currently renders, so an unchanged sync rebuilds nothing.
    recents: Vec<RecentProject>,
    /// The chords the settings resolve to, for the same reason.
    chords: MenuChords,
    /// Whether the items are carrying **no** accelerator while a chord is being captured — see
    /// [`suspend_accelerators`](Self::suspend_accelerators).
    suspended: bool,
}

impl MenuHandles {
    /// Point the File and Window menus at the focused window: its view of the recents, and
    /// which of the items its kind of window can actually carry out. Cheap and idempotent —
    /// each half no-ops when already right, so the focused window can call it on every
    /// recents change.
    /// **One** [`apply`](Self::apply) for both halves, at the end. Each half records what
    /// changed and none of them pushes on its own, because they share the item table: applying
    /// per half would write the recents' enabled state against the *old* gate and immediately
    /// overwrite it — two passes of ~24 calls across the objc boundary for one settled answer,
    /// on an effect that re-runs on every focus change and every recents write.
    fn sync(&mut self, recents: &[RecentProject], gate: Gate) {
        let moved = self.take_recents(recents) | self.take_gate(gate);
        self.sync_closable(gate.project);
        if moved {
            self.apply();
        }
    }

    /// Record what the focused window can do — see [`Gate`]. Returns whether it moved.
    fn take_gate(&mut self, gate: Gate) -> bool {
        let moved = self.gate != gate;
        self.gate = gate;
        moved
    }

    /// Re-point every accelerator at the chords the settings resolve to **now**.
    ///
    /// Called with the File-menu sync, off the same effect, because a rebind is app-global and
    /// only a window has a reactive scope to notice one in — the focused window drives this for
    /// the same reason it drives the File half, and a window that isn't focused never fights it.
    ///
    /// An item whose command reaches the app through the keyboard pipeline is **disabled while
    /// that command is unbound**, exactly as it ships at launch: there is no chord to synthesize,
    /// so the item would look live and do nothing. Quit and Close Project keep their accelerator
    /// treatment but never their enabled state, because they route through the close veto
    /// directly and work with no chord at all.
    pub fn sync_chords(&mut self, chords: &MenuChords) {
        if self.chords == *chords {
            return;
        }
        self.chords = chords.clone();
        self.apply();
    }

    /// Take every accelerator **off** the menubar, or put them back.
    ///
    /// Settings ▸ Keymap captures a rebind by listening for the next key press, and the OS
    /// resolves a menu accelerator *before* the window sees the key — so with the menubar armed,
    /// pressing ⌘C to bind it would copy instead, and the row would still be listening. Half the
    /// chords a user is likely to reach for are menu accelerators, so this is the difference
    /// between a capture that works and one that works for the keys nobody wants.
    ///
    /// A held flag rather than a `sync_chords(&Default)` call, so the focused window's routine
    /// sync — which fires on focus changes and on any settings write — cannot re-arm the menubar
    /// underneath a capture. While suspended a sync still records what the settings say; it just
    /// doesn't reach the items until the capture ends.
    ///
    /// **Whoever suspends owns putting it back, and must do so on losing focus as well as on
    /// finishing.** This flag is app-wide while the thing it protects is one window's key
    /// listener, and nothing else clears it: `sync_chords` deliberately cannot, and a menubar left
    /// suspended takes every gated item's chord *and* its enabled state with it, in every window,
    /// for as long as it is held. So the caller's condition is "a capture is in progress **and**
    /// my window is focused", not just the first half (`views::keymap`).
    pub fn suspend_accelerators(&mut self, suspended: bool) {
        if self.suspended == suspended {
            return;
        }
        self.suspended = suspended;
        self.apply();
    }

    /// Push onto the items everything about them that moves: the chords the menubar should be
    /// carrying — the settings' own, or nothing at all while a capture is in progress — and
    /// each scoped item's enabled state.
    ///
    /// One function for both because they are one property. An item that reaches its window
    /// through the keyboard pipeline is live only if it has **both** a chord to synthesize and
    /// a window that listens for it, so splitting them would mean two writers racing on
    /// `set_enabled` and the last sync winning.
    ///
    /// The chords are **destructured** rather than read field by field, for the reason
    /// `settings_merge!` is a macro: the pattern names every field, so a command that grows a
    /// menu item and forgets this list is a build error rather than an accelerator that silently
    /// never updates.
    fn apply(&self) {
        let none = MenuChords::default();
        let MenuChords {
            quit,
            open_settings,
            open_project,
            close_project,
            new_query,
            save_query,
            cycle_window,
            undo,
            redo,
            cut,
            copy,
            paste,
            select_all,
        } = match self.suspended {
            true => &none,
            false => &self.chords,
        };
        let set = |item: &MenuItem, chord: &Option<KeyChord>, scope: Option<bool>| {
            if let Err(err) = item.set_accelerator(chord.as_ref().and_then(accelerator)) {
                tracing::error!("menubar: updating an accelerator failed: {err}");
            }
            if let Some(in_scope) = scope {
                item.set_enabled(in_scope && chord.is_some());
            }
        };
        let Gate {
            workspace,
            project: _,
            workbench,
            cyclable,
            // Read through `gate.allows` below rather than destructured, because Check for
            // Updates… is the one item whose rule needs two of these fields.
            installed: _,
        } = self.gate;
        set(&self.quit, quit, None);
        set(&self.settings, open_settings, Some(workspace));
        self.check_updates
            .set_enabled(self.gate.allows(MenuCmd::CheckUpdates));
        set(&self.open_project, open_project, Some(workspace));
        set(&self.close_project, close_project, None);
        set(&self.new_query, new_query, Some(workbench));
        set(&self.save_query, save_query, Some(workbench));
        set(&self.cycle_window, cycle_window, Some(cyclable));
        set(&self.undo, undo, Some(true));
        set(&self.redo, redo, Some(true));
        set(&self.cut, cut, Some(true));
        set(&self.copy, copy, Some(true));
        set(&self.paste, paste, Some(true));
        set(&self.select_all, select_all, Some(true));
        self.recent
            .set_enabled(workspace && !self.recents.is_empty());
    }

    /// Rebuild **Open Recent** when the list has actually moved, and report whether it did.
    /// muda has no way to edit an item's label in place cheaply, so a change rebuilds the
    /// submenu wholesale; the equality guard is what keeps that off the hot path. The
    /// submenu's *enabled* state is [`apply`](Self::apply)'s, since it depends on the gate too.
    fn take_recents(&mut self, recents: &[RecentProject]) -> bool {
        let same = self.recents.len() == recents.len()
            && self
                .recents
                .iter()
                .zip(recents)
                .all(|(a, b)| a.path == b.path && a.name == b.name);
        if same {
            return false;
        }
        while self.recent.remove_at(0).is_some() {}
        for r in recents {
            let item = MenuItem::with_id(
                MenuId::new(format!("{RECENT_ID_PREFIX}{}", r.path)),
                &r.name,
                true,
                None,
            );
            if let Err(err) = self.recent.append(&item) {
                tracing::error!("menubar: appending a recent failed: {err}");
            }
        }
        self.recents = recents.to_vec();
        true
    }

    /// Add or remove the separator + **Close Project** pair.
    fn sync_closable(&mut self, closable: bool) {
        if closable == self.closable {
            return;
        }
        let items: &[&dyn IsMenuItem] = &[&self.separator, &self.close_project];
        let result = if closable {
            self.file.append_items(items)
        } else {
            self.file
                .remove(&self.separator)
                .and_then(|()| self.file.remove(&self.close_project))
        };
        match result {
            Ok(()) => self.closable = closable,
            Err(err) => tracing::error!("menubar: updating Close Project failed: {err}"),
        }
    }
}

/// The app-global slot the menubar's handles land in. `None` until Freya calls the builder
/// at `resumed`, which is after `main` creates this but before any window can use it.
pub type MenuState = State<Option<MenuHandles>>;

/// Create the slot. Call **once**, in `main`, before `launch` — not a hook.
pub fn create_global_menu() -> MenuState {
    State::create_global(None)
}

/// What the focused window is, and — for a project window — the open path File ▸ Open Recent
/// resolves through.
///
/// Named at every window root rather than derived from
/// [`WindowKind`](crate::platform::WindowKind), which carries the same three-way split, because
/// only the root has the [`OpenCtx`] and this way a root cannot claim to be a project window
/// without producing one.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Copy, PartialEq)]
pub enum MenuScope {
    /// A project window: its recents, its open path, and something to close, save into and
    /// open a tab in — plus the restart-question slot every workspace window carries.
    Project(OpenCtx, AskSlot),
    /// The launcher: the recents and Open… — that is what it is for — but no project yet.
    Launcher(AskSlot),
    /// A panel over one of the above: Settings, Export, Configure. None of the File or Window
    /// commands has a listener there.
    Panel,
}

impl MenuScope {
    /// Whether this scope claims a window the user *works* in — the same question
    /// [`WindowKind::is_workspace`](crate::platform::WindowKind) answers about the registry
    /// entry beside it, which is why `use_register_window` asserts the two agree: the split is
    /// one fact stated twice, and a new window kind that updates only one of them would enable
    /// Open… and Close Project against a window with no listener for either.
    pub fn is_workspace(self) -> bool {
        !matches!(self, Self::Panel)
    }

    /// The plain-data half, which is all the items need — and all that may be held in an
    /// app-global, since an [`OpenCtx`] must not outlive its window.
    ///
    /// **Reads reactively**, and must: two of the four flags move while the window stands
    /// still. `loaded` flips as the project subtree mounts, faults and remounts; the window
    /// count changes whenever any window anywhere opens or closes. Called from
    /// [`use_file_menu`]'s effect, so both subscribe it and the menubar follows them.
    ///
    /// Only a project window is `cyclable`, because the launcher and a project window never
    /// coexist — the launcher opens when the last project closes, and opening a project closes
    /// it — so the launcher is always the only workspace window there is.
    pub fn gate(self, windows: &Windows) -> Gate {
        match self {
            Self::Project(open, _) => Gate {
                workspace: true,
                project: true,
                workbench: *open.loaded.read(),
                cyclable: windows.workspace_count() > 1,
                installed: installed(),
            },
            Self::Launcher(_) => Gate {
                workspace: true,
                installed: installed(),
                ..Gate::default()
            },
            Self::Panel => Gate::default(),
        }
    }

    /// The open path to park in [`AppCtx::open`] while this window is focused, so File ▸ Open
    /// Recent honours its "Opening a project" preference. Only a project window has one.
    fn open(self) -> Option<OpenCtx> {
        match self {
            Self::Project(open, _) => Some(open),
            Self::Launcher(_) | Self::Panel => None,
        }
    }

    /// The restart-question slot this window carries out an
    /// [`AppCtx::update_request`](crate::state::AppCtx) with while it is focused, so App ▸
    /// Check for Updates… raises its dialog where the user is looking.
    ///
    /// A panel has none: it mounts no `UpdateConfirm`, so a question raised there would be one
    /// nobody is watching — the same failure the gate exists to prevent for every other item.
    fn update_ask(self) -> Option<AskSlot> {
        match self {
            Self::Project(_, ask) | Self::Launcher(ask) => Some(ask),
            Self::Panel => None,
        }
    }
}

/// Keep the menubar pointed at this window for as long as it is focused: the File menu's
/// recents, which items its kind of window can carry out, where Open Recent opens — and every
/// item's accelerator, against the live keymap.
///
/// **Called only from [`use_register_window`](crate::platform::use_register_window)**, which every
/// window root must call anyway, so a new kind of window cannot ship without saying what its
/// menubar is.
///
/// Focus is the gate because the menubar is app-global while its File half is about *one* window,
/// so exactly one window drives it at a time. (Freya's `Platform` is per-window, so despite its
/// name `is_app_focused` is this window's focus.)
///
/// The accelerators ride the same gate for a different reason: they are not about this window at
/// all, but a `State` change only wakes a window's scope, so somebody has to notice — and "the
/// focused window" is a rule already in force. Settings is itself focused while Apply is pressed,
/// and if it closes in the same breath the window that takes focus re-runs this effect anyway.
pub fn use_file_menu(app: &AppCtx, scope: MenuScope) {
    let focused = use_hook(Platform::get).is_app_focused;
    let config = use_config(ConfigChan::Recents);
    let settings = use_config(ConfigChan::Settings);
    let mut menu = app.menu;
    let mut focused_open = app.open;
    let mut asked = app.update_request;
    let updates = app.updates;
    let windows = app.windows;
    let open = scope.open();
    let ask = scope.update_ask();
    use_side_effect(move || {
        if !*focused.read() {
            return;
        }
        if *asked.read() {
            if let Some(ask) = ask {
                asked.set(false);
                raise(updates, ask);
            }
        }
        focused_open.set_if_modified(open);
        let recents = config.read();
        let chords = menu_chords(&settings.read().settings);
        let gate = scope.gate(&windows.read());
        if let Some(handles) = menu.write().as_mut() {
            handles.sync(&recents.recent_projects, gate);
            handles.sync_chords(&chords);
        }
    });
    use_drop(move || {
        let parked = *focused_open.peek();
        if parked == open {
            focused_open.set(None);
        }
    });
}

/// The accelerator chords, resolved from settings before the menu builder runs, so the builder
/// captures plain data rather than the settings handle — and re-resolved on every settings
/// change, which is what [`MenuHandles::sync_chords`] compares against what the items carry.
///
/// [`Default`] is the **no accelerators at all** set, which is what the menubar carries while a
/// chord is being captured ([`MenuHandles::suspend_accelerators`]).
#[derive(Clone, PartialEq, Default)]
pub struct MenuChords {
    pub quit: Option<KeyChord>,
    pub open_settings: Option<KeyChord>,
    pub open_project: Option<KeyChord>,
    pub close_project: Option<KeyChord>,
    pub new_query: Option<KeyChord>,
    pub save_query: Option<KeyChord>,
    pub cycle_window: Option<KeyChord>,
    pub undo: Option<KeyChord>,
    pub redo: Option<KeyChord>,
    pub cut: Option<KeyChord>,
    pub copy: Option<KeyChord>,
    pub paste: Option<KeyChord>,
    pub select_all: Option<KeyChord>,
}

/// The effective menubar chords, resolved from settings at launch.
pub fn menu_chords(settings: &Settings) -> MenuChords {
    let chord = |cmd| effective_chord(settings, cmd);
    MenuChords {
        quit: chord(Command::Quit),
        open_settings: chord(Command::OpenSettings),
        open_project: chord(Command::OpenProject),
        close_project: chord(Command::CloseProject),
        new_query: chord(Command::NewTab),
        save_query: chord(Command::SaveQuery),
        cycle_window: chord(Command::CycleWindow),
        undo: chord(Command::Undo),
        redo: chord(Command::Redo),
        cut: chord(Command::Cut),
        copy: chord(Command::Copy),
        paste: chord(Command::Paste),
        select_all: chord(Command::SelectAll),
    }
}

/// Build the menubar: the App menu, the File menu, the standard Edit menu, then Window.
///
/// Returns the [`MenuHandles`] the app keeps, because File and Window are **not** static:
/// the recents follow the config store, and most of the rest belongs to whichever window is
/// focused ([`MenuScope`]). Freya calls this from `resumed`, on the thread that called
/// `launch` — the same (main) thread the UI runs on and the only one muda allows menu work
/// on — so the handles can be mutated straight from a window's effect, with no renderer hop.
///
/// Every **scoped** item ships disabled, which is what [`Gate::default`] says: no window has
/// focus yet, so nothing that reaches one through the keyboard pipeline can work. The first
/// window to take focus syncs, and from then on the built state is never read again. Building
/// them enabled would mean a launch frame in which the menubar promised more than it could do.
pub fn app_menu(chords: MenuChords) -> (Menu, MenuHandles) {
    let pipeline_item = |cmd: MenuCmd, label: &str, chord: &Option<KeyChord>, scoped: bool| {
        MenuItem::with_id(
            cmd,
            label,
            !scoped && chord.is_some(),
            chord.as_ref().and_then(accelerator),
        )
    };
    let quit = MenuItem::with_id(
        MenuCmd::Quit,
        "Quit Strata",
        true,
        chords.quit.as_ref().and_then(accelerator),
    );
    let settings = pipeline_item(
        MenuCmd::OpenSettings,
        "Settings…",
        &chords.open_settings,
        true,
    );
    let check_updates = MenuItem::with_id(MenuCmd::CheckUpdates, "Check for Updates…", false, None);
    let app = Submenu::new("Strata", true);
    let items: &[&dyn IsMenuItem] = &[
        &PredefinedMenuItem::about(
            Some("About Strata"),
            Some(AboutMetadata {
                name: Some("Strata".to_string()),
                comments: Some("A local Athena-style parquet query workspace".to_string()),
                ..Default::default()
            }),
        ),
        &check_updates,
        &PredefinedMenuItem::separator(),
        &settings,
        &PredefinedMenuItem::separator(),
        &PredefinedMenuItem::hide(Some("Hide Strata")),
        &PredefinedMenuItem::hide_others(None),
        &PredefinedMenuItem::show_all(None),
        &PredefinedMenuItem::separator(),
        &quit,
    ];
    if let Err(err) = app.append_items(items) {
        tracing::error!("menubar: appending App menu items failed: {err}");
    }

    let file = Submenu::new("File", true);
    let new_query = pipeline_item(MenuCmd::NewQuery, "New Query", &chords.new_query, true);
    let open_project = pipeline_item(MenuCmd::OpenProject, "Open…", &chords.open_project, true);
    let save_query = pipeline_item(MenuCmd::SaveQuery, "Save Query", &chords.save_query, true);
    let recent = Submenu::new("Open Recent", false);
    let separator = PredefinedMenuItem::separator();
    let close_project = MenuItem::with_id(
        MenuCmd::CloseProject,
        "Close Project",
        true,
        chords.close_project.as_ref().and_then(accelerator),
    );
    let items: &[&dyn IsMenuItem] = &[
        &new_query,
        &PredefinedMenuItem::separator(),
        &open_project,
        &recent,
        &PredefinedMenuItem::separator(),
        &save_query,
    ];
    if let Err(err) = file.append_items(items) {
        tracing::error!("menubar: appending File menu items failed: {err}");
    }

    let edit_item = |cmd: MenuCmd, label: &str, chord: &Option<KeyChord>| {
        pipeline_item(cmd, label, chord, false)
    };
    let undo = edit_item(MenuCmd::Undo, "Undo", &chords.undo);
    let redo = edit_item(MenuCmd::Redo, "Redo", &chords.redo);
    let cut = edit_item(MenuCmd::Cut, "Cut", &chords.cut);
    let copy = edit_item(MenuCmd::Copy, "Copy", &chords.copy);
    let paste = edit_item(MenuCmd::Paste, "Paste", &chords.paste);
    let select_all = edit_item(MenuCmd::SelectAll, "Select All", &chords.select_all);
    let edit = Submenu::new("Edit", true);
    let items: &[&dyn IsMenuItem] = &[
        &undo,
        &redo,
        &PredefinedMenuItem::separator(),
        &cut,
        &copy,
        &paste,
        &PredefinedMenuItem::separator(),
        &select_all,
    ];
    if let Err(err) = edit.append_items(items) {
        tracing::error!("menubar: appending Edit menu items failed: {err}");
    }

    let cycle_window = pipeline_item(
        MenuCmd::CycleWindow,
        "Cycle Windows",
        &chords.cycle_window,
        true,
    );
    let window = Submenu::new("Window", true);
    let items: &[&dyn IsMenuItem] = &[
        &PredefinedMenuItem::minimize(None),
        &PredefinedMenuItem::maximize(None),
        &PredefinedMenuItem::separator(),
        &cycle_window,
    ];
    if let Err(err) = window.append_items(items) {
        tracing::error!("menubar: appending Window menu items failed: {err}");
    }

    let menu = Menu::new();
    for submenu in [&app, &file, &edit, &window] {
        if let Err(err) = menu.append(submenu) {
            tracing::error!("menubar: appending submenu failed: {err}");
        }
    }
    (
        menu,
        MenuHandles {
            file,
            recent,
            separator,
            quit,
            settings,
            open_project,
            close_project,
            new_query,
            save_query,
            cycle_window,
            undo,
            redo,
            cut,
            copy,
            paste,
            select_all,
            check_updates,
            closable: false,
            gate: Gate::default(),
            recents: Vec::new(),
            chords,
            suspended: false,
        },
    )
}

/// The launch menu handler: exhaustive dispatch over [`MenuCmd`], plus the recents, whose
/// ids carry a path rather than naming a fixed command.
///
/// Quit and Close Project route through the close veto (red-button semantics — the T2
/// confirm decides while a query runs), the first over every window and the second over the
/// focused one. A recent opens straight from here, since only this handler knows *which*
/// one was picked. Everything else synthesizes its command's *live* effective chord into the
/// focused window's keyboard pipeline, so the focused window (or element) and its bindings
/// decide — the same path as typed keys.
pub fn handle_menu_event(event: MenuEvent, mut ctx: RendererContext, app: AppCtx) {
    if let Some(path) = event.id().0.strip_prefix(RECENT_ID_PREFIX) {
        open_recent(&mut ctx, app, path);
        return;
    }
    if let Some(cmd) = MenuCmd::parse(event.id()) {
        dispatch(cmd, &mut ctx, app);
    }
}

/// **Carry one menu item out.** The action half of [`handle_menu_event`], split from the muda id
/// parsing around it so the in-app menubar can reach it too — the two surfaces present the same
/// items and must *do* the same things, and a second copy of this match is how they would stop.
///
/// Quit and Close Project route through the close veto (red-button semantics — the T2 confirm
/// decides while a query runs), the first over every window and the second over the focused one.
/// Everything else synthesizes its command's *live* effective chord into the focused window's
/// keyboard pipeline, so the focused window (or element) and its bindings decide — the same path as
/// typed keys.
pub fn dispatch(cmd: MenuCmd, ctx: &mut RendererContext, app: AppCtx) {
    match cmd {
        MenuCmd::Quit => platform::quit_windows(ctx),
        MenuCmd::CloseProject => ctx.request_close_window(None),
        MenuCmd::CheckUpdates => {
            let mut asked = app.update_request;
            asked.set(true);
        }
        cmd => {
            let Some(command) = cmd.key_command() else {
                return;
            };
            let Some(chord) = effective_chord(&app.config.peek().settings, command) else {
                return;
            };
            let Some((key, modifiers)) = synthetic_key(&chord) else {
                return;
            };
            ctx.send_key_press(None, key, Code::Unidentified, modifiers);
        }
    }
}

/// [`dispatch`], reached from a component rather than from the renderer.
///
/// The in-app menubar is drawn *in* a window, so a press arrives with a `Platform` and no
/// [`RendererContext`] — and every action here needs one, because they all act on the window map or
/// the keyboard pipeline. `post_callback` is the hop, exactly as `platform::quit` makes it for ⌘Q.
pub fn dispatch_from_window(cmd: MenuCmd, app: AppCtx) {
    drop(Platform::get().post_callback(move |_, ctx| dispatch(cmd, ctx, app)));
}

/// [`open_recent`], reached from a component — the same renderer hop as
/// [`dispatch_from_window`], for the one item that carries a path rather than a command.
pub fn open_recent_from_window(path: String, app: AppCtx) {
    drop(Platform::get().post_callback(move |_, ctx| open_recent(ctx, app.clone(), &path)));
}

/// Open a recent from the menubar — the one File item that carries data rather than
/// synthesizing a chord, so it can't reach the focused window through the keyboard pipeline
/// the way Open… does.
///
/// When a **project** window is focused it parked its open path in [`AppCtx::open`], and the
/// recent goes through that: same rules as ⌘O and the header switcher, which means
/// `OpenPref` — this window, a new one, or the prompt. The decision is
/// [`OpenCtx::decide`]'s, but carrying it out is ours: this runs on the renderer, which has
/// no `Platform`, so the two window outcomes are done directly against the window map (which
/// is exactly what `open_project` would do with one).
///
/// With the **launcher** focused there is no open path — a recent simply opens a window, and
/// the launcher stands down behind it, as pressing one of its own rows would.
fn open_recent(ctx: &mut RendererContext, app: AppCtx, path: &str) {
    let Some(root) = platform::resolve_recent(app.config, path) else {
        return;
    };
    let launcher = app.windows.peek().launcher();
    let focused = *app.open.peek();
    let target = match focused {
        Some(open) => open.decide(&app, root),
        None => match app.windows.peek().project(&root.to_string_lossy()) {
            Some(id) => OpenTarget::Focus(id),
            None => OpenTarget::NewWindow(root),
        },
    };
    let decided = !matches!(target, OpenTarget::Ask(_));
    match target {
        OpenTarget::Nothing => {}
        OpenTarget::Focus(id) => {
            if let Some(window) = ctx.windows.get_mut(&id) {
                window.window().focus_window();
            }
        }
        OpenTarget::NewWindow(root) => {
            let geometry = window_geometry_blocking(root.clone());
            ctx.launch_window(ProjectApp::window(app, root, geometry));
        }
        OpenTarget::ThisWindow(root) => {
            if let Some(open) = focused {
                open.reroot(&app, root);
            }
        }
        OpenTarget::Ask(root) => {
            if let Some(open) = focused {
                open.ask(root);
            }
        }
    }
    if let Some(id) = launcher.filter(|_| decided) {
        ctx.request_close_window(Some(id));
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// **Every command is in exactly one menu.** The guard on [`MENUS`], which is the structure
    /// two surfaces present: muda's menubar on macOS and `components::menu_bar` everywhere else.
    ///
    /// The realistic drift is a new [`MenuCmd`] variant wired into `app_menu` and forgotten here —
    /// the command would then exist on macOS and simply not on Linux, silently, because nothing
    /// else compares the two. Listing one twice is the other direction and just as wrong: a
    /// duplicate row is a second way to do one thing, and only one of them would carry the gate.
    #[test]
    fn every_command_is_in_exactly_one_menu() {
        let listed: Vec<MenuCmd> = MENUS
            .iter()
            .flat_map(|section| section.items)
            .filter_map(|item| match item {
                Item::Cmd(cmd) => Some(*cmd),
                Item::Separator | Item::Recent => None,
            })
            .collect();

        for cmd in MenuCmd::ALL {
            let times = listed.iter().filter(|listed| **listed == cmd).count();
            assert_eq!(times, 1, "{cmd:?} appears in MENUS {times} times, want 1");
        }
        assert_eq!(
            listed.len(),
            MenuCmd::ALL.len(),
            "MENUS lists a command that is not in MenuCmd::ALL"
        );
    }

    /// **Open Recent is in the File menu, once.** It is the one entry with no [`MenuCmd`] behind
    /// it, so the test above cannot see it, and both surfaces special-case it — muda as a submenu,
    /// the in-app bar as a labelled section. Two of them, or none, would be a real defect in a
    /// place neither surface would complain about.
    #[test]
    fn open_recent_sits_in_the_file_menu_once() {
        let recents: Vec<&str> = MENUS
            .iter()
            .flat_map(|section| section.items.iter().map(move |item| (section.title, item)))
            .filter(|(_, item)| matches!(item, Item::Recent))
            .map(|(title, _)| title)
            .collect();
        assert_eq!(recents, vec!["File"]);
    }

    #[test]
    fn menu_cmd_ids_round_trip() {
        for cmd in MenuCmd::ALL {
            assert_eq!(MenuCmd::parse(&MenuId::from(cmd)), Some(cmd));
        }
        assert_eq!(MenuCmd::parse(&MenuId::new("not.ours")), None);
    }

    #[test]
    fn every_dispatching_item_has_a_command_and_the_window_ones_do_not() {
        assert_eq!(MenuCmd::Quit.key_command(), None);
        assert_eq!(MenuCmd::CloseProject.key_command(), None);
        assert_eq!(MenuCmd::CheckUpdates.key_command(), None);
        for (cmd, command) in [
            (MenuCmd::OpenProject, Command::OpenProject),
            (MenuCmd::OpenSettings, Command::OpenSettings),
            (MenuCmd::NewQuery, Command::NewTab),
            (MenuCmd::SaveQuery, Command::SaveQuery),
            (MenuCmd::CycleWindow, Command::CycleWindow),
        ] {
            assert_eq!(cmd.key_command(), Some(command), "{cmd:?}");
            assert!(!command.is_edit(), "{cmd:?}");
        }
        for cmd in MenuCmd::ALL.into_iter().filter(|cmd| {
            !matches!(
                cmd,
                MenuCmd::Quit
                    | MenuCmd::CloseProject
                    | MenuCmd::CheckUpdates
                    | MenuCmd::OpenProject
                    | MenuCmd::OpenSettings
                    | MenuCmd::NewQuery
                    | MenuCmd::SaveQuery
                    | MenuCmd::CycleWindow
            )
        }) {
            assert!(cmd.key_command().unwrap().is_edit(), "{cmd:?}");
        }
    }

    #[test]
    fn default_chords_map_to_accelerators() {
        let chords = menu_chords(&Settings::default());
        for (name, chord) in [
            ("quit", &chords.quit),
            ("open_settings", &chords.open_settings),
            ("open_project", &chords.open_project),
            ("close_project", &chords.close_project),
            ("new_query", &chords.new_query),
            ("save_query", &chords.save_query),
            ("cycle_window", &chords.cycle_window),
            ("undo", &chords.undo),
            ("redo", &chords.redo),
            ("cut", &chords.cut),
            ("copy", &chords.copy),
            ("paste", &chords.paste),
            ("select_all", &chords.select_all),
        ] {
            let chord = chord.as_ref().unwrap_or_else(|| panic!("{name} unbound"));
            assert!(accelerator(chord).is_some(), "{name}");
            assert!(synthetic_key(chord).is_some(), "{name}");
        }
    }

    /// The gate is what keeps an item from looking live in a window that has no listener for
    /// it — the failure this task was opened on. Each flag is asserted for what it *is*, not
    /// for where a window falls on a scale, which is the whole reason [`Gate`] is four flags.
    #[test]
    fn a_panel_can_reach_none_of_the_scoped_commands() {
        let windows = Windows::default();
        let launcher = MenuScope::Launcher(State::create_global(None));
        assert_eq!(
            launcher.gate(&windows),
            Gate {
                workspace: true,
                project: false,
                workbench: false,
                cyclable: false,
                // Whatever this build is: a test binary is never a bundle, so the real answer here
                // is `false`, and asserting it as read keeps this test about the *scope* rather
                // than about how it was run.
                installed: installed(),
            }
        );
        assert_eq!(MenuScope::Panel.gate(&windows), Gate::default());
        assert!(MenuScope::Panel.open().is_none());
        assert!(launcher.open().is_none());
        assert!(MenuScope::Panel.update_ask().is_none());
        assert!(launcher.update_ask().is_some());
    }

    /// `MenuScope::Project` needs an `OpenCtx` and so a live window, which a unit test has no
    /// way to build — and reshaping `gate` to take a bool so one could would be shaping a
    /// production signature to be testable. What the arm actually turns on is
    /// two facts that are reachable here, so they are asserted directly: a project window is
    /// always a workspace window with a project, and the two that move are read live.
    #[test]
    fn a_project_window_gates_the_workbench_items_on_its_subtree() {
        let faulted = Gate {
            workspace: true,
            project: true,
            workbench: false,
            cyclable: false,
            installed: false,
        };
        assert_ne!(faulted, Gate::default());
        assert!(faulted.workspace && faulted.project && !faulted.workbench);
    }

    /// **Check for Updates… is gated on the build, and both menus read that from one place.**
    ///
    /// The regression this pins: the muda menubar applied `install_site().bundle().is_some()`
    /// inline while [`Gate::allows`] said the item was always live, so a `cargo run` build greyed
    /// it on macOS and offered it in the drawn menu — an item that can find an update and then
    /// has nowhere to put it.
    #[test]
    fn check_for_updates_needs_a_build_it_can_install_into() {
        let workspace = Gate {
            workspace: true,
            installed: true,
            ..Gate::default()
        };
        assert!(workspace.allows(MenuCmd::CheckUpdates));

        // A dev build: a real window, nowhere to install.
        let unbundled = Gate {
            installed: false,
            ..workspace
        };
        assert!(!unbundled.allows(MenuCmd::CheckUpdates));

        // A panel in an installed build: somewhere to install, no window to raise the answer in.
        let panel = Gate {
            workspace: false,
            ..workspace
        };
        assert!(!panel.allows(MenuCmd::CheckUpdates));

        // Quit is the one that really is always live, and must not have been caught by this.
        assert!(Gate::default().allows(MenuCmd::Quit));
    }

    #[test]
    fn synthetic_keys_mirror_the_chord() {
        let (key, modifiers) = synthetic_key(&KeyChord {
            primary: true,
            shift: true,
            alt: false,
            key: "z".to_string(),
        })
        .unwrap();
        assert_eq!(key, Key::Character("z".into()));
        assert!(modifiers.contains(Modifiers::ctrl_or_meta()));
        assert!(modifiers.contains(Modifiers::SHIFT));
        assert!(!modifiers.contains(Modifiers::ALT));

        let (key, _) = synthetic_key(&KeyChord {
            primary: true,
            shift: false,
            alt: false,
            key: "Enter".to_string(),
        })
        .unwrap();
        assert_eq!(key, Key::Named(NamedKey::Enter));

        assert!(synthetic_key(&KeyChord {
            primary: true,
            shift: false,
            alt: false,
            key: "NoSuchKey".to_string(),
        })
        .is_none());
    }

    #[test]
    fn named_and_symbol_keys_map() {
        for key in ["Enter", ",", "`", "t"] {
            let chord = KeyChord {
                primary: true,
                shift: false,
                alt: false,
                key: key.to_string(),
            };
            assert!(accelerator(&chord).is_some(), "{key}");
        }
        let chord = KeyChord {
            primary: true,
            shift: false,
            alt: false,
            key: "NoSuchKey".to_string(),
        };
        assert!(accelerator(&chord).is_none());
    }
}
