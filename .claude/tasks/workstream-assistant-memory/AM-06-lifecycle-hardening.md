# AM-06 · Lifecycle hardening

**Workstream:** Assistant memory · **Status:** ⬜ · **Depends on:** AM-02, AM-03

## Goal

The store's whole failure and change surface, made honest: a bundled-model bump re-embeds in
the background; a corrupt or unopenable store degrades to memory-off-for-this-window without
a dialog; Clear has one store-side implementation; and the tier statement holds end to end —
*the worst outcome is losing what the assistant learned, never what the user wrote.*

## Current state (verified 2026-08-13)

- AM-02 writes the `embed_model` tag into the table metadata on first embed; the model
  identity is an **app constant** (the bundled artifact), so a mismatch can only mean the app
  was updated with a deliberately bumped model — never user config.
- AM-03's mount path parks `None` on an open failure and logs; this task owns the wording and
  the reconciliation.
- The precedents: the config file's three tiers (absent = first use; unparseable = kept aside
  as `.corrupt` then replaced; unreadable = writes latched off for the process — AGENTS §2);
  `chat_store.rs:23-28`'s degradation ladder; the chats `Clear` confirm at the window root;
  a drop that is deleting data **finishes** and only its reporting is skipped (AGENTS §3).
  Lance is a directory, not a single JSON document — "kept aside" maps to renaming the
  directory to a `.corrupt-…` sibling, which `tidy_strata_dir` must **not** sweep (it is
  evidence, not a temp).
- Vectors are a cached derived fact (the workstream's own rule): any vector-side fault is
  answered by discard + re-embed, never by a user-visible error.

## Build

1. **Model-tag reconciliation** at `Memories::open`: tag matches → nothing; tag absent →
   nothing (AM-02 embeds lazily); tag differs from the app constant → null **all** vectors
   (one incremental update) and kick a background re-embed on the facade's runtime —
   `embed_batch` chunks of 64, merged per chunk so partial progress survives an interruption;
   unembedded rows score on the floor meanwhile. One tracing line at start and end.
2. **Open-failure taxonomy**: absent directory → create (first use); Lance open error on an
   existing directory → rename to `.strata/memory.lance.corrupt-<ts>/`, create fresh, log
   the rename (the unparseable tier — learned facts are regenerable, user text is not
   here); rename itself fails (the unreadable tier) → `None`, memory off for this window,
   logged once — **never** delete or overwrite what could not be read.
3. **`Memories::clear`**: one implementation — drop and recreate the table (cheaper and more
   honest than a delete-all scan); AM-05's Clear press calls it through its existing confirm.
   Idempotent; absent-is-success (the `clear_history` precedent, project.rs:392).
4. **Window teardown**: an in-flight distill or re-embed chunk finishes its store write (the
   facade's runtime outlives the window's interest; writes are chunk-atomic by Lance's own
   commit) — verify nothing UI-side blocks on it and the reporting is skipped, not the work.
5. **Tests**: tag-mismatch nulls and re-embeds (observable: vectors present → absent →
   present with the new tag); chunked re-embed interrupted after one chunk leaves a
   searchable store and resumes on next open; corrupt-directory rename path (feed Lance a
   garbage directory); rename-refused path latches off without touching the directory;
   `clear` then immediate `apply` works.

## Acceptance

- An app carrying a bumped model constant reopens a project, re-embeds in the background,
  and search quality degrades to the floor only for not-yet-re-embedded rows — no dialog,
  no user action.
- A corrupted store never blocks the window, never loses the corrupt bytes, and starts
  learning again immediately.
- Clear is one funnel shared by the panel.
- Full check green.

## Files

`crates/strata-core/src/memory.rs` (+ `embed.rs`) — reconciliation, taxonomy, clear ·
`crates/strata-core/src/project.rs` (only if the corrupt-rename helper lands beside the
other path fns) · `crates/strata-freya/src/apps/project/app.rs` (the open-failure wording,
if any surfaces beyond tracing) · tests beside the module.

## What is NOT this task

The panel's confirm UI (AM-05). The bundle/CI mechanics of shipping a new model version
(AM-02's pipeline; this task only reacts to the constant changing). Any store migration
beyond vectors — a future `Memory` schema change defines its own migration when it happens
(the version field exists; do not build speculative machinery now).
