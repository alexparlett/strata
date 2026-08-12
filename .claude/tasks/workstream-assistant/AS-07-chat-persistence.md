# AS-07 · Conversations survive the window

**Workstream:** Assistant · **Status:** ✅ · **Depends on:** AS-04 (the transcript it persists)

## What shipped, and the three places it differs from the plan below

Built 2026-08-11. The store, the list, the retention cap and the export are in; **AS-03 closes
with it**. Three decisions were taken during the build and the sections below are older than
them where they disagree:

1. **A restored conversation is continuable, so the model's own memory is stored too.** §2 as
   written kept the transcript alone. That restores a conversation you can read and cannot
   continue — the *appearance* of one — and it is what `INVARIANTS.md` already said ("the value
   that has to survive is the `Conversation` the model reads back"). Verified before building:
   the transcript cannot reconstruct that list, because the resolved `@`-mention bodies, the tool
   results, the captured reasoning parts and the `offer_sql` call/response pairs exist **only**
   there — and a failed turn plus the differing caps make the two lists genuinely diverge. The
   seam is `Conversation::{to_json, from_json}` in `strata-agent`, JSON-valued so `genai` still
   stops at that crate's edge.

   **The consequence, stated:** what rides on disk is genai's own serde shape at the pinned
   version, not a mirror vocabulary of ours. A `genai` upgrade that moves it must bump
   `CHAT_VERSION` or rely on `Read::Memoryless` — the third degradation tier, which opens the
   conversation with a fresh memory rather than losing the user's transcript.

2. **A stale `offer_sql` card degrades silently.** §2 asked it to say so "in the editor's own
   words". It says nothing: the card loses its Run press and renders as the ordinary code block
   explanatory SQL already renders as. The user never ran it, and a complaint that their catalog
   moved is not news about their conversation. The re-check is `tools.validate` on **hydrate** —
   a dry plan, so "reopening makes no engine call" reads as no run, no scan, no snapshot, no
   network, which is the bullet's spirit; there is no pure check that can know a table was
   dropped, because the catalog is the thing being asked. The mark is **never stored** — it is an
   answer about the catalog as it stands, so persisting it would leave a card retired after the
   table it named came back.

3. **Clear is per project, in the chat pane's own menu — not Settings.** §4 put it in
   Settings ▸ AI ▸ Chat, which is app-global while the files are per project; a control there
   would promise a sweep it cannot honestly perform (a project on an unmounted disk is
   unreachable by construction). Settings keeps the **cap** alone, which is what closes AS-03.

**And one thing the plan did not ask for** (Alex, same session): an **ellipsis menu** beside the
close button, holding *Export chat…* and *Clear conversations…*. Export writes **Markdown** — the
field's convention, and the JSON already lives on disk as the store, so a JSON export would
duplicate it. Per-row delete stays in the switcher, where it can reach a conversation that is not
the open one.

### What the adversarial review corrected before this landed

Ten defects, four of them in the first pass's own load-bearing wiring. Recorded because each was a
plausible-looking shape that was wrong:

- **Export chat did nothing at all.** `spawn` binds to the scope current during dispatch — a menu
  item's — and the same handler closed the menu, so the task was dropped before its first poll.
  The press now records the intent and the header's own scope performs it.
- **A pick change never marked the conversation dirty**, so changing the model and closing the
  window silently lost it — the exact case the field's doc claimed it caught. All three composer
  controls now edit through `Chats::repick`.
- **Root-scoped tasks wrote subtree state after an await.** `spawn_forever` survives a re-root that
  drops the state it then writes. The presses are scope-bound now; the turn task, which genuinely
  must outlive the pane, holds `Chat::running` until its record is written so `stop_all` still
  reaches it.
- **The confirm was mounted inside the chat header**, where its key barrier is last in document
  order: Esc over the open dialog would have cancelled a running query instead. It is at the
  window root now, and the per-row delete — which now removes a *file* — asks through it too.
- Eviction shelved a conversation without writing it; `chats/` temps were never swept; the stale
  offer mark was persisted and could never recover; two doc claims were false.

### The cancel race, recorded rather than fixed

The writes hang off three places: the fold task's `Settled` arm (race-free — AS-02 commits to the
model's memory *before* it emits `Settled`), the stop press, and the subtree's teardown. On the
last two the cancelled turn's own `Staged::commit` runs on the assistant runtime and may land
*after* the write. Both interleavings leave a valid provider tail, so the bounded cost is that a
conversation quit mid-turn may reload with the stopped turn visible in the transcript and absent
from the model's memory. The transcript is never lost. Closing it would mean awaiting the settle,
which contradicts "a cancel is a drop" — do not re-litigate.

## Goal (as planned)

A conversation outlives the window it was held in. Close the project, reopen it, and the chats
are where they were — a list, newest first, each openable back into the pane.

AS-04 shipped v1 with the opposite rule stated outright: *"No conversation persistence; closing
the window is the end of the transcript."* That was the right v1 boundary and is now the thing to
remove. It is a whole feature rather than a line: a store, a retention policy, a list surface, and
a decision about what a persisted step card still promises.

## Why this is not just "serialize the satellite"

The transcript is an in-memory fold of AS-02's event stream, and about half of what it holds is
alive rather than recorded — a pinned `@result`'s rows come from a snapshot that will not exist
next launch, a step card is a view over a run in a query session that is long gone. Persisting
naively either writes data the app cannot honour on reload, or writes the user's data to disk in
places nobody agreed to. Both questions are settled below.

## What to build

### 1. The store — one file per conversation, project-scoped

Chats belong to a **project**, not to the app: a conversation refers to that project's tables, its
tabs, its results, and means nothing beside a different one. So `.strata/`, on `history.jsonl`'s
precedent — a **satellite**, never a `ProjectState` store field and never `session.json`.

`.strata/chats/<uuid>.json`, one document per conversation. Not one big file: a conversation is
rewritten every time it grows, and a single `chats.json` makes every turn in every chat rewrite
every other one. Not JSONL either — a conversation is a document with a head (id, title, created,
the provider pick) and tens of turns, not an unbounded append log, and the atomic writer the rest
of `.strata/` uses (`util::write_atomic`, `util.rs:492` — temp file, rename over target) already
does exactly the right thing for a whole-document write at that size.

**Written at turn boundaries, never per delta.** A streaming reply arrives as many deltas a
second; the write happens when a turn *settles* — answered, failed, or cancelled. That also makes
the interrupted case fall out for free rather than needing a close-time hook: closing the window
drops the turn future, which is how AS-02 cancels, and a cancel settles the turn (its outstanding
tool calls are answered first, by contract), so the settle that writes has already happened.
A cancelled turn persists **marked as cancelled**, which is AS-04's rule for what it looks like
in the live transcript too. Cancelled is never failed, on disk as much as on screen.

**Add `chats/` to `ensure_gitignore`'s list** (`project.rs:445`). This is not optional and not
cosmetic: a transcript quotes the user's data — column values, row counts, whatever the assistant
read back in prose — and `project.json` is a *committed* file, so `.strata/` is a directory people
have in their repos. That function reconciles a missing line on every open, so existing projects
pick it up without a migration.

**Version the document.** A `version` field, and an unrecognised one is a file this build skips
with a log line, never a parse error that takes the pane down. `project.json`'s own schema-version
gate is the pattern.

### 2. What a persisted message keeps, and what it honestly drops

- **Prose, tool calls and their `Facts`** — SQL, query session, row count, `elapsed_ms`, a
  `stopped` reason. All of these are values already carried on AS-02's events, so a reloaded step
  card is showing what it always showed, not a re-derivation. Its *Open* and *Open and run*
  presses still work, because both are `actions::open_sql` over a string.
- **`offer_sql` cards** keep their statement and their Run press. The statement was validated
  against the catalog when the card was made, and the catalog may have moved since — so a reload
  re-validates before offering the press, in the editor's own words, rather than promising a Run
  that will not parse.
- **Mention chips** keep the *name* of what was pinned, never the resolved payload. A `@table`
  chip reloads as a chip; the `describe_table` result behind it was context for one send.
- **Inline mini-results are dropped, and say so.** They render from a run's own pages, which
  belong to a snapshot that is gone. The card keeps its facts (the row count is a recorded number)
  and loses the preview; a promote press is how you see rows again. Never a second results
  pipeline, and never a re-run on load — reopening a chat must not touch the engine.

### 3. The list surface

The pane's conversation list: newest first, each row a title, a relative time, and the provider it
last used. Reading the list must not read every transcript — load the **head** of each file, and
the turns on demand when one is opened.

A conversation has **no user-supplied title in v1**. It is derived from the first user message,
collapsed the way history collapses SQL for its preview (`util::collapse_sql`'s role, not
necessarily that function — this is prose). Renaming is a fair follow-on and is not this task.

New chat is a press; deleting one is a press with a confirm, on the app's own close-confirm terms
(one dialog, the engine's own wording where the engine has one).

### 4. Retention, and the two controls AS-03 is missing

This is what makes AS-03 incomplete rather than done. `history.jsonl` is capped and rotated from
config, with a Clear that unwrites the file, and chats need the same pair for the same reason —
an unbounded on-disk record of everything the user ever asked about their data is not a default
anyone opted into.

- **A cap** in `AppConfig`, alongside history's, applied on load exactly as history's is (the load
  rotates down, so lowering it in Settings actually shrinks the stored set rather than only
  showing less).
- **Clear all conversations**, in **Settings ▸ AI ▸ Chat**, which unwrites the files.

Both are `settings_merge!` fields like any other, and both live on the Chat pane beside the
new-chat defaults, because that is the pane about conversations.

## What is NOT this task

- No cross-project or app-global chat list. A conversation is its project's.
- No transcript search, no export, no rename, no pinning or folders.
- No re-running anything on load. Reopening a chat is a read of a file, and touches no engine.
- No sync, no cloud, no sharing.
- **Not the investigation workbench.** The workstream README banks a delegation surface that
  "arrives with transcript persistence, not before" — this task makes it *possible*, and does not
  start it. It stays its own task file when it comes.

## Acceptance

- A conversation held in a project window is there after a quit and relaunch, with its prose, its
  step cards and their facts, and its cancelled turns still marked cancelled.
- Reopening a chat makes no engine call and no network call.
- A step card reloaded from disk promotes into a tab (`Open` and `Open and run`) exactly as a
  live one; its inline preview is absent and the card says why rather than showing an empty grid.
- An `offer_sql` card whose statement no longer validates against the current catalog says so in
  the editor's register instead of offering a Run that cannot parse.
- A fresh `.strata/` and an existing one both end up ignoring `chats/`; a transcript never appears
  in `git status` in a project whose `.strata/project.json` is committed.
- Lowering the conversation cap in Settings removes stored conversations on the next open, and
  Clear leaves the directory empty; neither takes the pane down.
- A chat file from an unknown future version is skipped with a log line and the rest of the list
  loads.

## Freya components

The existing pane chrome from AS-04; standard `Button`/list rows; the app's one close/confirm
dialog shape for delete and clear (`CloseTarget`'s terms, not a second dialog); `form::Row` for
the two Settings controls.
