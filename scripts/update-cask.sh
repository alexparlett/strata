#!/usr/bin/env bash
#
# Write the Homebrew cask for a published Strata release into a checkout of the tap.
#
# The tap is a separate repository - Homebrew requires the name `homebrew-<tap>`, so the cask
# cannot live here. This script is the only thing that writes `Casks/strata.rb`, which is what
# keeps the cask's version, checksum, architecture and Gatekeeper note derived from the release
# they describe rather than typed in by hand after each one. The generated file says so at the
# top, because a cask edited in the tap is a cask the next release silently reverts.
#
#   ./scripts/update-cask.sh --tap ~/src/homebrew-strata                  # the version here
#   ./scripts/update-cask.sh --tap ~/src/homebrew-strata --tag v0.3.2     # a specific release
#   ./scripts/update-cask.sh --tap ... --dmg target/dist/Strata-0.3.2-universal.dmg
#   ./scripts/update-cask.sh --tap ... --commit                           # commit and push it
#
# Every fact in the cask comes from the DMG itself, not from what a run intended to build:
#
#   version + architecture   the asset's filename, checked against the tag
#   sha256                   shasum of the bytes Homebrew will download
#   Gatekeeper caveat        `spctl` on the file, so the note appears exactly while it is true
#
# That last one is why the DMG is fetched rather than trusting the checksum GitHub already
# publishes. An unnotarized build that installs silently and then refuses to open is the worst
# outcome a cask can have, and only the artifact can answer whether that is this build. The
# release workflow passes `--dmg` because it has the file already; a run from a laptop downloads
# it. Both paths do the same three reads.
#
set -euo pipefail

# Both paths this script takes are the caller's, so they are resolved against the directory the
# caller was standing in - not against the repo root it moves to.
CALLER_PWD="$PWD"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

abspath() {
  case "$1" in
    /*) printf '%s\n' "$1" ;;
    *) printf '%s\n' "$CALLER_PWD/$1" ;;
  esac
}

TOKEN="strata"

fail() {
  printf 'error: %s\n' "$1" >&2
  exit 1
}
step() { printf '\n==> %s\n' "$1" >&2; }
note() { printf '    %s\n' "$1" >&2; }

# ---------------------------------------------------------------------------------------------
# Arguments
# ---------------------------------------------------------------------------------------------

TAP=""
TAG=""
DMG=""
COMMIT=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    # Checked before the `shift 2`: with one argument left, `shift 2` returns non-zero and
    # `set -e` exits before the complaint can be printed. Same shape as scripts/version.sh.
    --tap)
      [[ -n "${2:-}" ]] || fail "--tap needs the path to a checkout of the tap repository"
      TAP="$2"
      shift 2
      ;;
    --tag)
      [[ -n "${2:-}" ]] || fail "--tag needs a release tag, e.g. v0.3.2"
      TAG="$2"
      shift 2
      ;;
    --dmg)
      [[ -n "${2:-}" ]] || fail "--dmg needs the path to a built DMG"
      DMG="$2"
      shift 2
      ;;
    --commit)
      COMMIT=1
      shift
      ;;
    -h | --help)
      awk 'NR>1 && /^#/ { sub(/^# ?/, ""); print; next } NR>1 { exit }' "${BASH_SOURCE[0]}"
      exit 0
      ;;
    *)
      fail "unknown argument '$1' (try --help)"
      ;;
  esac
done

[[ -n "$TAP" ]] || fail "--tap is required: the path to a checkout of the tap repository"
TAP="$(abspath "$TAP")"
if [[ -n "$DMG" ]]; then
  DMG="$(abspath "$DMG")"
fi
[[ -d "$TAP/.git" ]] || fail "'$TAP' is not a git checkout. Clone the tap first: gh repo clone alexparlett/homebrew-strata"

# The version follows the tag, and the tag defaults to what this checkout calls itself - the same
# number the Release workflow would build. version.sh is the only thing that knows where it lives.
if [[ -z "$TAG" ]]; then
  TAG="v$("$REPO_ROOT/scripts/version.sh")"
fi
VERSION="${TAG#v}"

# Where the release lives, read from the remote rather than written down twice. The cask's URL is
# a permanent public address, so it has to name the repository that actually publishes releases.
ORIGIN="$(git remote get-url origin 2>/dev/null)" ||
  fail "no 'origin' remote - the cask URL is built from it"
SLUG="$ORIGIN"
SLUG="${SLUG%.git}"
SLUG="${SLUG##*github.com[:/]}"
[[ "$SLUG" =~ ^[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$ ]] ||
  fail "could not read a GitHub owner/repo out of the origin remote '$ORIGIN'"

step "Cask for Strata $VERSION ($SLUG, tag $TAG)"

# ---------------------------------------------------------------------------------------------
# The artifact
# ---------------------------------------------------------------------------------------------

WORK=""
cleanup() {
  if [[ -n "$WORK" ]]; then
    rm -rf "$WORK"
  fi
}
trap cleanup EXIT

if [[ -n "$DMG" ]]; then
  [[ -f "$DMG" ]] || fail "no such file: $DMG"
  note "using $DMG"
else
  command -v gh >/dev/null 2>&1 ||
    fail "'gh' not found on PATH (needed to download the release DMG - or pass --dmg)"
  WORK="$(mktemp -d)"
  step "Downloading the $TAG DMG"
  gh release download "$TAG" --repo "$SLUG" --pattern '*.dmg' --dir "$WORK" >&2 ||
    fail "could not download a DMG from release $TAG"
  shopt -s nullglob
  FOUND=("$WORK"/*.dmg)
  shopt -u nullglob
  # One release, one DMG. Two would mean the same version was published for two architectures,
  # and a cask can only install one - so say which rather than picking.
  ((${#FOUND[@]} == 1)) ||
    fail "expected exactly one DMG on release $TAG, found ${#FOUND[@]}"
  DMG="${FOUND[0]}"
fi

ASSET="$(basename "$DMG")"
# `Strata-<version>-<arch>.dmg` is scripts/bundle-macos.sh's own naming, and the version in it is
# the manifest's. Reading both back is what stops a cask claiming a version the file it points at
# does not carry - the failure the release workflow's own version preflight exists to prevent,
# checked once more at the point the number becomes a public download URL.
[[ "$ASSET" =~ ^Strata-(.+)-([A-Za-z0-9_]+)\.dmg$ ]] ||
  fail "'$ASSET' is not named Strata-<version>-<arch>.dmg - nothing here can name it in a URL"
ASSET_VERSION="${BASH_REMATCH[1]}"
ARCH="${BASH_REMATCH[2]}"
[[ "$ASSET_VERSION" == "$VERSION" ]] ||
  fail "release $TAG attaches $ASSET, which is version $ASSET_VERSION, not $VERSION"

command -v shasum >/dev/null 2>&1 || fail "'shasum' not found on PATH"
SHA="$(shasum -a 256 "$DMG" | cut -d ' ' -f 1)"
note "sha256: $SHA"

# Whether the file carries an Apple notarization ticket, asked of the file rather than of this
# machine. `spctl -a` is the more obvious question - it is literally the assessment Gatekeeper
# makes when the DMG is opened - but it answers "accepted" for everything on a machine where
# assessments are switched off, and a CI runner is exactly the kind of machine nobody checks.
# A generator that quietly drops the caveat there ships a cask that installs an app which will
# not open. The stapled ticket is a property of the bytes, so every machine reads it the same.
xcrun --find stapler >/dev/null 2>&1 ||
  fail "'xcrun stapler' is not available - this script has to ask whether the DMG is notarized"
if xcrun stapler validate "$DMG" >/dev/null 2>&1; then
  NOTARIZED=1
  note "notarized: yes - the cask installs with no caveat"
else
  NOTARIZED=0
  note "notarized: no - the cask carries the quarantine note"
fi

# ---------------------------------------------------------------------------------------------
# The cask
# ---------------------------------------------------------------------------------------------

# A universal build runs anywhere, so it says nothing. A single-architecture one is refused on the
# other rather than downloaded and found not to run - `brew install` is where that is cheap.
ARCH_STANZA=""
case "$ARCH" in
  universal) ;;
  arm64) ARCH_STANZA=$'  depends_on arch: :arm64\n' ;;
  x86_64) ARCH_STANZA=$'  depends_on arch: :x86_64\n' ;;
  *) fail "'$ASSET' names an architecture this script has no cask stanza for: $ARCH" ;;
esac

mkdir -p "$TAP/Casks"
CASK="$TAP/Casks/$TOKEN.rb"

# An unquoted heredoc, so $VERSION and friends land - which means no backtick and no bare `$` may
# appear in the Ruby below. Ruby's own `#{...}` interpolation passes through untouched.
cat >"$CASK" <<RUBY
# Generated by scripts/update-cask.sh in $SLUG - do not edit here.
#
# Every release regenerates this file from the DMG it publishes, so an edit made in this
# repository lasts until the next one. Change the generator instead.
cask "$TOKEN" do
  version "$VERSION"
  sha256 "$SHA"

  url "https://github.com/$SLUG/releases/download/v#{version}/Strata-#{version}-$ARCH.dmg"
  name "Strata"
  desc "SQL query workspace for local parquet, CSV and JSON files"
  homepage "https://github.com/$SLUG"

  # Homebrew's default block for this strategy skips prereleases, and every Strata release so far
  # is marked as one - so the default would report no versions at all. Drafts are still skipped:
  # they have no downloadable asset behind them.
  livecheck do
    url :url
    regex(/^v?(\d+(?:\.\d+)+)\$/i)
    strategy :github_releases do |json, regex|
      json.map do |release|
        next if release["draft"]

        match = release["tag_name"]&.match(regex)
        next if match.nil?

        match[1]
      end
    end
  end

  # Strata updates itself: the in-app updater swaps the bundle in place, so brew upgrade would
  # otherwise reinstall over a newer app it has no way to see. Passing --greedy still upgrades it.
  auto_updates true
$ARCH_STANZA  # The floor LSMinimumSystemVersion names, and a bare symbol is the minimum: Homebrew
  # rewrites the ">= :big_sur" spelling to this one.
  depends_on macos: :big_sur

  app "Strata.app"

  # Projects hold their own state in a .strata directory beside the data, and are deliberately
  # left alone. This is the app-level state: settings, the model listings cache and user themes.
  # API keys live in the login keychain under com.alexparlett.strata, which zap cannot reach.
  zap trash: "~/Library/Application Support/Strata"
RUBY

# Only while it is true, which is why it is appended rather than templated: a caveat about an
# unsigned build left on a notarized one teaches testers to ignore caveats. The wording is the
# release page's, so somebody who reads both is not left comparing two instructions.
#
# A quoted heredoc, unlike the one above: nothing here is substituted, and `#{appdir}` is Ruby's
# to interpolate at install time. `caveats` comes last, after `zap` - Homebrew's stanza order.
if ((NOTARIZED == 0)); then
  cat >>"$CASK" <<'RUBY'

  caveats <<~EOS
    This build is not signed with an Apple Developer ID and is not notarized, so
    macOS will say it is damaged or from an unidentified developer. It is neither.
    Clear the download quarantine flag once:

      xattr -dr com.apple.quarantine "#{appdir}/Strata.app"

    Installing with --no-quarantine skips the step instead:

      brew reinstall --cask --no-quarantine strata
  EOS
RUBY
fi

printf 'end\n' >>"$CASK"

note "wrote $CASK"

# ---------------------------------------------------------------------------------------------
# Publishing it
# ---------------------------------------------------------------------------------------------

if ((COMMIT == 0)); then
  step "Not committed"
  note "review it, then commit and push from $TAP"
  exit 0
fi

step "Committing to the tap"
git -C "$TAP" add "Casks/$TOKEN.rb"
if git -C "$TAP" diff --cached --quiet; then
  note "the cask already describes $VERSION - nothing to commit"
  exit 0
fi
git -C "$TAP" commit -q -m "Strata $VERSION"
git -C "$TAP" --no-pager log -1 --oneline >&2
git -C "$TAP" push >&2
note "pushed"
