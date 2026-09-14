#!/usr/bin/env bash
#
# Build Strata into an AppImage — one file a Linux user can chmod +x and run, on any distribution
# new enough to have the glibc this machine built against.
#
# This is the whole pipeline, for the same reason `bundle-macos.sh` is: a release path that only
# exists in a workflow file is one nobody can debug.
#
#   ./scripts/bundle-linux.sh                    # build + bundle + package
#   ./scripts/bundle-linux.sh --no-appimage      # stop at the AppDir, for poking at it
#   ./scripts/bundle-linux.sh --debug            # bundle the debug binary, for a quick check
#
# **It needs `appimagetool`** for the last step, which is not packaged by most distributions. Either
# put it on PATH (Arch: `appimagetool-bin` from the AUR; elsewhere: the release binary from
# https://github.com/AppImage/appimagetool) or pass `--no-appimage` and package the AppDir yourself.
# The script says which one it could not find rather than half-producing an artifact.
#
# **Libraries are bundled, but not all of them.** An AppImage carrying its own libGL, libwayland or
# glibc is one that breaks the moment the host's graphics stack differs from the build machine's —
# those have to come from the system, and EXCLUDE below is that list with the reason attached. What
# gets carried is the application's own dependency closure: gtk, its pango/cairo/gdk train, OpenSSL,
# libxdo.
#
# **The renderer preference is set by AppRun, conditionally** — see the AppRun section, which is
# where the reasoning lives. It is not baked into the binary and never overrides a user who has said
# what they want.
#
set -euo pipefail

# ---------------------------------------------------------------------------------------------
# Identity
# ---------------------------------------------------------------------------------------------

APP_NAME="Strata"
# The Rust bin, which is not what the app is called. The desktop file's `Exec` is the one place the
# two names have to agree.
CARGO_BIN="strata-freya"
# Read out of the Rust source for the reason `bundle-macos.sh` reads it: it is the app's identity,
# the app itself uses it as the keystore service (`strata_core::secret::APP_ID`), and a copy here
# that drifted would name one thing two ways. It is also the desktop file's basename, which is what
# a desktop environment keys the window's icon off.
BUNDLE_ID_SRC="crates/strata-core/src/secret.rs"
ICON_SRC="assets/icon/strata.png"

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

# ---------------------------------------------------------------------------------------------
# Arguments
# ---------------------------------------------------------------------------------------------

MAKE_APPIMAGE=1
PROFILE="release"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --no-appimage)
      MAKE_APPIMAGE=0
      shift
      ;;
    --debug)
      PROFILE="debug"
      shift
      ;;
    -h | --help)
      # The header comment is the help text, read to the first non-comment line — same trick as
      # `bundle-macos.sh`, so editing the header cannot make --help print the shebang and code.
      awk 'NR>1 && /^#/ { sub(/^# ?/, ""); print; next } NR>1 { exit }' "${BASH_SOURCE[0]}"
      exit 0
      ;;
    *)
      echo "error: unknown argument '$1' (try --help)" >&2
      exit 2
      ;;
  esac
done

step() { printf '\n\033[1;34m==>\033[0m \033[1m%s\033[0m\n' "$1"; }
note() { printf '    %s\n' "$1"; }
fail() {
  printf '\n\033[1;31merror:\033[0m %s\n' "$1" >&2
  exit 1
}

# ---------------------------------------------------------------------------------------------
# Version and preflight
# ---------------------------------------------------------------------------------------------

BUNDLE_ID="$(sed -n 's/^pub const APP_ID: &str = "\(.*\)";$/\1/p' "$BUNDLE_ID_SRC" | head -1)"
[[ -n "$BUNDLE_ID" ]] || fail "could not read APP_ID out of $BUNDLE_ID_SRC"

# Through version.sh rather than a second copy of the same sed: that script is the one thing that
# knows where the number lives, and it is what the Release workflow bumps.
VERSION="$("$REPO_ROOT/scripts/version.sh")"
GIT_SHA="$(git rev-parse --short HEAD 2>/dev/null || echo "unknown")"
ARCH="$(uname -m)"

DIST="$REPO_ROOT/target/dist"
APPDIR="$DIST/$APP_NAME.AppDir"
APPIMAGE="$DIST/$APP_NAME-$VERSION-$ARCH.AppImage"

step "Strata $VERSION ($GIT_SHA) - $ARCH AppImage"

[[ "$(uname -s)" == "Linux" ]] || fail "this builds a Linux AppImage and only runs on Linux"
[[ -f "$ICON_SRC" ]] || fail "no icon at $ICON_SRC"

for tool in cargo ldd install; do
  command -v "$tool" >/dev/null 2>&1 || fail "'$tool' not found on PATH"
done

if [[ $MAKE_APPIMAGE -eq 1 ]] && ! command -v appimagetool >/dev/null 2>&1; then
  fail "'appimagetool' not found on PATH.
    Arch: install 'appimagetool-bin' from the AUR.
    Otherwise: take the release binary from https://github.com/AppImage/appimagetool.
    Or run with --no-appimage to stop at the AppDir."
fi

# ---------------------------------------------------------------------------------------------
# Build
# ---------------------------------------------------------------------------------------------

step "Building ($PROFILE)"

if [[ "$PROFILE" == "release" ]]; then
  cargo build --release --locked
else
  cargo build --locked
fi

BIN="$REPO_ROOT/target/$PROFILE/$CARGO_BIN"
[[ -x "$BIN" ]] || fail "no binary at $BIN"

# ---------------------------------------------------------------------------------------------
# AppDir
# ---------------------------------------------------------------------------------------------

step "Assembling the AppDir"

rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/lib" \
  "$APPDIR/usr/share/applications" \
  "$APPDIR/usr/share/icons/hicolor/1024x1024/apps"

install -m 755 "$BIN" "$APPDIR/usr/bin/$CARGO_BIN"

# The icon is needed in three places and they are not redundant: the AppDir root is what
# appimagetool reads (matched to the desktop file's `Icon=`), the hicolor path is what a desktop
# environment finds once the AppImage is integrated, and `.DirIcon` is what file managers show.
install -m 644 "$ICON_SRC" "$APPDIR/usr/share/icons/hicolor/1024x1024/apps/$BUNDLE_ID.png"
install -m 644 "$ICON_SRC" "$APPDIR/$BUNDLE_ID.png"
cp "$APPDIR/$BUNDLE_ID.png" "$APPDIR/.DirIcon"

# `StartupWMClass` is what links the running window back to this desktop entry, so the taskbar shows
# the app's own icon and name rather than a generic one. winit sets the Wayland app id / X11 class
# from the binary name unless told otherwise, which is why it is the *binary* here and not the
# bundle id.
cat > "$APPDIR/usr/share/applications/$BUNDLE_ID.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=$APP_NAME
Comment=A local Athena-style parquet query workspace
Exec=$CARGO_BIN %F
Icon=$BUNDLE_ID
Categories=Development;Database;
Terminal=false
StartupWMClass=$CARGO_BIN
MimeType=application/vnd.apache.parquet;text/csv;application/json;
DESKTOP
cp "$APPDIR/usr/share/applications/$BUNDLE_ID.desktop" "$APPDIR/$BUNDLE_ID.desktop"

# ---------------------------------------------------------------------------------------------
# Libraries
# ---------------------------------------------------------------------------------------------

step "Bundling libraries"

# **What must come from the host, never from us.** Three groups, and each is a way an AppImage
# breaks rather than a matter of size:
#
#   - the graphics stack (libGL, libEGL, libvulkan, libdrm, the X and Wayland client libraries).
#     These talk to whatever driver and compositor the user is running; a copy from the build
#     machine is a copy that does not match, and the failure is a black window or a segfault rather
#     than a missing symbol.
#   - glibc and its satellites (libc, libm, libdl, libpthread, librt, ld-linux). The dynamic loader
#     and libc must be one version; carrying half of the pair is the classic AppImage crash.
#   - libstdc++ and libgcc, which have to be at least as new as anything the host loads *into* us —
#     a Mesa driver built against a newer libstdc++ will not load against an older bundled one.
#
# Everything else is the application's own closure and is carried.
EXCLUDE='^(libGL|libGLX|libGLdispatch|libEGL|libOpenGL|libGLESv|libvulkan|libdrm|libgbm|
libX11|libXext|libXi|libXfixes|libXrender|libXrandr|libXcursor|libXdamage|libXcomposite|
libXinerama|libXau|libXdmcp|libXtst|libxcb|libxshmfence|libwayland|libxkbcommon|
libc\.|libm\.|libdl\.|libpthread\.|librt\.|libresolv\.|libnsl\.|libutil\.|ld-linux|
libstdc\+\+|libgcc_s|
libasound|libpulse|libdbus-1)'
EXCLUDE="$(printf '%s' "$EXCLUDE" | tr -d '\n')"

copied=0
skipped=0
while read -r _name _arrow path _addr; do
  # ldd prints three shapes: "soname => path (addr)", "path (addr)" for the loader on some systems,
  # and "linux-vdso.so.1 (addr)" for the kernel's virtual object. Only a line that resolved to a
  # real file is ours to consider.
  [[ -n "${path:-}" && -f "${path:-}" ]] || continue
  # **The basename, not ldd's first field.** That field is a soname for most entries but an
  # absolute path for the dynamic loader, and an exclude list anchored at `^` silently does not
  # match `/lib64/ld-linux-x86-64.so.2` — which is exactly the library that must never be carried.
  base="$(basename "$path")"
  if [[ "$base" =~ $EXCLUDE ]]; then
    skipped=$((skipped + 1))
    continue
  fi
  install -m 644 "$path" "$APPDIR/usr/lib/$base"
  copied=$((copied + 1))
done < <(ldd "$APPDIR/usr/bin/$CARGO_BIN" | sed 's/^[[:space:]]*//')

note "carried $copied libraries, left $skipped to the host"
[[ $copied -gt 0 ]] || fail "bundled nothing — the ldd parse or the exclude list is wrong"

# ---------------------------------------------------------------------------------------------
# AppRun
# ---------------------------------------------------------------------------------------------

step "Writing AppRun"

# **The renderer preference, and why it is a condition rather than a setting.**
#
# Freya picks Vulkan by default on Linux. On an NVIDIA GPU under Wayland that segfaults inside
# `vkAcquireNextImageKHR`, in `libnvidia-glcore` — a null jump in the driver, before any result code
# comes back, so nothing in Freya's swapchain handling can catch it (docs/PLATFORMS.md carries the
# trace). OpenGL avoids that path and is stable.
#
# Forcing OpenGL for *everyone* would be the wrong fix: on AMD and Intel the Vulkan path works and
# is the better one, and an AppImage that quietly downgrades them is a bug they cannot see. So the
# preference is set only where the crash lives — NVIDIA plus Wayland — and only when the user has
# not already said what they want, so `FREYA_RENDERER=vulkan ./Strata.AppImage` still means what it
# says.
cat > "$APPDIR/AppRun" <<'APPRUN'
#!/usr/bin/env bash
set -euo pipefail

HERE="$(dirname "$(readlink -f "${0}")")"
export LD_LIBRARY_PATH="$HERE/usr/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
export XDG_DATA_DIRS="$HERE/usr/share${XDG_DATA_DIRS:+:$XDG_DATA_DIRS}"

# See the bundle script's AppRun section: NVIDIA's Vulkan driver crashes under Wayland, and only
# there, so this is a condition and not a default. An explicit FREYA_RENDERER always wins.
if [[ -z "${FREYA_RENDERER:-}" ]] \
  && [[ -n "${WAYLAND_DISPLAY:-}" || "${XDG_SESSION_TYPE:-}" == "wayland" ]] \
  && [[ -d /sys/module/nvidia || -e /dev/nvidiactl ]]; then
  export FREYA_RENDERER=opengl
fi

exec "$HERE/usr/bin/strata-freya" "$@"
APPRUN
chmod 755 "$APPDIR/AppRun"

if [[ $MAKE_APPIMAGE -eq 0 ]]; then
  step "Done (AppDir only)"
  note "$APPDIR"
  exit 0
fi

# ---------------------------------------------------------------------------------------------
# Package
# ---------------------------------------------------------------------------------------------

step "Packaging"

rm -f "$APPIMAGE"
# **Update information, embedded in the file.** `gh-releases-zsync` is the AppImage world's own
# update convention: the string names where newer builds are published, and `AppImageUpdate` (or
# `--appimage-update`) reads it out of the file and fetches a delta.
#
# Strata's own in-app updater does not use it — that reads the GitHub releases API directly and
# swaps the whole file (`strata_core::update`) — so this is a second, external path to the same
# place. It costs one flag and it is what a user who manages AppImages with the ecosystem's tools
# will expect to find; a file that carries no update information is one those tools refuse.
#
# `latest` rather than a pinned tag, because the string is baked into a build that must go on
# naming where its *successors* appear.
UPDATE_INFO="gh-releases-zsync|alexparlett|strata|latest|$APP_NAME-*-$ARCH.AppImage.zsync"

# ARCH is appimagetool's own switch for the runtime it prepends, and it does not infer it.
ARCH="$ARCH" appimagetool -u "$UPDATE_INFO" "$APPDIR" "$APPIMAGE" >/dev/null

[[ -f "$APPIMAGE" ]] || fail "appimagetool produced no file"
chmod 755 "$APPIMAGE"

step "Done"
note "$APPIMAGE ($(du -h "$APPIMAGE" | cut -f1))"
# appimagetool writes the zsync file beside the AppImage when it is given update information. It is
# half of that mechanism — the tools fetch it to work out which blocks changed — so it is published
# alongside rather than left in the build directory.
[[ -f "$APPIMAGE.zsync" ]] && note "$APPIMAGE.zsync"
