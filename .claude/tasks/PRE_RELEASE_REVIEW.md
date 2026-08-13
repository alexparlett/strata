# Pre-release review — what it found, what was fixed, what was left

A six-lens correctness/security/design review of the app crates (~136k lines) ahead of the public
release: engine and SQL boundary, security, threading and async, Freya UI and state, persistence
and model, connections and release/CI. Each lens verified its claims against source before
reporting; the high and medium findings were then re-verified line by line.

**Outcome: no critical findings.** One high, nine medium, about twenty low. The high and every
medium are fixed; the low items are itemised below with a verdict each, because a review whose
tail is "and some other things" is a review nobody can close.

The pattern worth keeping: nearly every finding was a place where a rule this codebase *already
settled* had one site that predated it. The fixes are almost all "apply your own existing
mechanism here", which is why each one landed next to the rule it belongs to in
`docs/reference/` rather than as a new convention.

## Fixed — high

**A root-scoped task must not write a subtree's state.** `state/hooks.rs::refresh_table_rows` and
both arms of `views/dialogs/drop_confirm.rs::drop_row` used `spawn_forever` so the work would
outlive the tab or dialog that ordered it — correct as far as it went, but root scope also
outlives the *project subtree*, which a re-root and an engine restart (a `runtime.*` Settings
apply) unmount wholesale. The `EngineCtx` clone keeps the outgoing engine alive, so the call
completes and writes a freed store: `State::write_unchecked` panics and the release panic hook
ends the process. `refresh_table_rows` was additionally tracked by no lifecycle bookkeeping, so no
close confirm stood between the two.

Cancelling is the cure `Chats::stop_all` takes and is wrong here — a drop that is deleting a
table's data has to finish. The fork therefore grew the predicate the situation wants
(`State::is_alive` / `RadioStation::is_alive`, both additive), and the three sites ask after the
await. The work happens; only the reporting is skipped. Full text: `INVARIANTS.md`, "A root-scoped
task outlives the project subtree".

## Fixed — medium

| # | Finding | Where the rule now lives |
|---|---|---|
| 1 | App config: `unwrap_or_default()` on load conflated absent / unparseable / unreadable, and a write follows within seconds of launch — so one transient read failure persisted the defaults over every keybind, engine override, AI provider (orphaning its keystore entries), the agent token and the recents. The write was `File::create` + `to_writer`, which manufactures the unparseable file the loader then erased. | `INVARIANTS.md`, "The config file is read three ways" |
| 2 | `Engine::export` held its pin and in-flight count in the **caller's** future while the write detached, so closing the export window let a re-run retire the snapshot mid-stream and truncate the user's file. | `INVARIANTS.md`, under the snapshot-pin entry |
| 3 | A typed `COPY … TO` never examined its destination and could write into `.strata/tables/<slug>/` or the snapshot spool. | `INVARIANTS.md`, "A `COPY … TO` may not land in storage Strata owns"; `STATEMENTS_SPEC.md` §6.4 |
| 4 | The view funnel had no `__snap_` backstop where `register_external` has one, so ⌘S or a hand-edited `project.json` could register into the reserved namespace. | `STATEMENTS_SPEC.md` §4 |
| 5 | The reserved-name refusal covered only *intercepted* statements, so a plain `SELECT * FROM __snap_3` returned another tab's result with `__strata_ord` showing — and Export then wrote it to a file, around the fence that exists to stop exactly that. **This overturns a settled decision**; the reasoning is written out in `STATEMENTS_SPEC.md` §4 and `INVARIANTS.md`. | as above |
| 6 | `ModelPicker` captured its provider and `Ai` config in a once-built `use_side_effect` closure, and is un-keyed — so a repick never refreshed the model list, and a fresh setup offered nothing at the "pick a model" moment. | `FREYA_UI.md`, under the captured-value rule |
| 7 | `check_http_url` accepted userinfo, so a pasted `https://user:pass@host` put a password in the committed `project.json`. | `CONNECTIONS_SPEC.md`, address rules |
| 8 | The wrong-region probe string-matched `object_store`'s error prose with no test behind it and a caret dependency, so a patch bump could silently revert the probe's headline feature. Now pinned by a test that drives a real bare 301 from a loopback listener. | test is the record |
| 9 | Session autosave, history and defs writes `fsync` on the render thread — the freeze `open_project` was offloaded to remove. Autosave and history now go through `offload`; defs stays synchronous **on purpose** (it runs inside the store write guard that `drop_row`'s roll-back depends on). | `persist.rs` module docs |
| 10 | Session layout tolerance was asymmetric: `sidebar` survived a retired enum variant, `right` / `drawer` / `problems_tab` / `TabSnapshot.origin` did not — so one retired variant cost the user every open tab. Generalised into `retired_to` / `retired_open`. | `session.rs` module docs |
| 11 | `query/mod.rs` carried module-wide `dead_code` / `unused_imports` allows justified by P2-03, which landed. Removed; nothing was actually dead. | — |

Also fixed in passing: AWS's IP-address bucket rule (present in the GCS checker, missing from S3's,
while the spec claimed AWS's published rules), and the export page-window multiply, which was
unchecked where `fetch_page`'s was deliberately saturating.

## Fixed — low

- **`ensure_gitignore` discarded user lines on a transient read failure** (`unwrap_or_default()`
  then an atomic rewrite of only Strata's six lines). Now distinguishes NotFound from a failed read.
- **The internal-table slug was not injective**: a safe name could equal another name's
  `sanitized-hash8` form and land in the same directory, where a create removes what is there.
- **`TabChrome` read a `Role` beside a `tab`-themed destructure**; the close glyph is a theme field now.
- **Six `wrap_spacing` literals** bypassed the `components::metrics` scale with no stated exception.
- **Two text violations**: an em-dash in an Export hint, and a lowercase progress label among
  capitalised siblings.
- **Shared form fields froze the caller's `EventHandler`** in once-built effect closures
  (`NumberField`, `PathField`, `FieldControl`, `ValueField`'s `max_len`). Every current caller got
  away with it for an accidental reason; the first one closing over a row id would not have.
- **Both enforcement hooks failed open without `jq`** and were bypassable by quoting and line
  continuation. Both now normalise the command and fail closed, and
  **`.claude/hooks/test-hooks.sh`** is a runnable harness over both (31 cases).
- **`release.yml` interpolated `${{ github.ref_name }}` / `${{ inputs.version }}` straight into
  `run:` blocks** in a job holding the signing secrets; routed through `env:` like the file's own
  precedent.
- **`target/dist/` was never cleared**, so a previous version's artifacts could ride onto a release
  through the workflow's `*.dmg` / `*.zip` glob.
- **The notarytool Apple-ID password was on the command line** (visible to `ps` for the length of a
  submission); it reads from stdin now. The API-key rung CI uses was never exposed.
- **The MinIO rejected-credentials phase asserted only a non-empty error** — any failure passed,
  including a registration bug. It now asserts an access-denied/403.
- **`parked()`** was unreferenced pre-work; removed, and AGENTS.md §5's citation of it made honest.

## What the review of *this* work found

The fixes above were then reviewed as a diff in their own right, which turned up six defects in
them — three of them worse than what they replaced. Recorded because the pattern is the lesson:
every one was a fix that reached one step further than the evidence supported.

- **`notarytool --password -` does not exist.** The stdin form was invented, not looked up; it
  would have passed the literal string `-` and failed notarization on the Apple-ID rung, aborting
  the release build under `set -e`. `--help` documents exactly three ways in, and only the
  argument form scripts. Reverted, with the real fix (`store-credentials` +
  `--keychain-profile`) named as the setup step it is.
- **`use_reactive` on an `EventHandler` re-fires the effect every render.** `EventHandler`'s
  `PartialEq` is unconditionally `false` (freya-core `event_handler.rs:85`), so `use_reactive`
  writes its `State` on every render; an effect that `read`s it is therefore subscribed to
  something that always changes. `FieldControl` calls its handler *unconditionally*, so this
  turned one call per text change into one per render, each writing the draft that causes the
  next. Fixed by `peek`ing instead — which is what wanting *freshness without a trigger* actually
  spells.
- **The slug fix orphaned data it was meant to protect.** Hashing safe names that already look
  hashed changes the slug of tables **already on disk**, and `table_dir` re-derives that path from
  the name on every drop — so dropping such a table would delete nothing and strand the real
  directory forever. The collision it closed needs a user to name one table the hash of another;
  the regression needs only a table called `report-1a2b3c4d`. Reverted, and `slug`'s doc now says
  why the shortcut has to stay.
- **A failed keep-aside left the config writable**, so the one path where the bytes could not be
  preserved was also the one where the next write destroyed them — the exact opposite of what the
  new doc promised two lines above.
- **`alive()` was inserted between `drop_row`'s doc comment and `drop_row`**, silently reassigning
  thirty-five lines of rollback-policy reasoning to a two-line helper.
- **`split_once("://")` read any path containing `://` as a URL**, so a local target like
  `.strata/tables/sales/x://y` skipped the ownership fence entirely. Now shaped against RFC 3986
  and pinned by two tests.

## Left, deliberately

- **The chat transcript deep-clones the conversation on every render** (`transcript.rs:46`), and
  each `TurnRow` re-clones its turn and rebuilds `plain()`. O(conversation) allocation per
  streaming delta. Structural rather than an oversight: the block components own their payloads,
  so removing it means moving the chat blocks behind `Rc` — and the streaming path mutates the
  last reply in place, so `Rc::make_mut` could cost more than it saves. Bounded by conversation
  length and fine at pane scale. **Worth doing, worth measuring first, not a pre-release change.**
- **A hostile `project.json` can name any host file as a table source.** `resolve_source` returns
  absolute paths as-is and joins `..` with no containment, so opening an untrusted project exposes
  e.g. `/etc/passwd` as queryable, including to a read-only MCP agent. Comparable to opening a
  malicious document, and project-sharing between untrusted parties is not a supported flow — but
  if it ever becomes one, this needs a containment check first.
- **The MCP bearer token is plaintext in the app config.** Already reasoned about and accepted in
  `secret.rs`: minted locally, loopback-only, worthless off the machine. Noted so the next reviewer
  finds the reasoning rather than the finding.
- **`hyper 0.14` / `h2 0.3` / `rustls 0.21` in the graph** via `aws-config`. The locked versions
  carry the known fixes; rustls 0.21 is an EOL branch. Upstream's choice, nothing actionable here.
- **`HistoryEntry` has no `#[serde(default)]` fields**, and an older build's load-time compaction
  rewrites the file without newer-format lines. Acceptable for regenerable history — but adding a
  field to that struct is a breaking change for downgrades, and that is worth knowing before doing it.
- **`queue: max` in `ci.yml`** could not be verified against GitHub's schema offline. If it were
  ever silently ignored, the minio job falls back to the cancel behaviour its comment says it
  exists to prevent. One-time check against the docs.

## One flaky test, and it is not this branch's

`engine::store::tests::a_named_profile_signs_as_that_profile_and_not_as_the_environment`
intermittently panics inside `aws-smithy-http-client`'s rustls provider:

> TrustStore configured to enable native roots but no valid root certificates parsed!

**Flaky, not broken, and not caused by anything here.** During this review it failed three times in
a row — standalone in a fresh process, with the sandbox off, and on `main` in a worktree this
branch never touched — and then passed cleanly in a later full-suite run with all 589 lib tests
green. So it is a *transient* failure to read the machine's root certificates, not a deterministic
one, which is consistent with what the message says.

Ruled out as a cause: the dependency graph did not move — the only lock change on this branch is
`app_dirs2` gaining an edge from `strata-core`, and its own dependencies are `jni` /
`ndk-context` / `winapi` / `xdg`, none reachable on macOS and none touching TLS. The failing path
(`profile_credentials` → `aws-config` → a TLS client whose roots come from `rustls-native-certs`)
is untouched by this branch, and `strata-agent`'s provider tests — also untouched — sat over 60s
on client construction in the same run, which points at the same shared cause.

**And it is one symptom of three.** In the same sittings, freshly-built test binaries repeatedly
stalled at 0% CPU *before `main`* — a `sample` showed the whole process sitting in `_dyld_start` —
and `strata-agent`'s provider tests, which this branch also never touched, sat over 60 seconds
constructing clients. A wedged trust store, a wedged loader, and slow client construction are the
shape of macOS security services (`trustd` / `syspolicyd`) misbehaving on this machine, not three
separate bugs.

**Answered: it is this machine, not the pipeline.** The question left here was whether the flake
ever fires on a GitHub runner. PR #146's `test` job — which is the job this test runs in, since it
is a `strata-core` *lib* test and only `object_store_minio` is skipped there — passed on the first
attempt, alongside `minio` and `review`. So the trust-store read is sound on a clean macOS runner
and the fault is local to this workstation. A reboot is the first thing to try; nothing in CI or
in the code needs a change for it.

### What was actually verified, and how

A single `cargo test --workspace` never completed on this machine — it stalled in the loader, twice,
at different binaries. So the suite was verified **per target** instead, every one of them green:

| Target | Result |
|---|---|
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | clean, exit 0 |
| `strata-core` lib | 589 passed |
| `strata-core` `engine_export` / `snapshot_order` / `engine_chart` | 20 / 11 / 7 passed |
| `strata-core` `object_store_minio` | 1 passed, against a real MinIO container |
| `strata-model` | 26 passed |
| `strata-agent` lib + integration | 122 passed, plus its suites |
| `strata-freya` | 696 passed, 11 ignored, exit 0 |
| `UPDATE_SCHEMA=1 … schema_in_sync` | in sync (the new `tab.close` field maps to an existing role, so the schema did not move) |
| fork, as its own workspace | `freya-core` + `freya-radio` build clean |

The MinIO run matters twice over: it is the only thing that exercises the S3 credential bridge, and
it is what proves the tightened rejected-credentials assertion (a 403, not merely "an error")
matches what MinIO actually answers.

**Method note for whoever picks this up:** the failure was invisible for a while because
`cargo test … | tail -n` and `cargo test … | rg …` both report the *pipe's* exit status, not
cargo's. Two runs in this review looked green that way while one had not compiled at all.
Redirect to a file and check `$?`, or read the `test result:` lines.

## Verified sound (do not re-audit without new evidence)

The agent read-only boundary fails closed and has no escalation via `PREPARE`, `EXECUTE`, `SET` or
`COPY`; DataFusion 54's table functions were checked in dependency source and expose no
file-reading UDTF. The secret store has no `Serialize`/`Display`, redacts `Debug`, zeroes on drop,
and its no-key-in-config claim is asserted on serialized bytes. Statement classification survives
CTE-wrapped writes, multi-statement input and the capability split. The snapshot lifecycle's pins,
cancel races and flock claim protocol are correct. No `std` lock is held across an await anywhere.
`write_atomic` and the `session.json` corrupt/unreadable split are exemplary — which is precisely
what made the app config's exemption from both stand out. The release path's quoting,
tag-after-build ordering, version single-sourcing and secret hygiene are sound, and the
self-contained-bundle claim holds (both font families × three weights are embedded).
