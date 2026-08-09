# AS-03 · Settings ▸ Assistant

**Workstream:** Assistant · **Status:** ⬜ · **Depends on:** — (parallel with AS-01/02;
AS-02's config struct is the target shape — coordinate the field list)

## Goal

The control surface for the pluggable brain: a Settings section where the user picks the
provider and model, points at a custom endpoint when the provider needs one, and pastes an API
key when the provider needs one. AA-04 (Settings ▸ Agent access) is the pattern to build in
the image of — same window, same form idiom, same merge discipline.

## Fields

- **Provider** — `Select`: Anthropic · OpenAI · Gemini · Ollama · OpenAI-compatible. The field
  list below it is provider-dependent; a field the provider doesn't use is *absent*, not
  disabled (model impossible states out).
- **Model** — text `Input`, free-form (model names churn faster than any list we could keep;
  the provider's error for an unknown model is honest and current). Placeholder shows a
  sensible current example per provider.
- **Base URL** — only for Ollama (default `http://localhost:11434`) and OpenAI-compatible
  (required, no default).
- **API key** — only for keyed providers. Stored in app config exactly as the agent-access
  bearer token already is (the precedent; the config file is the app's, in the user's own
  profile). Show it masked with a reveal, like the token. The empty key is a valid state: AS-02
  falls back to the provider's own env var, and the field's help text says so ("Leave empty to
  use ANTHROPIC_API_KEY" — generate the var name from the provider, don't hand-type five
  strings). If a keychain story is ever wanted it is a deliberate follow-on, not this task.

No enable/disable toggle: the assistant is "configured or not", and the pane (AS-04) renders
the unconfigured state honestly. A toggle would be a second copy of that fact.

## Rules that bind this surface

- **One app-global config store; Settings is a channel; `write_config` is the sole write path.**
  New fields ride `Settings` via `settings_merge!` — a field that isn't merged is a build
  error (AGENTS.md §2).
- **The draft commits a per-field diff against its seed** — the existing draft machinery, no
  new apply logic.
- **Built from `components::form`** — `Form` > `Row` > control, never bespoke rows (§3).
- **A name two surfaces agree on is generated from one table**: the provider enum, its
  display names, its env-var names and its field requirements live in **one** place AS-02 and
  this pane both read (the AS-02 config module is the natural home — `strata-agent` is already
  a dependency direction the app has).
- **User-facing text in the IDE register** — terse, single-quoted identifiers, no hedging.

## What is NOT this task

- No connectivity "Test" button in v1 — the first send is the test, and the pane reports the
  provider's error verbatim. (If wanted later it belongs here, wired to AS-02's client
  construction, and is cheap then.)
- No per-conversation model switching — the loop reads current settings at send time; that is
  the whole story.

## Acceptance

- Switching provider swaps the visible field set; committed config round-trips through
  `write_config` and survives restart.
- The merge test: `settings_merge!` covers the new fields (exhaustiveness is the build).
- An unconfigured or half-configured state produces AS-02's typed error, and its message names
  this pane as the fix.
