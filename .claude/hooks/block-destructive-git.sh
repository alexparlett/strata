#!/usr/bin/env bash
# PreToolUse/Bash hook: refuse the git verbs that silently destroy uncommitted work.
#
# `git checkout` / `restore` / `reset` / `clean` overwrite or delete files with no undo and no
# diff shown first — AGENTS.md §7 forbids them, and the rule was still broken by chaining one
# into a compound command. A permission `deny` rule matches the command's *prefix*, so
# `foo && git checkout -- x` slips past it; this reads the whole command string instead.
#
# It matches `git <verb>` anywhere: after `&&`, `;`, `|`, inside `$(...)`, and through global
# options (`git -C /path checkout`). `git switch` is deliberately NOT blocked — it is the modern
# way to change or create a branch and cannot clobber a file path.
#
# ⚠️ The **trailing** context is any non-identifier character, not just whitespace-or-end. It used
# to be the latter, which left the verb's own terminators open: `git reset;`, `git clean|cat` and
# `echo $(git clean)` all slipped through a hook whose doc claimed to catch exactly those. Found
# while building the sibling `block-second-strata.sh`, which had inherited the same pattern.
#
# ⚠️ And the command is **normalized before it is matched**, because a regex over the raw string
# reads only one spelling of each command. The shell does not: `git "reset"`, `git re'set'` and a
# `git \` + newline + `reset` are all the same call, and all three walked past the pattern above
# while the doc claimed the rule was enforced. Quotes are stripped and line continuations and
# newlines folded to spaces, so every spelling collapses to the one the pattern describes.
#
# That over-matches on purpose: `echo "git reset is forbidden"` is refused too, since after
# stripping quotes it is indistinguishable from the real thing. For a guard whose whole job is to
# stand in front of unrecoverable data loss, a refusal the user can rephrase around is the cheap
# error and a miss is the expensive one.
#
# Reads the hook payload on stdin, prints a deny decision when it matches, and stays silent
# (allowing the call) otherwise.

set -uo pipefail

deny() {
  # Hand-built rather than through `jq`, because the one caller that needs it most is the arm
  # where `jq` is what is missing. The reason text carries no characters that need escaping.
  printf '{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"%s"}}\n' "$1"
  exit 0
}

# **Fail closed.** Without `jq` the payload cannot be read at all, and `command` would be the
# empty string — which matches nothing and silently allows every `git reset` for as long as the
# tool is missing. A hook that cannot judge must not answer "allow".
if ! command -v jq >/dev/null 2>&1; then
  deny "Blocked by .claude/hooks/block-destructive-git.sh: jq is not installed, so the hook cannot read the command it is meant to check. Install jq (brew install jq) or run the command yourself."
fi

command=$(jq -r '.tool_input.command // empty')

# One spelling per command — see the note above.
normalized=$(printf '%s' "$command" | tr '\n' ' ' | tr -d '"'"'"'\\')

# `git`, any global options, then a destroying verb.
pattern='(^|[^[:alnum:]_-])git([[:space:]]+-[^[:space:]]+([[:space:]]+[^[:space:]]+)?)*[[:space:]]+(checkout|restore|reset|clean)([^[:alnum:]_-]|$)'

if printf '%s' "$normalized" | grep -Eq "$pattern"; then
  deny 'Blocked by .claude/hooks/block-destructive-git.sh: git checkout/restore/reset/clean destroy uncommitted work and are forbidden by AGENTS.md §7. Ask the user to run it, or use a non-destroying alternative (git switch to change branch, git stash to park work, git diff to inspect).'
fi

exit 0
