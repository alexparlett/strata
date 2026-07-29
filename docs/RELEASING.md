# Releasing Strata

How a build gets from the repo to a tester's Mac. Two entry points, one mechanism:
[`scripts/bundle-macos.sh`](../scripts/bundle-macos.sh) does the whole job, and
[`.github/workflows/release.yml`](../.github/workflows/release.yml) is that script run on a GitHub
runner with the signing secrets wired in. CI does not reimplement any of it, so a build you make
on your laptop and a build the Release workflow publishes differ only in what is configured, never
in what is done.

---

## From GitHub, on demand

**Actions → Release → Run workflow.** Four inputs:

| Input | Default | What it does |
|---|---|---|
| **Architectures** | `universal` | `universal` runs on both Apple Silicon and Intel. `arm64` is roughly half the build time when every tester is on Apple Silicon. |
| **Tag this commit and publish a release page** | off | Off: the DMG appears as an artifact on the run page and nothing about the repo changes. On: the run also creates the tag and a release page with the DMG attached. |
| **Version** | *(blank)* | Blank uses the version in `crates/strata-freya/Cargo.toml`. Set it only to override. |
| **Mark as prerelease** | on | Keeps tester builds out of the "Latest release" slot. |

With the release box **off**, nothing about the repository changes: download the artifact from the
run page and hand the DMG over however you like. Artifacts expire after 30 days. Note this is not
a privacy mechanism — the repo is public, so any signed-in GitHub user can reach the artifact. It
just keeps unfinished builds off the releases page.

With it **on**, the run publishes a release page after the build succeeds. The tag is created at
that point and not before — a published release's tag [cannot be moved or
deleted](https://docs.github.com/en/repositories/releasing-projects-on-github), so tagging ahead
of a build that then fails would leave a permanent tag pointing at a broken commit. The workflow
also checks up front that the tag and release do not already exist, because finding that out
after a two-hour build is a bad trade.

The release notes are written from what the run actually did: an unsigned build's page carries the
Gatekeeper instructions below, a notarized one says to just open it. GitHub's generated changelog
lands underneath.

### Cutting a version

1. Bump `version` in `crates/strata-freya/Cargo.toml`.
2. Commit and push to `main`.
3. Run the workflow with **Tag this commit and publish a release page** ticked.

The crate version is the only place a version number is written. The bundle script reads it for
`CFBundleShortVersionString` and the workflow reads it for the tag, so the three cannot disagree.

`CFBundleVersion` — the build number macOS compares to decide what is newer — is
`git rev-list --count HEAD`, which is why the workflow checks out at full depth.

### Tagging locally instead

Pushing a `v*` tag by hand triggers the same workflow and publishes the same release. This does not
double-fire when the workflow creates a tag itself: GitHub does not trigger workflows from pushes
made with `GITHUB_TOKEN`.

---

## From your laptop

```bash
./scripts/bundle-macos.sh
```

Universal `.app` and DMG in `target/dist/`. For a quick check on your own machine:

```bash
./scripts/bundle-macos.sh --arch arm64
```

`--no-dmg` stops at the `.app`. `--help` lists the lot.

The first universal build is slow — Skia and DataFusion compile once per architecture. Subsequent
builds reuse the cargo cache per target.

---

## Signing, and what testers see

Signing is graduated, and the script reports which rung it took rather than claiming success it
did not achieve. Right now this repo is on the first rung.

| Configured | What the script does | What a tester does |
|---|---|---|
| Nothing | Ad-hoc signature | Has to clear the quarantine flag (below) |
| Developer ID cert | Real signature + hardened runtime | Still has to clear it — Gatekeeper wants the Apple ticket, not just a signature |
| Developer ID + notary credentials | Signed, submitted to Apple, stapled | Double-clicks it |

Only the third rung actually gets you a clean install. A Developer ID certificate on its own is a
half-step that costs a tester exactly as much as no signature at all.

> **Note on the certificate you have.** The keychain on this machine holds an *Apple Development*
> certificate. That is the wrong kind — it signs, but Apple will not notarize a build signed with
> it, so it produces a signature that still fails on a tester's Mac while looking like success
> locally. The script deliberately does not fall back to it. Distribution needs a **Developer ID
> Application** certificate, which requires a paid Apple Developer Program membership.

### The bypass testers need while builds are unsigned

macOS quarantines anything downloaded from a browser, and refuses to open an unnotarized app with
a message about it being damaged or from an unidentified developer. It is neither. Clearing the
flag once is enough:

```bash
xattr -dr com.apple.quarantine /Applications/Strata.app
```

Right-click → Open also works on some macOS versions, but Apple has been narrowing that path, so
the command is the thing to tell testers.

### Turning notarization on

Nothing in the script or the workflow changes — both already take the notarized path when the
credentials exist. Add these as repository secrets (**Settings → Secrets and variables →
Actions**):

| Secret | What it is |
|---|---|
| `MACOS_CERTIFICATE` | Your Developer ID Application certificate exported as `.p12`, base64-encoded |
| `MACOS_CERTIFICATE_PASSWORD` | The password you set on that `.p12` export |
| `MACOS_SIGN_IDENTITY` | The identity string, e.g. `Developer ID Application: Your Name (TEAMID)` |
| `AC_API_KEY` | An App Store Connect API key (`.p8`), base64-encoded |
| `AC_API_KEY_ID` | That key's ID |
| `AC_API_ISSUER_ID` | The issuer ID it belongs to |

To base64 a file for pasting into a secret:

```bash
base64 -i Certificates.p12 | pbcopy
```

An App Store Connect API key (Users and Access → Integrations) is preferred over an Apple ID and
app-specific password: it has no 2FA to get stuck on and can be revoked without touching the
account. The script accepts either — set `AC_APPLE_ID`, `AC_PASSWORD` and `AC_TEAM_ID` instead if
you would rather use the Apple ID route locally.

Locally, the script finds a Developer ID certificate in your keychain on its own; export the
notary variables in your shell to notarize from your laptop too.

---

## What is in the bundle

The binary is self-contained. Themes are compiled in (`include_str!` in `strata-core::theme`), so
there is no resource directory to keep in step, and the two fonts the themes name — IBM Plex Sans
and JetBrains Mono, neither of which ships with macOS — are embedded via `LaunchConfig::with_font`
in `main.rs`. Before that, a tester's Mac fell back to the system UI font and the whole type scale
went with it.

The icon is generated during the build from `assets/icon/strata.png` rather than kept as a
committed `.icns`, so there is no second copy of the artwork to drift from the design.

`LSMinimumSystemVersion` is 11.0, matching the `MACOSX_DEPLOYMENT_TARGET` the script builds with —
the floor Apple Silicon has anyway.
