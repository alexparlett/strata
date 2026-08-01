# AA-03c · The seam's remaining holes: one identity per session, per client

**Workstream:** Agent access · **Status:** ✅ · **DEV_TASKS:** — · **Depends on:** AA-03b

## Why this exists

AA-03b's review turned up fifteen defects; eleven were fixed in place, and a twelfth (§1 below)
landed in the same PR once a second review showed the estimate that deferred it was wrong. What
is left is **two defects and one small decision**, kept together because they are one family.

They are one family, which is the useful way to hold them: **the seam identifies things by
something that is not their identity.** A session's teardown is ordered by *nothing*, and an
agent is identified by *how long a value happens to live*. Each is right today and wrong under a
condition the app can already reach. (§1 was the third — a run bracketed by *project root* — and
is the worked example of what fixing one looks like.)

None is a regression from AA-03 — the first two are new surface that AA-03b's direct-engine
dispatch created, and the third was latent in AA-03 and became load-bearing when `AgentId`
started scoping every session.

---

## 1. ~~A run's bracket must name its registration~~ — done; one question left

**Fixed in AA-03b's PR** after review flagged it a second time. `Host::run` used to resolve its
window three times independently (`engine_for`, the `RunStarting` ask, the `RunSettled` notify),
each a `rev().find(root)` — so an engine restart, which remounts at the *same* root, could
execute the run on the outgoing engine while the incoming window recorded it, or deliver a settle
to a satellite that had never heard of the session, where it silently matched nothing and
stranded the row at `Running`.

`AgentDirectory::window` now takes the engine and both senders **together, under one lock**, and
the run carries that resolution through the whole bracket. Pinned by
`a_run_settles_into_the_registration_that_started_it`, which remounts at the same root mid-run
and asserts the settle lands on the registration that answered the start.

The estimate that put it here was wrong: the fix is contained inside `AgentDirectory` and changes
no signature. What is genuinely left is the **decision**, which is small:

**Decided: it stays silent, and the reasoning is now written at the `notify` call**
(`directory.rs`). Answering `WindowGone` was rejected — it would report a fault for a statement
that demonstrably succeeded, and an agent's recovery from a window loss is to run it again, so
it would pay for the same scan twice. Nothing is stranded by the silence either: if the
registration is gone the satellite that would have shown the row went with the window, and the
one case where a settle could have landed on a *live* satellite that never heard the start was
the same-root remount, which resolving the whole bracket once already makes unrepresentable.

## 2. Closing a session must not race the run being dispatched into it

**Where:** `crates/strata-freya/src/agent/directory.rs` (`Host::run`) and
`crates/strata-freya/src/apps/project/state/agent.rs` (`AgentAsk::CloseQuerySession`).

MCP permits concurrent requests on one connection, and nothing holds a barrier between the
`RunStarting` reply and `engine.query`:

```rust
let started = self.ask(project, |reply| AgentAsk::RunStarting { … }).await??;
// ← a concurrent close_query_session(S) fits here
let settled = match mode { … engine.query(ws, tag, sql, page_size).await … };
```

**What goes wrong.** The close removes S from the satellite and calls `cleanup_ws(S)`, which
aborts and retires *nothing* because no dispatch has happened yet. The run then registers a
`__snap_*` table and an in-flight entry on a workspace nothing holds — so no later
`close_query_session`, `AgentGone` or cap eviction can ever name it. The snapshot leaks for the
engine's life, and `Engine::is_running(S)` stays true, feeding a phantom into the T2 close
confirm. The same gap swallows an `AgentGone` that lands in it.

**Built: (c), the tombstone** — `Agents::closed` now answers `Closed::{Now, WhenItSettles,
NoSuchSession}`, and `run_settled` returns the session whose deferred teardown has come due.
Two corrections review forced on the first attempt, both worth keeping in view:

- **The abort is not deferred; only the row is.** The first version skipped `cleanup_ws`
  entirely on the tombstone arm, which broke `close_query_session`'s own promise that "a run
  still in flight in it is cancelled" — the predicate is `is_running()`, true for the whole
  execution and not just the pre-dispatch window, so a runaway scan would have burned to
  completion with the handle already dead. `cleanup_ws` is a no-op when nothing is dispatched,
  so it is now called on **both** arms and the tombstone defers only the satellite row and its
  second teardown.
- **What reaps a tombstone whose settle never comes** is `gone` — the row stays in the agent's
  session list, so a connection ending hands it back like any other, and `DispatchGuard` already
  covers the engine side of a dropped run future. The residual hole is a *single request*
  cancelled while the connection stays alive: the row stays `Running` for that connection's
  life. Bounded, engine-side clean, and no user-facing path depends on it (both the T2 confirm
  and the tool listing ask the engine, not the satellite). The shape that would close it is
  making the settle RAII on the caller's side — a guard that sends a `Stopped` notice on drop —
  rather than a trailing `send`. Left for whoever next touches `directory::run`.

`QuerySession::is_running` also became **any** run in flight rather than the newest, because all
three consumers want that reading and two of them destroy work when it is wrong. (The "trail cap
can hide a still-executing run" worry was chased and is not reachable: `Engine::query` supersedes
per `WsId`, so at most one run per session is genuinely executing and the row the cap discards
has already been aborted.)

**The options as they were weighed:**

- **(a) A lease.** `RunStarting` hands back a lease alongside the sequence number; the close path
  refuses (or defers) while one is out. Most explicit, and it puts the invariant in the type.
- **(b) Dispatch inside the driver.** Makes start-and-dispatch atomic with respect to close, but
  reintroduces the driver waiting on a query — which is exactly what AA-03b moved out, and what
  makes the loop serial-but-fast. **Do not take this one** without re-reading why the dispatch
  moved.
- **(c) Tombstone.** Close marks the session closed and leaves the teardown to whoever settles
  last; the settle notice finds the tombstone and does the `cleanup_ws` then.

(c) reads cleanest and keeps the driver non-blocking, but it needs an answer for the settle that
never comes — a dropped run future is now handled engine-side by `DispatchGuard` (AA-03b's
review), yet the *notice* still never arrives, so a tombstone could outlive its run. Whatever is
chosen has to say what reaps it.

**Note the adjacent rule already in place:** the session cap deliberately never evicts a session
with a run in flight (`Agents::opened`), because tearing down a live workspace makes the engine
settle `cancelled`, which the vocabulary then reports to the agent as "you stopped this" for a
cancellation the *app* performed. An explicit `close_query_session` on a running session **is**
allowed to cancel — that is the agent's own decision about its own work — so whatever fix lands
must keep those two cases apart rather than blanket-refusing.

---

## 3. An agent's identity must come from the transport, not from a value's lifetime

**Where:** `crates/strata-agent/src/server.rs` (the service factory) and
`crates/strata-agent/src/tools.rs` (`Connection`).

AA-03b made `StrataTools` *be* one agent: the factory closure calls `tools.connection()`, which
mints an `AgentId`, and `Connection::drop` retracts it. The comment claims "the factory runs once
per MCP session". That is true only on rmcp's **legacy** path.

rmcp 3.0.1 gates sessions on `use_session = legacy_session_mode && is_legacy_request(…)`, and
`uses_legacy_lifecycle` is `!uses_discover_lifecycle && version < V_2026_07_28`. A client
negotiating protocol `2026-07-28` — already in rmcp's `SUPPORTED` list — or sending the newer
per-request `_meta` takes the **stateless** branch, where `get_service()` is called *per request*
and the value is dropped when the response is written.

**What goes wrong.** Every request is a different agent. `open_query_session` mints S under agent
A; the next request's `run` arrives as agent B, `holds(B, S)` is false, and every call answers
`No open query session '…'`. The feature is silently dead for that client, and each request's
`Connection::drop` broadcasts an `AgentGone`. It is **latent today** only because
`ProtocolVersion::LATEST` is still `V_2025_11_25` — a client can ask for the newer version
explicitly, and an rmcp bump flips it for everyone at once.

A smaller instance is live now: `get_tool_schema` builds a throwaway service per tool name to
read its schema, each minting and retracting an id. Harmless since AA-03b's `Agents::knows` peek
stopped those retractions waking every window, but it is the same mechanism.

**Shape of the fix — and two things this section originally prescribed that turned out to be
wrong.** Neither survived contact with the rmcp source, and both are worth stating because they
are the obvious moves:

- **Pinning `legacy_session_mode` is not a stopgap, because there is nothing to pin.** It
  already defaults to `true`, and the flag can only turn sessions *off* — never force them on.
  `use_session = legacy_session_mode && is_legacy_request(…)`, so a client on the newer
  lifecycle goes stateless regardless.
- **Keying on `Mcp-Session-Id` fixes only the path that already works.** SEP-2567 removes
  sessions from the discover lifecycle and rmcp sets that header on exactly one response, inside
  the session branch — it is absent precisely where identity breaks. Worse, it is not the
  discriminator at all: `is_legacy_request` reads the request's `_meta` and protocol version and
  never consults it, so a client echoing a stale session id alongside per-request `_meta` takes
  the stateless branch and would be misread as owning its value.

**Built:** `tools::Caller`, which mirrors rmcp's own lifecycle predicate. No HTTP `Parts` in the
request extensions means stdio (AA-05) or the in-process pane (AA-06), where the value's
lifetime genuinely *is* the connection; a request whose `_meta` lacks the `2026-07-28` required
keys and whose version is older took the session branch, where it is too; anything else is
stateless and keys on `_meta` `clientInfo` through a roster that outlives the per-request value.
`StrataTools::connection()` stays the seam and `Connection`'s RAII retraction is unchanged, so
AA-05 and AA-06 are untouched.

Three consequences that are the honest cost of a protocol with no connection, all found in
review:

- **Identity is read from `_meta`, never from the peer.** rmcp reconstructs stateless
  `peer_info` with `Implementation::default()`, and that is *not* blank — it is `from_build_env`,
  so it reads `rmcp` and the rmcp version. Falling back to it labels every un-introduced client
  with the name of the MCP library it happens to use.
- **A blank identity is refused the session-scoped tools rather than pooled.** `clientInfo` is
  optional on that lifecycle (`DRAFT_REQUIRED_KEYS` is `protocolVersion` + `clientCapabilities`),
  and one minted id behind two different processes would let each list, page and close the
  other's query sessions — that id is the whole of both isolation checks. That is the AA-03 hole
  reopened by a bucket meant for display. The **project-scoped** tools keep working — the
  split is whether a tool needs to know whose agent is asking, not `read_only_hint`, which two
  of the refused five carry — and the refusal
  names the fix. **This is a product call worth revisiting** if a real client turns out to omit
  `clientInfo`: the alternative is accepting the pooling and documenting it.
- **Two windows of one client still share an agent.** Unavoidable — the protocol carries nothing
  else — and stated on `AgentId`.

Retraction splits the same way: RAII where there is a connection, and an idle sweep where there
is not. The sweep needed two guards review found missing. It **skips an agent with a call in
flight** (a `Busy` guard, refcounted per roster entry, re-stamping on drop), because retiring one
mid-run aborts its own query and the vocabulary then reports that as "you stopped this" — exactly
what `Agents::opened`'s eviction gate refuses to cause. And **every** tool stamps the clock, not
just the five session-scoped ones, or an agent following this server's own instructions (start
with `list_tables`, `describe_table`, `validate`, *then* open a session) is retired mid-workflow.
The final sweep runs from `AgentServer::drop`, not after the loop's `break`: `shutdown_background`
drops tasks rather than polling them, so anything past the `break` would never run.

**The constraint that shapes it:** AA-05 (stdio) and AA-06 (in-process) have no `Mcp-Session-Id`
at all. So the identity *source* is per-frontend and `StrataTools::connection()` should stay the
seam — what changes is that the HTTP transport supplies a stable key instead of relying on a
drop. Keep `Connection`'s RAII retraction; it is right for stdio and for the chat pane, where the
value's lifetime genuinely *is* the connection's.

---

## What is deliberately not here

- **`SidebarPane::Agents` and older builds.** Reported and dismissed: a downgrade discarding
  `session.json` is not a case worth designing for.
- **Cancelling an agent's run from the pane.** Still a design call (AA-03b's "left for a later
  pass"), not a defect.
- **The Agents pane's render cost** (deep-cloning the run trail per render). Real, measured, and
  bounded by the caps; it belongs to whoever next touches that surface, not to a seam task.

## Acceptance

- A run's engine, its `RunStarting` and its `RunSettled` all resolve to **one** registration; an
  engine restart mid-run cannot bracket it across two mounts. Test: restart between the ask and
  the settle and assert the outcome is either recorded once or reported to the agent — never
  silently dropped.
- A `close_query_session` concurrent with a `run` on the same session leaves **no** orphaned
  workspace: after both settle, `Engine::is_running` is false and nothing is registered for that
  `WsId`. Test drives the two concurrently rather than asserting the ordering by inspection.
- An agent keeps one identity across its whole connection on **both** of rmcp's paths. Test:
  drive the vocabulary over the real transport with a client negotiating `2026-07-28` and assert
  `open_query_session` → `run` → `read_page` all succeed under one agent.
- The existing AA-03b suite still passes unchanged — none of these fixes should need a test
  rewritten, which is itself a signal the shapes are right.
