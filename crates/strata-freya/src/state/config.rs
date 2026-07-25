//! The **app-global config store**: one reactive [`AppConfig`] for the whole process —
//! the user [`Settings`], the recent-projects list, and the set of projects with a window
//! open right now.
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
/// The split that matters is `Settings` vs. the project lists: `use_open_project` writes
/// `Recents`/`Open` on every window mount and close (and the launcher will, on every pin),
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
/// [`use_open_project`] as they open, which also means a crash leaves a usable restore
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

/// Tie a window's open project to the app-global config for as long as the window lives:
/// on mount the project is promoted to the top of the recents and joins the open-set; when
/// the window closes it leaves the open-set. The *recent* stays — outliving the window is
/// the whole point of a recent.
///
/// `root` is the project folder ([`RecentProject::path`](strata_core::config::RecentProject::path)),
/// already canonicalized by whoever opened the window.
///
/// Quitting closes windows too, so this drop runs then as well — and must **not** remove
/// anything, or the persisted open-set would end up empty and "Reopen projects on startup"
/// would restore nothing. [`is_quitting`] is what tells the two apart, and that difference
/// is the whole feature: quitting with three projects open reopens those three, while
/// closing all three by hand means "start me at the launcher".
pub fn use_open_project(station: ConfigStation, name: &str, root: &Path) {
    let path = root.to_string_lossy().into_owned();
    let name = name.to_string();
    use_hook({
        let path = path.clone();
        move || {
            write_config(station, &[ConfigChan::Recents, ConfigChan::Open], |cfg| {
                cfg.push_recent(&name, &path);
                cfg.add_open(&path);
            });
        }
    });
    use_drop(move || {
        if is_quitting() {
            return;
        }
        write_config(station, &[ConfigChan::Open], |cfg| cfg.remove_open(&path));
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
/// The write is synchronous and immediate rather than debounced like the session
/// autosave: config changes are discrete user events (open a project, pin a recent, flip a
/// setting), not a keystroke stream, so there is no burst to coalesce and nothing in
/// flight to lose on a crash. A control that streams values (a drag-slider) would need to
/// commit on settle rather than per frame.
pub fn write_config(
    mut station: ConfigStation,
    channels: &[ConfigChan],
    edit: impl FnOnce(&mut AppConfig),
) {
    // A `RadioStation` is only mutable *through* a channel guard, so an empty `channels`
    // has no write path of its own — and running the edit inside the per-channel loop ran
    // it zero times there, then persisted the untouched config: a silently dropped edit.
    // Every field belongs to an audience, so naming none is a caller bug; take the
    // conservative reading (all of them, so nothing can be left stale) and say so.
    let channels = if channels.is_empty() {
        tracing::error!("write_config: no audience named; notifying all of them");
        &ALL_AUDIENCES[..]
    } else {
        channels
    };
    let mut channels = channels.iter();
    // The edit runs exactly once, under the first channel's guard — whose drop is that
    // channel's notification. Non-empty by construction, so it always runs.
    if let Some(first) = channels.next() {
        edit(&mut station.write_channel(*first));
    }
    for chan in channels {
        // The remaining audiences: an empty write, taken and dropped one at a time (each
        // guard holds the value borrow until it notifies).
        drop(station.write_channel(*chan));
    }
    config::save(&station.peek());
}
