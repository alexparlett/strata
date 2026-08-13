//! The **app-global config store**: one reactive [`AppConfig`] for the whole process —
//! the user [`Settings`](strata_core::config::Settings), the recent-projects list, and the
//! set of projects with a window open right now.
//!
//! **Why global, not per-window.** Every field here is written by one window and read by
//! the others. Opening a project in window A must appear in the launcher's recents and in
//! window B's project picker; `open_projects` is by definition the *set of windows*, which
//! no single window knows; a settings change must repaint every window at once. A
//! per-window store would give each window its own divergent copy of a machine-global
//! fact.
//!
//! **Why one store and not three globals.** `AppConfig` is what the file on disk holds, so
//! one struct means one load, one write, and no field can be clobbered by a partial save
//! (the old app's read-modify-write dance existed only to avoid exactly that). Granularity
//! comes from [`ConfigChan`] instead: a project open wakes the recents readers, not the
//! theme.
//!
//! **Disk is a startup input, never a live source.** [`create_global_config`] reads the
//! file once in `main`; after that the store is the truth and every mutation goes through
//! [`write_config`], which persists. Nothing re-reads the file to answer a question.

use std::path::Path;

use freya::prelude::{use_drop, use_hook, State};
use freya::radio::{
    use_radio, use_radio_station, use_share_radio, Radio, RadioAntenna, RadioChannel, RadioStation,
};
use strata_core::config::{self, AppConfig};

use crate::platform::is_quitting;

/// The audiences of the app-global config — a write on one wakes only its subscribers.
///
/// Deliberately three, not one per field: these are the *surfaces* that care. Recents and
/// the open-set are split because the window switcher only tracks what's open, while the
/// launcher's list is mostly about recents.
///
/// The split that matters is `Settings` vs. the project lists: [`use_claim_open`] and
/// [`use_promote_recent`] write `Open`/`Recents` on every window mount and close (and the
/// launcher will, on every pin),
/// and waking the `Settings` subscribers for that would sweep every mounted `use_hint` —
/// one per menu row and tooltip. Going *finer* inside `Settings` (Theme / Keymap / Grid)
/// would only optimize Settings-window toggles: human-speed, once-per-click, and the theme
/// derivation already no-op-guards on `Theme.name`. A control that streams values is the
/// case to revisit — and it wants commit-on-settle (see [`write_config`]) more than it
/// wants another channel.
///
/// Note channels only govern *notification*. Reads are a `ReadRef` borrow of the whole
/// `AppConfig`, never a copy, so reaching a field through `.settings` costs nothing.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ConfigChan {
    /// Any field of [`Settings`](strata_core::config::Settings) — theme, keybinds, prefs.
    Settings,
    /// The recent-projects list (order, pins, removals).
    Recents,
    /// The set of projects with an open window.
    Open,
}

impl RadioChannel<AppConfig> for ConfigChan {}

/// The app-global config store, created in `main` and handed to every window root.
pub type ConfigStation = RadioStation<AppConfig, ConfigChan>;

/// A subscribing handle on one [`ConfigChan`] of the app-global config.
pub type ConfigRadio = Radio<AppConfig, ConfigChan>;

/// Load the persisted config into the one app-global store, and take the persisted
/// open-set out of it. Call **once**, in `main`, before `launch` — this is not a hook.
///
/// `RadioStation::create_global` is the multi-window sharing the 0.4 release post shows for
/// `State::create_global`, not a separate mechanism — the station *is* two
/// `State::create_global`s (value + listener map), so it shares across windows for the same
/// reason: `UnsyncStorage` + a leaked owner, reachable from every window because they all
/// run on the one renderer thread (`WinitRenderer` keeps them in a single map). The fork's
/// `examples/feature_multi_window_state.rs` is the worked example. It goes undocumented on
/// the website, which is the only reason this looks exotic.
///
/// The returned paths are the projects that had a window when the app last exited — the
/// "Reopen projects on startup" list. They are *taken*, not left in the store, because as
/// **live** state they are stale by definition: no window exists yet in this process, so
/// nothing here could ever remove them and they would accumulate forever, telling the
/// launcher a project is open when it isn't. Windows re-add themselves through
/// [`use_claim_open`] as they open, which also means a crash leaves a usable restore
/// list rather than a growing one.
pub fn create_global_config() -> (ConfigStation, Vec<String>) {
    let mut cfg = config::load();
    let reopen = std::mem::take(&mut cfg.open_projects);
    (ConfigStation::create_global(cfg), reopen)
}

/// Share the app-global store with this window's tree, so components reach it with
/// [`use_config`] / [`use_config_station`] instead of prop-threading. Call once per window
/// root. The station is `Copy` — this shares the one global, it doesn't fork it.
pub fn use_share_config(station: ConfigStation) {
    use_share_radio(move || station);
}

/// Subscribe this component to one channel of the app-global config. Requires
/// [`use_share_config`] in this window root (or an ancestor).
pub fn use_config(chan: ConfigChan) -> ConfigRadio {
    use_radio(chan)
}

/// The app-global config store from context — for consumers that only ever [`peek`] it
/// (event handlers resolving a key chord, the close guard), which must not subscribe.
///
/// [`peek`]: RadioStation::peek
pub fn use_config_station() -> ConfigStation {
    use_radio_station()
}

/// A subscribing handle on `chan`, built straight from `station` rather than looked up in
/// context — for hooks that are *handed* the station (the window root's theme derivation),
/// so they don't silently depend on [`use_share_config`] having run first.
pub fn use_config_channel(station: ConfigStation, chan: ConfigChan) -> ConfigRadio {
    use_hook(move || Radio::new(State::create(RadioAntenna::new(chan, station))))
}

/// **The window's claim on a project**: it joins the persisted open-set on mount and leaves it
/// on close — but not on quit.
///
/// Called by `ProjectRoot`, so every arm of the project subtree claims alike — a window loading a
/// project, showing one, or reporting that it could not be loaded is in every case *a window on
/// that project*, which is what makes a quit reopen it and resurface a fault honestly. Deliberately
/// **not** paired with the recents promotion below, which a project has to earn by loading.
///
/// The add half is load-bearing rather than symmetry: an unpaired remove-on-drop loses the entry on
/// every remount of the subtree, and the quit after that would silently forget the window.
///
/// Quitting closes windows too, so this drop runs then as well — and must **not** remove anything,
/// or the persisted open-set would end up empty. [`is_quitting`] tells the two apart, and that
/// difference is the whole feature: quitting with three projects open reopens those three, while
/// closing all three by hand means "start me at the launcher".
pub fn use_claim_open(station: ConfigStation, root: &Path) {
    let path = root.to_string_lossy().into_owned();
    use_hook({
        let path = path.clone();
        move || {
            write_config(station, &[ConfigChan::Open], |cfg| cfg.add_open(&path));
        }
    });
    use_drop(move || {
        if is_quitting() {
            return;
        }
        write_config(station, &[ConfigChan::Open], |cfg| cfg.remove_open(&path));
    });
}

/// **A project earns its place in the recents by opening**: head the list with `root` once, on
/// mount. Called by `ProjectLoaded` — the arm that exists only because the load succeeded — so
/// neither a project still loading nor one that faulted can head a list of places worth
/// returning to.
///
/// Mount-only, with no drop half: outliving the window is the whole point of a recent.
///
/// `name` is the project's own name from the defs it just loaded, which is the other reason this
/// half waits for the load — before it there is nothing to call the entry.
pub fn use_promote_recent(station: ConfigStation, name: &str, root: &Path) {
    let path = root.to_string_lossy().into_owned();
    let name = name.to_string();
    use_hook(move || {
        write_config(station, &[ConfigChan::Recents], |cfg| {
            cfg.push_recent(&name, &path);
        });
    });
}

/// Every audience — the fallback [`write_config`] uses when a caller names none. Extend it
/// when a [`ConfigChan`] is added.
const ALL_AUDIENCES: [ConfigChan; 3] =
    [ConfigChan::Settings, ConfigChan::Recents, ConfigChan::Open];

/// Mutate the app-global config and persist it — **the** write path; nothing else calls
/// [`config::save`].
///
/// `channels` are the audiences the edit touches (opening a project touches both
/// [`ConfigChan::Recents`] and [`ConfigChan::Open`]): the edit runs once, each listed
/// channel is notified, and the file is written once, after the UI has been woken.
///
/// The write is synchronous and immediate rather than debounced like the session autosave: config
/// changes are discrete user events, not a keystroke stream, so there is no burst to coalesce. A
/// control that streams values would need to commit on settle rather than per frame.
///
/// Returns whether the edit reached disk. The in-memory store is updated and every listed channel
/// notified **either way**, so the `Err` is about the durable copy alone: the setting holds for
/// this run and reverts at the next launch.
///
/// Callers that represent a **deliberate commit** report it where the user is looking
/// (`SettingsCtx::apply`); the bookkeeping writes deliberately do not, because the user did not ask
/// for them and nine call sites announcing the same failure of the same file is the stacked
/// near-duplicate AGENTS.md §3 rules out. Making a bookkeeping failure visible wants **one standing
/// condition**, which is not built.
pub fn write_config(
    mut station: ConfigStation,
    channels: &[ConfigChan],
    edit: impl FnOnce(&mut AppConfig),
) -> bool {
    let channels = if channels.is_empty() {
        tracing::error!("write_config: no audience named; notifying all of them");
        &ALL_AUDIENCES[..]
    } else {
        channels
    };
    let mut channels = channels.iter();
    if let Some(first) = channels.next() {
        edit(&mut station.write_channel(*first));
    }
    for chan in channels {
        drop(station.write_channel(*chan));
    }
    match config::save(&station.peek()) {
        Ok(()) => true,
        Err(e) => {
            tracing::error!("{e}");
            false
        }
    }
}
