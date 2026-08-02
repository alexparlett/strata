---
name: adversarial-review
description: Review a change through isolated, hostile critics that assume it is wrong — then refute every finding before reporting it. Use before merging, on a change you just wrote yourself, when a normal review came back clean too easily, or when asked to red-team, stress-test, or adversarially review a diff or task.
argument-hint: "[low|medium|high|max] [PR number/URL, git ref, path, or task id] [--lens a,b]"
---

# Adversarial review

A normal review asks "is this good?". This one asks the three questions the author cannot ask
honestly about their own work:

1. **Does it work?** — what breaks it, at runtime, on the worst input.
2. **Does it work the way it claims?** — is the mechanism the one the contract calls for, or one
   that merely passes today's case.
3. **Does it need to exist?** — is any of this earning its place, or is it scope, scaffolding and
   pre-work nobody asked for.

The value is in **who asks**. A model that can see the reasoning behind a change rates that change
more favourably than a neutral reader would, and the gap is widest on the changes that are actually
wrong — self-review reproduces the reasoning error rather than catching it. So the critics run in
**fresh, isolated contexts with read-only tools**, and get artifacts only: the diff, the files, and
the written contract. Never the story.

Two stages with opposite biases, and that is the design. Discovery is tuned to **over-report** — a
hostile critic that hedges finds nothing. The gate is tuned to **kill**, defaulting to refuted on
ambiguous evidence. Hostile discovery alone is a noise machine; a single careful pass is the rubber
stamp it was built to replace. Neither half works without the other.

## Run it

This skill collects the settings and hands off to
[`.claude/workflows/adversarial-review.js`](../../workflows/adversarial-review.js), which owns the
fan-out, the panel votes and the tally. Orchestration is deterministic on purpose: a review whose
verdict is computed from the same tally every time is one you can act on, and a synthesiser that
can round a `BLOCK` down is not a gate.

### 1. Resolve the scope

An uncommitted change sits in one of **four disjoint states**, and **each git command sees exactly
one of them**. Miss a command and you miss that state entirely: the diff comes back empty, the
critics review nothing, and the run returns `CLEAN` over unreviewed code — on precisely this
skill's headline case, a change you just wrote.

| State | Only this shows it |
|---|---|
| committed on this branch | `git diff "${CLAUDE_CODE_BASE_REF:-origin/HEAD}...HEAD"` |
| staged, not committed | `git diff --cached` |
| unstaged working tree | `git diff` |
| untracked | `git ls-files --others --exclude-standard` |

```bash
git status --porcelain
git diff "${CLAUDE_CODE_BASE_REF:-origin/HEAD}...HEAD" && git diff && git diff --cached
git ls-files --others --exclude-standard
```

`git status --porcelain` is the inventory, not a substitute: it *names* every state (`??`
untracked, `M`/`A` staged in the first column) but carries no content, and it abbreviates a
directory to one line — `?? .claude/agents/` is a directory, not a file — which is why the
`ls-files` expansion is separate.

Untracked files have no hunks: **the whole file is the change**, and the brief must say so, or a
critic told to "read the changed files" goes looking for a diff that does not exist.

**Do not edit this command by substitution.** It has already regressed once: a rewrite that added
untracked coverage dropped `--cached` in the same stroke, trading the untracked hole for an
identical staged one, and staged-not-committed is the single most common state for a change you
are about to review. Check any replacement against all four rows above.

`"${CLAUDE_CODE_BASE_REF:-origin/HEAD}...HEAD"` rather than `main...HEAD`: the three-dot form is
already merge-base relative, but the base branch is not always `main`, and the harness publishes
the right one. Use `@{u}..` when you want only what this branch added on top of its upstream — and
state the commit count, because a branch is often many commits and `HEAD~1` silently reviews one.

Targets:

- **no argument** — the three readings above. The default.
- **a PR** — `123`, a URL, or `owner/repo/pull/123`:

  ```bash
  gh pr view <n> --json title,body,files,baseRefName && gh pr diff <n>
  ```

  **The diff is ground truth; the PR description is a claim *about* it.** Put the description in the
  contract as a claim to audit, never in the scope as context to believe — it is exactly the
  self-narration the isolation exists to keep out, and a description that disagrees with its diff is
  a finding the contract lawyer should return. `gh pr list` if no number was given.

  Check before spending anything: a closed or merged PR, a draft, or one already reviewed
  (`gh pr view <n> --comments`) is not worth six critics. Say which it was and stop.
- **a ref** (`HEAD~3`, a tag) — `git diff <ref>`.
- **a path** — that file or directory in full, not only its changed lines.
- **a task id** (`AA-04`, `P4-01`) — the diff above, plus that file under `.claude/tasks/` read as
  the acceptance contract.

### 2. Read enough to name the contract and the triggers

Read the changed files, and the `docs/reference/` entry for each area the diff touches (routing
table in [CLAUDE.md](../../../CLAUDE.md)). You are **not** forming an opinion here — you are
identifying which written rules are in play, and which gated lenses the diff has woken:

| Trigger | Fires when the diff touches |
|---|---|
| `trust` | `strata-agent`, MCP, the SQL/DDL policy, path or file IO, config writes, export/`COPY` |
| `freya` | any `strata-freya` component, layout, event or theme change |

### 3. Call the workflow

```
Workflow({ name: "adversarial-review", args: {
  scope:     "<what to review, and the exact git command that produced the diff>",
  tier:      "low" | "medium" | "high" | "max",
  contract:  "<the AGENTS.md sections, docs/reference/ files and task file in play>",
  triggers:  ["trust", "freya"],
  fileCount: <changed file count>,
  lenses:    ["saboteur", …]   // only when --lens was passed
}})
```

### 4. Report the findings, then state the verdict

The workflow returns `{verdict, tally, findings, report, level, ran}`.

**If `verdict` is `FAILED`, report that and stop.** Every critic died, so nothing was reviewed —
do not call `ReportFindings` with an empty list, and never describe it as clean. Say the review did
not run, quote `message`, and offer to re-run. An empty findings card is indistinguishable from a
clean pass, which is the one thing this must never be mistaken for.

**Call `ReportFindings` first**, so the host renders the findings card — grouped by file, with
category chips, a `verdict` badge per row, and the effort level in the header. `report` is already
in that tool's exact shape and sorted most-severe first, so pass it straight through and transform
nothing:

```
ReportFindings({ findings: <report>, level: <level> })
```

Building it in the script rather than mapping it here is deliberate: a shape the caller has to
transform is a shape the caller can get wrong, and this card is the artifact most people actually
read. **Do not also print the findings as prose** — the tool's own rule, and a duplicated list is
just noise beside a card that already scrolls.

Then add the part the card has no field for. `ReportFindings` carries no severity and no gate
result, so the verdict is yours to state, in three lines under the card:

```markdown
**Ran:** <ran, verbatim>  ·  **Verdict:** BLOCK / CONCERNS / CLEAN (<n> critical, <n> warning, <n> note)

<two or three sentences: where the real risk sits, and the single most important fix.>
```

Report `ran` **verbatim** — a tier is a claim about how hard the change was looked at, and a `low`
report that reads like a `max` one is the failure this whole method exists to prevent. Carry the
workflow's own log lines too, wherever a lens was skipped, a batch dropped or a shape collapsed.

If the user presses **Apply fixes**, re-report afterwards with `outcome` set per finding
(`fixed` / `skipped` / `no_change_needed`). Never re-report a finding as `fixed` without having
changed the line it names.

## What the tier buys

Effort is **yours to spend**, and it is a real quality dial — a `max` critic is a better critic
than a `high` one, and the workflow passes the level through to every agent rather than pinning
one. The tier buys two things:

| Tier | Effort | Discovery shape | Panel |
|---|---|---|---|
| `low` | medium | one critic carrying every lens | 1 voter |
| `medium` | high | the 4 always-on lenses, gated ones on trigger | 1 voter |
| `high` | xhigh | all 6 lenses, gated ones regardless of trigger | 3 voters, majority keeps |
| `max` | max | all 6 lenses | 3 voters, **plus** the red-team pass |

`max`'s extra phase is what the tier is really for: every survivor is **red-teamed** — not on
whether it is real, which the panel settled, but on whether its severity is honest and its failure
as reachable as the claim implies. A `max` run can therefore return *fewer* findings than a `high`
one, and better ones.

A small change (3 files or fewer) collapses to the single-critic shape at `medium`, and says so.
Proportionate to the change, still panel-verified.

### The panel reads batches, not one candidate each

A voter takes **ten candidates at a time and votes on each**, so the panel costs
`voters × ceil(sites/10)` agents rather than `voters × sites`. Every voter still reads
independently and the majority is still per-candidate — only the packing changed.

This is not a micro-optimisation, and the arithmetic is not academic. Measured on a 7-file diff:
six critics raised **53 candidates over 32 distinct sites**, which per-candidate voting billed as
**159 voters — 165 agents in flight**, with 21 of them re-judging a site another lens had already
raised. Batched and deduplicated, the same review is **18** (6 critics + 3 voters x ceil(32/10)). Discovery is deliberately tuned to
over-report; putting the widest possible gate directly behind a firehose is how that design choice
turns into a bill.

Two bounds keep it there, and both announce themselves rather than truncating quietly:

- **Dedup runs before the panel, never after** — and it keys on **position *and* claim**, not on
  `file:line` alone. Lenses hunt different things, so two of them citing one line is routine and is
  not agreement: "unwrap panics on an empty batch" and "a second handler replaces the first" can sit
  on the same line and share nothing. Merging on position alone deletes one of them before the panel
  can judge it *and* reports the collision as convergence, promoting a severity for agreement that
  never happened. Claims are clustered by content-word overlap, and the threshold is set to
  **under-merge**: failing to merge two identical claims costs one panel slot and a missed
  promotion, while merging two different ones destroys a finding outright.
- **A single lens is capped at 12 candidates**, keeping its own highest severities. A critic past
  that is listing observations, and the long tail is what the panel pays for.

## What the tier does not buy

The gate's floor is one voter, never zero, and three things stay fixed at every tier because each
is the whole difference between this and a slower `/code-review`:

1. **Isolation.** Merging lenses into one agent to save tokens gives you one reviewer with six
   headings — correlated bias wearing the costume of a panel. `low` folds the lenses into a single
   critic *and tells it so* ("you are the ONLY critic"), which is honest; it does not pretend six
   independent critics agreed.
2. **The refutation gate.** Cut it and you ship unfiltered hostile output — the false-positive
   flood that gets a reviewer switched off after two runs.
3. **Whole-file reading.** Diff-only critics miss interaction defects, which are most of them.

Promotion works the same way at every tier: a site reached independently by two or more lenses
moves up one severity. That signal is only worth anything because the critics cannot see each
other — convergence means something only if it could have failed, which is also why the merge is
claim-aware: a predicate that cannot tell agreement from collision is not reading the signal it
claims to read. Promotion runs **before** the red team, never after, so `max`'s severity correction
is the last word rather than something a later pass silently undoes.

## Rules that keep it honest

- **Never put your reasoning in the brief.** Not why the change was made, not your own assessment,
  not which parts you think are fine, not anything from this session's history. If it is not in the
  repo, the critics do not get it. A brief that explains the intent rebuilds the echo chamber the
  isolation exists to break, one paragraph at a time. The commit *subject* and the task file are
  fair game — those are the claim being audited.
- **Every lens must name its strongest candidate**, even a weak one; "LGTM" from a hostile lens
  means the lens did not run. That obligation is on **discovery** only.
- **A `CLEAN` verdict is a result.** Discovery over-produces and the gate is allowed to kill all of
  it. Never invent a finding to fill an empty section, and never inflate a `NOTE` to make the run
  look worthwhile.
- **The reviewer never fixes** — a checker that can fix starts reviewing its own fixes. Fixes are a
  separate decision, after the report. Be precise about how much of that is enforced: the agent
  definitions withhold `Edit`, `Write` and `NotebookEdit`, and this repo's `PreToolUse` hook blocks
  destructive git, but `Bash` is granted whole for `git`/`rg` and unscoped `Bash` is a write path.
  Agent `tools:` takes bare tool names — its parenthesised form names `Agent`s and `Workflow`s, not
  command patterns — so scoped `Bash(git diff:*)` is only available on a command or skill's
  `allowed-tools:`. Until that is applied here, read-only is *withheld editors plus a rule in the
  prompt*, not a sandbox, and saying otherwise would be the kind of claim this skill exists to catch.
- **Report what actually ran.** The workflow logs every skipped lens, dropped batch and collapsed
  shape. Carry those into the report — silent truncation reads as coverage.

## Known limits

- **Negation blindness** is mitigated by the contract lawyer's negation pass, not solved. This
  never becomes the gate that replaces `cargo test` and `schema_in_sync`; it runs in front of them.
- **A vague contract yields vague criticism.** The critics can only enforce what is written down.
  A thin task file produces a thin contract lawyer, which is itself worth knowing before the change
  lands.
- **A large diff exceeds a single critic.** Split by area and run the set per area rather than
  handing over a diff nobody can hold.

## Neighbours

- **`/code-review`** (the `code-review@claude-code-plugins` command) — four parallel agents over a
  PR, per-issue validation subagents, a confidence threshold, and `--comment` to post inline. The
  right tool for **someone else's** PR, and it is tuned the opposite way to this one on purpose:
  it tells its reviewers *"if you are not certain an issue is real, do not flag it"*, because a bot
  commenting on every PR pays for a false positive in reviewer trust. This skill is invoked
  deliberately, so discovery over-reports and a panel does the killing. Borrow its refusals, not its
  timidity — the categories it will never flag (pre-existing, linter-catchable, rule silenced at the
  line, pedantry) are in the refuter's kill list, and they are about *what is never worth raising*,
  which is a different thing from hedging on what is.

  It also **gives its reviewers the PR title and description as author intent**, where this skill
  hands them over as a claim to audit. Both are right for their case: reviewing someone else's work,
  intent is legitimate context you lack; reviewing your own, it is the contaminant the isolation
  exists to remove.
- **`/security-review`** — one deep lens over the branch. The trust auditor here is triage, not a
  replacement.
- **This skill** — for a change **you** just wrote, or one that came back clean suspiciously fast.
  Spend it where an escaped defect is expensive; that is what the tier is for.
