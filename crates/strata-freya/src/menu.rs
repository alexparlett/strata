//! The application menubar (macOS): the App menu — About · Hide/Show · Quit — a File menu
//! (Close Project), and a standard Edit menu — Undo · Redo · Cut · Copy · Paste · Select
//! All — built with muda through the fork's `menu` feature.
//!
//! **Quit and Close Project are different things, and both route through the close veto**,
//! never Cocoa's `terminate:` (muda's `PredefinedMenuItem::quit()` sends exactly that, the
//! thing that bypassed the T2 confirm). Quit asks *every* window to close and marks the app
//! as quitting, so the open projects stay in the persisted open-set and the next launch
//! reopens them; Close Project asks only the focused window, which drops it from that set
//! and puts the launcher up if it was the last — the same thing its red button does.
//!
//! **The Edit menu is custom items too**, not muda's predefined set: the predefined
//! items send Cocoa first-responder selectors (`undo:` / `copy:` / …) that a Skia view
//! never receives — the exact swallowing tangle the Dioxus app fought (DEV_TASKS F8).
//! Instead each item's event **synthesizes the command's effective chord into the
//! focused window's keyboard pipeline** ([`NativeEventExt::send_key_press`]), so menu
//! clicks and accelerator presses flow through the exact same path as typed keys — the
//! focused element (SQL editor, find input, …) and its `EditBindings` decide.
//! First-responder semantics, without Cocoa.
//!
//! Accelerators derive from the keymap (`effective_chord`), keeping it the single
//! source of truth; the OS handles an accelerator before the window sees the key, so
//! the corresponding in-window listener simply never fires while the menu carries it —
//! same command either way. They are resolved at launch and then **kept in step with the
//! settings** (P4-08): [`MenuHandles::sync_chords`] re-applies every one off
//! `ConfigChan::Settings`, so a rebind reaches the menubar as it reaches every tooltip. It has
//! to, and for a sharper reason than tidiness — a stale accelerator is not merely wrong text,
//! it is the OS *consuming* the old chord before the window can see it, so the item would keep
//! firing on a shortcut the user rebound away, and the new one would do nothing for the items
//! whose command is only reachable through the pipeline.
//!
//! **Deliberately not ported from the Dioxus app**: its `global-hotkey` OS-hotkey layer
//! (`strata-dioxus` `use_shortcuts`) and its `PredefinedMenuItem` Edit set. Both were
//! webview workarounds — OS hotkeys fired before wry swallowed the keys, and predefined
//! items worked only because WKWebView answers Cocoa's first-responder selectors. With
//! native winit delivery every key reaches the keymap's listeners directly (resolved
//! live, per focused window), so the hotkey manager, its focus-gated registration, and
//! its chord→`Code` table have no job here.

use freya::menu::accelerator::Accelerator;
use freya::menu::{
    AboutMetadata, IsMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu,
};
use freya::prelude::{
    use_drop, use_hook, use_side_effect, Code, Key, Modifiers, ModifiersExt, NamedKey,
    NativeEventExt, Platform, RendererContext, State, WritableUtils,
};
use strata_core::config::{Command, KeyChord, RecentProject, Settings};
use strata_core::keymap::effective_chord;

use crate::apps::project::ProjectApp;
use crate::platform::{self, OpenCtx, OpenTarget};
use crate::state::{use_config, AppCtx, ConfigChan};

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
    /// Pick a project folder and open it.
    OpenProject,
    /// Close the focused window (and open the launcher if it was the last).
    CloseProject,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
}

impl MenuCmd {
    const ALL: [Self; 10] = [
        Self::Quit,
        Self::OpenSettings,
        Self::OpenProject,
        Self::CloseProject,
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
            Self::OpenProject => "strata.file.open-project",
            Self::CloseProject => "strata.file.close-project",
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
    /// and **Settings…**, which pins itself above whichever window asked. `None` for the
    /// window-lifecycle items, which the handler routes through the close path instead.
    fn key_command(self) -> Option<Command> {
        match self {
            Self::Quit | Self::CloseProject => None,
            Self::OpenSettings => Some(Command::OpenSettings),
            Self::OpenProject => Some(Command::OpenProject),
            Self::Undo => Some(Command::Undo),
            Self::Redo => Some(Command::Redo),
            Self::Cut => Some(Command::Cut),
            Self::Copy => Some(Command::Copy),
            Self::Paste => Some(Command::Paste),
            Self::SelectAll => Some(Command::SelectAll),
        }
    }
}

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
        (Some(_), None) => Key::Character(chord.key.clone().into()),
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

/// The mutable half of the menubar: the File menu's parts and every item that carries an
/// accelerator, kept after construction so the menu can follow the app.
///
/// Three things make it dynamic. **Open Recent** mirrors the config store's recents, which
/// move on every open, pin and remove. **Close Project** applies only to a window that has
/// one, so it (and the separator above it) is pulled from the menu while the launcher is
/// the focused window rather than sitting there greyed — a menubar is app-global, but this
/// half of it is about the focused window. And the **accelerators** follow the keymap, which
/// Settings ▸ Keymap can rebind at any moment ([`sync_chords`](Self::sync_chords)).
pub struct MenuHandles {
    file: Submenu,
    recent: Submenu,
    separator: PredefinedMenuItem,
    /// Every item that carries an accelerator, in the order [`MenuChords`] names them.
    quit: MenuItem,
    settings: MenuItem,
    open_project: MenuItem,
    close_project: MenuItem,
    undo: MenuItem,
    redo: MenuItem,
    cut: MenuItem,
    copy: MenuItem,
    paste: MenuItem,
    select_all: MenuItem,
    /// Whether the separator + Close Project pair is currently in the File menu.
    closable: bool,
    /// The recents the submenu currently renders, so an unchanged sync rebuilds nothing.
    recents: Vec<RecentProject>,
    /// The chords the settings resolve to, for the same reason.
    chords: MenuChords,
    /// Whether the items are carrying **no** accelerator while a chord is being captured — see
    /// [`suspend_accelerators`](Self::suspend_accelerators).
    suspended: bool,
}

impl MenuHandles {
    /// Point the File menu at the focused window: its view of the recents, and whether it
    /// has a project to close. Cheap and idempotent — each half no-ops when already right,
    /// so the focused window can call it on every recents change.
    pub fn sync(&mut self, recents: &[RecentProject], closable: bool) {
        self.sync_recents(recents);
        self.sync_closable(closable);
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
        self.apply_chords();
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
        self.apply_chords();
    }

    /// Push the chords the menubar should be carrying onto the items — the settings' own, or
    /// nothing at all while a capture is in progress.
    ///
    /// The chords are **destructured** rather than read field by field, for the reason
    /// `settings_merge!` is a macro: the pattern names every field, so a command that grows a
    /// menu item and forgets this list is a build error rather than an accelerator that silently
    /// never updates.
    fn apply_chords(&self) {
        let none = MenuChords::default();
        let MenuChords {
            quit,
            open_settings,
            open_project,
            close_project,
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
        let set = |item: &MenuItem, chord: &Option<KeyChord>, gated: bool| {
            if let Err(err) = item.set_accelerator(chord.as_ref().and_then(accelerator)) {
                tracing::error!("menubar: updating an accelerator failed: {err}");
            }
            if gated {
                item.set_enabled(chord.is_some());
            }
        };
        set(&self.quit, quit, false);
        set(&self.settings, open_settings, true);
        set(&self.open_project, open_project, true);
        set(&self.close_project, close_project, false);
        set(&self.undo, undo, true);
        set(&self.redo, redo, true);
        set(&self.cut, cut, true);
        set(&self.copy, copy, true);
        set(&self.paste, paste, true);
        set(&self.select_all, select_all, true);
    }

    /// Rebuild **Open Recent** when the list has actually moved. muda has no way to edit an
    /// item's label in place cheaply, so a change rebuilds the submenu wholesale; the
    /// equality guard is what keeps that off the hot path.
    fn sync_recents(&mut self, recents: &[RecentProject]) {
        let same = self.recents.len() == recents.len()
            && self
                .recents
                .iter()
                .zip(recents)
                .all(|(a, b)| a.path == b.path && a.name == b.name);
        if same {
            return;
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
        // An empty list would be a submenu you can open onto nothing; disabling it says
        // "there are none" the way every other macOS Open Recent does.
        self.recent.set_enabled(!recents.is_empty());
        self.recents = recents.to_vec();
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

/// Keep the menubar pointed at this window for as long as it is focused: the File menu's
/// recents, whether it has a project to close, where Open Recent opens — and every item's
/// accelerator, against the live keymap. Call once in a window root, passing this window's
/// [`OpenCtx`] — `Some` for a project window, `None` for the launcher.
///
/// One parameter for both File halves because they are the same fact: a window with a project
/// has something to close **and** somewhere to open into, and a window without has neither.
///
/// Focus is the gate because the menubar is app-global but its File half is about *one*
/// window — so exactly one window drives it at a time, and a window that isn't focused
/// never fights the one that is. (Freya's `Platform` is per-window, so despite its name
/// `is_app_focused` is this window's focus.)
///
/// The accelerators ride the same gate for a different reason: they are not about this window
/// at all, but a `State` change only wakes a window's scope, so somebody has to be the one to
/// notice — and "the focused window" is a rule already in force here. The Settings window is
/// itself focused while Apply is pressed, so the rebind lands from there; and if it closes in
/// the same breath, the window that takes focus re-runs this effect and syncs anyway.
pub fn use_file_menu(app: &AppCtx, open: Option<OpenCtx>) {
    let focused = use_hook(Platform::get).is_app_focused;
    let config = use_config(ConfigChan::Recents);
    let settings = use_config(ConfigChan::Settings);
    let mut menu = app.menu;
    let mut focused_open = app.open;
    use_side_effect(move || {
        if !*focused.read() {
            return;
        }
        // `set_if_modified`, not `set`: this effect also rides `ConfigChan::Recents`, so it
        // re-runs on every project open, close and pin — and re-parking an identical handle
        // would notify the slot's audience for a value that never changed.
        focused_open.set_if_modified(open);
        let recents = config.read();
        let chords = menu_chords(&settings.read().settings);
        if let Some(handles) = menu.write().as_mut() {
            handles.sync(&recents.recent_projects, open.is_some());
            handles.sync_chords(&chords);
        }
    });
    // A window that goes must not leave its open path parked: those are its own `State`s, and
    // a menubar press that reached a dead one would panic rather than open anything. Only
    // ours is cleared — by the time this runs, another window may already have parked its
    // own. (Comparing first, into a value: a `peek()` temporary held across the `set` is a
    // borrow panic on the same slot. `State`'s equality is handle identity, so this never
    // reads the dead window's value.)
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
        undo: chord(Command::Undo),
        redo: chord(Command::Redo),
        cut: chord(Command::Cut),
        copy: chord(Command::Copy),
        paste: chord(Command::Paste),
        select_all: chord(Command::SelectAll),
    }
}

/// Build the menubar: the App menu, the File menu, then the standard Edit menu.
///
/// Returns the [`MenuHandles`] the app keeps, because the File menu is **not** static: its
/// recents follow the config store, and Close Project belongs only to a window that has a
/// project. Freya calls this from `resumed`, on the thread that called `launch` — the same
/// (main) thread the UI runs on and the only one muda allows menu work on — so the handles
/// can be mutated straight from a window's effect, with no renderer hop.
pub fn app_menu(chords: MenuChords) -> (Menu, MenuHandles) {
    let quit = MenuItem::with_id(
        MenuCmd::Quit,
        "Quit Strata",
        true,
        chords.quit.as_ref().and_then(accelerator),
    );
    // Settings…, in the App menu where macOS puts it. Like the Edit set it rides the
    // keyboard pipeline (an unbound command has no chord to dispatch, so its item ships
    // disabled), which is also how the window that asked is identified: the synthesized press
    // lands in the *focused* window, whose listener opens Settings pinned above itself.
    let settings = MenuItem::with_id(
        MenuCmd::OpenSettings,
        "Settings…",
        chords.open_settings.is_some(),
        chords.open_settings.as_ref().and_then(accelerator),
    );
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

    // File: getting *into* a project (Open… · Open Recent) and out of this one (Close
    // Project, distinct from Quit above it). The recents submenu and the closing pair are
    // filled in by `MenuHandles::sync` once a window is focused and can say what to show.
    let file = Submenu::new("File", true);
    // Open… rides the same keyboard pipeline as the Edit set, so it follows the same rule:
    // an unbound command has no chord to dispatch and its item ships disabled, rather than
    // looking live and doing nothing.
    let open_project = MenuItem::with_id(
        MenuCmd::OpenProject,
        "Open…",
        chords.open_project.is_some(),
        chords.open_project.as_ref().and_then(accelerator),
    );
    let recent = Submenu::new("Open Recent", true);
    let separator = PredefinedMenuItem::separator();
    let close_project = MenuItem::with_id(
        MenuCmd::CloseProject,
        "Close Project",
        true,
        chords.close_project.as_ref().and_then(accelerator),
    );
    let items: &[&dyn IsMenuItem] = &[&open_project, &recent];
    if let Err(err) = file.append_items(items) {
        tracing::error!("menubar: appending File menu items failed: {err}");
    }

    // An unbound command has no chord to dispatch through the keyboard pipeline, so
    // its item ships disabled — the shortcut and the menu stay one mechanism.
    let edit_item = |cmd: MenuCmd, label: &str, chord: &Option<KeyChord>| {
        MenuItem::with_id(
            cmd,
            label,
            chord.is_some(),
            chord.as_ref().and_then(accelerator),
        )
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

    let menu = Menu::new();
    for submenu in [&app, &file, &edit] {
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
            undo,
            redo,
            cut,
            copy,
            paste,
            select_all,
            closable: false,
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
    let config = app.config;
    if let Some(path) = event.id().0.strip_prefix(RECENT_ID_PREFIX) {
        open_recent(&mut ctx, app, path);
        return;
    }
    match MenuCmd::parse(event.id()) {
        Some(MenuCmd::Quit) => platform::quit_windows(&mut ctx),
        Some(MenuCmd::CloseProject) => ctx.request_close_window(None),
        Some(cmd) => {
            let Some(command) = cmd.key_command() else {
                return;
            };
            let Some(chord) = effective_chord(&config.peek().settings, command) else {
                return;
            };
            let Some((key, modifiers)) = synthetic_key(&chord) else {
                return;
            };
            ctx.send_key_press(None, key, Code::Unidentified, modifiers);
        }
        None => {}
    }
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
    // Normalise *before* the registry lookup: windows are registered under the canonical
    // root, while a recent's stored path may be a symlink or the pre-Freya `.strata` shape
    // (`migrate_paths` strips the segment but never canonicalizes). Looking up the raw path
    // would miss and open a second window on a project that already has one. A recent whose
    // folder is gone is dropped by `resolve_recent`, and the focused window's
    // `use_file_menu` rebuilds the submenu without it.
    let Some(root) = platform::resolve_recent(app.config, path) else {
        return;
    };
    let launcher = app.windows.peek().launcher();
    let focused = *app.open.peek();
    let target = match focused {
        Some(open) => open.decide(&app, root),
        // No open path: whatever window the recent needs is a new one, unless the project
        // already has one.
        None => match app.windows.peek().project(&root.to_string_lossy()) {
            Some(id) => OpenTarget::Focus(id),
            None => OpenTarget::NewWindow(root),
        },
    };
    // Only an outcome that actually puts something on screen stands the launcher down. `Ask`
    // has opened nothing yet — the question is still on screen and Cancel is one of its
    // answers — so closing the launcher there would spend it on an open that may never happen.
    let decided = !matches!(target, OpenTarget::Ask(_));
    match target {
        OpenTarget::Nothing => {}
        OpenTarget::Focus(id) => {
            if let Some(window) = ctx.windows.get_mut(&id) {
                window.window().focus_window();
            }
        }
        OpenTarget::NewWindow(root) => {
            ctx.launch_window(ProjectApp::window(app, root));
        }
        // Both of these are the focused window's own state, so they need no window handle —
        // and `focused` is `Some` by construction, since only its `decide` returns them.
        // The re-root still goes through its gate: it may raise the confirm instead, if the
        // project it would tear down has queries running.
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
    // The launcher exists only while there's nothing else to look at.
    if let Some(id) = launcher.filter(|_| decided) {
        ctx.request_close_window(Some(id));
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn menu_cmd_ids_round_trip() {
        for cmd in MenuCmd::ALL {
            assert_eq!(MenuCmd::parse(&MenuId::from(cmd)), Some(cmd));
        }
        assert_eq!(MenuCmd::parse(&MenuId::new("not.ours")), None);
    }

    #[test]
    fn every_dispatching_item_has_a_command_and_the_window_ones_do_not() {
        // The window-lifecycle items route through the close path, not the key pipeline.
        assert_eq!(MenuCmd::Quit.key_command(), None);
        assert_eq!(MenuCmd::CloseProject.key_command(), None);
        // Open… and Settings… ride the key pipeline like the Edit set, but aren't *editing*
        // commands: their listener is the focused window's, not the focused editor's.
        assert_eq!(
            MenuCmd::OpenProject.key_command(),
            Some(Command::OpenProject)
        );
        assert_eq!(
            MenuCmd::OpenSettings.key_command(),
            Some(Command::OpenSettings)
        );
        for cmd in MenuCmd::ALL.into_iter().filter(|cmd| {
            !matches!(
                cmd,
                MenuCmd::Quit
                    | MenuCmd::CloseProject
                    | MenuCmd::OpenProject
                    | MenuCmd::OpenSettings
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
            ("close_project", &chords.close_project),
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

    #[test]
    fn synthetic_keys_mirror_the_chord() {
        // ⇧⌘Z: character key, primary + shift folded into modifier flags.
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

        // Named keys go through keyboard-types' vocabulary.
        let (key, _) = synthetic_key(&KeyChord {
            primary: true,
            shift: false,
            alt: false,
            key: "Enter".to_string(),
        })
        .unwrap();
        assert_eq!(key, Key::Named(NamedKey::Enter));

        // An unmappable name degrades to "no dispatch", not a panic.
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
        // An unbindable oddball degrades to "no accelerator", not a panic.
        let chord = KeyChord {
            primary: true,
            shift: false,
            alt: false,
            key: "NoSuchKey".to_string(),
        };
        assert!(accelerator(&chord).is_none());
    }
}
