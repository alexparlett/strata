# AS-03 · Settings ▸ Assistant: the provider roster

> **AS-05 landed.** The surface to call is `strata_core::secret`: `SecretRef::mint()` for a new
> roster entry, `r.put(&secret)` / `r.get()` / `r.delete()`, `Secret::new(&draft_text)` (which
> returns `None` for a blank field — so "cleared" and "delete the store entry" are the same
> branch), and `SecretError` for what Apply renders. Every call blocks: `task::offload`. Two
> things this pane owes back to AS-05's acceptance: the marker's first real `Settings` field
> (and with it the `write_config` round trip), and the manual check that the bundled, signed
> app reads its own item — see `AS-05-secret-store.md` ▸ *What is not proved yet*.

**Workstream:** Assistant · **Status:** ⬜ · **Depends on:** AS-05 (the key-reference type;
the pane can be built against that vocabulary before the store itself lands) — parallel with
AS-01/02; AS-02's per-send selection struct is the target shape — coordinate the field list

## Reshaped 2026-08-09

This task was first written as one global provider + model + key, with an explicit "no
per-conversation model switching" line. Alex overturned that after reviewing IntelliJ's AI
Assistant: **Settings owns the roster** (which brains exist, their endpoints, their keys —
slow-changing, secret-bearing) and **the chat surface owns the pick** (which entry, which
model, what effort — per-conversation intent, AS-04's composer footer). The def/runtime split,
applied to the assistant. The workstream README records the decision; do not re-merge the two
surfaces.

## Goal

A Settings section where the user maintains a **list of named provider entries** and marks one
as the default. Each entry: display name · provider kind (Anthropic · OpenAI · Gemini ·
Ollama · OpenAI-compatible) · default model · base URL (kind-dependent) · API key
(kind-dependent, held in the AS-05 secret store). AA-04 (Settings ▸ Agent access,
`views/agent_access.rs`) remains the form-idiom pattern; **connections are the roster
pattern** — named defs, per-provider field sets, the naming rules in one place
(`Provider::check_address`'s shape, `strata-model/src/connection.rs`).

## Shape

- **An entry is a def keyed by `Uuid`** — the saved-query precedent, not the connection one:
  the per-conversation pick (AS-04) references an entry, and renaming an entry must not break
  that reference, so identity is minted, never the display name. Committed as a map keyed by
  that id; edited as rows whose UI ids come from the list's own counter (the engine-properties
  precedent, `views/engine/model.rs`).
- **Default entry** — one roster-level `Option<Uuid>`, the seed for a new conversation.
  Deleting the default clears it; the empty default is a valid state AS-04 renders honestly.
  No silent re-point to "whatever is first".
- **Provider kind drives the field set.** A field the kind doesn't use is *absent*, not
  disabled (model impossible states out). Base URL only for Ollama (default
  `http://localhost:11434`) and OpenAI-compatible (required, no default).
- **Model** — free-form text `Input` (model names churn faster than any list we could keep;
  the provider's error for an unknown model is honest and current). Placeholder shows a
  sensible current example per kind. This is the entry's *default* model — AS-04 may override
  it per conversation.
- **API key** — the config field is a **key reference (AS-05), never the secret**. The row
  shows the state (key stored / not stored), takes a paste into the draft, and Apply routes
  the secret through the store while `write_config` commits only the marker. The empty state
  is valid: AS-02 falls back to the provider's own env var, and the help text says so ("Leave
  empty to use ANTHROPIC_API_KEY" — the var name comes from the provider table, never
  hand-typed).

No enable/disable toggle: an empty roster *is* the unconfigured state, and the pane (AS-04)
renders it honestly. A toggle would be a second copy of that fact.

## Rules that bind this surface

- **One app-global config store; Settings is a channel; `write_config` is the sole write
  path.** New fields ride `Settings` via `settings_merge!` — a field that isn't merged is a
  build error (AGENTS.md §2).
- **The draft commits a per-field diff against its seed** — for the roster that diff is the
  committed map. The **secret is not part of the diff**: it lives in the draft's memory only,
  goes through AS-05 at Apply, and only its marker merges.
- **A free-form list setting is edited as rows and committed as a map; UI row ids from a
  counter, never the name** (AGENTS.md §2). Persisted identity is the entry `Uuid`.
- **Built from `components::form`** — `Form` > `Row` > control, never bespoke rows (§3).
- **A name two surfaces agree on is generated from one table**: the provider-kind enum, its
  display names, its env-var names, its field requirements and its effort support live in
  **one** place AS-02, this pane and AS-04 all read (the AS-02 config module is the natural
  home — `strata-agent` is already a dependency direction the app has).
- **User-facing text in the IDE register** — terse, single-quoted identifiers, no hedging.

## What is NOT this task

- **The secret store mechanism (AS-05).** This pane consumes its reference type and calls it
  at Apply; it builds no keystore code.
- **The per-conversation selector (AS-04).** This pane never renders in the chat surface; it
  is where entries are *made*, not picked.
- No connectivity "Test" button in v1 — the first send is the test, and the pane reports the
  provider's error verbatim. (If wanted later it belongs here, wired to AS-02's client
  construction, and is cheap then.)

## Acceptance

- Roster CRUD round-trips through `write_config` and survives restart; entry identity
  survives a rename (a stored default — and AS-04's pick — still resolves).
- Switching an entry's kind swaps the visible field set; committing an entry whose kind
  requires a base URL without one is refused in the form, naming the field.
- A pasted key never appears in the written config file (assert on the file's bytes); the
  marker does; clearing the key removes the store entry through AS-05.
- The merge test: `settings_merge!` covers the new fields (exhaustiveness is the build).
- An unconfigured or half-configured state produces AS-02's typed error, and its message names
  this pane as the fix.
