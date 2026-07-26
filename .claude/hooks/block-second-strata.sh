#!/usr/bin/env bash
# PreToolUse/Bash hook: one Strata window across every session.
#
# Strata is a native window on the Mac's display, and several agent sessions can be live at once
# (a worktree each). Without this, every one of them will happily `cargo run` its own build: you
# end up with two or three identical windows, no way to tell which worktree owns which, and each
# instance quietly clobbering the others' app config — `AppConfig` is read *once* at startup with
# no file watching, so the last instance to write wins for recents, settings and the open-project
# set (see AGENTS.md §2).
#
# So a launch is refused while any Strata is alive, and the refusal says which worktree owns the
# one that is. Deliberately a refusal and not a kill: the running window may be the thing a human
# is looking at right now, and no session gets to close another's on its own initiative.
#
# Scope is *agent* Bash calls. A human running `cargo run` in their own terminal is untouched —
# this enforces the convention between sessions, it is not an app-level single-instance lock.
# That one is a real feature and belongs to the multi-window task (P4-01) — see its file.
#
# Reads the hook payload on stdin, prints a deny decision when it matches, and stays silent
# (allowing the call) otherwise.

set -uo pipefail

command=$(jq -r '.tool_input.command // empty')

# `cargo` (with any global options) then `run` / `r`. Matched anywhere in the string — after `&&`,
# `;`, `|`, inside `$(…)` — like the destructive-git hook, since chaining is how the rule gets
# broken. `cargo test` / `build` / `clippy` open no window and are not matched.
#
# Both context classes are "not an identifier character" rather than whitespace-or-end, so the
# verb's own terminators count: `$(cargo run)` and `cargo run;` are launches too. (Whitespace-only
# was the bug the git hook shipped with — see there.) `-` is outside the class, so a hypothetical
# `cargo run-foo` is not a match, while `cargo run --release` is.
launch='(^|[^[:alnum:]_-])cargo([[:space:]]+-[^[:space:]]+([[:space:]]+[^[:space:]]+)?)*[[:space:]]+(run|r)([^[:alnum:]_-]|$)'

printf '%s' "$command" | grep -Eq "$launch" || exit 0

# A live Strata is a running *binary* under some worktree's `target/`, which is what makes this
# work across worktrees: each has its own build, and all of them match. The pattern cannot
# self-match the shell running it — the literal `(debug|release)` in this argv is not what the
# regex accepts after `target/`.
pid=$(pgrep -f 'target/(debug|release)/strata-freya' | head -1) || true
[ -n "$pid" ] || exit 0

# Which worktree owns it: the process's cwd, since cargo invokes the binary by a relative path
# (`ps -o comm=` only ever says `target/debug/strata-freya`).
owner=$(lsof -a -p "$pid" -d cwd -Fn 2>/dev/null | sed -n 's/^n//p' | head -1)
owner=${owner:-unknown directory}

reason="Blocked by .claude/hooks/block-second-strata.sh: Strata is already running (pid $pid) from $owner. One window across all sessions — a second instance clobbers the shared app config (recents, settings, the open-project set). Ask the user to close that window, or to kill it themselves (kill $pid) if it is not the one they are using."
jq -nc --arg reason "$reason" '{
  hookSpecificOutput: {
    hookEventName: "PreToolUse",
    permissionDecision: "deny",
    permissionDecisionReason: $reason
  }
}'

exit 0
