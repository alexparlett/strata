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
- **CI runs that same check on every PR** (`.github/workflows/ci.yml`): `cargo test --workspace
  --locked` on **macOS** (the platform we ship — a green Linux build proves nothing about the muda
  menubar or the traffic-light gutter), with `submodules: true`, because the build resolves Freya by
  local path and without the fork checkout nothing compiles. `--workspace` and not a bare
  `cargo test`, which `default-members` would narrow to `strata-freya` alone. It asserts the
  submodule sits at the recorded gitlink **before** compiling, so §6's unpushed-fork-commit trap
  fails in seconds with that named as the cause instead of as a missing method 40 minutes in.
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

