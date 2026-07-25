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
# Reads the hook payload on stdin, prints a deny decision when it matches, and stays silent
# (allowing the call) otherwise.

set -uo pipefail

command=$(jq -r '.tool_input.command // empty')

# `git`, any global options, then a destroying verb.
pattern='(^|[^[:alnum:]_-])git([[:space:]]+-[^[:space:]]+([[:space:]]+[^[:space:]]+)?)*[[:space:]]+(checkout|restore|reset|clean)([[:space:]]|$)'

if printf '%s' "$command" | grep -Eq "$pattern"; then
  reason='Blocked by .claude/hooks/block-destructive-git.sh: git checkout/restore/reset/clean destroy uncommitted work and are forbidden by AGENTS.md §7. Ask the user to run it, or use a non-destroying alternative (git switch to change branch, git stash to park work, git diff to inspect).'
  jq -nc --arg reason "$reason" '{
    hookSpecificOutput: {
      hookEventName: "PreToolUse",
      permissionDecision: "deny",
      permissionDecisionReason: $reason
    }
  }'
fi

exit 0
