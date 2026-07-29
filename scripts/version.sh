#!/usr/bin/env bash
#
# Read, resolve or write the version Strata calls itself.
#
# `crates/strata-freya/Cargo.toml` is the single place that number is written: the bundle script
# reads it for CFBundleShortVersionString and the DMG name, and the Release workflow reads it for
# the tag and the release title. This script is the only thing that knows where it lives, so those
# three cannot drift, and a bump is a command anyone can run rather than a `sed` typed into a YAML
# file that nobody can test.
#
#   ./scripts/version.sh                  # print the current version
#   ./scripts/version.sh --resolve minor  # print what a minor bump would produce, changing nothing
#   ./scripts/version.sh --bump minor     # resolve a minor bump and write it
#   ./scripts/version.sh --bump 0.4.0     # write an explicit version
#
# `--resolve` is separate so a caller can decide what a build will call itself *before* committing
# to it: the Release workflow checks the tag does not already exist up front, which is worth knowing
# two hours before the build ends rather than after. It also needs no cargo, so it can run before
# the toolchain is set up.
#
# Writing updates `Cargo.lock` as well. It records the member's own version and the release build
# passes `--locked`, so a manifest bumped on its own fails the build on a lockfile error.
#
# stdout is the version and nothing else, so `V="$(./scripts/version.sh --bump patch)"` works.
# Progress goes to stderr.
#
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

MANIFEST="crates/strata-freya/Cargo.toml"

fail() {
  printf 'error: %s\n' "$1" >&2
  exit 1
}
note() { printf '    %s\n' "$1" >&2; }

# The first `version = ...` at the start of a line is the package's own. Dependency versions in
# this manifest are all inside inline tables (`serde = { version = "1", ... }`), so they never
# start a line, and `head -1` keeps this true even if a `[package]`-shaped section is added later.
current() {
  local v
  v="$(sed -n 's/^version[[:space:]]*=[[:space:]]*"\(.*\)"/\1/p' "$MANIFEST" | head -1)"
  [[ -n "$v" ]] || fail "could not read the version out of $MANIFEST"
  printf '%s\n' "$v"
}

# A bump needs plain MAJOR.MINOR.PATCH to have an unambiguous answer; an explicit version may carry
# a prerelease or build suffix, which is the escape hatch for anything this arithmetic cannot mean.
resolve() {
  local spec="$1" cur ma mi pa
  cur="$(current)"
  case "$spec" in
    major | minor | patch)
      [[ "$cur" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] ||
        fail "cannot $spec-bump '$cur': it is not a plain MAJOR.MINOR.PATCH. Pass the version you want instead."
      IFS=. read -r ma mi pa <<<"$cur"
      case "$spec" in
        major) printf '%d.0.0\n' "$((ma + 1))" ;;
        minor) printf '%d.%d.0\n' "$ma" "$((mi + 1))" ;;
        patch) printf '%d.%d.%d\n' "$ma" "$mi" "$((pa + 1))" ;;
      esac
      ;;
    *)
      [[ "$spec" =~ ^[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?$ ]] ||
        fail "'$spec' is neither a bump nor a version. Expected major, minor, patch or MAJOR.MINOR.PATCH."
      printf '%s\n' "$spec"
      ;;
  esac
}

write() {
  local new="$1" cur
  cur="$(current)"

  if [[ "$new" == "$cur" ]]; then
    note "version is already $new - nothing to write"
    printf '%s\n' "$new"
    return 0
  fi

  command -v cargo >/dev/null 2>&1 || fail "'cargo' not found on PATH (needed to update Cargo.lock)"
  # Resolving the lockfile reads every path dependency, and Freya is one of them.
  if [[ ! -f "crates/freya/Cargo.toml" ]]; then
    fail "crates/freya is not checked out. Run: git submodule update --init --checkout"
  fi

  # awk, not `sed -i`: the in-place flag differs between BSD and GNU sed, and only the *first*
  # matching line may be replaced.
  awk -v v="$new" '
    !done && /^version[[:space:]]*=/ { print "version = \"" v "\""; done = 1; next }
    { print }
  ' "$MANIFEST" >"$MANIFEST.tmp"
  mv "$MANIFEST.tmp" "$MANIFEST"
  [[ "$(current)" == "$new" ]] || fail "wrote $MANIFEST but it still reads $(current)"
  note "$MANIFEST: $cur -> $new"

  # `--workspace` restricts the update to workspace members, so bumping a version cannot quietly
  # move a dependency at the same time. Offline first: the only thing that changed is a member's
  # own version, so the registry index has nothing to say about it, and a release runner should not
  # need the network to renumber a build.
  if cargo update --workspace --offline >/dev/null 2>&1; then
    note "Cargo.lock updated (offline)"
  else
    cargo update --workspace >&2
    note "Cargo.lock updated"
  fi

  printf '%s\n' "$new"
}

MODE="print"
SPEC=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    # The value is checked before the `shift 2`, not after: `shift 2` with one argument left is a
    # non-zero return, which under `set -e` exits the script before any complaint can be printed.
    --resolve)
      [[ -n "${2:-}" ]] || fail "--resolve needs a bump (major, minor, patch) or a version"
      MODE="resolve"
      SPEC="$2"
      shift 2
      ;;
    --bump)
      [[ -n "${2:-}" ]] || fail "--bump needs a bump (major, minor, patch) or a version"
      MODE="write"
      SPEC="$2"
      shift 2
      ;;
    -h | --help)
      # The header comment is the help text, read to the first non-comment line so editing the
      # header cannot silently make --help print the shebang and the first twenty lines of code.
      awk 'NR>1 && /^#/ { sub(/^# ?/, ""); print; next } NR>1 { exit }' "${BASH_SOURCE[0]}"
      exit 0
      ;;
    *)
      fail "unknown argument '$1' (try --help)"
      ;;
  esac
done

case "$MODE" in
  print) current ;;
  resolve) resolve "$SPEC" ;;
  write) write "$(resolve "$SPEC")" ;;
esac
