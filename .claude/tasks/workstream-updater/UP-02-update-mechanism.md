# UP-02 · Check / download / verify / install mechanism

**Workstream:** Updater · **Status:** ✅ (built 2026-08-12, verified end to end against the
published v0.3.1) · **Depends on:** UP-01 (the `.app.zip` asset + `TEAM_ID`)

## Goal
The whole mechanism, window-free: ask GitHub what the newest release is, download its update
archive in the background, verify it against Apple's chain, stage it, and install it on a
quit-shaped restart. UP-03 puts surfaces on this; nothing here paints.

## Current state (verified 2026-08-12)
- No updater code exists (`auto-update|check for updates|self_update` greps clean).
- The app knows its version as `env!("CARGO_PKG_VERSION")` — used once, in the launcher rail
  (`apps/launcher/views/rail.rs:68`). **The frontend crate's version is the number**: it is what
  `scripts/version.sh` bumps and what the tag/DMG/plist are all derived from. `strata-core` is
  independently versioned, so a check compiled into core must take the version as an argument,
  never read its own `env!`.
- The architectural rule for network code is stated at
  `strata-agent/src/assistant/provider.rs:775-777`: reqwest and a Tokio runtime stay out of
  `strata-freya` entirely. The shape to copy is `list_models_blocking` (`provider.rs:780-793`) —
  a blocking fn owning a current-thread runtime and a one-off client — called from the frontend
  inside `task::offload` (the listings refresh does exactly this, `state/listings.rs:272,317`).
- The app-global-flag precedent is `Probes` (`state/listings.rs:167-181`): one
  `State::create_global` slot, created in `main` (`main.rs:121`), carried on `AppCtx`
  (`state/mod.rs:48-81`), deliberately **not persisted**. Its doc argues the exact case: the
  guarded thing is app-global, so per-window copies are two bugs.
- The quit path: `platform/windows.rs:453` `quit()` → `begin_quit()` + close every window;
  `begin_quit`/`end_quit`/`is_quitting` at `windows.rs:212-231`. **`end_quit` must be called on
  every path that dismisses a close confirm or the flag latches** — the update flow inherits
  that obligation. Open-set persistence on quit is `state/config.rs:142-165`.
- `main` ends at `launch(launch_config);` with nothing after it (`main.rs:180`). Whether the
  fork's `launch` **returns** when the event loop ends is unverified (winit 0.30's `run_app`
  does return on macOS unless something exits the process first) — settle it from the fork
  source before choosing the install shape below.

## Build

**`strata_core::update`** — a satellite module beside `models.rs`, blocking API in
`list_models_blocking`'s shape. Dependencies: `reqwest = "0.12"` (already compiled — `object_store`
pins 0.12.28; match the features the lockfile already resolves rather than enabling a second TLS
backend; `strata-agent` carries 0.13 because `genai` does, and the two coexist)
and `semver = "1"` (in the lock via `rustc_version`; new direct declaration, no new download).
Declare both in `strata-core`'s own manifest — the workspace root deliberately declares only
the fork crates.

1. **Check** — `check_blocking(current: &str) -> Result<Check, String>`.
   `GET https://api.github.com/repos/alexparlett/strata/releases?per_page=10` (the **list**, not
   `/latest`, which excludes prereleases; slug a named const beside the fn; GitHub requires a
   `User-Agent`; state a timeout). The client for both the check and the download carries a
   **custom redirect policy refusing any non-`https` hop** — reqwest's default follows an
   `https`→`http` redirect, and while the codesign layer would still catch a tampered payload,
   there is no reason to ever leave TLS (the asset download legitimately redirects to
   `objects.githubusercontent.com`, which is `https`). Skip drafts, keep prereleases, strip the
   tag's leading `v`, parse with `semver`, pick the newest — and offer it **only if strictly
   newer** than `current`, so a replayed or forged listing can never downgrade a running app. Answer: up to date, or newer — with version, the
   release page URL, and the asset whose name ends `.app.zip` (UP-01's contract). A newer
   release **without** that asset still reports the page URL: the offer degrades to "open the
   release page", never errors. Unparseable tags are skipped, not fatal — one odd tag must not
   blind the updater.
2. **Download + verify + stage** — `download_blocking(asset, on_progress) -> Result<PathBuf, String>`.
   Stream to a fresh staging directory under `std::env::temp_dir()`, extract with
   `/usr/bin/ditto -x -k` (`Command`; the tool that made the archive — a Rust unzip that drops
   xattrs or symlinks produces a bundle that fails verification), then verify **all three,
   fail closed**:
   - `codesign --verify --deep --strict <staged.app>` exits zero;
   - `codesign -dvv` reports `TeamIdentifier=` equal to `secret::TEAM_ID` (an ad-hoc signature
     has none and refuses here — a dev build can never be offered as an update);
   - the staged `Info.plist`'s `CFBundleIdentifier` equals `secret::APP_ID`.
   The verified bundle path is the answer. Verification failure deletes the staging dir.
3. **Install eligibility** — a helper answering *where* the running bundle is: walk
   `current_exe()` ancestors to the first `.app`. No `.app` (a `cargo run` build) → the updater
   is inert: no startup check, no offer. Bundle present but its parent directory unwritable →
   the check still runs; the offer degrades to opening the release page.
4. **App-side state** — `strata-freya/src/state/updates.rs`, the `Probes` pattern:
   `pub type UpdateStatus = State<Update>` with
   `Idle | Checking | UpToDate | Available { version, asset, page_url } | Downloading |
   Ready { staged: PathBuf } | Failed { why }`, `create_global_updates()` in `main`, field on
   `AppCtx`. Not persisted — a check result is a fact about a request made minutes ago. Actions
   (check, download) run the blocking fns via `task::offload` and write the slot; the startup
   check runs **once per process** (guard it the way listings guards its one refresh), gated on
   the UP-03 setting, and only when eligible per (3).
5. **Install** — the press records the staged path in a **process-global** slot (a
   `static OnceLock`/`Mutex<Option<PathBuf>>` — it must outlive every window and every scope)
   and calls `platform::windows::quit()`. The normal quit machinery runs: every close confirm
   keeps its say, the open-set persists, and **a cancelled quit (`end_quit`) clears the intent
   slot and puts the status back to `Ready`** — staged, reusable, nothing lost.
   Then the swap, **after the event loop ends**:
   - *Preferred shape (verify first — see Current state):* if the fork's `launch` returns after
     quit, `main` grows a tail: read the intent slot; if set — copy the staged bundle to a
     `.staged-…` sibling of the target (same directory ⇒ same volume, so the next step is a
     rename), rename the current bundle aside to `.old-…`, rename the staged one in, best-effort
     delete `.old-…`, spawn the new app (`open -n <path>` or `Command::spawn` the binary
     detached), return. If the rename-in fails, rename the old one back — the failure is a
     logged degrade to "open the release page", never a half-installed app.
   - *Fallback:* if `launch` does not return and making it return is disproportionate, a
     detached helper (`/bin/sh -c 'while kill -0 <pid> …; do sleep …; done; <swap>; open …'`)
     spawned at the press and **killed on a cancelled quit**. Prefer fixing the fork
     (AGENTS.md §6) — the helper is a second copy of the swap logic living in a string.

**Testing.** The pure parts — release-JSON parse, newest-pick, asset selection, tag skipping —
are unit tests over inline fixture JSON in `update.rs`. Do not mock HTTP and do not bend the
signatures for tests (AGENTS.md §1); codesign verification against a real release is the manual
Mac check, and needs UP-01's release published.

## Acceptance
- [x] `strata_core::update`: check/download/verify as blocking fns; reqwest/tokio nowhere in
      `strata-freya`'s manifest. *(`reqwest 0.12` + `semver 1` declared on `strata-core` only,
      matching the features `object_store` already resolves so no second TLS backend is built.)*
- [x] The check offers prereleases, skips drafts and unparseable tags, answers "page only"
      when the zip asset is missing, and never offers a version ≤ the running one. *(Unit tests
      over inline fixture JSON in `update.rs`, plus a live run against the real release list.)*
- [x] No request leaves `https`: the redirect policy refuses a scheme downgrade. *(One custom
      policy on both requests, which also bounds the hops.)*
- [x] Verification fails closed on: bad signature, wrong team, wrong bundle id, ad-hoc build —
      each with its own message. *(Two of the four exercised against **real** bundles built from
      the published archive: an ad-hoc re-sign is refused as "carries no Apple team signature",
      and a file added inside a verified bundle is refused with codesign's own "a sealed resource
      is missing or invalid". The other two are unreachable without a second signing identity.)*
- [x] A `cargo run` build neither checks nor offers; an unwritable install location degrades to
      the release page. *(`site()` answered `Unbundled` under `cargo run` and the startup check
      stood down; writability is answered by writing a probe file, since a directory's mode bits
      do not settle it on macOS. The **degrade** itself is UP-03's — `install_site()` is the
      handle its affordance reads.)*
- [x] Install runs only after the event loop ends; a cancelled quit keeps the staged update and
      `end_quit` runs; a failed swap rolls back and the old app still launches. *(The tail in
      `main` was confirmed to run on a real ⌘Q — `launch` returns, so the **preferred shape**
      below was taken and no helper script exists. The swap and its cleanup are unit-tested
      including the failure arm. The press that records the intent is UP-03's dialog, so the
      cancelled-quit path is wired but not yet pressable — see the note below.)*
- [x] Startup check runs once per process, only when the setting is on.

## What was built, and where it differs from the plan

- **`Settings::check_updates` landed here, not in UP-03.** The acceptance box above asks the
  startup check to be gated on a setting, and a gate with no way to be off is not a gate. The
  **field** (`config.rs`, its `Default`, the `settings_merge!` list) is the mechanism's gate and
  is in this change; the **row** in Settings ▸ System and the `settings_index!` entry are
  untouched and remain UP-03 step 1. Default `true`, `#[serde(default)]`, so existing config
  files are unaffected.
- **`launch` returns, so `main` grew a tail.** Settled from source first (winit 0.30's `run_app`
  is `run_on_demand` on macOS, and Freya's renderer calls `event_loop.exit()` rather than ending
  the process — only iOS and Web are documented as never returning), then confirmed empirically:
  a ⌘Q logs `the event loop has ended` from `install_pending`. The fallback helper script in step
  5 was therefore **not** built and should not be.
- **A worker outlives the window that started it**, which the plan's "run via `task::offload` and
  write the slot" does not by itself handle: a task is bound to its window's root scope, and the
  launcher closes the moment a project opens, so a download settling after that would strand the
  status on `Downloading` and orphan a verified 111 MB bundle. The worker therefore parks its
  settled status in a process-global and `use_updates` adopts it on every mount. Nothing polls —
  there is always a workspace window, and its mount is the second wake.
- **A `Ready` status is not re-checked.** A manual check that found something newer would leave a
  verified bundle nobody asked to discard.
- **Progress is throttled on the worker** (1 MB steps). A write per network chunk is a repaint per
  network chunk, which is thousands of them for this archive.
- **The staging layout is a contract**: `<temp>/strata-update-<uuid>/<Name>.app`, so a staged
  bundle's parent *is* its staging folder and `discard` needs no second value. It refuses to
  delete a folder that is not one of ours.
- **`download` and `install` carry an `#[allow(dead_code)]` with the reason**: their presses are
  UP-03's launcher affordance, palette command and confirm dialog (AGENTS.md §5). Removing those
  two allows is part of UP-03.

## Known gap (not this task's to fix)

A **Dock-icon Quit** sends Cocoa's `terminate:` un-vetoed (winit 0.30 exposes no
`applicationShouldTerminate`; noted in `main.rs` and owned by P6-02), so it ends the process
before `main`'s tail. That cannot be reached from the install press, which goes through
`platform::quit()` — but a user who presses Restart, gets a close confirm, and then quits from
the Dock loses the install rather than performing it. The staged bundle is still on disk and the
press can be made again after relaunch.

## References
- `strata-agent/src/assistant/provider.rs:736-793` — the fetch shape to copy.
- `state/listings.rs` — satellite orchestration; `Probes` (`:167-181`) for the global slot.
- `platform/windows.rs:212-231,453-467` — quit semantics the install must ride.
- `.claude/tasks/workstream-updater/README.md` — the settled decisions (quarantine, ditto,
  quit-shaped install).
