//! **In-app update** (UP-02) — what the newest release is, how it becomes a verified bundle on
//! disk, and how that bundle takes the running one's place.
//!
//! Window-free by construction: nothing here paints, and nothing here knows what a window is.
//! The app-side half is `strata_freya::state::updates`, which runs these on a worker thread and
//! keeps a status slot; the surfaces are UP-03.
//!
//! **Blocking**, on `list_models_blocking`'s shape: every entry point owns a current-thread runtime
//! and a one-off client for the length of one call, which is what keeps `reqwest` and Tokio out of
//! `strata-freya` entirely — the frontend runs these through `task::offload`.
//!
//! **The version is an argument, never `env!`.** This crate is versioned independently of the app,
//! so a check reading its own `CARGO_PKG_VERSION` would compare the release against the wrong
//! number; [`check_blocking`] takes the running app's version and refuses one it cannot parse.
//!
//! **What makes a download safe to install** is the content layer, not the transport: [`verify`]
//! refuses anything whose signature does not verify strictly, does not name [`TEAM_ID`], or does
//! not claim [`APP_ID`]. A MITM or a compromised CDN can withhold an update, never substitute one,
//! and the offer requires a strictly newer semver so a replayed listing cannot downgrade. There is
//! no system check behind ours: quarantine is opt-in and set by browsers, so a file the app
//! downloads never carries one and Gatekeeper never assesses the bundle that gets swapped in.
//!
//! **The staging layout is a contract.** A download lands in `<temp>/strata-update-<uuid>/` and the
//! archive unpacks *into that folder*, so a staged bundle's parent **is** its staging folder —
//! which is what lets [`discard`] sweep one from the path alone.
//!
//! **macOS.** Not `cfg`-gated because it does not need to be: [`site`] finds no bundle anywhere
//! else, and an updater with no install site is inert.

use std::env;
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(debug_assertions)]
use std::sync::OnceLock;
use std::time::Duration;

use semver::Version;
use serde::Deserialize;
use uuid::Uuid;

use crate::secret::{APP_ID, TEAM_ID};

/// The repository releases are cut from. One spelling, read by the URL below and by nothing
/// else.
const REPO: &str = "alexparlett/strata";

/// How many releases back the check looks. The newest is almost always the first entry; the
/// window exists so a run of drafts (which are skipped) cannot hide the newest published one.
const PER_PAGE: usize = 10;

/// The filename suffix UP-01's release pipeline attaches the update archive under. The version
/// in the filename is informative; the tag is what says which release it is.
const UPDATE_ASSET: &str = ".app.zip";

/// How long the check waits for GitHub, end to end. A version check is something the app does
/// at startup without being asked, so it either answers quickly or it is not worth waiting for.
const CHECK_TIMEOUT: Duration = Duration::from_secs(20);

/// How long a download waits for the *next* bytes. A total timeout would be wrong here: the
/// archive is a third of a gigabyte, and how long that legitimately takes is the user's
/// connection rather than anything this code can bound. A stall is what we can recognise.
const READ_TIMEOUT: Duration = Duration::from_secs(60);

/// How long either request waits to get a connection at all.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// The most redirects either request follows. GitHub's asset URLs take exactly one hop, to
/// `objects.githubusercontent.com`; the allowance is generous and finite.
const MAX_HOPS: usize = 5;

/// The staging folder's name stem, under the OS temp directory. Also the guard [`discard`]
/// checks before it deletes anything.
const STAGE_PREFIX: &str = "strata-update-";

/// What the archive is called inside its staging folder while it is being written.
const ARCHIVE: &str = "update.zip";

const CODESIGN: &str = "/usr/bin/codesign";
/// The archiver that made the release zip. A Rust unzip that drops extended attributes or
/// flattens symlinks produces a bundle whose signature no longer verifies, so the tool that
/// wrote it is the tool that reads it.
const DITTO: &str = "/usr/bin/ditto";
const PLIST_BUDDY: &str = "/usr/libexec/PlistBuddy";
const OPEN: &str = "/usr/bin/open";

/// The update archive attached to a release — UP-01's `.app.zip`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Asset {
    pub name: String,
    pub url: String,
    /// What the release page says it weighs, for a progress bar that has a denominator before
    /// the first byte arrives.
    pub size: u64,
}

/// A release newer than the running app.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Offer {
    /// The tag with its leading `v` stripped, which is the number the app shows.
    pub version: String,
    /// The release page, always — this is what an offer degrades to when there is nothing to
    /// install or nowhere to install it.
    pub page_url: String,
    /// **What changed**, as GitHub holds it: the release body, which is Markdown. Carried
    /// rather than fetched a second time, because the check has already read it, and carried
    /// **unparsed** — the surface that shows it (UP-03's report card) renders it with the app's
    /// own Markdown viewer, so this crate does no Markdown work and this stays one string.
    ///
    /// Empty for a release with no body, which is the same nothing as a release note nobody
    /// wrote: the card simply draws no panel.
    pub notes: String,
    /// The update archive, or `None` for a release that carries none. **Not an error**: a
    /// release cut before UP-01, or one whose build failed halfway, is still a real release
    /// worth pointing at.
    pub asset: Option<Asset>,
}

/// What GitHub's release list says about the running version.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Check {
    UpToDate,
    Newer(Offer),
}

/// Where the running app is installed, which is what decides whether an update can be offered
/// at all.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Site {
    /// Not running out of a bundle — a `cargo run` build. The updater is inert: no startup
    /// check and no offer, because a dev build is not an installation.
    Unbundled,
    /// The bundle is there, but the folder holding it cannot be written, so the swap could not
    /// happen. The check still runs and the offer degrades to opening the release page.
    ReadOnly(PathBuf),
    /// The bundle, in a folder this process can replace it in.
    Writable(PathBuf),
}

impl Site {
    /// The bundle this process is running out of, wherever it is.
    pub fn bundle(&self) -> Option<&Path> {
        match self {
            Site::Unbundled => None,
            Site::ReadOnly(app) | Site::Writable(app) => Some(app),
        }
    }
}

/// **Ask GitHub whether there is a newer release**, and answer with what to do about it.
///
/// The **list** endpoint rather than `/latest`, which excludes prereleases — testers live on
/// prereleases, and the Release workflow marks them by default, so `/latest` would go quiet for
/// exactly the people the updater is for.
///
/// `current` is the running app's version (`env!("CARGO_PKG_VERSION")` at the frontend). An
/// unparseable one is refused here rather than treated as "very old": the comparison below is
/// the only thing standing between a forged listing and a downgrade.
pub fn check_blocking(current: &str) -> Result<Check, String> {
    let current = Version::parse(current.trim())
        .map_err(|e| format!("The running version '{current}' is not a version number: {e}."))?;

    let body = runtime()?.block_on(async {
        let client = client(Some(&current))
            .timeout(CHECK_TIMEOUT)
            .build()
            .map_err(|e| format!("Could not build an HTTP client: {e}."))?;
        let response = client
            .get(releases_url())
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
            .map_err(|e| format!("Could not reach GitHub: {e}."))?
            .error_for_status()
            .map_err(|e| format!("GitHub refused the request: {e}."))?;
        response
            .text()
            .await
            .map_err(|e| format!("Could not read GitHub's answer: {e}."))
    })?;

    newest(&body, &current)
}

/// **Fetch, unpack and verify an update**, answering with the staged bundle's path.
///
/// `on_progress` is called with the bytes written so far and the total the server declared, as
/// often as chunks arrive. It runs on the calling (worker) thread, so a caller writing UI state
/// from it has to get itself back to the render thread first.
///
/// Everything happens inside one staging folder, and a failure at any step takes the whole
/// folder with it: a half-unpacked or unverified bundle must never be left somewhere a later
/// press could find and trust.
pub fn download_blocking(
    asset: &Asset,
    on_progress: impl FnMut(u64, Option<u64>),
) -> Result<PathBuf, String> {
    let stage = env::temp_dir().join(format!("{STAGE_PREFIX}{}", Uuid::new_v4()));
    fs::create_dir_all(&stage)
        .map_err(|e| format!("Could not make a staging folder for the update: {e}."))?;

    match stage_update(asset, &stage, on_progress) {
        Ok(app) => Ok(app),
        Err(why) => {
            if let Err(e) = fs::remove_dir_all(&stage) {
                tracing::warn!("could not clear {}: {e}", stage.display());
            }
            Err(why)
        }
    }
}

/// **Is the updater pointed at a local server?** — `STRATA_UPDATE_ORIGIN`, debug builds only,
/// served by `examples/fake_releases.rs`.
///
/// The app asks because two things it owns have to answer differently while it is: a dev build
/// has no install site, so the surfaces would draw nothing at all; and the swap has no bundle to
/// put anywhere, so the install is refused rather than taken.
pub fn is_local() -> bool {
    local_origin().is_some()
}

/// **Where the running app is installed**, and whether it can be replaced there.
///
/// The bundle is the first `.app` above the executable, which is what makes this true for the
/// real layout (`Strata.app/Contents/MacOS/strata`) without hardcoding it. Writability is
/// answered by writing: a directory's permission bits do not settle it on macOS, where an
/// application folder can be group-writable, read-only on a mounted image, or governed by a
/// profile.
pub fn site() -> Site {
    let Ok(exe) = env::current_exe() else {
        return Site::Unbundled;
    };
    let Some(app) = bundle_of(&exe) else {
        return Site::Unbundled;
    };
    match app.parent() {
        Some(dir) if writable(dir) => Site::Writable(app),
        _ => Site::ReadOnly(app),
    }
}

/// **Put `staged` where `target` is.** Call only once no window exists — never against the
/// bundle of a running app.
///
/// Three steps, in the one order that has a way back from each: copy the staged bundle to a
/// sibling of the target (same folder, so the next step is a rename rather than a second copy),
/// rename the target aside, rename the copy in. A failure at the last step renames the original
/// back, so the outcome is either the new app or the old one and never half of either.
///
/// The copy is `ditto` for the reason the unpack is: a bundle copied by anything that drops
/// extended attributes stops verifying.
pub fn install(staged: &Path, target: &Path) -> Result<(), String> {
    let dir = target
        .parent()
        .ok_or_else(|| format!("'{}' has no folder to install into.", target.display()))?;
    let stamp = Uuid::new_v4();
    let incoming = dir.join(format!(".strata-staged-{stamp}.app"));
    let outgoing = dir.join(format!(".strata-old-{stamp}.app"));

    let copied = Command::new(DITTO)
        .arg(staged)
        .arg(&incoming)
        .output()
        .map_err(|e| format!("Could not run '{DITTO}': {e}."))?;
    if !copied.status.success() {
        sweep(&incoming);
        return Err(format!(
            "The update could not be copied into place. {}",
            said(&copied.stderr)
        ));
    }

    if let Err(e) = fs::rename(target, &outgoing) {
        sweep(&incoming);
        return Err(format!("The installed app could not be moved aside: {e}."));
    }
    if let Err(e) = fs::rename(&incoming, target) {
        if let Err(back) = fs::rename(&outgoing, target) {
            return Err(format!(
                "The update could not be installed ({e}) and '{}' could not be put back ({back}). It is at '{}'.",
                target.display(),
                outgoing.display()
            ));
        }
        sweep(&incoming);
        return Err(format!("The update could not be installed: {e}."));
    }

    sweep(&outgoing);
    Ok(())
}

/// Start `app` as a new process. `-n` because this one is still exiting, and `LaunchServices`
/// would otherwise treat the request as "the app is already running" and do nothing.
pub fn relaunch(app: &Path) -> Result<(), String> {
    Command::new(OPEN)
        .arg("-n")
        .arg(app)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Could not start '{}': {e}.", app.display()))
}

/// **Open a release page in the browser.** The link-out every surface that mentions an update
/// offers: what changed, and — where this app cannot replace itself ([`Site::ReadOnly`]) — the
/// download to install by hand.
///
/// Here rather than in the app because the page is the updater's own artifact: the URL comes
/// from the same listing [`check_blocking`] read, and `open` is already this module's way of
/// reaching Launch Services.
pub fn open_page(url: &str) {
    if let Err(e) = Command::new(OPEN).arg(url).spawn() {
        tracing::error!("could not open '{url}': {e}");
    }
}

/// Drop the staging folder a [`download_blocking`] answer lives in.
///
/// From the bundle path alone, because the staging layout is a contract (see the module doc).
/// It refuses to delete anything whose folder is not one of ours, so a path that came from
/// somewhere else can only ever be a no-op.
pub fn discard(app: &Path) {
    let Some(stage) = app.parent() else { return };
    let ours = stage
        .file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|name| name.starts_with(STAGE_PREFIX));
    if !ours {
        tracing::warn!("not clearing {}: not a staging folder", stage.display());
        return;
    }
    sweep(stage);
}

/// The endpoint the check reads. Built here so the slug is written once.
fn releases_url() -> String {
    let origin = local_origin().unwrap_or("https://api.github.com");
    format!("{origin}/repos/{REPO}/releases?per_page={PER_PAGE}")
}

/// **Where the releases come from, when it is not GitHub** — `STRATA_UPDATE_ORIGIN`, and only
/// in a **debug** build.
///
/// The updater is inert outside a bundle and there is never a newer release to hand, so nothing
/// downstream of the check can be looked at in a dev build. Pointing it at
/// `http://127.0.0.1:8787` fixes that without a fake anywhere: the request, the JSON, the offer,
/// the download and its progress are the shipping code, and only the *server* is local. The dev
/// server is `examples/fake_releases.rs`.
///
/// The whole body is `cfg`'d out of a release build, so a shipped app reads no such variable and
/// cannot be pointed anywhere — which is what makes an environment variable acceptable here, and
/// what keeps [`verify`]'s relaxation below honest.
fn local_origin() -> Option<&'static str> {
    #[cfg(not(debug_assertions))]
    {
        None
    }
    #[cfg(debug_assertions)]
    {
        static ORIGIN: OnceLock<Option<String>> = OnceLock::new();
        ORIGIN
            .get_or_init(|| {
                let origin = env::var("STRATA_UPDATE_ORIGIN").ok()?;
                let origin = origin.trim().trim_end_matches('/').to_string();
                tracing::warn!("reading releases from {origin}, not GitHub");
                Some(origin)
            })
            .as_deref()
    }
}

/// A current-thread runtime for the length of one call — `list_models_blocking`'s trade,
/// for the same reason: a one-off request does not justify a runtime with a lifetime.
fn runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("Could not start a worker for the request: {e}."))
}

/// The client both requests are built from: a user agent (GitHub requires one), a connect
/// timeout, and the redirect policy.
///
/// `version` is the running app's, when the caller has one. The download does not — it is
/// reached with an [`Asset`] and nothing else — and it says so by leaving the segment out
/// rather than by naming a version nobody is running.
///
/// **No request ever leaves `https`.** reqwest follows an `https` to `http` redirect by
/// default. The signature check would still catch a tampered payload, but there is no reason
/// to ever drop off TLS, and the asset download's own redirect to
/// `objects.githubusercontent.com` is `https` like everything else.
fn client(version: Option<&Version>) -> reqwest::ClientBuilder {
    let agent = match version {
        Some(version) => format!("Strata/{version} (+https://github.com/{REPO})"),
        None => format!("Strata (+https://github.com/{REPO})"),
    };
    reqwest::Client::builder()
        .user_agent(agent)
        .connect_timeout(CONNECT_TIMEOUT)
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            let scheme = attempt.url().scheme().to_owned();
            if scheme != "https" {
                attempt.error(format!("refusing a redirect to '{scheme}': not https"))
            } else if attempt.previous().len() >= MAX_HOPS {
                attempt.error(format!("refusing more than {MAX_HOPS} redirects"))
            } else {
                attempt.follow()
            }
        }))
}

/// **Read GitHub's release list and decide.** Pure, so the whole policy is testable without a
/// request: drafts are skipped, prereleases are kept, a tag that is not a version is skipped
/// rather than fatal (one odd tag must not blind the updater), and the newest is offered
/// **only if it is strictly newer** than what is running.
fn newest(body: &str, current: &Version) -> Result<Check, String> {
    let releases: Vec<Release> = serde_json::from_str(body)
        .map_err(|e| format!("GitHub's answer could not be read: {e}."))?;

    let best = releases
        .into_iter()
        .filter(|release| !release.draft)
        .filter_map(|release| {
            let tag = release.tag_name.trim();
            match Version::parse(tag.strip_prefix('v').unwrap_or(tag)) {
                Ok(version) => Some((version, release)),
                Err(e) => {
                    tracing::debug!("skipping release '{tag}': {e}");
                    None
                }
            }
        })
        .max_by(|(a, _), (b, _)| a.cmp(b));

    let Some((version, release)) = best.filter(|(version, _)| version > current) else {
        return Ok(Check::UpToDate);
    };

    Ok(Check::Newer(Offer {
        version: version.to_string(),
        page_url: release.html_url,
        notes: release
            .body
            .unwrap_or_default()
            .replace("\r\n", "\n")
            .trim()
            .to_string(),
        asset: release
            .assets
            .into_iter()
            .find(|asset| asset.name.ends_with(UPDATE_ASSET))
            .map(|asset| Asset {
                name: asset.name,
                url: asset.browser_download_url,
                size: asset.size,
            }),
    }))
}

/// One release, as much of it as the decision needs. `prerelease` is deliberately absent: they
/// are offered, so nothing branches on it.
#[derive(Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    html_url: String,
    /// The release notes. `Option` rather than a defaulted `String` because GitHub sends the
    /// field as `null` for a release with no body, which a `String` field would refuse to read
    /// — and one such release in the list would blind the updater entirely.
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    assets: Vec<ReleaseAsset>,
}

#[derive(Deserialize)]
struct ReleaseAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

/// Fetch, unpack and verify, inside a staging folder the caller cleans up on failure.
fn stage_update(
    asset: &Asset,
    stage: &Path,
    on_progress: impl FnMut(u64, Option<u64>),
) -> Result<PathBuf, String> {
    let archive = stage.join(ARCHIVE);
    fetch(&asset.url, &archive, on_progress)?;

    let unpacked = Command::new(DITTO)
        .arg("-x")
        .arg("-k")
        .arg(&archive)
        .arg(stage)
        .output()
        .map_err(|e| format!("Could not run '{DITTO}': {e}."))?;
    if !unpacked.status.success() {
        return Err(format!(
            "The update archive could not be unpacked. {}",
            said(&unpacked.stderr)
        ));
    }
    if let Err(e) = fs::remove_file(&archive) {
        tracing::warn!("could not clear {}: {e}", archive.display());
    }

    let app = bundle_in(stage)?;
    verify(&app)?;
    Ok(app)
}

/// Stream `url` into `dest`, reporting progress as it goes.
fn fetch(
    url: &str,
    dest: &Path,
    mut on_progress: impl FnMut(u64, Option<u64>),
) -> Result<(), String> {
    runtime()?.block_on(async {
        let client = client(None)
            .read_timeout(READ_TIMEOUT)
            .build()
            .map_err(|e| format!("Could not build an HTTP client: {e}."))?;
        let mut response = client
            .get(url)
            .send()
            .await
            .map_err(|e| format!("Could not start the download: {e}."))?
            .error_for_status()
            .map_err(|e| format!("The download was refused: {e}."))?;

        let total = response.content_length();
        let mut file =
            File::create(dest).map_err(|e| format!("Could not write the download: {e}."))?;
        let mut got = 0u64;
        on_progress(got, total);
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| format!("The download stopped: {e}."))?
        {
            file.write_all(&chunk)
                .map_err(|e| format!("Could not write the download: {e}."))?;
            got += chunk.len() as u64;
            on_progress(got, total);
        }
        file.flush()
            .map_err(|e| format!("Could not write the download: {e}."))
    })
}

/// The one `.app` in a freshly unpacked staging folder.
fn bundle_in(stage: &Path) -> Result<PathBuf, String> {
    let mut found: Option<PathBuf> = None;
    let entries =
        fs::read_dir(stage).map_err(|e| format!("Could not read the unpacked update: {e}."))?;
    for entry in entries {
        let path = entry
            .map_err(|e| format!("Could not read the unpacked update: {e}."))?
            .path();
        if path.extension().is_some_and(|ext| ext == "app") {
            if found.is_some() {
                return Err("The update archive holds more than one application.".into());
            }
            found = Some(path);
        }
    }
    found.ok_or_else(|| "The update archive holds no application.".into())
}

/// **The three facts that make a downloaded bundle safe to install**, each refused in its own
/// words so a failure says which one it was.
///
/// Order matters. The strict verify comes first because it is what seals everything else: the
/// `Info.plist` read below is only worth trusting once the signature covering it has been
/// checked.
///
/// **A local origin is the one thing that relaxes this**, and only in a debug build: a bundle
/// the dev server made carries no Apple signature and never could, so a check pointed at
/// `127.0.0.1` would stop at the first step and `Ready` would be unreachable on a developer's
/// machine. The relaxation is keyed on [`local_origin`] rather than on a flag of its own, so it
/// cannot be switched on for a bundle that came from GitHub, and it is `cfg`'d out of a release
/// build with the origin itself. It says so in the log, loudly, because a skipped signature
/// check is the one thing here worth never doing by accident.
fn verify(app: &Path) -> Result<(), String> {
    if let Some(origin) = local_origin() {
        tracing::warn!("not verifying {}: it came from {origin}", app.display());
        return Ok(());
    }
    let checked = Command::new(CODESIGN)
        .arg("--verify")
        .arg("--deep")
        .arg("--strict")
        .arg(app)
        .output()
        .map_err(|e| format!("Could not run '{CODESIGN}': {e}."))?;
    if !checked.status.success() {
        return Err(format!(
            "The downloaded update is not correctly signed. {}",
            said(&checked.stderr)
        ));
    }

    let described = Command::new(CODESIGN)
        .arg("-d")
        .arg("-vv")
        .arg(app)
        .output()
        .map_err(|e| format!("Could not run '{CODESIGN}': {e}."))?;
    if !described.status.success() {
        return Err(format!(
            "The downloaded update's signature could not be read. {}",
            said(&described.stderr)
        ));
    }
    let report = String::from_utf8_lossy(&described.stderr);
    match field(&report, "TeamIdentifier") {
        None | Some("not set") => return Err(
            "The downloaded update carries no Apple team signature, so it is not a release build."
                .into(),
        ),
        Some(team) if team != TEAM_ID => {
            return Err(format!(
                "The downloaded update is signed by team '{team}', not '{TEAM_ID}'."
            ))
        }
        Some(_) => {}
    }

    let claimed = bundle_id(app)?;
    if claimed != APP_ID {
        return Err(format!(
            "The downloaded update identifies itself as '{claimed}', not '{APP_ID}'."
        ));
    }
    Ok(())
}

/// What the staged bundle's `Info.plist` calls itself.
fn bundle_id(app: &Path) -> Result<String, String> {
    let plist = app.join("Contents").join("Info.plist");
    let read = Command::new(PLIST_BUDDY)
        .arg("-c")
        .arg("Print :CFBundleIdentifier")
        .arg(&plist)
        .output()
        .map_err(|e| format!("Could not run '{PLIST_BUDDY}': {e}."))?;
    if !read.status.success() {
        return Err(format!(
            "The downloaded update does not name a bundle identifier. {}",
            said(&read.stdout)
        ));
    }
    Ok(String::from_utf8_lossy(&read.stdout).trim().to_string())
}

/// One `Key=value` line out of a `codesign -dvv` report. Whole-line prefix, so `Identifier`
/// cannot read `TeamIdentifier`'s line.
fn field<'a>(report: &'a str, key: &str) -> Option<&'a str> {
    report
        .lines()
        .find_map(|line| line.trim().strip_prefix(key)?.strip_prefix('='))
        .map(str::trim)
}

/// The first `.app` above `exe`, which is the bundle it is installed in.
fn bundle_of(exe: &Path) -> Option<PathBuf> {
    exe.ancestors()
        .find(|path| path.extension().is_some_and(|ext| ext == "app"))
        .map(Path::to_path_buf)
}

/// Whether this process can write into `dir`, answered by writing into it. Nothing about a
/// directory's mode bits settles the question on macOS.
fn writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".{STAGE_PREFIX}probe-{}", Uuid::new_v4()));
    match File::create(&probe) {
        Ok(_) => {
            sweep(&probe);
            true
        }
        Err(_) => false,
    }
}

/// Best-effort removal of something we made. A leftover is a log line, never a reported
/// failure — by the time this is called the thing it belongs to has already succeeded or been
/// undone.
fn sweep(path: &Path) {
    let gone = if path.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    if let Err(e) = gone {
        if e.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!("could not clear {}: {e}", path.display());
        }
    }
}

/// How much of a tool's own output a message carries.
const SAID_MAX: usize = 300;

/// A tool's own words, trimmed to something a dialog can hold.
fn said(output: &[u8]) -> String {
    let text = String::from_utf8_lossy(output);
    let text = text.trim();
    match text.char_indices().nth(SAID_MAX) {
        Some((at, _)) => format!("{}...", &text[..at]),
        None => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A release list in GitHub's own shape, so the parse under test is the parse that runs.
    ///
    /// `r###` because a release body opens with a Markdown heading: the `"##` that starts one
    /// would close an `r#` or an `r##` string.
    fn releases() -> &'static str {
        r###"[
          {
            "tag_name": "v0.4.0",
            "draft": false,
            "prerelease": true,
            "html_url": "https://github.com/alexparlett/strata/releases/tag/v0.4.0",
            "body": "## What's new\r\n\r\n- Charts got a Shape panel\r\n",
            "assets": [
              {
                "name": "Strata-0.4.0-universal.dmg",
                "browser_download_url": "https://example.invalid/Strata-0.4.0-universal.dmg",
                "size": 124023018
              },
              {
                "name": "Strata-0.4.0-universal.app.zip",
                "browser_download_url": "https://example.invalid/Strata-0.4.0-universal.app.zip",
                "size": 111166586
              }
            ]
          },
          {
            "tag_name": "v0.3.1",
            "draft": false,
            "prerelease": true,
            "html_url": "https://github.com/alexparlett/strata/releases/tag/v0.3.1",
            "assets": []
          }
        ]"###
    }

    fn check(body: &str, current: &str) -> Check {
        newest(body, &Version::parse(current).unwrap()).expect("read the list")
    }

    fn offer(body: &str, current: &str) -> Offer {
        match check(body, current) {
            Check::Newer(offer) => offer,
            Check::UpToDate => panic!("expected an offer"),
        }
    }

    /// The ordinary case: the newest published release, its page, and the one asset an
    /// installer can use — the DMG beside it is the first-install artifact and not this.
    #[test]
    fn the_newest_release_is_offered_with_its_update_archive() {
        let offer = offer(releases(), "0.3.1");
        assert_eq!(offer.version, "0.4.0");
        assert!(offer.page_url.ends_with("/v0.4.0"), "{}", offer.page_url);
        let asset = offer.asset.expect("the archive");
        assert_eq!(asset.name, "Strata-0.4.0-universal.app.zip");
        assert_eq!(asset.size, 111_166_586);
    }

    /// **The offer carries what changed, as written.** GitHub's body is Markdown and reaches
    /// the surface unparsed — rendering it is the app's, not this crate's — but its line
    /// endings are normalized here, because a `\r` reaches the text shaper as a glyph even
    /// after the Markdown parser has had it. A release with no body,
    /// which GitHub sends as `null`, is simply nothing to show; reading that field as a
    /// `String` would refuse the whole list.
    #[test]
    fn the_offer_carries_the_release_notes_and_survives_a_release_with_none() {
        let offer = offer(releases(), "0.3.1");
        assert_eq!(
            offer.notes, "## What's new\n\n- Charts got a Shape panel",
            "the notes were rewritten or left with a carriage return"
        );

        let body = r#"[
          {"tag_name": "v0.5.0", "draft": false, "prerelease": false, "body": null,
           "html_url": "https://example.invalid/v0.5.0", "assets": []}
        ]"#;
        assert_eq!(self::offer(body, "0.3.1").notes, "");
    }

    /// **Never a version that is not strictly newer.** Equal is up to date, and older is up to
    /// date too: a replayed or forged listing must not be able to walk a running app backwards.
    #[test]
    fn only_a_strictly_newer_release_is_offered() {
        assert_eq!(check(releases(), "0.4.0"), Check::UpToDate);
        assert_eq!(check(releases(), "0.5.0"), Check::UpToDate);
        assert_eq!(check(releases(), "1.0.0"), Check::UpToDate);
    }

    /// A prerelease is a release — testers live on them, which is why the check reads the list
    /// rather than `/latest`. A draft is not: it is not published and its assets may not exist.
    #[test]
    fn drafts_are_skipped_and_prereleases_are_not() {
        let body = r#"[
          {"tag_name": "v0.9.0", "draft": true, "prerelease": false,
           "html_url": "https://example.invalid/v0.9.0", "assets": []},
          {"tag_name": "v0.5.0", "draft": false, "prerelease": true,
           "html_url": "https://example.invalid/v0.5.0", "assets": []}
        ]"#;
        assert_eq!(offer(body, "0.3.1").version, "0.5.0");
    }

    /// **One odd tag must not blind the updater.** A tag that is not a version is skipped and
    /// the rest of the list still decides.
    #[test]
    fn a_tag_that_is_not_a_version_is_skipped() {
        let body = r#"[
          {"tag_name": "nightly", "draft": false, "prerelease": true,
           "html_url": "https://example.invalid/nightly", "assets": []},
          {"tag_name": "v0.5.0", "draft": false, "prerelease": false,
           "html_url": "https://example.invalid/v0.5.0", "assets": []}
        ]"#;
        assert_eq!(offer(body, "0.3.1").version, "0.5.0");
    }

    /// The list is not ordered by version, so the newest is picked rather than taken from the
    /// front — and a `v` prefix is optional, since it is a tag convention rather than the
    /// version.
    #[test]
    fn the_newest_is_picked_rather_than_the_first() {
        let body = r#"[
          {"tag_name": "v0.4.0", "draft": false, "prerelease": false,
           "html_url": "https://example.invalid/v0.4.0", "assets": []},
          {"tag_name": "0.10.0", "draft": false, "prerelease": false,
           "html_url": "https://example.invalid/0.10.0", "assets": []},
          {"tag_name": "v0.9.0", "draft": false, "prerelease": false,
           "html_url": "https://example.invalid/v0.9.0", "assets": []}
        ]"#;
        assert_eq!(offer(body, "0.3.1").version, "0.10.0");
    }

    /// **A newer release with no update archive still reports its page.** The offer degrades to
    /// "open the release page"; it is not an error, and it is not silence either.
    #[test]
    fn a_release_without_the_archive_still_offers_its_page() {
        let offer = offer(releases(), "0.2.0");
        assert_eq!(offer.version, "0.4.0");
        let body = r#"[
          {"tag_name": "v0.5.0", "draft": false, "prerelease": false,
           "html_url": "https://example.invalid/v0.5.0",
           "assets": [{"name": "Strata-0.5.0-universal.dmg",
                       "browser_download_url": "https://example.invalid/x.dmg", "size": 1}]}
        ]"#;
        let degraded = self::offer(body, "0.3.1");
        assert!(degraded.asset.is_none());
        assert_eq!(degraded.page_url, "https://example.invalid/v0.5.0");
    }

    /// Nothing published at all is up to date rather than an error — a brand new repository is
    /// not a fault.
    #[test]
    fn an_empty_list_is_up_to_date() {
        assert_eq!(check("[]", "0.3.1"), Check::UpToDate);
    }

    /// The running version is refused before anything is compared, because the comparison is
    /// the whole of the downgrade protection.
    #[test]
    fn an_unreadable_running_version_is_refused() {
        assert!(Version::parse("not-a-version").is_err());
        assert!(check_blocking("not-a-version").is_err());
    }

    /// The field read is a whole-line prefix, so the shorter key cannot read the longer key's
    /// line — which would have the updater compare the bundle id against the team.
    #[test]
    fn the_signature_report_is_read_one_whole_key_at_a_time() {
        let report = "Executable=/Applications/Strata.app/Contents/MacOS/strata\n\
                      Identifier=com.alexparlett.strata\n\
                      Format=app bundle with Mach-O universal\n\
                      TeamIdentifier=397J3SJ3D4\n";
        assert_eq!(field(report, "TeamIdentifier"), Some("397J3SJ3D4"));
        assert_eq!(field(report, "Identifier"), Some("com.alexparlett.strata"));
        assert_eq!(field(report, "Authority"), None);
    }

    /// An ad-hoc signature reports the absence as a value, which is why the refusal reads both
    /// that and a missing line as the same answer.
    #[test]
    fn an_ad_hoc_signature_reports_no_team() {
        assert_eq!(
            field(
                "Signature=adhoc\nTeamIdentifier=not set\n",
                "TeamIdentifier"
            ),
            Some("not set")
        );
    }

    /// The bundle is the first `.app` above the executable, not a fixed number of levels up:
    /// that is what makes it true for the real layout without hardcoding it, and false for a
    /// `cargo run` build, which is what keeps the updater inert there.
    #[test]
    fn the_bundle_is_the_first_app_above_the_executable() {
        assert_eq!(
            bundle_of(Path::new("/Applications/Strata.app/Contents/MacOS/strata")),
            Some(PathBuf::from("/Applications/Strata.app"))
        );
        assert_eq!(
            bundle_of(Path::new("/Users/me/dev/strata/target/debug/strata")),
            None
        );
    }

    /// A tool's own words are carried, but bounded: `codesign` can answer with a great deal
    /// more than a dialog has room for.
    #[test]
    fn a_tools_words_are_carried_and_bounded() {
        assert_eq!(
            said(b"  code object is not signed at all\n"),
            "code object is not signed at all"
        );
        let long = said("x".repeat(SAID_MAX * 2).as_bytes());
        assert!(long.ends_with("..."));
        assert_eq!(long.len(), SAID_MAX + 3);
    }

    /// A folder holding one named file, so a swap can be told apart by what is inside it.
    fn bundle(at: PathBuf, marker: &str) -> PathBuf {
        fs::create_dir_all(&at).unwrap();
        fs::write(at.join("who"), marker).unwrap();
        at
    }

    fn who(at: &Path) -> String {
        fs::read_to_string(at.join("who")).unwrap()
    }

    /// **The swap leaves the new app where the old one was, and nothing else behind.** The
    /// siblings it works through are named for this process and would be a visible mess in an
    /// application folder if any of them survived.
    #[test]
    fn installing_replaces_the_target_and_clears_up() {
        let root = env::temp_dir().join(format!("strata-test-{}", Uuid::new_v4()));
        let staged = bundle(root.join("staged").join("Strata.app"), "new");
        let target = bundle(root.join("Applications").join("Strata.app"), "old");

        install(&staged, &target).expect("installed");

        assert_eq!(who(&target), "new");
        assert_eq!(
            who(&staged),
            "new",
            "the staged bundle was moved, not copied"
        );
        let left: Vec<_> = fs::read_dir(root.join("Applications"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(left.len(), 1, "left behind: {left:?}");

        fs::remove_dir_all(&root).unwrap();
    }

    /// **A swap that cannot start leaves the folder exactly as it was.** The copy has already
    /// landed by the time the target is moved aside, so a failure there has to take it back
    /// out again or the next launch finds a stray bundle beside the app.
    #[test]
    fn a_swap_that_fails_leaves_nothing_behind() {
        let root = env::temp_dir().join(format!("strata-test-{}", Uuid::new_v4()));
        let staged = bundle(root.join("staged").join("Strata.app"), "new");
        let apps = root.join("Applications");
        fs::create_dir_all(&apps).unwrap();
        let target = apps.join("Strata.app");

        let why = install(&staged, &target).expect_err("refused");
        assert!(why.contains("moved aside"), "{why}");
        assert!(!target.exists());
        assert_eq!(
            fs::read_dir(&apps).unwrap().count(),
            0,
            "a copy was left behind"
        );

        fs::remove_dir_all(&root).unwrap();
    }

    /// [`discard`] deletes a staging folder and nothing else: the path it is handed comes back
    /// from a download, and a path from anywhere else must be unable to remove a directory.
    #[test]
    fn discard_only_clears_a_staging_folder() {
        let stage = env::temp_dir().join(format!("{STAGE_PREFIX}{}", Uuid::new_v4()));
        let app = stage.join("Strata.app");
        fs::create_dir_all(&app).unwrap();
        discard(&app);
        assert!(!stage.exists());

        let elsewhere = env::temp_dir().join(format!("strata-test-{}", Uuid::new_v4()));
        let inside = elsewhere.join("Strata.app");
        fs::create_dir_all(&inside).unwrap();
        discard(&inside);
        assert!(elsewhere.exists(), "a folder that is not ours was deleted");
        fs::remove_dir_all(&elsewhere).unwrap();
    }
}
