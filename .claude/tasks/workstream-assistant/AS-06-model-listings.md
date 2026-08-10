# AS-06 · Model listings: the pick is the provider's own, and it survives a restart

**Workstream:** Assistant · **Status:** ⬜ · **Depends on:** AS-03

## Goal

A model is **chosen from what its provider serves**, everywhere it is chosen — Settings ▸ AI ▸
Chat today, the composer footer when AS-04 lands. That means the fetched list has to outlive the
window that fetched it, because a `Select` whose only content arrives from a network call is an
empty `Select` every time the app starts.

Today the list exists and is already the right list: `provider::list_models` is a live `GET`
against the provider's own endpoint, and `Probes` holds the answer per `ProviderKind`. Two things
are missing. The Chat pane's default model is still a **typed box** with that list offered as a
suggestion beneath it, and `Probes` is window state — quit the app and every provider is unlisted
again until the user presses Test.

## Why the list is the only offer

genai prescribes nothing about models: a name is an opaque `String` that goes into the request
payload (`ModelIden::new(adapter, name)`), and `Client::all_model_names` is a live call to the
provider's own list endpoint. What *looks* like a supported-model table — `AdapterKind::from_model`
— is a routing heuristic for callers who did not name an adapter, and Strata always names one
(`PROVIDERS`), so it is bypassed. There is therefore no static list anywhere in the stack that a
free-text box is protecting the user from, and no reason for one: the provider can be asked.

A typed name buys nothing and costs a turn. `gpt-5-turbo-imaginry` is accepted by every layer we
own and refused by the vendor, after the send, in a transcript.

## What to build

### 1. The listing cache — an app-scoped satellite, not a config field

`strata_core::models`: what each provider last reported, and when.

```rust
pub struct Listing { pub models: Vec<String>, pub fetched: SystemTime }
pub struct Listings(BTreeMap<ProviderKind, Listing>);
```

**A satellite, on history's precedent** (`history.jsonl` is not a store field). A fetched list is
a cache of a remote fact, not something the user edited — and `AppConfig` is user intent, written
through `write_config`, the one funnel that notifies the settings audience. Routing a background
refresh through it would persist and broadcast a change nobody made. So: its own file, loaded once
at startup exactly as config is, written by the fetch that fills it.

Use the **same mechanism as config** rather than inventing a path — `preferences` with the
existing `AppInfo { name: "Strata", author: "Strata" }` and the key `"models"` beside `"config"`
(`strata-core/src/config.rs:15`). A missing or unreadable file is an empty `Listings`, never an
error: the expected absence is first launch.

**It holds names and timestamps and nothing else.** No key, no `SecretRef`, no endpoint — the
neighbouring module is `strata_core::secret` and this one must stay boring enough that nobody has
to check.

### 2. Refresh: stale-while-revalidate at the point of use, not at launch

**Not a startup dial-out.** Refreshing every configured provider at launch spends a network round
trip and puts a key on the wire per provider, on every start, for a session that mostly never
opens Settings or the chat pane — and a read the user waits for has to be an *arm* rather than a
freeze, which at startup has no surface to be an arm on.

Instead, at the point a list is *shown*:

- The `Select` renders the cached listing **immediately**. A working setup never waits behind a
  spinner, and never sees an empty dropdown.
- Opening the surface kicks **one** background refresh per shown provider whose listing is absent
  or older than the staleness bound — `task::offload`, cancelling is dropping the answer. The
  answer replaces the entry and rewrites the satellite.
- The bound is stated where the poll is, not left implicit: **24 hours**. Model rosters move on
  the order of weeks; the cost of being a day behind is one missing new name, and the recovery is
  already built.
- The **Test** press in AI ▸ Providers stays the explicit refresh, and now writes the satellite
  instead of only `Probes`.

### 3. Invalidation is where it already happens

A listing describes a request against one address with one credential. When either moves, the
answer describes a request nobody would make now — which is exactly what `probes.forget(kind)`
already means, and already exists at both sites (`configure.rs`'s Save, on a changed URL and on a
changed key). Drop the listing on the same line. Disabling a provider removes its key (AS-03), so
its listing goes with it.

### 4. The two pickers

Both are gestures into the one mechanism (§5), not two caches.

- **Settings ▸ AI ▸ Chat** — the default-model field becomes a `Select`. This is the change to a
  surface AS-03 shipped; AS-03 stays done and this task owns the edit.
- **The composer footer** — AS-04's model control, which its task file already specifies as a
  `Select` over this list. AS-04 consumes; it does not build a second one.

**The offered set is `reported ∪ {the current pick}`, in both.** The list endpoint is not the chat
endpoint: a proxy or a private deployment can serve `/chat/completions` and no `/models` at all
(genai carries hardcoded lists for Cohere and Baidu for that exact reason), and an offline laptop
serves neither. A strict picker over an empty answer would strand a setup that works. So the
configured model is always selectable, and a failed or absent listing says which provider would
not answer rather than silently offering one item.

**The list stays unfiltered.** genai returns every `id` the provider names, so OpenAI's carries
`text-embedding-3-large`, `whisper-1` and `dall-e-3` beside the chat models. Do not add a static
name filter to tidy it — that is the prescribed-model table this whole design avoids, and it would
hide a new chat model on the day it ships. A non-chat pick fails on the first send in the
provider's own words. If it becomes a real irritation the fix is a capability the adapter reports,
upstream in genai or in a fork, never a list here.

## What is NOT this task

- No change to `provider::list_models` — it already answers correctly.
- No new provider vocabulary; `PROVIDERS` is untouched.
- No filtering, ranking or grouping of the reported names.
- No per-project scope. Listings are a property of the provider and the machine, like the config
  they sit beside — never `SessionState`, never `.strata/`.

## Acceptance

- Settings ▸ AI ▸ Chat picks its default model from a `Select`; there is no free-text model box
  left in the app.
- A provider tested in one run of the app is still listed after a quit and relaunch, with no
  network call before the pane is opened.
- Opening Chat with a listing older than a day refreshes it in the background: the stale list is
  usable throughout, and the new one replaces it without a flash or a lost selection.
- Changing a provider's base URL or key drops its listing; the pane says it is unlisted rather
  than offering names from the old endpoint.
- With the machine offline, a configured model is still selectable and the pane names the
  provider that could not be reached.
- Nothing blocks the render thread: the refresh is offloaded, and closing the window mid-fetch
  drops the answer rather than waiting for it.
- The satellite file contains model names and timestamps only — asserted on the serialized bytes,
  as `strata_core::ai`'s own roster test does.

## Freya components

`Select` (standard, never a hand-rolled dropdown), the existing `components::form` `Row` for the
Chat pane's field, `task::offload` for the fetch.
