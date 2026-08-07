# Engine model

How the DataFusion boundary is shaped, and the policies it enforces. The invariant form is in
[AGENTS.md](../../AGENTS.md) §2; snapshot lifecycle is [SNAPSHOT_SPEC.md](../SNAPSHOT_SPEC.md).


The engine (`strata_core::engine::Engine`) is a **direct-call async facade**: it owns a private
multi-thread Tokio runtime (DataFusion's operators need a Tokio context; query CPU never touches
the render thread), spawns each call onto it, and the caller awaits the `JoinHandle` — which is
executor-agnostic, so Freya's non-Tokio UI executor awaits engine methods like any async fn. No
UI-side runtime, no channels, no request ids. freya-query capabilities call the facade directly
(`engine.query(…)`, `engine.fetch_page(…)`); snapshot lifecycle (supersede / cancel / retire) is
the facade's own bookkeeping — see **`docs/SNAPSHOT_SPEC.md`**. Snapshots are **Arrow IPC**, not
parquet, so a result's type survives the round trip (parquet cannot write a union or a zero-field
struct at all); compressed they are the same size on disk. The export null-gate's exact counts come
from the write pass (`query::SnapshotStats`), not a footer. In Freya the handle is `EngineCtx`
(an `Arc<Engine>` + Deref) held in context — not stored in any god-object `AppState`. Managed DDL
policy: the editor runs `SELECT`/`EXPLAIN`/`SHOW`/`DESCRIBE` **only**. Views are Save's artifact,
never typed DDL — ⌘S / Save-as-view wraps the buffer's *plain query* in `CREATE OR REPLACE VIEW`
itself (`Engine::create_view`), so typed `CREATE`/`DROP VIEW` is blocked (validation points at
Save / the catalog), like `CREATE EXTERNAL TABLE` / CTAS / `INSERT` (use Table Config) and the
hard-blocked `CREATE DATABASE`/`SCHEMA`.

**A remote scheme is something we register, and a connection is what registers it.** DataFusion
core resolves nothing: there is no built-in "read `s3://…`", so an embedder builds an
`object_store` and calls `register_object_store` **per bucket** or every scan of it fails with *no
suitable object store found*. That call is the whole of what a connection does — which is why a
[`ConnectionDef`](../CONNECTIONS_SPEC.md)'s identity is exactly what the registry keys on (scheme +
authority, no path — so it is `ConnectionDef::url()`, **never the bucket**: `s3://lake` and
`gs://lake` are two connections, and anything addressing one by bucket answers one row twice and
leaves the other unanswered), why the def stores the **authority alone** and derives the scheme
from the provider (two statements of one fact can disagree), and why connections are the **first**
phase of `register::register_pass`: a table registered before its bucket's store fails on a def
that is perfectly correct. `engine::store::connect` is all-or-nothing — it probes the credential
chain *before* registering, and on `Err` deregisters whatever an earlier pass left, so a connection
is never both refused and live and the `Reg` row that folds its outcome means what it says.
`object_store` alone is env-only (it does not read `~/.aws` profiles and does not do SSO), so the S3
arm wraps **`aws-config`**'s resolved credentials in an `object_store::CredentialProvider` —
resolving per request, so short-lived credentials refresh themselves. **Ambient and Named profile
are two different providers**, not one chain with a setting: naming a profile on the default chain
only configures its Profile arm, which sits behind `Environment`, so an exported `AWS_ACCESS_KEY_ID`
silently wins and the chosen profile is never read. **No arm anywhere in that module takes a
secret**, and that absence is the feature: a connection carries a profile *name* and a key *file
path*, never a key.

**The SQL function set is the live registry, not a list we keep.** `build_context` registers
`datafusion-functions-json`'s Postgres-style accessors (`json_get` / `->` / `->>`; **not** `?`,
which sqlparser reads as a placeholder before the crate's planner sees it — `json_contains` is the
spelling that works) over Utf8
columns holding JSON text, and that call is the whole integration: `engine::functions::snapshot`
walks `ctx.udfs()`, so anything registered reaches autocomplete, signature help and the docs panel
with no per-function table and no way for the completion pool and the engine to disagree. Adding a
UDF family means one `register_*` call in `build_context` and nothing else.
(`.claude/tasks/workstream-json-polymorphic/` — WJ-01, and WJ-02 for the union-tolerant JSON
reader that makes the accessors pay off.)

> The Dioxus-era `Command`/`Event` channel protocol + worker loop was **deleted from
> `strata-core`** with P2-01. `crates/strata-dioxus` still references it and therefore **no longer
> builds** — it is kept as *reference code only* for porting features to Freya. Don't try to fix
> its build.
