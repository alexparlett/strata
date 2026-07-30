---
name: freya-submodule
description: Check out and verify the crates/freya fork submodule. Use in any fresh worktree or clone before the first build, when crates/freya is empty or missing, when git submodule status shows a - or + prefix, or when a build fails with missing freya crates or a "no method named ..." error on a freya type.
---

# Initialize the Freya fork checkout

`git worktree add` does **not** populate submodules, and every worktree carries its **own**
`crates/freya` checkout — so a fresh worktree has an empty fork directory, and nothing builds
(the workspace resolves Freya by local path, root `Cargo.toml` `[workspace.dependencies]`).

## The sequence

1. Read the state first:

```bash
git submodule status
```

   - `-<sha>` — not initialized (the fresh-worktree case). Continue.
   - `+<sha>` — checked out at a **different** commit than the superproject records. If you have
     just deliberately edited/committed in the fork, that is expected; otherwise continue — the
     update below moves it to the recorded gitlink.
   - `<sha>` (space prefix) — already correct; stop here.

2. Populate / move to the recorded commit:

```bash
git submodule update --init --checkout
```

3. Verify — no prefix, and the checkout matches the gitlink the index wants:

```bash
git submodule status
```

```bash
git ls-files -s crates/freya && git -C crates/freya log -1 --format=%H
```

   The two SHAs must match (the `ls-files` line's second column is the gitlink).

## If step 2 fails to fetch the commit (the unpushed-gitlink trap)

An error like `fatal: remote error: upload-pack: not our ref <sha>` (or "Fetched in submodule
path 'crates/freya', but it did not contain <sha>") means the recorded fork commit was **never
pushed** to `github.com:alexparlett/freya` — AGENTS.md §6's trap, which has bitten before
(P4-03/P4-04). The object usually exists in the **main repo's** fork checkout. The main worktree
is the first entry in `git worktree list`, so derive it rather than assuming a path, fetch from
its fork checkout, then re-run the update now the object is local:

```bash
git -C crates/freya fetch --no-tags "$(git worktree list --porcelain | awk '/^worktree /{print $2; exit}')/crates/freya" $(git ls-files -s crates/freya | awk '{print $2}')
```

```bash
git submodule update --checkout
```

Then tell Alex the fork commit needs pushing — fresh clones and CI hit the same wall until it is
on the remote, and CI asserts the gitlink before compiling for exactly this reason.

## Cautions

- **Do not** reach for `git checkout` / `reset` inside the submodule to "fix" its state by hand —
  destructive git is hook-blocked, and `git submodule update --checkout` is the correct,
  non-destructive mover anyway (it refuses nothing you need; a dirty fork working tree is
  preserved or the command says why not).
- When editing fork files by absolute path afterwards, confirm the path runs through **this
  worktree's** `crates/freya`, not the main repo's copy — each worktree has its own.
- A build error about fork API ("no method named `set_window_parent`") is diagnosed by the same
  comparison as step 3: gitlink vs actual checkout, before concluding anything about the API.
