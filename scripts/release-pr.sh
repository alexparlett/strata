#!/usr/bin/env bash
#
# Open the pull request that moves Strata's version.
#
# A release is cut from a commit that is already on main, so the version has to get there first,
# and main takes changes only through a reviewed pull request. That is this: a branch, the bump
# `scripts/version.sh` writes, one commit carrying nothing else, and a PR.
#
#   ./scripts/release-pr.sh minor     # branch, bump, commit, push, open the PR
#   ./scripts/release-pr.sh 0.4.0     # the same for an exact version
#   ./scripts/release-pr.sh patch --no-pr
#
# Merge the PR once CI is green, then run the Release workflow with "Tag this commit and publish a
# release page" ticked. It reads the version out of the manifest, so there is nothing to type into
# it twice.
#
# The PR is opened with *your* `gh` credentials on purpose. A workflow could do all of this, but a
# pull request opened with GITHUB_TOKEN starts no workflow runs, so the required checks would never
# report on it and it could never be merged. That is also why the version bump is not something the
# Release workflow can do for itself any more.
#
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

MANIFEST="Cargo.toml"

fail() {
  printf 'error: %s\n' "$1" >&2
  exit 1
}
note() { printf '    %s\n' "$1" >&2; }

SPEC=""
OPEN_PR=true

while [[ $# -gt 0 ]]; do
  case "$1" in
    --no-pr)
      OPEN_PR=false
      shift
      ;;
    -h | --help)
      awk 'NR>1 && /^#/ { sub(/^# ?/, ""); print; next } NR>1 { exit }' "${BASH_SOURCE[0]}"
      exit 0
      ;;
    -*)
      fail "unknown argument '$1' (try --help)"
      ;;
    *)
      [[ -z "$SPEC" ]] || fail "give one bump or version, not '$SPEC' and '$1'"
      SPEC="$1"
      shift
      ;;
  esac
done

[[ -n "$SPEC" ]] || fail "expected a bump (major, minor, patch) or a version. Try --help."
command -v gh >/dev/null 2>&1 || fail "'gh' not found on PATH"

# Only these two files are committed, so an unrelated edit in the tree is none of this script's
# business - but an edit to one of *these* is: it would ride the release commit, or read as a bump
# that has already happened.
if ! git diff --quiet -- "$MANIFEST" Cargo.lock ||
  ! git diff --cached --quiet -- "$MANIFEST" Cargo.lock; then
  fail "$MANIFEST or Cargo.lock has uncommitted changes - commit or set them aside first"
fi

# Resolving writes nothing, so a typo or an already-released number is rejected before a branch
# exists to clean up.
VERSION="$(./scripts/version.sh --resolve "$SPEC")"
TAG="v$VERSION"
BRANCH="release/$TAG"

if git rev-parse -q --verify "refs/heads/$BRANCH" >/dev/null; then
  fail "branch $BRANCH already exists - delete it, or pick another version"
fi
if git ls-remote --exit-code --tags origin "$TAG" >/dev/null 2>&1; then
  fail "$TAG is already tagged - pick a later version"
fi

# The branch this is cut from is also what the PR asks to merge into, so a detached HEAD has no
# answer rather than a wrong one.
BASE="$(git rev-parse --abbrev-ref HEAD)"
[[ "$BASE" != "HEAD" ]] || fail "HEAD is detached - switch to the branch this release should merge into"

note "branching $BRANCH off $BASE"
git switch -c "$BRANCH" >/dev/null

# The lockfile records every member's version and the release build passes `--locked`, so the two
# move together or the build fails on the lockfile. `version.sh` writes both.
./scripts/version.sh --bump "$VERSION" >/dev/null
git commit -q -m "Release $VERSION" -- "$MANIFEST" Cargo.lock
note "committed: $(git log -1 --oneline)"

git push -q -u origin "HEAD:refs/heads/$BRANCH"
note "pushed $BRANCH"

if [[ "$OPEN_PR" != "true" ]]; then
  note "--no-pr: open the pull request yourself when you are ready"
  exit 0
fi

gh pr create \
  --base "$BASE" \
  --head "$BRANCH" \
  --title "Release $VERSION" \
  --body "$(
    printf '%s\n' \
      "Moves the crate version to $VERSION so a release can be cut from \`$BASE\`." \
      "" \
      "Merging this is the deliberate act: afterwards, run the Release workflow with" \
      "\"Tag this commit and publish a release page\" ticked and it will build \`$VERSION\`," \
      "tag it and publish the release page." \
      "" \
      "Opened by \`scripts/release-pr.sh\`."
  )"
