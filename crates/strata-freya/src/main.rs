//! Strata — the Freya (Skia / native) frontend. The Freya-port target; rides the
//! shared `strata-core` alongside the transitional `strata-dioxus` app. See
//! `docs/FREYA_PORT_PLAN.md` (§3 for this crate's internal layout).
//!
//! Layout grows per phase: `apps/<window>/` holds one self-contained OS window each
//! (the project window and the launcher today), `platform/` the window model that spawns
//! and focuses between them. Top-level `state/` (global singletons), `components/` (DS
//! widgets) and `theme.rs` are shared by every window.
//!
//! No Tokio runtime here on purpose: the engine facade owns a private runtime, and the
//! UI just awaits its methods (`JoinHandle`s are executor-agnostic) — see
//! `strata_core::engine` and `docs/SNAPSHOT_SPEC.md` §7.

use std::path::{Path, PathBuf};
use std::{env, fs, io, process};

use apps::launcher::LauncherApp;
use apps::project::{window_geometry_blocking, ProjectApp};
use freya::prelude::*;
use strata_agent::serve_stdio;
use strata_core::config::AppConfig;
use strata_core::engine::purge_snapshot_root;
use strata_core::project as project_io;

use crate::agent::create_global_agent;
use crate::platform::{create_global_open, create_global_windows};
use crate::state::{create_global_config, create_global_theme_preview, AppCtx};
use crate::theme::ThemesCtx;

mod agent;
mod apps;
pub mod components;
mod keymap;
mod menu;
mod platform;
mod state;
mod task;
mod theme;

fn main() {
    // **Before anything app-global.** `strata mcp <project>` is a headless MCP server, not a
    // window: it must not build the theme registry, read app config, touch the windows
    // registry or embed fonts, none of which exist for it — so the branch is taken here,
    // ahead of all of them, rather than after a launch config has been assembled.
    let folder = match cli(env::args().skip(1)) {
        Cli::Mcp(folder) => return headless(&folder),
        Cli::Usage => {
            eprintln!("{USAGE}");
            process::exit(2);
        }
        Cli::Gui(folder) => folder,
    };
    // First thing: nothing logged before this exists. Every `tracing::*` call in the app
    // and in `strata-core` is a no-op until a subscriber is installed.
    init_logging(Log::Stdout);
    // Clear snapshot leftovers from a previous crashed run (each live engine only ever
    // cleans its own subdirectory — safe only here, before any engine exists).
    purge_snapshot_root();
    // Discover the theme registry once (built-ins + the user themes dir) — every window
    // shares this one handle via context.
    let themes = ThemesCtx::discover();
    // The app-global **reactive config**: the whole `AppConfig` — settings, recents, and
    // the open-project set — loaded from disk once here and shared by every window. Disk
    // is a startup input, never a live source: from now on this store is the truth and
    // `write_config` is the only thing that writes the file. Writes are per-channel, so a
    // project opening wakes the recents readers without touching the theme.
    //
    // The theme itself is pure *derived* state: each window's `use_strata_theme` resolves
    // the settings selection (+ OS appearance while Sync-with-OS is on, via Freya's
    // per-window `Platform.preferred_theme`) through the shared registry — no stored
    // applied-theme id to keep coherent.
    let (config, reopen) = create_global_config();
    // …and the app-global **live** window registry: which windows exist right now, so a
    // project that already has one is focused rather than opened twice.
    let windows = create_global_windows();
    // The Settings window's live theme preview — the one half of its uncommitted draft every
    // *other* window reads, which is what makes picking a theme repaint them all at once
    // while the choice is still uncommitted. Empty except while that window is open.
    let preview = create_global_theme_preview();
    // The menubar. Its builder runs at `resumed`, on this very thread, and hands back the
    // File menu's handles — which land in a third app-global so the focused window can keep
    // Open Recent and Close Project pointed at itself (`menu::use_file_menu`). The builder
    // captures the resolved chords rather than the config handle, since accelerators are
    // read once; the event *handler* holds the live handles, so dispatch resolves current
    // bindings and can open a recent straight from the renderer.
    let menu_chords = menu::menu_chords(&config.peek().settings);
    let menu_state = menu::create_global_menu();
    // …and beside the menu handles, the slot the focused window parks its open path in, so
    // File ▸ Open Recent honours that window's "Opening a project" preference (this window /
    // a new one / ask) instead of always launching a window. Open… needs no slot: it reaches
    // the focused window as a synthesized chord, like every other menu command.
    let focused_open = create_global_open();
    // Agent access (AA-03): the cross-thread service directory a project window lends its
    // engine and its ask channel to, plus the slot holding whatever MCP server is listening.
    // Nothing listens yet — a workspace window's `use_agent_server` starts one only if the
    // `agent_access` setting is on, which it is not by default.
    let agent = create_global_agent();
    // Everything a window — or the menubar handler — is handed, in one value.
    let app = AppCtx {
        themes,
        config,
        windows,
        preview,
        menu: menu_state,
        open: focused_open,
        agent,
    };
    let menu_app = app.clone();
    let launch_config = with_embedded_fonts(LaunchConfig::new())
        // The muda menubar replaces winit's default menu at resume. Crucially its
        // Quit is a *custom* item routed through the close-request path (red-button
        // semantics, T2 confirm keeps its say) — winit's own Quit sent Cocoa's
        // `terminate:` directly, swallowing ⌘Q before the keymap AND bypassing the
        // `on_close` veto. (Known gap: a Dock-icon "Quit" still `terminate:`s
        // un-vetoed — winit 0.30 exposes no `applicationShouldTerminate`; its 0.31
        // "bring your own app delegate" closes this, see P6-02.)
        .with_menu(
            move || {
                let (menu, handles) = menu::app_menu(menu_chords);
                let mut menu_state = menu_state;
                menu_state.set(Some(handles));
                menu
            },
            move |event, ctx| menu::handle_menu_event(event, ctx, menu_app.clone()),
        );
    // One window per project to restore, or the launcher. `with_window` may be called any
    // number of times, so the whole restore set opens as the app's initial windows — no
    // first-window-spawns-the-rest dance.
    let launch_config = match startup(&config.peek(), reopen, folder) {
        Startup::Projects(roots) => roots.into_iter().fold(launch_config, |cfg, root| {
            // Geometry is a launch input, resolved before the window exists. Blocking for it is
            // free here — there is no event loop yet to hold up — and it is bounded either way
            // (`GEOMETRY_DEADLINE`), so a project on a mount that stopped answering no longer
            // keeps the app from starting at all.
            let geometry = window_geometry_blocking(root.clone());
            cfg.with_window(ProjectApp::window(app.clone(), root, geometry))
        }),
        Startup::Launcher => launch_config.with_window(LauncherApp::window(app)),
    };
    launch(launch_config);
}

/// The families the themes name (`themes/*.json` `fonts`), embedded rather than assumed. Neither
/// ships with macOS, so on any machine that has not installed them by hand every surface fell back
/// to the system UI font — which is the whole type scale gone, silently, and only on somebody
/// else's machine. A build we hand to a tester has to look like the build we drew.
///
/// One file per weight the themes actually ask for: 400, 500 and 600 are the only values that
/// appear across `typography` and the component overrides. Registering all three under **one**
/// alias is what makes them a family rather than three families sharing a name — Skia's
/// `TypefaceFontProvider` appends every typeface registered under an alias into a single style set
/// and matches the requested weight against it (`freya-winit`'s launch does the registering).
///
/// Glyphs neither family covers still resolve: the launch keeps the system font manager as the
/// *default* one and only adds ours as dynamic, so fallback is unaffected.
static EMBEDDED_FONTS: [(&str, &[u8]); 6] = [
    (
        "IBM Plex Sans",
        include_bytes!("../../../assets/fonts/IBMPlexSans-Regular.ttf"),
    ),
    (
        "IBM Plex Sans",
        include_bytes!("../../../assets/fonts/IBMPlexSans-Medium.ttf"),
    ),
    (
        "IBM Plex Sans",
        include_bytes!("../../../assets/fonts/IBMPlexSans-SemiBold.ttf"),
    ),
    (
        "JetBrains Mono",
        include_bytes!("../../../assets/fonts/JetBrainsMono-Regular.ttf"),
    ),
    (
        "JetBrains Mono",
        include_bytes!("../../../assets/fonts/JetBrainsMono-Medium.ttf"),
    ),
    (
        "JetBrains Mono",
        include_bytes!("../../../assets/fonts/JetBrainsMono-SemiBold.ttf"),
    ),
];

/// Register [`EMBEDDED_FONTS`] on the launch config.
fn with_embedded_fonts(config: LaunchConfig) -> LaunchConfig {
    EMBEDDED_FONTS
        .iter()
        .fold(config, |config, (family, data)| {
            config.with_font(*family, *data)
        })
}

/// The subcommand that means "serve, don't open a window". An exact string: a bare folder as
/// the first argument still means `strata <project>`, which is what it has always meant.
const MCP_SUBCOMMAND: &str = "mcp";

/// What to print when the command line is not one of the two forms.
const USAGE: &str = "usage: strata [<project folder>]\n       strata mcp <project folder>";

/// What the command line asks for.
#[derive(Debug, PartialEq)]
enum Cli {
    /// Open the GUI, on the named folder if there is one.
    Gui(Option<String>),
    /// Serve MCP over stdio for one project and never open a window.
    Mcp(String),
    /// Neither form — print [`USAGE`] and stop.
    Usage,
}

/// Read the arguments after the executable's own.
///
/// Pure, over an iterator rather than `env::args`, so the three forms are testable: the
/// alternative is a subcommand nobody can assert on until the app is launched.
///
/// `strata mcp` with no folder is a **usage error naming the form**, not a launcher: it asks
/// for a server, and a client that spawned it would otherwise be handed a GUI it cannot speak
/// to. A second path after the folder is refused for the same reason — this host serves one
/// project by construction, so a caller passing two has misunderstood something and silently
/// dropping the second would hide it.
fn cli<A: IntoIterator<Item = String>>(args: A) -> Cli {
    let mut args = args.into_iter();
    match args.next() {
        Some(first) if first == MCP_SUBCOMMAND => match (args.next(), args.next()) {
            (Some(folder), None) => Cli::Mcp(folder),
            _ => Cli::Usage,
        },
        // Extra arguments after a folder are ignored exactly as they always were: the GUI
        // opens the first one, and macOS hands a launched app arguments of its own.
        folder => Cli::Gui(folder),
    }
}

/// `strata mcp <project>`: serve the agent-access vocabulary over stdio against a plain
/// engine, and exit when the client disconnects.
///
/// Nothing app-global is built, because none of it exists for a server with no window — and
/// the one thing it would be tempting to read, app config, is deliberately left alone: this
/// process cannot see the app's `datafusion.*` overrides, so the engine runs the defaults
/// (the spec's "The headless host"; a `--config` flag can arrive when somebody wants one).
fn headless(folder: &str) {
    // **stderr, always.** stdout is the MCP transport's, and one stray log line on it is a
    // parse error at the client — so the subscriber is pointed away from it before anything
    // this process does can log, `strata-core` included.
    init_logging(Log::Stderr);
    // The same sweep the GUI does, in the same place and for the same reason: once at
    // startup, before this process has an engine of its own. A live engine's snapshot
    // directory is lock-claimed, so an app running beside this one is untouched.
    purge_snapshot_root();
    // Through the shared normalisation, like every other open path: naming a project's own
    // `.strata` directory serves the project rather than a fresh one inside it. A path that
    // will not resolve is reported by `resolve_project_folder` itself.
    let Some(root) = platform::resolve_project_folder(Path::new(folder)) else {
        process::exit(1);
    };
    if let Err(e) = serve_stdio(root) {
        tracing::error!("{e}");
        process::exit(1);
    }
}

/// What the app opens on launch.
enum Startup {
    /// Reopen these project folders, one window each — the set that had a window at the
    /// last quit (or a folder named on the command line).
    Projects(Vec<PathBuf>),
    /// The welcome window: nothing to reopen.
    Launcher,
}

/// Decide the launch windows, RustRover's rule: when "Reopen projects on startup" is on,
/// reopen **every** project that had a window at the last quit (filtered to the ones still
/// on disk); otherwise show the welcome window. A folder named on the command line
/// (`strata path/to/project`) wins outright — that's an explicit "open this".
///
/// `reopen` is the persisted open-set, taken out of the store by `create_global_config`:
/// stale by definition once the process is running, since windows re-add themselves as they
/// open. Note that only a *quit* leaves it populated — closing every window by hand empties
/// it, which is what makes "I closed everything" mean "start me at the launcher".
///
/// A path that won't resolve is reported and skipped rather than fatal: a project folder
/// that has been moved or deleted since the last run is ordinary, and the launcher is a
/// perfectly good place to land.
fn startup(config: &AppConfig, reopen: Vec<String>, folder: Option<String>) -> Startup {
    if let Some(arg) = folder {
        // Through the shared normalisation, like every other open path: naming a project's
        // own `.strata` directory opens the project, not a fresh one scaffolded inside it.
        return match platform::resolve_project_folder(Path::new(&arg)) {
            Some(root) => Startup::Projects(vec![root]),
            None => Startup::Launcher,
        };
    }
    if config.settings.reopen_on_startup {
        let roots: Vec<PathBuf> = reopen
            .iter()
            .filter_map(|path| match fs::canonicalize(path) {
                // A folder that no longer holds a project isn't reopened — restoring a
                // window would silently scaffold a fresh `.strata/` into it.
                Ok(root) if project_io::exists_at(&root) => Some(root),
                Ok(root) => {
                    tracing::warn!("not reopening `{}`: no project there", root.display());
                    None
                }
                Err(e) => {
                    tracing::warn!("not reopening `{path}`: {e}");
                    None
                }
            })
            .collect();
        if !roots.is_empty() {
            return Startup::Projects(roots);
        }
    }
    Startup::Launcher
}

/// Where the log goes — a decision the headless branch has to make and the GUI does not.
enum Log {
    Stdout,
    /// `strata mcp`: stdout carries the MCP framing, so a log line on it is a parse error at
    /// the client rather than noise.
    Stderr,
}

/// Install a tracing subscriber. Defaults to `warn` for deps + `info` for every `strata*`
/// crate (`EnvFilter` matches targets by prefix, so one directive covers `strata_freya`,
/// `strata_core`, `strata_model`, …); override with `RUST_LOG`. `try_init` is a no-op if a
/// subscriber is already installed.
fn init_logging(to: Log) {
    use tracing_subscriber::EnvFilter;
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn,strata=info"));
    let subscriber = tracing_subscriber::fmt().with_env_filter(filter);
    let _ = match to {
        Log::Stdout => subscriber.try_init(),
        Log::Stderr => subscriber.with_writer(io::stderr).try_init(),
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(argv: &[&str]) -> Vec<String> {
        argv.iter().map(ToString::to_string).collect()
    }

    /// The two GUI forms, unchanged: no arguments is the startup routing's decision, and a
    /// folder is an explicit "open this".
    #[test]
    fn no_arguments_and_a_folder_both_open_the_gui() {
        assert_eq!(cli(args(&[])), Cli::Gui(None));
        assert_eq!(
            cli(args(&["/data/sales"])),
            Cli::Gui(Some("/data/sales".into()))
        );
    }

    /// The subcommand is the **exact** string, so a project folder that happens to be called
    /// `mcp` is still opened in a window — it is a path, and paths are the other form.
    #[test]
    fn the_subcommand_is_the_exact_string_and_takes_the_folder_after_it() {
        assert_eq!(
            cli(args(&["mcp", "/data/sales"])),
            Cli::Mcp("/data/sales".into())
        );
        assert_eq!(cli(args(&["./mcp"])), Cli::Gui(Some("./mcp".into())));
        assert_eq!(
            cli(args(&["MCP", "/data/sales"])),
            Cli::Gui(Some("MCP".into()))
        );
    }

    /// A server asked for with nothing to serve — or with two projects, which this host
    /// cannot hold — is a usage error rather than a window: a client that spawned this is
    /// waiting on stdout for MCP, and a GUI would leave it waiting forever.
    #[test]
    fn the_subcommand_without_exactly_one_folder_is_a_usage_error() {
        assert_eq!(cli(args(&["mcp"])), Cli::Usage);
        assert_eq!(cli(args(&["mcp", "/a", "/b"])), Cli::Usage);
    }
}
