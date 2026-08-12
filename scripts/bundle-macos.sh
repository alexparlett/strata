#!/usr/bin/env bash
#
# Build Strata into a macOS .app, a DMG you can hand to a tester, and the zip the in-app updater
# installs from.
#
# This is the whole pipeline. CI calls this script rather than reimplementing it in YAML, so
# `./scripts/bundle-macos.sh` on a laptop and a tagged release build produce the same artifact by
# construction — a release path that only exists in a workflow file is one nobody can debug.
#
#   ./scripts/bundle-macos.sh                    # universal, for testers
#   ./scripts/bundle-macos.sh --arch arm64       # this Mac only, for a quick local check
#   ./scripts/bundle-macos.sh --no-dmg           # stop after the update archive
#
# Signing is graduated, and each rung is what the machine can actually honour:
#
#   nothing configured  -> ad-hoc signature. The app runs, but Gatekeeper quarantines it after a
#                          download, so testers need the bypass documented in docs/RELEASING.md.
#   Developer ID        -> real signature + hardened runtime, ready to notarize.
#   + notary creds      -> submitted to Apple, stapled. Testers just double-click.
#
# The rungs are detected, never assumed: the script reports which one it took, because "signed"
# and "notarized" are the difference between a tester opening the app and a tester filing a bug
# about a broken download.
#
set -euo pipefail

# ---------------------------------------------------------------------------------------------
# Identity
# ---------------------------------------------------------------------------------------------

APP_NAME="Strata"
# The bundle id is the app's permanent identity to macOS - it keys the quarantine record, the
# window-restore state and (once notarized) the Apple ticket. Changing it later orphans all of
# that.
#
# It is **read out of the Rust source**, because the app needs the same string at runtime: it is
# the keystore service every Strata credential is filed under (`strata_core::secret::APP_ID`,
# AS-05), and Keychain access is scoped by the signature of the bundle claiming that identity. A
# copy here that drifted from the constant would put the app's own keys under a name the bundle
# does not claim, which is a class of bug nobody would look for. There is no environment override
# for the same reason - one identity, one place, and the app can see it. (The read itself is in
# the Version section below, which is where `fail` exists to complain with.)
BUNDLE_ID_SRC="crates/strata-core/src/secret.rs"
# The Rust bin, which is not what the bundle is called. `CFBundleExecutable` below is the one
# place the two names have to agree.
CARGO_BIN="strata-freya"
# 11.0 is the floor Apple Silicon has anyway, so naming it costs the x86_64 half nothing real and
# buys a build that fails at compile time instead of on a tester's older Mac.
export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-11.0}"

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

# ---------------------------------------------------------------------------------------------
# Arguments
# ---------------------------------------------------------------------------------------------

ARCH_MODE="universal"
MAKE_DMG=1

while [[ $# -gt 0 ]]; do
  case "$1" in
    --arch)
      # Checked before the `shift 2`, not after: shifting two off one remaining argument returns
      # non-zero, and under `set -e` that exits the script before any complaint can be printed - so
      # `--arch` with no value used to fail with no output at all.
      [[ -n "${2:-}" ]] || {
        echo "error: --arch needs a value (universal, arm64 or x86_64)" >&2
        exit 2
      }
      ARCH_MODE="$2"
      shift 2
      ;;
    --no-dmg)
      MAKE_DMG=0
      shift
      ;;
    -h | --help)
      # The header comment is the help text. Read to the first non-comment line rather than a
      # fixed range, so editing the header cannot silently make --help print the shebang and the
      # first twenty lines of code.
      awk 'NR>1 && /^#/ { sub(/^# ?/, ""); print; next } NR>1 { exit }' "${BASH_SOURCE[0]}"
      exit 0
      ;;
    *)
      echo "error: unknown argument '$1' (try --help)" >&2
      exit 2
      ;;
  esac
done

case "$ARCH_MODE" in
  universal) TARGETS=("aarch64-apple-darwin" "x86_64-apple-darwin") ;;
  arm64) TARGETS=("aarch64-apple-darwin") ;;
  x86_64) TARGETS=("x86_64-apple-darwin") ;;
  *)
    echo "error: --arch must be universal, arm64 or x86_64 (got '$ARCH_MODE')" >&2
    exit 2
    ;;
esac

step() { printf '\n\033[1;34m==>\033[0m \033[1m%s\033[0m\n' "$1"; }
note() { printf '    %s\n' "$1"; }
fail() {
  printf '\n\033[1;31merror:\033[0m %s\n' "$1" >&2
  exit 1
}

# ---------------------------------------------------------------------------------------------
# Version and identity
# ---------------------------------------------------------------------------------------------

# The bundle id, out of the Rust constant that is also the app's keystore service (see the Identity
# section). An empty result means the constant was renamed or reformatted, and stamping an empty
# CFBundleIdentifier would produce a bundle macOS treats as a different app on every build - so it
# is a hard stop rather than a warning.
BUNDLE_ID="$(sed -n 's/^pub const APP_ID: &str = "\(.*\)";$/\1/p' "$BUNDLE_ID_SRC" | head -1)"
[[ -n "$BUNDLE_ID" ]] || fail "could not read APP_ID out of $BUNDLE_ID_SRC"

# The Apple team the app expects its own updates to be signed by, out of the constant next to
# APP_ID. Read here for the same reason and on the same terms: it is the app's identity, the app
# has to see it at runtime, and an empty result means the constant moved. Empty would also make the
# cross-check in the Signing section compare against nothing and pass, so it is a hard stop.
TEAM_ID="$(sed -n 's/^pub const TEAM_ID: &str = "\(.*\)";$/\1/p' "$BUNDLE_ID_SRC" | head -1)"
[[ -n "$TEAM_ID" ]] || fail "could not read TEAM_ID out of $BUNDLE_ID_SRC"

# The crate version is the single source of truth for what a build calls itself, so a release is a
# version bump plus a tag and never a number typed into two places. Read it through version.sh
# rather than with a second copy of the same sed: that script is the one thing that knows where the
# number lives, and it is also what the Release workflow bumps. It fails loud on its own, and the
# assignment propagates that under `set -e`.
VERSION="$("$REPO_ROOT/scripts/version.sh")"

# CFBundleVersion has to increase for macOS to treat a build as newer, and the marketing version
# stands still across the several builds one version produces. The commit count is monotonic and
# needs no state; outside a git checkout the version itself is a fine stand-in.
BUILD_NUMBER="$(git rev-list --count HEAD 2>/dev/null || echo "0")"
GIT_SHA="$(git rev-parse --short HEAD 2>/dev/null || echo "unknown")"

DIST="$REPO_ROOT/target/dist"
APP="$DIST/$APP_NAME.app"

step "Strata $VERSION (build $BUILD_NUMBER, $GIT_SHA) - $ARCH_MODE"

# ---------------------------------------------------------------------------------------------
# Preflight
# ---------------------------------------------------------------------------------------------

[[ "$(uname -s)" == "Darwin" ]] || fail "this builds a macOS .app and only runs on macOS"

# The build resolves Freya from the local submodule checkout, so an uninitialised one fails deep
# in a long compile with a baffling missing-method error. Same check CI makes, for the same reason.
if [[ ! -f "crates/freya/Cargo.toml" ]]; then
  fail "crates/freya is not checked out. Run: git submodule update --init --checkout"
fi

for tool in cargo lipo sips iconutil codesign plutil; do
  command -v "$tool" >/dev/null 2>&1 || fail "'$tool' not found on PATH"
done

# ---------------------------------------------------------------------------------------------
# Build
# ---------------------------------------------------------------------------------------------

step "Building"

for target in "${TARGETS[@]}"; do
  if ! rustup target list --installed 2>/dev/null | grep -qx "$target"; then
    note "adding rust target $target"
    rustup target add "$target"
  fi
done

for target in "${TARGETS[@]}"; do
  note "cargo build --release --target $target"
  cargo build --release --locked --target "$target" --bin "$CARGO_BIN"
done

# ---------------------------------------------------------------------------------------------
# Assemble the bundle
# ---------------------------------------------------------------------------------------------

step "Assembling $APP_NAME.app"

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

BINARIES=()
for target in "${TARGETS[@]}"; do
  BINARIES+=("target/$target/release/$CARGO_BIN")
done

# `lipo -create` on a single input is a copy, so one path covers both modes.
lipo -create -output "$APP/Contents/MacOS/$APP_NAME" "${BINARIES[@]}"
chmod +x "$APP/Contents/MacOS/$APP_NAME"
note "architectures: $(lipo -archs "$APP/Contents/MacOS/$APP_NAME")"
note "size: $(du -h "$APP/Contents/MacOS/$APP_NAME" | cut -f1)"

# The icon is generated from the committed 1024 master rather than kept as a checked-in .icns, so
# there is no second copy of the artwork to drift from the design.
ICONSET="$DIST/$APP_NAME.iconset"
rm -rf "$ICONSET"
mkdir -p "$ICONSET"
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" assets/icon/strata.png --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
  sips -z "$((size * 2))" "$((size * 2))" assets/icon/strata.png \
    --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/AppIcon.icns"
rm -rf "$ICONSET"
note "icon: $(du -h "$APP/Contents/Resources/AppIcon.icns" | cut -f1)"

cat >"$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleName</key>
	<string>$APP_NAME</string>
	<key>CFBundleDisplayName</key>
	<string>$APP_NAME</string>
	<key>CFBundleIdentifier</key>
	<string>$BUNDLE_ID</string>
	<key>CFBundleExecutable</key>
	<string>$APP_NAME</string>
	<key>CFBundleIconFile</key>
	<string>AppIcon</string>
	<key>CFBundleShortVersionString</key>
	<string>$VERSION</string>
	<key>CFBundleVersion</key>
	<string>$BUILD_NUMBER</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleInfoDictionaryVersion</key>
	<string>6.0</string>
	<key>LSMinimumSystemVersion</key>
	<string>$MACOSX_DEPLOYMENT_TARGET</string>
	<key>LSApplicationCategoryType</key>
	<string>public.app-category.developer-tools</string>
	<!-- Without this every surface renders at 1x and is resampled: the Skia canvas would be
	     blurry on exactly the displays the app is designed for. -->
	<key>NSHighResolutionCapable</key>
	<true/>
	<!-- Strata draws with Skia and does not need the discrete GPU; letting macOS keep the
	     integrated one is battery a query workspace has no reason to spend. -->
	<key>NSSupportsAutomaticGraphicsSwitching</key>
	<true/>
	<key>NSHumanReadableCopyright</key>
	<string>Strata</string>
</dict>
</plist>
PLIST

plutil -lint "$APP/Contents/Info.plist" >/dev/null || fail "generated Info.plist is malformed"

# Classic four-byte type/creator file. Cheap, and some of macOS's older bundle handling still
# looks for it.
printf 'APPL????' >"$APP/Contents/PkgInfo"

# ---------------------------------------------------------------------------------------------
# Sign
# ---------------------------------------------------------------------------------------------

step "Signing"

# An explicit identity wins; otherwise look for a Developer ID Application cert in the keychain.
# Deliberately *not* falling back to an "Apple Development" cert: it signs, but Apple refuses to
# notarize it, so it would buy a signature that still fails on a tester's Mac while reading like
# success here.
SIGN_IDENTITY="${MACOS_SIGN_IDENTITY:-}"
if [[ -z "$SIGN_IDENTITY" ]]; then
  SIGN_IDENTITY="$(security find-identity -v -p codesigning 2>/dev/null |
    sed -n 's/.*"\(Developer ID Application: .*\)"/\1/p' | head -1 || true)"
fi

SIGNED_REAL=0
if [[ -n "$SIGN_IDENTITY" ]]; then
  note "identity: $SIGN_IDENTITY"
  # --options runtime is the hardened runtime, which notarization requires. --timestamp gets a
  # trusted timestamp so the signature outlives the certificate.
  codesign --force --timestamp --options runtime \
    --sign "$SIGN_IDENTITY" "$APP"
  SIGNED_REAL=1
else
  note "no Developer ID Application certificate found - signing ad-hoc"
  note "testers will need the Gatekeeper bypass in docs/RELEASING.md"
  codesign --force --sign - "$APP"
fi

codesign --verify --deep --strict "$APP" || fail "the signature did not verify"

# The team the bundle is signed by is what the updater checks a downloaded bundle against, so it
# has to be the team compiled into that bundle. An app signed by a team its own updater refuses is
# a release that can never update itself, and nothing after this point can notice - the signature
# is valid, the notarization succeeds, and the failure only appears on a tester's machine one
# version later.
#
# Read back out of the signature rather than parsed out of the identity string: codesign accepts a
# certificate hash as an identity too, and the signature is what is actually true either way.
# `codesign -d` prints to stderr, hence the redirect.
if [[ "$SIGNED_REAL" -eq 1 ]]; then
  SIGNED_TEAM="$(codesign -dvvv "$APP" 2>&1 | sed -n 's/^TeamIdentifier=//p' | head -1)"
  [[ "$SIGNED_TEAM" == "$TEAM_ID" ]] || fail "signed by team '$SIGNED_TEAM', but $BUNDLE_ID_SRC compiles in TEAM_ID '$TEAM_ID' - the updater only installs a bundle signed by that team, so this build could never update itself"
  note "team: $SIGNED_TEAM"
fi

# ---------------------------------------------------------------------------------------------
# Notarize
# ---------------------------------------------------------------------------------------------

# Apple takes either an App Store Connect API key or an Apple ID with an app-specific password.
# The key is the better fit for CI (no 2FA, revocable on its own), so it is preferred when both
# are present.
#
# The credentials are passed at each call site rather than collected into an array: macOS ships
# bash 3.2, where an empty array expanded under `set -u` is an unbound-variable error rather than
# nothing, so "the args, if any" is a shape that breaks precisely in the no-credentials case this
# script is expected to take most often.
HAVE_NOTARY=0
if [[ -n "${AC_API_KEY_PATH:-}" && -n "${AC_API_KEY_ID:-}" && -n "${AC_API_ISSUER_ID:-}" ]]; then
  HAVE_NOTARY=1
elif [[ -n "${AC_APPLE_ID:-}" && -n "${AC_PASSWORD:-}" && -n "${AC_TEAM_ID:-}" ]]; then
  HAVE_NOTARY=2
fi

# Submit one file and wait for Apple's verdict. `notarytool submit --wait` exits non-zero on a
# rejection, so `set -e` stops the build rather than shipping something Apple refused.
notarize() {
  case "$HAVE_NOTARY" in
    1) xcrun notarytool submit "$1" --key "$AC_API_KEY_PATH" --key-id "$AC_API_KEY_ID" \
      --issuer "$AC_API_ISSUER_ID" --wait ;;
    2) xcrun notarytool submit "$1" --apple-id "$AC_APPLE_ID" --password "$AC_PASSWORD" \
      --team-id "$AC_TEAM_ID" --wait ;;
    *) fail "notarize() called with no credentials configured" ;;
  esac
}

NOTARIZED=0

# Notarizing an unsigned build is not a thing Apple will do, so the credentials only matter once
# there is a real signature to submit.
if [[ "$HAVE_NOTARY" -ne 0 && "$SIGNED_REAL" -eq 1 ]]; then
  step "Notarizing"
  # Deliberately not in $DIST. The release workflow attaches every zip in that directory, and this
  # one is a transient the submission needs rather than an artifact. A rejected submission or a
  # failed staple aborts the build under `set -e` before the cleanup below, and nothing empties
  # $DIST between runs - so a zip left there would ride the next successful build onto the release
  # page as a second, stale asset.
  NOTARY_DIR="$(mktemp -d)"
  NOTARY_ZIP="$NOTARY_DIR/$APP_NAME-notarize.zip"
  # ditto, not zip: it is the only archiver that preserves the bundle's symlinks and extended
  # attributes, and a bundle that lost them is rejected.
  ditto -c -k --keepParent "$APP" "$NOTARY_ZIP"
  note "submitting to Apple (this waits for the verdict, usually a few minutes)"
  notarize "$NOTARY_ZIP"
  # Stapling writes the ticket into the bundle so it validates offline - without it a tester with
  # no network, or Apple having a bad day, still gets blocked.
  xcrun stapler staple "$APP"
  rm -rf "$NOTARY_DIR"
  NOTARIZED=1
elif [[ "$HAVE_NOTARY" -ne 0 ]]; then
  note "notary credentials are set but the build is only ad-hoc signed - skipping notarization"
else
  note "no notary credentials - skipping notarization"
fi

# ---------------------------------------------------------------------------------------------
# Update archive
# ---------------------------------------------------------------------------------------------

# The DMG is the first-install artifact - it carries the drag-to-Applications gesture, which is a
# thing a person does. This zip is the programmatic one: it is what the in-app updater downloads,
# verifies and swaps in (UP-02), and its `.app.zip` suffix is the contract the updater's asset
# selection keys on.
#
# ditto, not zip, for the same reason the notarize submission uses it: it is the only archiver that
# round-trips a signed bundle's symlinks and extended attributes, and a bundle that lost them fails
# the strict codesign verify the updater runs before it installs anything.
#
# Built *after* stapling, so the archived bundle carries the notarization ticket and validates
# offline. It is built on an unsigned run all the same - the artifact set is the same shape on
# every rung, and a build that quietly stopped producing the update artifact would only be noticed
# by an updater that had nothing to fetch.
step "Building the update archive"
ZIP="$DIST/$APP_NAME-$VERSION-$ARCH_MODE.app.zip"
rm -f "$ZIP"
ditto -c -k --keepParent "$APP" "$ZIP"
note "size: $(du -h "$ZIP" | cut -f1)"

# ---------------------------------------------------------------------------------------------
# DMG
# ---------------------------------------------------------------------------------------------

DMG=""
if [[ "$MAKE_DMG" -eq 1 ]]; then
  step "Building the disk image"
  DMG="$DIST/$APP_NAME-$VERSION-$ARCH_MODE.dmg"
  STAGE="$DIST/dmg"
  rm -rf "$STAGE" "$DMG"
  mkdir -p "$STAGE"
  cp -R "$APP" "$STAGE/"
  # The Applications symlink is the whole "drag to install" gesture. Without it testers run the
  # app from the mounted image, where it is read-only and every restart starts over.
  ln -s /Applications "$STAGE/Applications"
  hdiutil create -volname "$APP_NAME $VERSION" -srcfolder "$STAGE" \
    -ov -format UDZO -quiet "$DMG"
  rm -rf "$STAGE"

  # The DMG carries its own signature and ticket. A stapled app inside an unsigned image still
  # trips Gatekeeper on the image itself, which is the first thing the tester opens.
  if [[ "$SIGNED_REAL" -eq 1 ]]; then
    codesign --force --timestamp --sign "$SIGN_IDENTITY" "$DMG"
    if [[ "$NOTARIZED" -eq 1 ]]; then
      notarize "$DMG"
      xcrun stapler staple "$DMG"
    fi
  fi
  note "size: $(du -h "$DMG" | cut -f1)"
fi

# ---------------------------------------------------------------------------------------------
# Report
# ---------------------------------------------------------------------------------------------

step "Done"
note "app: $APP"
note "update archive: $ZIP"
[[ -n "$DMG" ]] && note "dmg: $DMG"

if [[ "$NOTARIZED" -eq 1 ]]; then
  note "signed and notarized - testers can open this normally"
elif [[ "$SIGNED_REAL" -eq 1 ]]; then
  note "signed but NOT notarized - Gatekeeper will still block a downloaded copy"
else
  note "ad-hoc signed - testers must follow the bypass in docs/RELEASING.md"
fi
