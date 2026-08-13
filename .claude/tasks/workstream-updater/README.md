# Workstream: Updater (UP)

In-app update capability: the app learns a newer release exists, offers it, downloads and
verifies it in the background, and installs it on a quit-shaped restart. Three tasks, strictly
ordered — the release pipeline grows the artifact first, the mechanism consumes it, the surfaces
sit on the mechanism.

## Decisions already made (do not re-litigate; the reasoning is recorded here)

- **Hand-rolled against the GitHub Releases API, not Sparkle.** Sparkle is an Objective-C
  framework with its own Cocoa UI (nothing like the app's dress), an appcast feed to generate,
  and a framework to embed and sign. Every piece it would buy — version check, download,
  verification, swap-and-relaunch — is small, and the release pipeline already exists. The Rust
  updater crates (`self_update`, the Tauri/cargo-packager ones) are binary- or
  framework-oriented and do not handle `.app` bundles.
- **Verification is Apple's chain, not a custom key.** Releases are Developer ID signed and
  notarized (configured 2026-08-12). The updater verifies the *staged* bundle — strict codesign
  verify, TeamIdentifier against a compiled-in constant, bundle id against `APP_ID` — and fails
  closed. An earlier plan to ed25519-sign the archive with a key in CI is superseded: Apple
  manages this PKI, and a second key would be a second thing to lose.
- **Verification is ours to do at all.** The quarantine xattr is opt-in (browsers set it); a
  file the app downloads itself never gets one, so Gatekeeper never assesses the swapped-in
  bundle. There is no system check behind ours.
- **The network is untrusted by construction.** TLS (with a redirect policy that refuses to
  leave `https`) covers the transport, but authenticity rests on the content layer: nothing
  installs without the Developer ID signature chaining to `TEAM_ID`, so a MITM — or a
  compromised CDN — can at worst withhold an update or deny service, never substitute one. The
  offer requires a strictly newer semver, so a replayed listing cannot downgrade. The residual
  trust root is the signing key itself, which lives in the repo's Actions secrets: protecting
  the GitHub account (and rotating that `.p12` if the repo is ever compromised) *is* the
  updater's security boundary.
- **The update artifact is a `ditto` zip beside the DMG.** The DMG stays the first-install
  artifact (the drag-to-install gesture); the zip is the programmatic one — `ditto` is the only
  archiver that round-trips a signed bundle's symlinks and extended attributes, and it is what
  the notarize path already uses.
- **Install is quit-shaped and happens after the event loop ends.** Never mutate the live
  bundle mid-run. The press stages intent, the normal `quit()` runs (every close confirm keeps
  its say, the open-set persists exactly as on any quit), and the swap + relaunch happen once
  no window exists. A cancelled quit keeps the staged update and loses nothing.
- **Prereleases are offered.** Testers live on prereleases (the Release workflow defaults to
  marking them). A stable/prerelease channel setting is a possible follow-up, not part of this.
- **The updater is inert outside a bundle.** A `cargo run` dev build is not an installation;
  it neither nags nor offers.
- **Check at startup + on demand, no timer.** AGENTS.md §2: poll only what nothing on our side
  can observe, and name the reason at the poll. One check per process launch (gated by a
  setting), plus a manual command. A long-running app not learning of a release until relaunch
  is accepted; a periodic re-check with a stated staleness bound (the `models::STALE_AFTER`
  shape) is the follow-up if that ever bites.

## Tasks

| # | Task | Status | Depends on |
|---|---|---|---|
| UP-01 | Release-side: update archive + team identity | ✅ | — |
| UP-02 | Check / download / verify / install mechanism | ✅ | UP-01 |
| UP-03 | Surfaces: launcher affordance, dialog, setting, menubar item | ✅ | UP-02 |

Order is the dependency chain. v0.3.1 (2026-08-12) is the published release carrying the zip, and
UP-02 was verified end to end against it — download, `ditto` unpack, and all three signature
checks, plus a stapled-ticket and `spctl` confirmation.

**Two corrections worth carrying forward.** `Settings::check_updates` was added by UP-02, not
UP-03: the mechanism's startup check is gated on it, and a gate with no way to be off is not a
gate. UP-03 owned the Settings **row** and the search-index entry, and shipped both.

And the workstream **traded one surface for another**: App ▸ *Check for Updates…* on the menubar
was asked for while UP-03 was in flight, and the palette row UP-03 had planned was cut once it
landed — two places that talk about updates are enough, and a third is one more to keep in step.
The menubar item is the reason the offer became one pure decision (`updater::Affordance`) rather
than a condition each surface restates, the reason the confirm's slot sits on the project
*window* rather than in its subtree, and — because the menu handler runs outside Freya's context
— the reason a press there is recorded rather than performed. See the UP-03 file.
