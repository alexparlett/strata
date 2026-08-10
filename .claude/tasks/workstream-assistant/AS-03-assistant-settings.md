# AS-03 · Settings ▸ AI: the provider roster

> **AS-05 landed.** The surface to call is `strata_core::secret`: `SecretRef::mint()` for a new
> entry, `r.put(&secret)` / `r.get()` / `r.delete()`, `Secret::new(&draft_text)` (which returns
> `None` for a blank field — so "cleared" and "delete the store entry" are the same branch), and
> `SecretError` for what Apply renders. Every call blocks: `task::offload`. Two things this pane
> owes back to AS-05's acceptance: the marker's first real `Settings` field (and with it the
> `write_config` round trip), and the manual check that the bundled, signed app reads its own
> item — see `AS-05-secret-store.md` ▸ *What is not proved yet*.

**Workstream:** Assistant · **Status:** 🟡 · **Depends on:** AS-02 (the provider table and
`Selection`), AS-05 (the key-reference type)

> **Built, and not yet seen working end to end.** Every pane, the config vocabulary, the probe
> and the keystore commit are in and the suite is green — but two acceptance items are *code*
> rather than *evidence*, and both need a person at the window: the roster surviving a restart,
> and the signed bundle reading its own Keychain item (AS-05 owes the same check). Everything
> below is written as built; this line is what stops it reading as verified.

## Reshaped 2026-08-10 — the design handoff, and the model that is not here

Written first as one global provider + model + key. Reshaped once (2026-08-09) into a roster of
named `Uuid` entries each carrying a **default model**. Both are now superseded by the design
handoff's **AI** group, and by the observation that killed the middle version:

> *"I don't understand why we need a model on the roster entry at all. It's meant to configure
> and enable a provider, and the model is selected per chat window."*

That is what AS-02's own module doc already said — `provider.rs`: *"Settings maintains the
roster (AS-03 — which brains exist, their endpoints, their keys), a chat conversation holds the
pick (AS-04 — which entry, which model, what effort)."* The "default model" line was the
outlier, not the code. **A provider entry carries what addresses the provider and nothing about
what it is asked**: the `ConnectionDef` shape exactly, where a connection names a bucket and a
*table* names the connection.

The handoff then replaced the roster's structure outright. Settings gains an **AI** group with
three children — **Providers**, **Chat**, and **MCP** (the renamed Agent access pane) — because
outbound model credentials and inbound MCP hosting are different capabilities that were sharing
a screen. Canvas: `Settings.dc.html` (`catProviders` / `catChat` / `catAgent` blocks) and
`strata-windows.js` ▸ `SW.aiState()` / `SW.mcpState()`.

## Goal

Three panes under one **AI** nav group.

**AI ▸ Providers** — a fixed list of the provider kinds `genai` speaks to, each a row with a
toggle. Enabling a row reveals its credential inline and a **Test** action. Below it, a second
section of **named custom endpoints** the user adds and removes. Nothing here names a model.

**AI ▸ Chat** — the new-chat defaults: provider · model · effort, sourced from *enabled*
providers only, so the pane can never offer a model it has no credential for.

**AI ▸ MCP** — today's Agent access pane, renamed and moved under the group. No behaviour
change; it keeps its rows, its anchors and its search keywords.

AA-04 (`views/agent_access.rs`) remains the form-idiom pattern.

## Shape

### The eight are keyed by kind; the custom ones are keyed by a minted id

Two lists, because they are two different things and the difference is structural:

- **A built-in provider's identity is its kind.** Anthropic is Anthropic; there is no second
  one, nothing to name, and nothing to rename. So it is keyed by `ProviderKind` and the row is
  drawn from `PROVIDERS` whether the user has ever touched it or not. Absent from config means
  "never enabled", which is the same thing the toggle says.
- **A custom endpoint's identity is minted.** The canvas has no OpenAI-compatible row and AS-02
  built that kind deliberately — llama.cpp, vLLM, LM Studio, a gateway — and there is no reason
  to have only one. So it is a user-managed list in the same row anatomy: **name** · base URL ·
  optional key · toggle · Test. Keyed by `Uuid`, because the display name is the thing whose
  whole purpose is to be retyped and the chat defaults point at it (the saved-query precedent,
  not the connection one).

```rust
// strata-core
pub enum BrainRef { Builtin(ProviderKind), Custom(Uuid) }

pub struct Ai {
    pub providers: BTreeMap<ProviderKind, ProviderSetup>,
    pub endpoints: BTreeMap<Uuid, CustomEndpoint>,
    pub default_brain: Option<BrainRef>,
    pub default_model: String,
    pub default_effort: Option<Effort>,
}
```

`BrainRef` is what a conversation points at, and it is what makes "several panes on several
providers" one value rather than a mode — AS-02's `Selection` is still built per send, from a
`BrainRef` plus the conversation's model and effort.

### A field the kind does not use is absent, not disabled

The credential field is what the kind's `KeyUse`/`BaseUrl` policy says it is, straight off
`PROVIDERS`: a masked key with a reveal for the seven keyed kinds, a URL for Ollama, and **both**
for a custom endpoint (URL required, key optional). One expanded area, one to three boxes.

The **empty key is a valid state**: `KeyUse::Env` falls back to the provider's own variable, and
the row's subtext says which — the name comes from the table, never hand-typed.

### Test is `all_model_names`, so the test and the list are one call

There is no ping in `genai` and there does not need to be. `Client::all_model_names(adapter,
ProviderConfig { endpoint, auth })` is a live `GET` against the endpoint with the credential for
every kind we offer, and its answer is exactly what AI ▸ Chat's model dropdown needs. So one
call serves both: Test reports "verified · N models" or the provider's own error, and the list
it returned is what the model dropdown offers.

Editing a credential clears the verification — a stale "verified" beside a changed key is a lie.

### The model is a *picked* name that can still be typed

AI ▸ Chat's model control is a dropdown over the enabled provider's listed models, each carrying
a **REASONS** badge when `efforts(kind, model)` is non-empty. It must also accept a typed name:
a list can 401, a gateway can 404 `/models`, and a private deployment can name a model no list
reports. A dropdown that could not be typed into would make an unlisted model unreachable.

This is the whole of the model question, and it lives here rather than on a provider row.

### Effort is AS-02's rungs in the canvas's shell

The canvas draws a fixed four-way `Minimal · Low · Medium · High` that dims for a non-reasoning
model. The interaction is kept and the ladder is not: AS-02 settled that the rungs are a **set
per model** drawn from `Low · Medium · High · XHigh · Max`, verified against what the pinned
`genai` actually sends, with `Minimal` excluded as one vendor's spelling of `Low`. So the
segmented control renders `efforts(kind, model)` — three segments for most models, five for the
newest Claude family — and when the set is empty the whole control dims with the note naming the
model, which is the canvas's own behaviour and AS-02's `NoSuchEffort` wording.

Offering a rung the model does not take is the same "a field silently ignored is a lie on
screen" the base URL and the key are refused for.

## What this task moves in AS-02

`strata-agent` depends on `strata-core`, so `Settings` cannot name a type the assistant crate
owns. The minimum that moves, and no more:

- **`ProviderKind` and `Effort` move to `strata_core::ai`** — they are persisted tokens, which
  their own docs already say. `PROVIDERS`, `Efforts`, `Rungs` and everything genai-shaped stay
  in `strata-agent`, next to the pin their rung lists are tied to. **One table, relocated
  nothing.** The inherent methods become free functions in the provider module
  (`provider::info`, `::label`, `::efforts`), because the orphan rule forbids `impl Display for
  ProviderKind` in a crate that does not define it; `SelectionError`'s messages read
  `provider::label(*kind)` where they read `{kind}`.
- **Four kinds join the table**: DeepSeek, Groq, xAI, Cohere — all `AdapterKind` variants with
  declared env vars in the pinned `genai`. Their `Efforts` rules cover only the families
  verifiable against its source; everything else on those kinds gets no control, because
  `Only` is default-closed and falling behind must cost a knob the user can report rather than a
  menu the provider refuses.
- **The env-var names get a drift test.** `AdapterKind::default_key_env_name()` is public, so
  every `KeyUse::Env(var)` is asserted equal to genai's own answer for that adapter. The name
  stays written in our table (the help text needs a `&'static str`), but it can no longer fall
  out of step with the auth it describes — the same reason `ModelReadsAsEffort` asks
  `ReasoningEffort::from_model_name` rather than copying its keyword list.
- **Three messages point at the wrong surface.** `NoModel` and `ModelReadsAsEffort` say "in
  Settings > Assistant" for a value Settings will no longer hold. The line is now clean:
  **an error about the provider names Settings ▸ AI ▸ Providers; an error about the model names
  the chat pane.**
- **`provider::list_models`** — the one place `all_model_names` is called, beside the one place
  a client is built, resolving endpoint and auth through the same match `Brain::resolve` uses.

## Rules that bind this surface

- **One app-global config store; Settings is a channel; `write_config` is the sole write path.**
  New fields ride `Settings` via `settings_merge!` — a field that isn't merged is a build error.
- **The draft commits a per-field diff against its seed.** The **secret is not part of the
  diff**: it lives in the draft's memory only, goes through AS-05 at Apply, and only its marker
  merges.
- **A free-form list setting is edited as rows and committed as a map; UI row ids from a
  counter, never the name.** Applies to the custom endpoints; persisted identity is the `Uuid`.
- **Built from `components::form`** — `Form` > `Row` > control. Where the canvas's provider row
  genuinely diverges from a form row, name it in `form/mod.rs`'s "known divergences".
- **A name two surfaces agree on is generated from one table**: the kind, its label, its key
  policy, its base-URL policy and its effort rule are `PROVIDERS`, read by this pane and by
  AS-04, restated by neither.
- **Only real facts.** The canvas's "N models · M reasoning" subline is knowledge a fetch
  produces, so before a Test the row says what it actually knows — the fallback variable for a
  keyed kind, the default endpoint for Ollama — and becomes the model count once a list has
  come back.
- **Nothing blocking on the render thread**: keystore reads and `list_models` both go through
  `task::offload`.
- **User-facing text in the IDE register.**

## Named divergences from the canvas

Recorded here so they are not re-litigated as gaps:

- **A ninth kind, in its own section.** The canvas has eight rows and no OpenAI-compatible;
  custom endpoints are a second, user-managed list below them.
- **The effort ladder is per model** (above), not a fixed four.
- **The subline states what is known** (above), not a model count the app has not fetched.
- **The model dropdown accepts a typed name** (above).
- **A disabled provider that was the default.** The canvas re-points the default at another
  enabled provider. Followed, but the re-point is *visible*: it happens on the pane the user is
  looking at, in the draft, before Apply — never silently at read time.

## What is NOT this task

- **The secret store mechanism (AS-05).** This pane consumes its reference type and calls it at
  Apply; it builds no keystore code.
- **The chat pane (AS-04)** — the multi-chat switcher, the composer's per-chat model and effort
  pickers, the transcript. This pane is where brains are *configured*, not picked. AS-04 reads
  the same `Ai` defaults and the same `provider::list_models`.
- No agent-access behaviour change: MCP is a rename and a move.

## Corrected by review — do not re-introduce

An xhigh adversarial pass over the first cut found twelve defects. The shapes worth keeping:

- **A guard that reads absence and emptiness as different states writes on mount.** The URL
  effect compared `Option<String>` against `Some("")`, so every built-in with no config entry
  failed its own guard and got one created — opening the pane dirtied the draft with no edit and
  Apply persisted seven empty provider rows. `base_url_of` returns `String` now: absent and empty
  *are* the same answer.
- **A guard inside an effect peeks; it does not read.** `base_url_of`/`name_of` used `.read()`,
  subscribing every row's effects to the whole draft — one keystroke re-ran sixteen effects
  across eight rows. The engine grid's `PropRow` peeks for exactly this reason, and the comment
  above these claimed to be following it.
- **A cleared box is the answer, including for a test.** `build_ask` flattened an empty typed key
  to `None` and fell back to the *stored* one, so clearing a key and pressing Test reported
  "verified" using the credential Apply was about to delete. Touched-ness decides now, not
  emptiness.
- **Two ways to strand a secret in the keystore**, both closed: a partial `commit` discarded the
  markers for keys that had already landed (a retry then minted fresh refs, orphaning another
  entry each time), and `commit` used `entry().or_default()` merely to *read* a key slot, so
  asking about a provider created a config row for it.
- **An effort outlives the model that offered it.** Retyping the model to one with no rungs left
  `default_effort` set and unreachable — the control was gone, and `Brain::resolve` refuses such
  a `Selection` before a socket opens, so every new chat would fail its first send.
- **A page with no named settings is a `Page`, not an `Anchor`.** `AiProviders` was indexed as a
  setting no row carried, so its hit navigated and then singled nothing out.

## Acceptance

- Provider enable/disable, credentials and the custom-endpoint list round-trip through
  `write_config` and survive restart; a custom endpoint's identity survives a rename (the chat
  default still resolves).
- Enabling a kind reveals exactly the fields its `PROVIDERS` row declares; a custom endpoint
  with no base URL is refused in the form, naming the field.
- A pasted key never appears in the written config file (assert on the file's bytes); the marker
  does; clearing the key removes the store entry through AS-05.
- Test reports the provider's own words on failure and a model count on success, and editing the
  credential clears the result.
- AI ▸ Chat offers only enabled providers; its effort segments are `efforts(kind, model)` and
  the control dims with the model named when that set is empty.
- `settings_merge!` covers the new field (exhaustiveness is the build); every `KeyUse::Env`
  matches `AdapterKind::default_key_env_name()`.
- An unconfigured or half-configured state produces AS-02's typed error, and its message names
  the surface that fixes it — Settings for the provider, the chat pane for the model.
