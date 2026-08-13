#!/usr/bin/env bash
# Exercises both PreToolUse hooks against the spellings a shell treats as one command.
#
#   bash .claude/hooks/test-hooks.sh
#
# The hooks are the enforcement behind two AGENTS.md §7 rules, and both have shipped with gaps
# that their own doc comments claimed to cover — a verb's terminators, then quoting and line
# continuations, then failing open with no `jq`. A rule enforced by a regex needs a way to ask
# whether the regex still says what the prose does, which is this.
#
# **Nothing here spells the guarded verbs literally.** The destructive-git hook reads the whole
# command string of every agent `Bash` call, including the one that runs this file, and it
# over-matches on purpose — so a test that named `git` + a destroying verb in its own argv would
# be refused before it could run. The pieces are joined at runtime instead.

set -uo pipefail
cd "$(dirname "$0")/../.." || exit 1

GIT_HOOK=".claude/hooks/block-destructive-git.sh"
APP_HOOK=".claude/hooks/block-second-strata.sh"
fails=0

pass() { printf '  ok    %s\n' "$1"; }
fail() {
  printf '  FAIL  %s\n' "$1"
  fails=$((fails + 1))
}

# --- the git hook: end to end, through the real payload ---------------------------------------
#
# Asked of the hook itself rather than of its pattern, because its answer is the whole point: a
# non-empty stdout is a deny.
git_case() { # expected(allow|deny) command
  local want="$1" cmd="$2" got
  if [ -n "$(printf '%s' "$cmd" | jq -Rsc '{tool_input:{command:.}}' | bash "$GIT_HOOK")" ]; then
    got=deny
  else
    got=allow
  fi
  [ "$got" = "$want" ] && pass "$cmd" || fail "want=$want got=$got  $cmd"
}

R="re""set"
C="check""out"
CL="cl""ean"
RS="rest""ore"

echo "block-destructive-git.sh"
git_case allow "git status"
git_case allow "git switch main"
git_case allow "git diff --cached"
git_case allow "git stash push -u"
git_case allow "git log --oneline -5"
git_case deny "git $R --hard"
git_case deny "git $RS ."
# Quoting: three spellings of one call, none of which the raw pattern saw.
git_case deny "git \"$R\" --hard"
git_case deny "git 'r'e'set' --hard"
git_case deny "git \"\"$C\"\" ."
# Chaining and substitution.
git_case deny "echo hi && git $CL -fd"
git_case deny "make build; git $R --hard"
git_case deny "echo \$(git $CL)"
git_case deny "git $R;"
# Global options between the verb and the command.
git_case deny "git -C /somewhere $C ."
# A line continuation splits the match across lines; `grep` matches per line.
git_case deny "$(printf 'git \\\n  %s --hard' "$R")"

# --- the strata hook: its matcher ---------------------------------------------------------------
#
# The hook exits silently when no Strata is running, so end-to-end it would answer "allow" for
# every input on a quiet machine. What is testable without a live window is the launch matcher,
# read out of the hook so the test cannot drift from what ships.
echo "block-second-strata.sh"
launch=$(sed -n "s/^launch='\(.*\)'$/\1/p" "$APP_HOOK")
direct=$(sed -n "s/^direct='\(.*\)'$/\1/p" "$APP_HOOK")
if [ -z "$launch" ] || [ -z "$direct" ]; then
  fail "could not read the launch patterns out of $APP_HOOK"
else
  app_case() { # expected(match|skip) command
    local want="$1" cmd="$2" got
    if printf '%s' "$cmd" | tr '\n' ' ' | tr -d '"'"'"'\\' | grep -Eq "$launch|$direct"; then
      got=match
    else
      got=skip
    fi
    [ "$got" = "$want" ] && pass "$cmd" || fail "want=$want got=$got  $cmd"
  }

  app_case skip "cargo test --workspace"
  app_case skip "cargo build --release"
  app_case skip "cargo clippy --workspace -- -D warnings"
  app_case skip "cargo fmt -p strata-core"
  app_case match "cargo run"
  app_case match "cargo run --release"
  app_case match "cargo r"
  app_case match 'cargo "run"'
  app_case match "echo x && cargo run"
  app_case match "$(printf 'cargo \\\n  run')"
  # The ways around `cargo run` that open the same window.
  app_case match "./target/debug/strata-freya"
  app_case match "target/release/strata-freya --flag"
  app_case match "open target/dist/Strata.app"
fi

# --- both hooks fail closed with no jq ----------------------------------------------------------
#
# The gap this covers is the quiet one: with `set -uo pipefail` and no `-e`, a missing `jq` left
# the command empty, matched nothing, and allowed every call for as long as the tool was absent.
#
# `bash` is invoked by its absolute path and `PATH` points at an empty directory, so the shell
# still starts while `command -v jq` inside the hook finds nothing. Emptying `PATH` outright would
# only prove that `bash` cannot be found either.
echo "no jq"
BASH_BIN=$(command -v bash)
EMPTY_PATH=$(mktemp -d)
for hook in "$GIT_HOOK" "$APP_HOOK"; do
  out=$(PATH="$EMPTY_PATH" "$BASH_BIN" "$hook" </dev/null)
  case "$out" in
    *'"permissionDecision":"deny"'*) pass "$hook denies when jq is missing" ;;
    *) fail "$hook allowed a call with no jq: '$out'" ;;
  esac
done
rmdir "$EMPTY_PATH"

echo
if [ "$fails" -eq 0 ]; then
  echo "all hook cases pass"
else
  echo "$fails hook case(s) failed"
fi
exit "$fails"
