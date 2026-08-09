# The fork, git, and verification

AGENTS §6 and §7 in full: when and how to change the Freya fork, and what counts as having
verified a change. [AGENTS.md](../../AGENTS.md) carries the one-line form of each rule.

## The Freya fork: when and how to change it

`crates/freya` is a git submodule of `github.com:alexparlett/freya`, resolved by **local checkout
path** — edits are picked up on the next `cargo build`, no push needed locally.

- **Fix limitations in the fork, not around it.** When an app design starts reaching for a
  workaround (a registry, a scale-factor correction, a duplicated theme token), the right move is
  usually a semantic fix in the fork — deterministic listener ordering, logical `root_size`,
  `SelectPlacement`, disabled colors on `ButtonColors`, `set_window_parent` all landed this way.
  The platform-specific half goes in its own `freya-winit` module beside `traffic_light.rs`
  (`cfg`-gated, a documented no-op elsewhere), the primitive on `RendererContext` (the only place
  that holds every window at once), and the discoverable API on `WinitPlatformExt` hopping to it —
  so app code never touches objc2 or a raw winit handle.
- Follow the fork's own `AGENTS.md` conventions when editing it; keep changes upstream-shaped
  (themed tokens, doc comments, examples).
- **After changing the fork, push it** — the committed gitlink must exist on the fork remote or
  fresh clones/CI can't init the submodule. This is not a formality: P4-03's `set_window_parent`
  commit was never pushed, so P4-04's worktree could not build the app at all (`no method named
  set_window_parent`), and no amount of `git submodule update` fixes it — the object isn't on the
  remote to fetch. If you hit that, the commit is in the **main repo's** `crates/freya` checkout:
  `git -C crates/freya fetch --no-tags /abs/path/to/main/repo/crates/freya <sha>` then
  `git merge --ff-only <sha>` (additive, and it keeps your own uncommitted fork edits as long as
  that commit touches different files — check with `git show --stat` first). Then push it.
- **Worktree traps — use the `freya-submodule` skill** (`.claude/skills/freya-submodule`), which
  owns the full sequence: `git worktree add` does not update submodules, so in any new worktree
  run `git submodule update --init --checkout` before the first build, then `git submodule status`
  (no `+` prefix). A `+` means the checkout is not the commit the superproject recorded; compare
  `git ls-files -s crates/freya` (the gitlink the index wants) against `git -C crates/freya log -1`
  before concluding anything about a build error in fork API. The skill also carries the recovery
  for the unpushed-gitlink trap above (fetch the sha from the main repo's checkout by absolute
  path, then update again). And every worktree has its **own** `crates/freya` checkout: when
  editing fork files by absolute path, confirm the path goes through *your* worktree, not the main
  repo's copy.

## Git, worktrees, and verification

- **Formatting is the `fmt` skill, never `cargo fmt --all`.** `--all` means "all packages *and
  their local path-based dependencies*" (its own `--help` says so), and `crates/freya` is a path
  dependency — so `--all` reformats the fork, whose `rustfmt.toml` our stable toolchain does not
  apply. Measured once: 344 files, 4006 deletions, none intended, and invisible in
  `git submodule status` because the gitlink never moves. Use `.claude/skills/fmt`, which names the
  four it owns explicitly (and fails closed on a stale list — `cargo fmt -p` errors out entirely
  on a non-member, so a wrong list formats *nothing*). `strata-code-editor` is one of the four as of
  the 2026-07 freya update: it was held out to keep a `diff -u` against upstream legible, and that
  stopped being how anyone reads the crate once it grew to ~2x upstream's size with `completion.rs`
  having no upstream counterpart at all. What is still tracked is upstream's *changes*, read as fork
  commits. `crates/freya` stays out, unchanged.
- **Build + `schema_in_sync` is the check.** After any theme change:
  `UPDATE_SCHEMA=1 cargo test -p strata-freya schema_in_sync` (the committed
  `themes/theme.schema.json` must match `theme.rs`'s `REGISTRY`). Sandboxes that can't build verify
  against fork source and hand off to a Mac build (see CLAUDE.md's environment note).
- **A change you wrote is reviewed by critics who cannot see why you wrote it** — the
  `adversarial-review` skill, in front of the build check and never in place of it. A model that
  can see the reasoning behind a diff rates that diff more favourably than a neutral reader, and
  the gap is widest on the diffs that are actually wrong: self-review reproduces the reasoning
  error rather than catching it, which is why a session that just built something and then reviews
  it reliably returns "looks right". So the critics are **isolated read-only subagents**
  (`.claude/agents/adversarial-reviewer.md`), one per lens, handed artifacts only — diff, files,
  and the written contract (`AGENTS.md`, the `docs/reference/` entry, the task file). Never the
  story: a brief that explains the intent rebuilds the echo chamber the isolation exists to break.
  **Effort is the user's dial and the panel is not on it.** `/adversarial-review low|medium|high|max`
  buys reasoning effort *and* panel width together — 1 voter at low/medium, a 3-voter majority panel
  at high, and at `max` a further phase that red-teams every survivor on whether its severity is
  honest, so a `max` run can return *fewer* findings than a `high` one and better ones.
  **A voter reads a batch of candidates, and dedup comes before the panel.** Ten at a time, so the
  panel costs `voters × ceil(sites/10)` and not `voters × sites` — independence and the
  per-candidate majority are unchanged, only the packing. That is not a micro-optimisation:
  measured on a 7-file diff, six critics raised 53
  candidates over 32 distinct sites, which per-candidate voting billed as **165 agents in flight**,
  21 of them re-judging a site another lens had already raised; batched and deduplicated the same
  review is 18. Dedup runs before the panel and never after — convergence is the
  promotion signal, to be counted once rather than paid for six times — and a single lens is capped
  at 12 candidates, logged, because discovery is tuned to over-report and putting the widest gate
  directly behind a firehose is how that choice becomes a bill.
  **The merge keys on position *and* claim, and promotion runs before the red team.** Lenses hunt
  different things, so two of them citing one line is routine and is not agreement; keying on
  `file:line` alone deletes one claim before the panel can judge it *and* reports the collision as
  convergence, promoting a severity for agreement that never happened — the promotion predicate
  then cannot distinguish the two, which is exactly the property it exists to read. Claims are
  clustered by content-word overlap, deliberately shallow and deterministic, with the threshold set
  to **under-merge**: a missed merge costs one panel slot and one promotion, a wrong merge destroys
  a finding outright and manufactures the agreement that promotes what is left. Promotion then runs
  **before** the red team rather than after it, or `max`'s only extra phase would be nullified on
  precisely the multi-lens sites it is most likely to reach, and the red team would be judging a
  severity that is not the one the report prints.
  A tier never buys the panel away: its floor is one voter, never zero,
  and isolation and whole-file reading are fixed at every tier because those three are the whole
  difference from a slower `/code-review`. `low` folds the lenses into one critic **and tells it
  so** ("you are the ONLY critic"), rather than pretending six independent critics agreed. That is
  the `claude-security` scan's shape, which collapses to a single researcher on a small diff and
  logs that it is "still panel-verified". Which is also why this is a **workflow**
  (`.claude/workflows/adversarial-review.js`) and not an Agent-tool fan-out: only `Workflow`'s
  `agent()` takes a per-call `effort`, so an Agent-tool version could not offer the dial at all —
  static frontmatter is all it has. The verdict is computed **in the script** from the severity
  tally, never asked of a model, because a synthesiser that can round a `BLOCK` down is not a gate.
  The tier is reported verbatim: a tier is a claim about how hard the change was looked at, and a
  `low` report that reads like a `max` one is the failure this method exists to prevent.
  **Discovery fails closed, not just the panel.** `agent()` resolves to null when a subagent dies
  or is skipped, and an empty candidate list reads downstream exactly like a clean diff, so a review
  in which every critic died would report `CLEAN` - and at `low`, or `medium` on a small change,
  there is exactly *one* critic, so a single dead agent is enough. A critic returning `findings: []`
  is a real clean result; a critic returning nothing at all is an absence of evidence. The two must
  not collapse: all-dead returns `FAILED` with a message saying so, because an empty findings card
  is indistinguishable from a clean pass.
  **Scope is four disjoint readings, and a description is a claim.** An uncommitted change sits in one of **four disjoint
  states**, and each git command sees exactly **one** of them: committed on this branch
  (`git diff "${CLAUDE_CODE_BASE_REF:-origin/HEAD}...HEAD"`), staged but not committed
  (`git diff --cached`), unstaged working tree (`git diff`), and untracked
  (`git ls-files --others --exclude-standard`). Miss a command and that whole state is invisible:
  the diff returns empty, no candidates are raised, and the run reports `CLEAN` over unreviewed
  code — on precisely this skill's headline case, a change you just wrote. Both halves of that have
  now happened on this very branch. First the untracked hole: the four files adding the skill were
  untracked, produced nothing from any `git diff`, and had to be named by hand in the scope brief
  twice. Then, fixing it, the rewrite dropped `git diff --cached` in the same stroke and traded the
  untracked hole for an identical staged one — and staged-not-committed is the *most* common state
  for a change about to be reviewed. Hence the table rather than a command: `git status --porcelain`
  is the inventory only, naming every state (`??` untracked, `M`/`A` staged) but carrying no content
  and abbreviating a directory to one line, so it can never stand in for the four. Untracked files
  have no hunks — the whole file is the change, and the brief must say so or a critic told to read
  "the changed files" hunts a diff that does not exist. The commands are run **one per line, never
  chained with `&&`** — a short-circuit is the third way a state goes missing, after a dropped
  command and a substituted one, and it is the least visible: `git diff
  "${CLAUDE_CODE_BASE_REF:-origin/HEAD}...HEAD"` exits **128** in any clone where
  `git remote set-head origin -a` never ran, so the staged and unstaged reads after it never
  execute. Measured in a scratch repo: the chained form reported 0 changed files where the separate
  form reported 2. And a non-zero exit means that state is **unread, not empty** — they print the
  same nothing, and only one of them may be reported as clean, so an unresolvable base is fixed or
  named in the brief rather than passed over. Do not edit the command by substitution;
  check any replacement against all four states. A PR target is `gh pr view <n>` + `gh pr diff <n>`,
  and its description belongs in the **contract**, not the scope: the diff is ground truth and the
  description is a claim *about* it, so a description that disagrees with its own diff is a finding
  the contract lawyer returns rather than context that explains the code away. An unread file and a
  clean file are not the same answer, and the critics are told to say which one they are giving.
  **A stage that cannot verify fails closed; a stage that only corrects keeps and marks.** These
  pull opposite ways and both are right. The panel drops a site whose batch lost a voter: a finding
  it could not verify must not reach the report, so a short panel is not keepable. The red team is
  the reverse — it only ever lowers a severity or removes one, so failing closed there would throw
  away work the panel already confirmed. It keeps the finding, marks it `redTeamed: false`, and
  names the batch that never answered. What is *not* optional either way is saying so. The original
  bug was silent on every channel at once: `parallel` resolved the dead batch to null,
  `.filter(Boolean)` erased the index so nothing could name it, the drop-count log stayed quiet
  because nothing had been dropped, and `ran.adversarialPhase` still read `true` — so findings
  shipped with severities nobody had checked under a report claiming `max` had checked them. Hence
  the batch index is carried through (as the panel's ballots already carried theirs) and
  `adversarialPhase` is `'partial'` with `adversarialUncovered` whenever a batch is missing. A phase
  may under-deliver; it may not claim otherwise.
  **Findings go through `ReportFindings`, and the script hands over the exact shape.** The host
  renders a findings card — grouped by file, category chips, a per-row verdict badge, the effort
  level in the header, and Apply-fixes/Walk-through actions — which is the artifact most people
  actually read, so the script returns `report` already in that tool's shape rather than leaving
  the caller to map it: a shape the caller has to transform is a shape the caller can get wrong.
  The panel is the verify pass, so every row carries a verdict — `CONFIRMED` when the vote was
  unanimous, `PLAUSIBLE` when one voter refused, because flattening those to one word discards the
  panel's own disagreement. Two things the card has no field for stay in prose beneath it: the
  severity tally and the `BLOCK`/`CONCERNS`/`CLEAN` gate result. Never print the findings as prose
  as well — a duplicated list is noise beside a card that already scrolls.
  Two stages with opposite biases, because neither survives alone — hostile discovery is tuned to
  over-report (a critic that hedges finds nothing), then a **refutation gate** takes every
  candidate to a fresh skeptic told to kill it, defaulting to refuted on ambiguous evidence.
  Discovery without the gate is a false-positive flood that gets the reviewer switched off after
  two runs; the gate without hostile discovery is the rubber stamp it replaced. Two rules carry the
  weight. **Each lens must name its strongest candidate** — "LGTM" from a hostile lens means the
  lens did not run — but that obligation is on discovery only, so a `CLEAN` verdict after the gate
  kills everything is a *result*, never a reason to invent or inflate a finding. And the **negation
  pass**: models underweight negation, so a rule phrased "never X" is the one a critic waves
  through, and `AGENTS.md` is written almost entirely in "never" — the fix is to grep the diff for
  the forbidden thing itself rather than asking whether the rule was violated, turning an absence
  the model must notice into a query the search answers.
- **CI runs that same check on every PR** (`.github/workflows/ci.yml`): `cargo test --workspace
  --locked` on **macOS** (the platform we ship — a green Linux build proves nothing about the muda
  menubar or the traffic-light gutter), with `submodules: true`, because the build resolves Freya by
  local path and without the fork checkout nothing compiles. `--workspace` and not a bare
  `cargo test`, which `default-members` would narrow to `strata-freya` alone. It asserts the
  submodule sits at the recorded gitlink **before** compiling, so §6's unpushed-fork-commit trap
  fails in seconds with that named as the cause instead of as a missing method 40 minutes in.
- **Only the tests that need the container runtime queue for it, and the split is a test target.**
  Everything the shared MinIO worker forces on a job — the repo-wide queue below, the cloud agent,
  the release step, the capacity retry — was being paid for by the whole suite, when one test file
  needs it. So the workflow is two jobs. `minio` keeps the apparatus and runs
  `cargo test -p strata-core --locked --test object_store_minio`: the **binary entire**, because
  `crates/strata-core/tests/object_store_minio.rs` is the only file in the workspace that mentions
  testcontainers, so drawing the line at the test target means a test added to it is covered
  without a workflow edit. It is also the cheap job to leave queueing — one package and its
  dependencies, so no Skia and none of the fork's UI crates. `test` is the same `cargo test
  --workspace --locked` as before with those tests named in a `--skip`, and queues behind nothing;
  it is the job a PR is normally waiting on, and now nothing on another branch can hold it up.
  The two lists are not kept in agreement by hand and must not be: `test` has **no** container
  runtime, so a minio test renamed or added without amending the skip runs there, finds nothing,
  and fails loud — which is what that test is built to do rather than pass quietly. Do not split
  this by package, by "slow vs fast", or by taste; the only axis that carries the property is
  whether the runtime is needed.
- **The container runtime is a single shared worker, so the job that uses it serializes repo-wide —
  and it queues rather than cancels.** Testcontainers Cloud gives this account one worker at a time
  and the connections test (W7) drives a real MinIO through it, so a second overlapping run is refused
  with `Failed to get a worker: ErrValidator: too many concurrent requests` — or that same refusal
  arriving as a truncated response, `hyper::Error(IncompleteMessage)`, which is the same fault and
  not a second one. The test then fails loud, correctly: it cannot tell a busy provider from a
  missing one, and it must never call the latter fine. Merging a PR is exactly when the overlap
  happens — the merge commit pushes to main while other branches are still building — which is why
  **main** was the ref that kept failing while its own PR had just passed. So the `minio` job carries
  a second, **job-level** concurrency group with a constant name: the workflow-level group is
  per-ref and supersedes within a branch, this one is repo-wide and only ever waits. Waiting is
  free — a pending job holds no runner and `timeout-minutes` does not start until it runs. The
  `queue: max` is load-bearing rather than decoration: the default, `single`, keeps **one** pending
  job per group and cancels any previously pending one, so a third concurrent run would be silently
  cancelled — and a cancelled run on main is no coverage of main at all, which is the failure the
  serialization exists to stop. `queue: max` cannot be combined with `cancel-in-progress: true`,
  which is the other reason superseding stays at the workflow level. Raising the limit instead is a
  billing decision, not an engineering one.
- **A cloud session outlives the job that opened it, so the job releases it — and the test still
  waits out a handover it cannot watch.** Serializing the job was shipped first as the whole fix
  and was not: it stops two *live* jobs colliding and does nothing about a session left behind by a
  job that has ended. The agent is started as a background process and nothing ever stops it — the
  runner VM is torn down under it — so the session, and the worker assigned to it, stays checked out
  on the provider's side until it times out there. That is how a fully serialized run on main was
  still refused a worker **five minutes after the only other job had finished**. Hence the
  `action: terminate` step, and `if: always()` rather than the default: a **cancelled** job is the
  case that matters most, because `cancel-in-progress` means we generate those deliberately and a
  cancelled run kills the agent in the way least likely to release anything. Even then the release
  happens where nothing on our side can observe it finishing, which is why
  `object_store_minio.rs` also **retries a capacity refusal, and only that** — the two spellings of
  one fault (`too many concurrent requests`, or the truncated response `hyper` calls
  `IncompleteMessage`), on a bounded budget. Every other failure panics with the message it always
  had: "no runtime" must keep failing loudly, or it reads as "the code is fine". Do not collapse
  these three into one mechanism — each covers a hole the others cannot reach.
- **The release path is a script CI calls, never a pipeline written in YAML.**
  `scripts/bundle-macos.sh` builds the universal binary, assembles the `.app`, signs, notarizes and
  makes the DMG; `.github/workflows/release.yml` sets up secrets and runs it. So the build a
  laptop makes and the build a release publishes differ only in what is *configured*, never in what
  is *done* — a release path that exists only inside a workflow file is one nobody can run when it
  breaks. Two rules the script holds. Signing **degrades honestly and says which rung it took**:
  ad-hoc with nothing configured, real signature with a Developer ID, notarized when notary
  credentials exist — and it deliberately will **not** fall back to an *Apple Development*
  certificate, which signs but cannot be notarized, so it would buy a signature that still fails on
  a tester's Mac while reading like success locally. And **the tag is created after the build, not
  before**: a published release's tag cannot be moved or deleted, so `gh release create --target`
  mints it only once there is a DMG to attach.
- **The version lives in one file and is reached through one script; a bump rides the publish.**
  `scripts/version.sh` is the only thing that knows the number is in
  `crates/strata-freya/Cargo.toml` — the bundle script reads it through that, and the Release
  workflow resolves *and writes* through it. Writing, not only reading, is the fix for a real bug: a
  version passed to the workflow moved the tag and not the manifest, and the bundle script reads the
  manifest, so `v0.4.0` shipped `Strata-0.2.0-universal.dmg`. Resolving is a separate entry point
  (`--resolve` touches nothing and needs no cargo) so a typo or a taken tag is rejected before a
  runner installs a toolchain, and writing updates `Cargo.lock` because the release build passes
  `--locked`. Then the tag rule above, pointed at the commit: a bump is **refused without the
  release box** rather than performed and discarded, so "just build me a DMG" cannot move the
  repository's version; and the commit is **pushed after the build and never rebased**, because the
  tag names that commit and a rebase would make a permanent tag point at a tree nothing ever built.
  The release notes are the signing rule again — written by `claude-code-action`, `continue-on-error`,
  falling back to GitHub's changelog with a warning that says so, because better notes are a better
  release page and not a precondition for having one.
- **The app bundle is self-contained, and that is a claim each new asset has to keep.** Themes are
  `include_str!`'d and the two families the themes name (`themes/*.json` `fonts`) are
  `include_bytes!`'d and registered through `LaunchConfig::with_font` in `main.rs` — because
  neither IBM Plex Sans nor JetBrains Mono ships with macOS, and a font that is merely *installed
  on the developer's machine* fails silently and only on somebody else's, falling back to the
  system UI font with the whole type scale going with it. Naming a new family or weight in a theme
  means embedding it in the same change; the weights are 400/500/600 because that is exactly what
  `typography` and the component overrides ask for. The icon is the same rule pointed the other
  way: `assets/icon/strata.png` is the master and the `.icns` is **generated during the bundle**,
  so there is no committed second copy of the artwork to drift from the design.
- **One Strata window across every session — enforced.** Several sessions can be live in several
  worktrees, and each can build its own binary; a second instance clobbers the shared app config
  (read once at startup, last writer wins for recents / settings / the open-project set). So
  `.claude/hooks/block-second-strata.sh` refuses `cargo run` while any Strata is alive anywhere,
  naming the worktree that owns it. A **refusal, not a kill**: the running window may be what the
  user is looking at. This is a convention between agent sessions, *not* an app-level single-
  instance lock — that is a real feature (one process, N windows, a second launch focuses) and
  belongs to P4-01.
- **No destructive git — now enforced, not merely agreed.** `git checkout`/`restore`/`reset`/
  `clean` are **blocked outright** for agents by a `PreToolUse` hook
  (`.claude/hooks/block-destructive-git.sh`, wired in `.claude/settings.json`). It reads the whole
  command string, so chaining one behind `&&`, `;` or `$(…)` does not get past it — which is
  exactly how the rule was broken while it was only written down. Both hooks bound the verb with
  "not an identifier character" on **each** side: the git one originally required whitespace-or-end
  *after* the verb, so `git reset;`, `git clean|cat` and `$(git clean)` slipped through the very
  chaining forms it claimed to catch (found while building the Strata hook, which had copied the
  pattern). If you add a third hook, copy the fixed pattern and test the terminator forms. Ask the user to run it, or reach
  for something that destroys nothing: `git switch` to change branch, `git stash` to park work,
  `git diff` to inspect. Any other delete/overwrite of work you didn't just create still follows
  the original rule: **standalone**, with an explicit description, and not at all when there is
  substantial uncommitted work in the tree unless you have asked. Cleaning up a failed script means
  removing the exact files it created.
- **Task files are the working contract.** Each `.claude/tasks/` file is self-contained; keep it
  true — record corrections, wiring notes, and ownership seams there as part of the change (the
  `FetchCatalog` correction and the P4-01 fail-loud seam both live in task files because sessions
  read them cold).

