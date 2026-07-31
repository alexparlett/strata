---
name: fmt
description: Format the Strata crates without reformatting the Freya fork. Use whenever you would reach for cargo fmt — before a commit, after a large edit, or when a review asks for formatting.
---

# Format Strata

```bash
cargo fmt -p strata-freya -p strata-core -p strata-model -p strata-code-editor -p strata-command-macro -p strata-agent
```

That is the whole thing. Run it from the repo root (or any worktree root).

(`strata-forms` / `strata-forms-macro` were named here until they were deleted in 812afbc, which
made this command **fail outright** — `cargo fmt` errors on a `-p` it cannot resolve rather than
skipping it, so the stale list did not merely miss those two, it formatted *nothing at all*,
silently, for as long as it was wrong. If this command ever fails that way again, re-derive the
list from "When the member list changes" below rather than dropping the failing name and hoping.)

## Never `cargo fmt --all`

`--all` does not mean "all workspace members". From `cargo fmt --help`:

> `--all` — Format all packages, **and also their local path-based dependencies**

`crates/freya` is our Freya fork, resolved as a **local path dependency** (root `Cargo.toml`,
`[workspace.dependencies]`), so `--all` formats it *by design*. The root manifest's
`exclude = ["crates/freya"]` does not save you: exclude controls workspace *membership*, and
`--all` reaches past membership into path deps.

The damage is quiet and large. The fork carries its own `rustfmt.toml` — `imports_layout = Vertical`,
`imports_granularity = Crate`, `group_imports = StdExternalCrate` — none of which our stable
toolchain applies, so our rustfmt happily collapses the fork's vertical imports across **hundreds**
of files. Measured once: 344 files, 882 insertions, 4006 deletions, none of it intended.

It is easy to miss, which is why this skill exists rather than a line in a doc:

- `git status` in the superproject shows one line, `m crates/freya` — a lowercase `m`, meaning the
  submodule's *working tree* is dirty, not the gitlink.
- `git submodule status` shows **no** `+` prefix, because the recorded commit hasn't moved.
- Nothing fails to build, and the PR diff is unaffected — so it survives review and sits in the
  worktree until someone commits it into the fork by accident.

## If it already happened

Park it; don't delete it. `git checkout` / `restore` / `reset` / `clean` are hook-blocked for
agents (`.claude/hooks/block-destructive-git.sh`), and stash is both allowed and reversible:

```bash
git -C crates/freya stash push -u -m "accidental cargo fmt churn"
```

Then confirm both are clean:

```bash
git -C crates/freya status --short && git submodule status
```

The stash is recoverable with `git -C crates/freya stash pop` — check the diff is *only* import
re-wrapping before dropping it, in case a real fork edit got swept in with it.

## `strata-code-editor` is ours now

It was excluded here for a long time, and that exclusion is **no longer right** — it is in the
list above, on purpose.

The reasoning that kept it out: `crates/strata-code-editor` is **vendored** from Freya's own editor
and was written in that project's layout (vertical imports, `StdExternalCrate` grouping), so our
stable rustfmt collapsed all of it — ~190 lines across seven files of pure noise in an unrelated
PR, and a permanent divergence from the upstream source a `diff -u` was meant to stay legible
against.

What changed is the divergence itself. The vendored crate is now ~3500 lines against upstream's
~1800, and every file differs structurally rather than incidentally: highlighting is
theme-independent (`SyntaxKind` instead of a baked `Color`), diagnostics and an autocomplete popup
are ours outright (`completion.rs` has no upstream counterpart at all), the type lives on the
theme, and `CodeEditorData` grew half its public surface. A file-level `diff -u` against upstream
stopped being the way anyone reads this crate somewhere around P2-04. What is left to track is the
handful of upstream *changes* — two of them between the vendoring base and the 2026-07 fork
update — and those are read as fork commits, not as a whole-file diff.

So the crate is formatted like the rest of our code, with our stable toolchain and the repo's
default config (we carry no `rustfmt.toml`; the fork's is inside `crates/freya`). Do **not** reach
for `cargo +nightly fmt -p strata-code-editor` to "keep the upstream layout" — that was the old
advice, it now produces the same output as the line above anyway (there is no config for nightly to
apply), and running it separately just invites the whole crate to churn twice.

`crates/freya` is still excluded, and that has not softened at all — see above.

## When the member list changes

The command names the five it owns explicitly, which is the point — it cannot silently grow to
include a path dependency. If a crate is added to `members` in the root `Cargo.toml`, add it here
too. To check the list matches:

```bash
cargo metadata --no-deps --format-version 1 | python3 -c "import json,sys; print(' '.join('-p '+p['name'] for p in json.load(sys.stdin)['packages']))"
```

`--no-deps` is what keeps the fork out of that answer, for the same reason `-p` keeps it out of the
formatting.

## Editing the fork itself

If you have *deliberately* changed fork files, format them with the fork's own config, from inside
it — it is its own workspace and its `rustfmt.toml` wants nightly:

```bash
cargo +nightly fmt --manifest-path crates/freya/Cargo.toml
```

See `AGENTS.md` §6 for the rest of the fork workflow (push the submodule, or a fresh clone can't
init it).
