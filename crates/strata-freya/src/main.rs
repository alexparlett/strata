//! Strata — the Freya (Skia / native) frontend, over the shared core crates (`strata-core`
//! for app services, `strata-engine` for the DataFusion boundary).
//!
//! `apps/<window>/` holds one self-contained OS window each, `platform/` the window model that
//! spawns and focuses between them. Top-level `state/` (global singletons), `components/` (design
//! system) and `theme.rs` are shared by every window.
//!
//! No Tokio runtime here on purpose: the engine facade owns a private one and the UI just awaits
//! its methods, `JoinHandle`s being executor-agnostic.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::{env, fs, io, process};

use apps::launcher::LauncherApp;
use apps::project::{window_geometry_blocking, ProjectApp};
use freya::prelude::*;
use strata_agent::assistant::Assistant;
use strata_agent::serve_stdio;
use strata_core::config::AppConfig;
use strata_core::project as project_io;
use strata_core::secret::open_keystore;
use strata_engine::purge_snapshot_root;
use tracing_appender::non_blocking::WorkerGuard;

use crate::agent::create_global_agent;
use crate::platform::{create_global_open, create_global_windows};
use crate::state::{
    create_global_config, create_global_listings, create_global_probes,
    create_global_theme_preview, create_global_updates, install_pending, AppCtx,
};
use crate::theme::ThemesCtx;
use crate::updater::create_global_update_request;

mod agent;
mod apps;
pub mod components;
mod keymap;
mod menu;
mod platform;
mod state;
mod task;
mod theme;
mod updater;

fn main() {
    let folder = match cli(env::args().skip(1)) {
        Cli::Mcp(folder) => return headless(&folder),
        Cli::Usage => {
            eprintln!("{USAGE}");
            process::exit(2);
        }
        Cli::Gui(folder) => folder,
    };
    let _guard = init_logging(Log::Stdout);
    if let Err(err) = open_keystore() {
        tracing::error!("{err}");
    }
    purge_snapshot_root();
    let themes = ThemesCtx::discover();
    let (config, reopen) = create_global_config();
    let windows = create_global_windows();
    let preview = create_global_theme_preview();
    let menu_chords = menu::menu_chords(&config.peek().settings);
    let menu_state = menu::create_global_menu();
    let focused_open = create_global_open();
    let agent = create_global_agent();
    let listings = create_global_listings();
    let probes = create_global_probes();
    let updates = create_global_updates();
    let update_request = create_global_update_request();
    let assistant = match Assistant::new() {
        Ok(assistant) => Some(Rc::new(assistant)),
        Err(e) => {
            tracing::error!("{e}");
            None
        }
    };
    let app = AppCtx {
        themes,
        config,
        windows,
        preview,
        menu: menu_state,
        open: focused_open,
        agent,
        listings,
        probes,
        assistant,
        updates,
        update_request,
    };
    let menu_app = app.clone();
    let launch_config = with_embedded_fonts(LaunchConfig::new()).with_menu(
        move || {
            let (menu, handles) = menu::app_menu(menu_chords);
            let mut menu_state = menu_state;
            menu_state.set(Some(handles));
            menu
        },
        move |event, ctx| menu::handle_menu_event(event, ctx, menu_app.clone()),
    );
    let launch_config = match startup(&config.peek(), reopen, folder) {
        Startup::Projects(roots) => roots.into_iter().fold(launch_config, |cfg, root| {
            let geometry = window_geometry_blocking(root.clone());
            cfg.with_window(ProjectApp::window(app.clone(), root, geometry))
        }),
        Startup::Launcher => launch_config.with_window(LauncherApp::window(app)),
    };
    launch(launch_config);
    install_pending();
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
    let _guard = init_logging(Log::Stderr);
    purge_snapshot_root();
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
        return match platform::resolve_project_folder(Path::new(&arg)) {
            Some(root) => Startup::Projects(vec![root]),
            None => Startup::Launcher,
        };
    }
    if config.settings.reopen_on_startup {
        let roots: Vec<PathBuf> = reopen
            .iter()
            .filter_map(|path| match fs::canonicalize(path) {
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
#[derive(Clone, Copy)]
enum Log {
    Stdout,
    /// `strata mcp`: stdout carries the MCP framing, so a log line on it is a parse error at
    /// the client rather than noise.
    Stderr,
}

impl Log {
    /// The log file's name stem — **and the two must differ**, because these are two processes.
    ///
    /// An MCP client can spawn `strata mcp` while the app is open, and a rolling appender is not
    /// a shared-writer abstraction: each one enforces `max_log_files` by listing the files whose
    /// name matches its own prefix and unlinking the oldest. Pointed at one stem, the two
    /// processes prune each other — including, on a day boundary, a file the other still holds
    /// open, whose remaining lines then go to an unlinked inode and are never seen again. Two
    /// stems make two independent rotations, and telling an app run from a headless one is worth
    /// having in its own right.
    fn stem(self) -> &'static str {
        match self {
            Self::Stdout => "strata",
            Self::Stderr => "strata-mcp",
        }
    }
}

/// How many days of logs are kept. A week covers "it did the thing on Tuesday, here is the
/// file" without a folder that grows forever on a machine nobody sweeps.
const LOG_RETENTION: usize = 7;

/// Where the log file goes, following [`user_themes_dir`](strata_core::theme::user_themes_dir)'s
/// resolution exactly — `$HOME`, then a per-OS convention.
///
/// `~/Library/Logs/Strata` on macOS, which is the platform's own answer and the one Console.app
/// lists under **Log Reports**, so the file is reachable without knowing the path. Elsewhere the
/// XDG state directory, because a log is state rather than config — it is not something the user
/// edits, and it must not sit in the folder where they drop themes.
///
/// `None` when there is no `$HOME` to hang it off, which is a real case (a bare launchd context)
/// and not a fault: [`init_logging`] then keeps the console writer and says so.
fn log_dir() -> Option<PathBuf> {
    let home = PathBuf::from(env::var_os("HOME")?);
    #[cfg(target_os = "macos")]
    let dir = home.join("Library/Logs/Strata");
    #[cfg(not(target_os = "macos"))]
    let dir = home.join(".local/state/Strata/logs");
    Some(dir)
}

/// Install a tracing subscriber. Defaults to `warn` for deps + `info` for every `strata*`
/// crate (`EnvFilter` matches targets by prefix, so one directive covers `strata_freya`,
/// `strata_core`, `strata_model`, …); override with `RUST_LOG`. `try_init` is a no-op if a
/// subscriber is already installed.
///
/// **Two writers, always.** The console one is what a developer running `cargo run` reads; the
/// file is the only one an *installed* build has. A `.app` launched from Finder inherits no
/// terminal — its stdout goes to the unified log, where it is neither a file a tester can send
/// nor a stream `RUST_LOG` can be pointed at — so without the file half, the build that most
/// needs a diagnosis is the build that cannot produce one. This was not hypothetical: an S3
/// connection failing with a message the panels truncated had no second copy anywhere.
///
/// **The returned guard must be held for the life of the process.** The file writer is
/// non-blocking — a worker thread owns the actual writes so no `tracing::` call ever waits on
/// disk — and dropping the guard is what flushes and stops it. Dropped early, the last and most
/// interesting lines before a crash are the ones that never land.
///
/// `None` when there is no file half (no `$HOME`, or the directory cannot be made); the console
/// writer is installed either way, so logging never depends on the file working.
#[must_use = "dropping the guard stops the file writer and loses buffered lines"]
fn init_logging(to: Log) -> Option<WorkerGuard> {
    use tracing_subscriber::fmt::writer::MakeWriterExt;
    use tracing_subscriber::EnvFilter;

    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn,strata=info"));

    let file = log_dir().and_then(|dir| {
        if let Err(e) = fs::create_dir_all(&dir) {
            eprintln!("no log file ({}): {e}", dir.display());
            return None;
        }
        let appender = tracing_appender::rolling::Builder::new()
            .rotation(tracing_appender::rolling::Rotation::DAILY)
            .filename_prefix(to.stem())
            .filename_suffix("log")
            .max_log_files(LOG_RETENTION)
            .build(&dir)
            .map_err(|e| eprintln!("no log file ({}): {e}", dir.display()))
            .ok()?;
        Some(tracing_appender::non_blocking(appender))
    });

    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false);
    match (to, file) {
        (Log::Stdout, Some((file, guard))) => {
            let _ = subscriber.with_writer(io::stdout.and(file)).try_init();
            Some(guard)
        }
        (Log::Stderr, Some((file, guard))) => {
            let _ = subscriber.with_writer(io::stderr.and(file)).try_init();
            Some(guard)
        }
        (Log::Stdout, None) => {
            let _ = subscriber.try_init();
            None
        }
        (Log::Stderr, None) => {
            let _ = subscriber.with_writer(io::stderr).try_init();
            None
        }
    }
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
