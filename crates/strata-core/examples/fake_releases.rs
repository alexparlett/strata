//! **A releases server for a developer's machine** — the other half of `STRATA_UPDATE_ORIGIN`.
//!
//! The updater cannot be looked at in a dev build: `cargo run` is not an installation, so there
//! is no install site, every affordance is `Inert` and the menubar item ships disabled — and
//! even bundled, there is no newer release to hand unless you cut one.
//!
//! So point the check at this instead of at GitHub. **Nothing about the app is faked**: it makes
//! the same request, parses the same JSON into the same `Offer`, downloads a real archive with
//! real progress, and unpacks it with the same `ditto`. Only the server is local — the one thing
//! that gives way is the signature check (`update::verify`), because a bundle this made carries
//! no Apple signature and never could.
//!
//! ```bash
//! cargo run -p strata-core --example fake_releases          # terminal one
//! STRATA_UPDATE_ORIGIN=http://127.0.0.1:8787 cargo run      # terminal two
//! ```
//!
//! Then **drive it**: type a scenario at this server's prompt and press App ▸ *Check for
//! Updates…* in the app — which re-checks even over an offer it already has, so the app follows
//! whatever the server last said.
//!
//! | scenario | what the app does |
//! |---|---|
//! | `none` | an empty release list: "Strata … is up to date" |
//! | `offer` | a release with an archive: the rail's "Update now", the card, the changelog |
//! | `page` | a release carrying only a DMG: the degraded "Open the release page" |
//! | `slow` | `offer`, served a chunk at a time, so `Downloading` is watchable |
//! | `fail` | the endpoint answers 500: the report card's "The update failed" |
//!
//! `offer` and `slow` end in a staged bundle and the restart question. Confirming it is refused
//! with a log line rather than a swap — `state::updates::install` asks `update::is_local` —
//! because there is nothing here anyone should install.
//!
//! **Std only, on purpose.** Two routes and a prompt do not justify a web framework in a project
//! that has none, and a dev tool that drags in a dependency tree is one nobody keeps building.

use std::env;
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// Where the server listens. The app is pointed at it by `STRATA_UPDATE_ORIGIN`.
const ADDR: &str = "127.0.0.1:8787";

/// The version every scenario offers. Absurd on purpose: it has to beat whatever the app's
/// `CARGO_PKG_VERSION` is today, and it should be obvious in a screenshot that it is not real.
const VERSION: &str = "9.9.9";

/// How big the pretend archive is. Big enough that a download is a thing you can watch (the app
/// moves its progress every megabyte), small enough to build in a blink and cost nothing.
///
/// The padding is **noise, not zeros**: `ditto` deflates the archive, and 48 MB of zeros came out
/// as a 52 KB download that was over before any progress could be drawn. Incompressible bytes
/// keep the archive the size it says it is.
const PADDING: usize = 48 << 20;

/// How much of the archive `slow` writes per tick, and how long it waits between ticks — a
/// download that lasts about half a minute, which is what makes `Downloading` a state you can
/// look at rather than a flicker.
const SLOW_CHUNK: usize = 1 << 20;
const SLOW_TICK: Duration = Duration::from_millis(600);

/// A release body in the shape GitHub hands one over, and long enough to prove the changelog
/// panel scrolls rather than growing the card.
const NOTES: &str = "## What's new\n\n\
     - **Check for Updates** answers even when there is nothing to install\n\
     - The changelog is rendered here, from the release's own Markdown\n\
     - Charts got a Shape panel, and the trendline moved onto its own read\n\n\
     ## Fixed\n\n\
     - A `peek` guard resolved in a `match` head no longer panics the update press\n\
     - `COPY ... TO` refuses a target inside `.strata/`\n\
     - The data-sources tree virtualizes, so a lake with 4000 tables scrolls\n\n\
     ## Known issues\n\n\
     - The Postgres connector is read-only in this release\n\
     - A view over a remote relation needs a refresh after a server-side rename\n";

/// What the server is currently telling the app. An atomic rather than a lock because it is one
/// byte written by the prompt and read by the connections, and neither ever waits for the other.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Scenario {
    None,
    Offer,
    Page,
    Slow,
    Fail,
}

impl Scenario {
    fn parse(word: &str) -> Option<Scenario> {
        match word {
            "none" => Some(Scenario::None),
            "offer" => Some(Scenario::Offer),
            "page" => Some(Scenario::Page),
            "slow" => Some(Scenario::Slow),
            "fail" => Some(Scenario::Fail),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Scenario::None => "none",
            Scenario::Offer => "offer",
            Scenario::Page => "page",
            Scenario::Slow => "slow",
            Scenario::Fail => "fail",
        }
    }

    fn says(self) -> &'static str {
        match self {
            Scenario::None => "nothing newer; the app reports it is up to date",
            Scenario::Offer => "a release with an archive; the app offers to update",
            Scenario::Page => "a release with only a DMG; the app offers the release page",
            Scenario::Slow => "a release with an archive, served slowly",
            Scenario::Fail => "an error; the app reports the check failed",
        }
    }

    fn from_byte(byte: u8) -> Scenario {
        match byte {
            1 => Scenario::Offer,
            2 => Scenario::Page,
            3 => Scenario::Slow,
            4 => Scenario::Fail,
            _ => Scenario::None,
        }
    }

    fn as_byte(self) -> u8 {
        match self {
            Scenario::None => 0,
            Scenario::Offer => 1,
            Scenario::Page => 2,
            Scenario::Slow => 3,
            Scenario::Fail => 4,
        }
    }
}

fn main() {
    let archive = match build_archive() {
        Ok(archive) => archive,
        Err(why) => {
            eprintln!("could not build the update archive: {why}");
            return;
        }
    };
    let size = match fs::metadata(&archive) {
        Ok(meta) => meta.len(),
        Err(why) => {
            eprintln!("could not measure {}: {why}", archive.display());
            return;
        }
    };

    let listener = match TcpListener::bind(ADDR) {
        Ok(listener) => listener,
        Err(why) => {
            eprintln!("could not listen on {ADDR}: {why}");
            return;
        }
    };

    let scenario = Arc::new(AtomicU8::new(Scenario::Offer.as_byte()));
    println!("strata fake releases on http://{ADDR}");
    println!("archive: {} ({} MB)", archive.display(), size >> 20);
    println!();
    println!("point the app at it:");
    println!("  STRATA_UPDATE_ORIGIN=http://{ADDR} cargo run");
    println!();
    help();
    prompt(Scenario::Offer);

    let serving = Arc::clone(&scenario);
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let scenario = Arc::clone(&serving);
            let archive = archive.clone();
            thread::spawn(move || {
                let state = Scenario::from_byte(scenario.load(Ordering::Relaxed));
                if let Err(why) = serve(stream, state, &archive, size) {
                    eprintln!("  ! {why}");
                }
            });
        }
    });

    for line in BufReader::new(io::stdin()).lines() {
        let Ok(line) = line else { break };
        let word = line.trim();
        match word {
            "" => {}
            "quit" | "exit" => break,
            "help" | "?" => help(),
            _ => match Scenario::parse(word) {
                Some(picked) => {
                    scenario.store(picked.as_byte(), Ordering::Relaxed);
                    println!("  serving {}: {}", picked.name(), picked.says());
                    println!("  now press App > Check for Updates... in the app");
                }
                None => println!("  '{word}' is not a scenario; type help"),
            },
        }
        prompt(Scenario::from_byte(scenario.load(Ordering::Relaxed)));
    }
}

fn help() {
    println!("scenarios (type one and press App > Check for Updates... in the app):");
    for scenario in [
        Scenario::None,
        Scenario::Offer,
        Scenario::Page,
        Scenario::Slow,
        Scenario::Fail,
    ] {
        println!("  {:<6} {}", scenario.name(), scenario.says());
    }
    println!("  quit   stop the server");
}

fn prompt(scenario: Scenario) {
    print!("[{}] > ", scenario.name());
    let _ = io::stdout().flush();
}

/// Answer one request: the releases list, the archive, or a 404.
///
/// The paths are GitHub's own, because the app builds them from the same slug it always does —
/// this server only stands where `api.github.com` would.
fn serve(
    mut stream: TcpStream,
    scenario: Scenario,
    archive: &Path,
    size: u64,
) -> Result<(), String> {
    let mut head = [0u8; 2048];
    let read = stream
        .read(&mut head)
        .map_err(|e| format!("could not read the request: {e}"))?;
    let request = String::from_utf8_lossy(&head[..read]);
    let path = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/")
        .to_string();

    if path.starts_with("/repos/") {
        println!();
        println!("  -> releases list ({})", scenario.name());
        if scenario == Scenario::Fail {
            let body = "{\"message\":\"the release list is having a bad day\"}";
            write_head(
                &mut stream,
                "500 Internal Server Error",
                "application/json",
                body.len() as u64,
            )?;
            return stream
                .write_all(body.as_bytes())
                .map_err(|e| format!("could not write the answer: {e}"));
        }
        let body = releases(scenario, size);
        write_head(&mut stream, "200 OK", "application/json", body.len() as u64)?;
        stream
            .write_all(body.as_bytes())
            .map_err(|e| format!("could not write the answer: {e}"))?;
        prompt(scenario);
        return Ok(());
    }

    if path.starts_with("/download/") {
        println!();
        println!("  -> archive ({})", scenario.name());
        write_head(&mut stream, "200 OK", "application/zip", size)?;
        let sent = send_archive(&mut stream, archive, scenario);
        prompt(scenario);
        return sent;
    }

    let body = "not here";
    write_head(
        &mut stream,
        "404 Not Found",
        "text/plain",
        body.len() as u64,
    )?;
    stream
        .write_all(body.as_bytes())
        .map_err(|e| format!("could not write the answer: {e}"))
}

fn write_head(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    length: u64,
) -> Result<(), String> {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {length}\r\n\
         Connection: close\r\n\r\n"
    );
    stream
        .write_all(head.as_bytes())
        .map_err(|e| format!("could not write the answer: {e}"))
}

/// The archive, whole or a chunk at a time.
///
/// `slow` is the only reason this is not one `write_all`: the app moves its progress every
/// megabyte, so a download that finishes in a blink never shows a `Downloading` state at all.
fn send_archive(stream: &mut TcpStream, archive: &Path, scenario: Scenario) -> Result<(), String> {
    let bytes = fs::read(archive).map_err(|e| format!("could not read the archive: {e}"))?;
    if scenario != Scenario::Slow {
        return stream
            .write_all(&bytes)
            .map_err(|e| format!("could not write the archive: {e}"));
    }
    for chunk in bytes.chunks(SLOW_CHUNK) {
        stream
            .write_all(chunk)
            .map_err(|e| format!("could not write the archive: {e}"))?;
        stream
            .flush()
            .map_err(|e| format!("could not write the archive: {e}"))?;
        thread::sleep(SLOW_TICK);
    }
    Ok(())
}

/// The release list, in GitHub's own shape — the fields `update::Release` reads and nothing
/// more, since anything else would be describing a parse that does not happen.
fn releases(scenario: Scenario, size: u64) -> String {
    if scenario == Scenario::None {
        return "[]".to_string();
    }
    let assets = match scenario {
        // A release whose only artifact is the first-install DMG. The app finds no `.app.zip`,
        // so the offer degrades to the release page rather than promising an install.
        Scenario::Page => serde_json::json!([{
            "name": format!("Strata-{VERSION}-universal.dmg"),
            "browser_download_url": format!("http://{ADDR}/download/Strata-{VERSION}-universal.dmg"),
            "size": size,
        }]),
        _ => serde_json::json!([{
            "name": format!("Strata-{VERSION}-universal.app.zip"),
            "browser_download_url": format!("http://{ADDR}/download/Strata-{VERSION}-universal.app.zip"),
            "size": size,
        }]),
    };
    let list = serde_json::json!([{
        "tag_name": format!("v{VERSION}"),
        "draft": false,
        "prerelease": true,
        "html_url": format!("http://{ADDR}/releases/tag/v{VERSION}"),
        "body": NOTES,
        "assets": assets,
    }]);
    list.to_string()
}

/// `len` bytes a deflater cannot shrink — an xorshift rather than a dependency, because the only
/// property wanted here is "does not compress", and the archive has to keep the size it declares
/// or there is no download to watch.
fn noise(len: usize) -> Vec<u8> {
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    let mut bytes = Vec::with_capacity(len);
    while bytes.len() < len {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        bytes.extend_from_slice(&state.to_le_bytes());
    }
    bytes.truncate(len);
    bytes
}

/// **Build the archive the app will really download and really unpack.**
///
/// A `Strata.app` with the two things the staging path looks for — an `.app` at the top level of
/// the zip, made by the same `ditto` that reads it — plus a padding file, because a download
/// nobody can see is not worth watching. It is written once at startup and served from disk.
fn build_archive() -> Result<PathBuf, String> {
    let root = env::temp_dir().join("strata-fake-releases");
    let app = root.join("Strata.app");
    let archive = root.join("Strata-update.zip");
    if archive.exists() {
        return Ok(archive);
    }

    let macos = app.join("Contents/MacOS");
    fs::create_dir_all(&macos).map_err(|e| format!("could not make {}: {e}", macos.display()))?;
    fs::write(
        app.join("Contents/Info.plist"),
        include_str!("fake_releases_info.plist"),
    )
    .map_err(|e| format!("could not write the plist: {e}"))?;
    fs::write(
        macos.join("strata"),
        "#!/bin/sh\necho 'this is not Strata'\n",
    )
    .map_err(|e| format!("could not write the executable: {e}"))?;
    fs::write(app.join("Contents/Resources.bin"), noise(PADDING))
        .map_err(|e| format!("could not write the padding: {e}"))?;

    let zipped = Command::new("/usr/bin/ditto")
        .arg("-c")
        .arg("-k")
        .arg("--sequesterRsrc")
        .arg("--keepParent")
        .arg(&app)
        .arg(&archive)
        .output()
        .map_err(|e| format!("could not run ditto: {e}"))?;
    if !zipped.status.success() {
        return Err(String::from_utf8_lossy(&zipped.stderr).trim().to_string());
    }
    Ok(archive)
}
