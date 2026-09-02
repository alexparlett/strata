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
| **Tag this commit and publish a release page** | off | Off: the build's artifacts appear on the run page and nothing about the repo changes. On: the run also creates the tag and a release page with them attached. |
| **How the release notes are written** | `claude` | `claude` summarises the commits since the last release into a *What's new* section. `generated` uses GitHub's changelog on its own. |
| **Mark as prerelease** | on | Keeps tester builds out of the "Latest release" slot. |

There is no version input, and no bump. The run builds whatever version the manifest already says,
which is what [cutting a version](#cutting-a-version) put there.

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

### Cutting a version

Two steps, because `main` takes changes only through a reviewed pull request — the version bump
included.

```bash
./scripts/release-pr.sh minor
```

That branches off wherever you are, writes the new version into the workspace `Cargo.toml` and
`Cargo.lock` through `scripts/version.sh`, commits those two files and nothing else, pushes the
branch and opens a **Release 0.4.0** pull request. It takes an exact version as well as a bump, and
`--no-pr` stops after the push. Everything it can refuse, it refuses before it has done anything: a
number that is already tagged, a branch that already exists, an uncommitted edit to either of the
files it is about to commit.

Merge it once CI is green. Then **Actions → Release**, tick **Tag this commit and publish a release
page**, and run it. There is no version to type in: the run reads the manifest, so what it builds is
what was merged.

The order is the part worth knowing, because each half of it fixes something:

- The version reaches `main` **before the build**, in a commit that has passed the required checks —
  and the release run then writes nothing at all. It used to bump and push that commit itself, at
  the end of the build. The ruleset on `main` refuses such a push, and no amount of retrying would
  help: a commit pushed with `GITHUB_TOKEN` starts no workflow run, so the checks the rule requires
  could never report on it. A version bump is a change to the repository and goes through a pull
  request like every other one.
- The DMG's filename, `CFBundleShortVersionString` and the tag are one number, because all three are
  the manifest's. When the workflow took a version as an input instead, `v0.4.0` shipped a DMG
  called `Strata-0.2.0-universal.dmg` — the bundle script reads the manifest, and the manifest had
  not moved.
- The tag is created **after the build**, so a build that fails leaves no permanent tag behind, and
  it names the commit that was actually built rather than the branch. If `main` moved while the
  build ran, the release still describes the tree the DMG came from.

Forgetting the bump costs seconds rather than a build: the manifest still reads a version that is
already released, and the run's first step refuses it by name. The opposite outcome — a merged bump
with no release behind it, because the build failed — needs no recovery at all. Fix what broke and
run Release again; it will build the same number, which nothing has claimed yet.

The root `Cargo.toml`'s `[workspace.package] version` is the only place a version number is
written, and [`scripts/version.sh`](../scripts/version.sh) is the only thing that knows that. Every
crate inherits it with `version.workspace = true`, so one command moves all eight — they ship as one
application, and a number per crate would be eight chances for the DMG, the plist and the tag to
disagree. The bundle script, the release script above and the workflow all read the version through
that one script, and a bump is a command rather than an edit plus a `sed` buried in a YAML file:

```bash
./scripts/version.sh                  # what a build here would call itself
./scripts/version.sh --resolve minor  # what a minor bump would produce, changing nothing
./scripts/version.sh --bump minor     # write it, Cargo.lock included
```

The lockfile matters: it records every member's version and the release build passes `--locked`,
so a manifest bumped on its own fails the build.

`CFBundleVersion` — the build number macOS compares to decide what is newer — is
`git rev-list --count HEAD`, which is why the workflow checks out at full depth.

### Release notes

The release page is three things in this order: the install instructions, a **What's new** section,
and GitHub's generated changelog.

The install instructions are written from what the run actually did — an unsigned build carries the
Gatekeeper command below, a notarized one says to just open it. They stay at the top on purpose: on
an unsigned build, "why will it not open" is the tester's first question and "what changed" is the
second.

*What's new* is written by [`anthropics/claude-code-action`](https://github.com/anthropics/claude-code-action)
from the `git log` between the last reachable `v*` tag and the commit being released, aimed at
testers rather than contributors. It needs the `CLAUDE_CODE_OAUTH_TOKEN` secret — the same one the
review workflow uses. Set **How the release notes are written** to `generated` to skip it.

It degrades the way signing does. The step is `continue-on-error`, so a missing secret, a failed
step or an empty result all still publish the release with GitHub's changelog, and the run says
which happened. Better notes are a better release page, not a precondition for having one.

### Tagging locally instead

Pushing a `v*` tag by hand triggers the same workflow and publishes the same release. This does not
double-fire when the workflow creates a tag itself: GitHub does not trigger workflows from pushes
made with `GITHUB_TOKEN`.

A tag push carries its own version, and the run checks it against the manifest at the commit the
tag points at. They have to agree: the DMG's filename and its `Info.plist` are read out of the
manifest, so a tag on a commit that calls itself something else would publish `v0.4.0` with
`Strata-0.3.2-universal.dmg` attached. Tag the commit the release PR merged, not the one before it.

---

## What a build produces

Three things in `target/dist/`, whichever entry point built them:

| File | What it is for |
|---|---|
| `Strata.app` | The bundle the other two are made from. Signed, and stapled if the build notarized. |
| `Strata-<version>-<arch>.dmg` | The **first install**. It carries the drag-to-Applications gesture, which is the thing a person does once. |
| `Strata-<version>-<arch>.app.zip` | The **update**. What the in-app updater downloads, verifies and swaps in. |

The zip is a `ditto` archive, which is the only kind that round-trips a signed bundle's symlinks
and extended attributes — a `zip` of the same bundle fails the strict signature check the updater
runs before it installs anything. It is written after stapling, so the archived bundle carries the
notarization ticket and validates with no network.

Its `.app.zip` suffix is a contract rather than a convention: it is what the updater's asset
selection keys on, so a release page carries exactly one asset ending in it. The version in the
filename is informative — the updater compares the release **tag**, which is the authoritative
number, and reads nothing out of the asset's name beyond that suffix.

Both are attached to a published release, and both are uploaded as run artifacts when the release
box is off. An unsigned build produces the same three files: the artifact set does not change shape
with the signing rung, only what the files are worth does.

---

## Homebrew

```bash
brew tap alexparlett/strata
brew trust alexparlett/strata
brew install --cask strata
```

That installs the DMG the release page carries — the same file, from the same URL, checked against
the same bytes.

Three commands rather than one, and both of the extra two are Homebrew's rules rather than ours.
`brew install --cask alexparlett/strata/strata` no longer taps on the user's behalf ("If you trust
this tap, tap it explicitly"), and since `HOMEBREW_REQUIRE_TAP_TRUST` became the default, a cask in
a third-party tap is refused until `brew trust` has been given for the tap or the cask. Neither can
be worked around from this side, so the install instructions say all three.

The cask lives in **[alexparlett/homebrew-strata](https://github.com/alexparlett/homebrew-strata)**,
a repository of its own: Homebrew resolves a tap by the name `homebrew-<tap>`, and this repo is not
called that. Nothing else is in it.

Nobody edits it either. [`scripts/update-cask.sh`](../scripts/update-cask.sh) generates
`Casks/strata.rb` from the DMG a release publishes, and the Release workflow runs it in the step
after the publish. Every line of the cask is read off the artifact rather than off what the run
meant to build:

| In the cask | Read from |
|---|---|
| `version` | the release tag, checked against the version in the DMG's filename |
| `sha256` | `shasum` of the file Homebrew will download |
| `depends_on arch:` | the architecture in that filename — absent for a universal build |
| `caveats` | whether the DMG carries an Apple notarization ticket |

That last row is why the script fetches the artifact rather than trusting the checksum GitHub
already publishes for it. A cask that installs quietly and leaves you with an app macOS refuses to
open is the worst outcome available here, so the quarantine instructions appear exactly while they
are true, and the question is asked of the file (`stapler validate`) rather than of the machine
generating the cask — `spctl` accepts everything on a host where Gatekeeper assessments are off,
which a CI runner may well be.

The cask also declares `auto_updates`, because Strata's in-app updater swaps the bundle in place:
without it `brew upgrade` would reinstall over a newer app it has no way to see. `--greedy` still
upgrades it.

### What checks the result

The tap runs `brew style` and `brew audit --cask --online` on every push, which is where a bad
generation is caught — and the only place `brew audit` can run at all, since it refuses to start on
a Mac whose Command Line Tools are out of date.

One audit check is skipped by name: `github_prerelease_version`, because Strata's releases are
deliberately prereleases (tester builds, kept out of the "Latest release" slot) and Homebrew holds
that a cask should not point at one. It is a whole check rather than a warning, so skipping it by
name is what keeps the URL and homepage checks running. It stops being a skip the day the
**Mark as prerelease** input starts defaulting to off.

### The token, and doing it by hand

The workflow step needs one secret, `HOMEBREW_TAP_TOKEN` — a fine-grained PAT with **Contents:
read and write** on `alexparlett/homebrew-strata` and nothing else. This repo's `GITHUB_TOKEN`
cannot write to another repository, which is the whole reason it exists.

Without the secret the release publishes normally and the run posts a notice saying the cask was
not updated. A token that is present and does not work fails the step: the release is already out,
and a cask silently stuck a version behind is worse than a red run. Either way the recovery is the
same script:

```bash
gh repo clone alexparlett/homebrew-strata
./scripts/update-cask.sh --tap homebrew-strata --tag v0.3.2 --commit
```

`--tag` defaults to the version this checkout calls itself, so the tag is only worth passing for an
older release. Without `--commit` the file is written and the commit is left to you. `--dmg` takes
a DMG that is already on disk instead of downloading one, which is what the workflow passes.

---

## From your laptop

```bash
./scripts/bundle-macos.sh
```

Universal, into `target/dist/`. For a quick check on your own machine:

```bash
./scripts/bundle-macos.sh --arch arm64
```

`--no-dmg` stops after the update archive. `--help` lists the lot.

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

There is a second cost to the ad-hoc rung, and it grows once the app stores a secret. macOS grants
keychain access against the *designated requirement* recorded on the item, and an ad-hoc signature
has no stable anchor to record — the requirement pins to the binary's own hash, so every ad-hoc
build is a different application as far as the Keychain is concerned, and a tester is asked to
allow access again after each update. A Developer ID signature anchors on the bundle identifier
plus the team certificate, which stays the same across versions. That identifier is
`strata_core::secret::APP_ID`, read out of the Rust source by the bundle script for exactly this
reason.

`strata_core::secret::TEAM_ID` sits beside it and is read the same way. It is the team the in-app
updater requires a downloaded bundle to be signed by, so a signed build whose signature names a
different team is a **hard failure** — the message names both. An app signed by a team its own
updater refuses is a release that can never update itself, and the build is the only place that
can notice: the signature is valid, notarization succeeds, and nothing goes wrong until a tester
one version later gets no update at all.

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
notary variables in your shell to notarize from your laptop too. One name differs from the
secrets table: locally the script wants `AC_API_KEY_PATH` — the path to the `.p8` file — where
the workflow secret `AC_API_KEY` holds the base64-encoded contents (the workflow writes the file
and does the translation).

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
