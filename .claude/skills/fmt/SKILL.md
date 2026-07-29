---
name: fmt
description: Format the Strata crates without reformatting the Freya fork. Use whenever you would reach for cargo fmt — before a commit, after a large edit, or when a review asks for formatting.
---

# Format Strata

```bash
cargo fmt -p strata-freya -p strata-core -p strata-model
```

That is the whole thing. Run it from the repo root (or any worktree root).

(`strata-forms` / `strata-forms-macro` were named here until they were deleted in 812afbc, which
made this command fail outright — `cargo fmt` errors on a `-p` it cannot resolve rather than
skipping it. See "When the member list changes" below; that check catches this in one command.)

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

## Not `strata-code-editor` either

`crates/strata-code-editor` is **vendored** from Freya's own editor and carries that project's
layout (vertical imports, `StdExternalCrate` grouping) even though it is a workspace member. Our
stable rustfmt collapses all of it — measured once at ~190 lines across seven files, landing as
pure noise in an unrelated PR and permanently diverging the vendored source from upstream. It is
excluded here for the same reason `crates/freya` is: it is not ours to reformat.

Format it deliberately, with the layout it is written in, if you have actually changed it:

```bash
cargo +nightly fmt -p strata-code-editor
```

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
