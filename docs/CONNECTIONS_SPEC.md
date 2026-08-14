# Connections — reading from remote sources

How Strata reads parquet/CSV/JSON out of S3, GCS and plain HTTP(S), and how it queries a live
PostgreSQL. A **connection** is a project-scoped description of one remote source: the bucket,
origin or server it names, the provider that serves it, and a *reference* to where credentials
live. Tables read through an object-store connection by naming it; a database connection needs no
tables at all — see [Database connections](#database-connections).

Two rules shape everything below:

- **No connection field is a secret value.** A connection carries only non-secret metadata —
  bucket, region, endpoint, an auth *mode*, a server and a role — plus at most a named `~/.aws`
  profile, a service-account key **file path**, or the bare statement that this machine's OS
  keystore holds a password. There is no key or token field anywhere in the model
  (`crates/strata-model/src/connection.rs`), so one cannot be persisted by accident. Object-store
  credentials resolve at query time from the machine's own provider chains, and never touch
  Strata; a database password is held by `strata_core::secret` and read per use.
- **DataFusion resolves nothing itself.** There is no built-in "read `s3://…`": the embedder
  builds an `object_store` and registers it per bucket, or every scan fails with *"No suitable
  object store found"*. Registering that store is the whole of what an object-store connection
  *does* (`crates/strata-core/src/engine/store.rs`). A database connection is the same shape
  against a different registry: it builds a connection pool and registers a **catalog**
  (`crates/strata-core/src/engine/db.rs`).

## Providers

Four providers — **S3**, **GCS**, **HTTP** and **PG** (`ProviderId::ALL`, pinned at four by test).
The provider is an **explicit picker** in the editor, never inferred from a typed URL scheme.

The first three register an object store; PG registers a catalog. Where a surface asks *which
connection do these files read through* it offers `ProviderId::OBJECT_STORES` rather than `ALL` —
the Configure window's LOCATION **TYPE** pill, and nothing else.

- **S3-compatible** stores (Cloudflare R2, MinIO, Alibaba OSS, Tencent COS) ride the S3 provider
  via its **Endpoint** field plus an **Allow plain HTTP** toggle — they are not separate
  providers. An `http://` endpoint without the toggle is refused by name, because the underlying
  HTTP client is built `https_only` and would otherwise fail every request with a bare
  "builder error".
- **HTTP** is a public origin: always anonymous, no auth control, no region — the address itself
  is a whole URL, scheme included.

## Identity and persistence

**A connection's identity is its URL** — `ConnectionDef::url()`. For an object store that is
scheme *and* authority, because that is exactly what DataFusion's object-store registry keys on.
Never the bucket alone: `s3://lake` and `gs://lake` share a bucket and are two different
connections over two different stores. Everything that addresses a connection (a registration
outcome, a store row, the Configure picker, a table def, a derived keystore slot) names it by this
URL. The pane's sort order is the **address**, so `upsert_connection` replaces by URL and inserts
in address order.

The def stores the **address** and derives the scheme from the provider:

- **S3 / GCS** — the bucket name alone (`acme-lake`). Storing the scheme too would be two
  statements of one fact that can disagree: an `s3://` bucket under a GCS provider would read one
  way and register another.
- **HTTP** — the whole origin (`http://aserver:8484`). `http` and `https` are two different
  origins, so the scheme is part of the address and only the person typing knows which their
  server speaks.
- **PG** — `host:port/database` (`db.internal:5432/analytics`). Its URL is
  `postgres://{user}@{address}`, which does *not* stop at the authority: a database connection
  keys no object-store registry, and the two further things that make two of them different — the
  database and the **role** — belong in its identity. Two roles over one database are two
  connections with two sets of visible schemas, and the provider crate's own join-pushdown context
  agrees, keying on host + port + db + user.

Defs persist in the committed `.strata/project.json` (`ProjectDefs::connections`), beside the
tables and views. Nothing in a def needs gitignoring: a profile *name* and a key *file path* hold
nothing a colleague may not have. The shape is a tagged provider — the provider *is* its own
settings, so an S3 region cannot be written down on a GCS bucket:

```json
{ "address": "acme-lake",
  "provider": { "provider": "s3", "region": "eu-west-2",
                "auth": { "mode": "profile", "name": "analytics" },
                "endpoint": "", "allow_http": false },
  "client_config": { "timeout": "30s" } }
{ "address": "lake",                "provider": { "provider": "gcs", "auth": { "mode": "service-account", "path": "…" } } }
{ "address": "http://aserver:8484", "provider": { "provider": "http" } }
```

`client_config` is absent unless set; every provider setting is `#[serde(default)]`, so a def
written before a setting existed still loads. Older files that stored the field as `bucket` (and,
for HTTP, without a scheme) load through a serde alias plus `ConnectionDef::migrated`.

## Auth modes

Per provider, and only secret-free options exist (`S3Auth` / `GcsAuth` in
`crates/strata-model/src/connection.rs`):

| Provider | Modes |
|---|---|
| S3 | **Ambient** (the host's whole chain) · **Named profile** (a profile from `~/.aws/config`) · **Anonymous** (unsigned, public bucket) |
| GCS | **Ambient** (Application Default Credentials) · **Service-account file** (a key-file *path*, never inline JSON) · **Anonymous** |
| HTTP | none — always anonymous |

## How credentials resolve

Credentials resolve **at query time** from the machine's own chains; the app never copies a key
out of them.

**S3** wraps the AWS SDK's resolved credential provider in an `object_store`
`CredentialProvider` (`engine::store::SdkCredentials`) that re-resolves **per request**, not once
at build. That is the point of wrapping the provider rather than copying a key out of it:
SSO / assumed-role / IMDS credentials expire in minutes, and the SDK's provider is the thing that
knows how to refresh them — an `aws sso login` in another terminal just works. The `aws-config`
dependency exists because `object_store` alone is env-only: it reads `AWS_*` variables plus
IMDS/ECS/web-identity, but does not parse `~/.aws` profiles, does not do SSO, and ignores
`AWS_PROFILE`.

**Ambient and Named profile are two different providers, not one chain with a setting.**
Naming a profile on `aws-config`'s default chain only configures that chain's *Profile arm* —
the chain stays `Environment → Profile → WebIdentity → ECS → IMDS` — so a Strata launched from a
shell exporting `AWS_ACCESS_KEY_ID` would sign as the *environment* identity while the row showed
the chosen profile, and a misspelled profile name would still connect green. So **Ambient** is
`aws_config::defaults(…)` (the whole chain, whatever answers) and **Named profile** is
`ProfileFileCredentialsProvider` standalone: that profile's own mechanism (`source_profile`,
`role_arn`, `sso_session`, `credential_process`) and no fallback to anyone else's identity.
Pinned by test (`store.rs`, `a_named_profile_signs_as_that_profile_and_not_as_the_environment`).

**GCS** is native `object_store`: Ambient is ADC (`GOOGLE_APPLICATION_CREDENTIALS`, then the
gcloud ADC file, then the GCE/GKE metadata server); Service-account uses
`with_service_account_path` — the key file's path, never its contents. One consequence worth
knowing: the builder installs the GCE metadata arm without a request, so an ambient GCS
connection on a machine with no credentials at all still registers cleanly and fails at first
read — the one arm whose status cannot be known without asking the bucket.

**S3 region is required and never defaulted.** `AmazonS3Builder` silently assumes `us-east-1`
when the region is blank (arrow-rs#2795), which resolves to a real endpoint serving a different
bucket's worth of nothing — so the engine refuses a blank region rather than letting the default
stand, and the editor blocks Save on the same terms.

## Connecting is all-or-nothing

Both arms settle through one body (`engine::connect::settle`), which takes the take-back as an
argument: the registries differ — an object store keyed by URL, a catalog keyed by its SQL name —
and the contract does not.

`engine::store::connect` **probes the credential chain before registering**: it resolves the
chain once, throws the answer away, and only then registers the store. On `Err` nothing is
registered — including anything an earlier pass registered under the same URL, which is
deregistered rather than left behind. A connection is never both refused and live, which is what
makes its status a single honest row: without the probe, a credential-less connection would
register happily and the diagnosis would land on every table over the bucket as one opaque
signing error each.

One honest limit: the probe checks that the host can *produce* a credential, not that the bucket
*accepts* it. Wrong-but-well-formed credentials connect green and surface at the first read.

## Address rules

Each provider's published naming rules live in **one place** — `Provider::check_address` — called
by both the engine's `connect` and the connection editor, so a name refused at the field is
refused by the engine in the same words:

- **S3** — AWS's general-purpose bucket rules: 3–63 characters;
  lowercase/digits/dots/hyphens; alphanumeric at both ends; no `..`; not formatted as an IP
  address. The S3-compatible stores are all at least this strict, so applying AWS's rules refuses
  nothing they would have accepted. (The IP rule is AWS's own and was missing while the GCS
  checker beside it had the identical one, so an IP-shaped bucket passed the field and died later
  on the store's error — the exact outcome the check exists to prevent.)
- **GCS** — Google's rules, which are deliberately *not* the same: underscores allowed, a dotted
  name may run to 222 characters (each part to 63), no dotted-decimal IP, no `goog` prefix, no
  `google` anywhere.
- **HTTP** — a whole origin URL. Anything after the authority is **refused by name** rather than
  trimmed: the registry keys on scheme + authority, so a path here would register under a key
  nothing looks up. The message quotes the part to drop and says it belongs to the table that
  reads through the connection. **Userinfo is refused too** — `https://alice:hunter2@files.example.com`
  is a well-formed origin and the ordinary way a protected file drop is handed around, so it gets
  pasted here; every word of a `ConnectionDef` rides in the committed, shared `.strata/project.json`,
  and it would be echoed on the Connections row and in the Forget confirm besides. It is asked of
  the host part only, before the path is trimmed, so an `@` inside a *path* is answered by the
  path's own message instead. No provider Strata supports authenticates this way, so nothing is
  lost by refusing it.
- **PG** — `host:port/database`. The port is required and never defaulted to 5432: a Postgres off
  5432 is the ordinary case for a container, a tunnel or a pooler, and a def reading
  `db.internal/analytics` while it means `:5432` shows one thing and connects to another — the
  same argument that keeps S3's region out of `object_store`'s silent default. A scheme is refused
  (the provider supplies it), a second `/` is refused (one database), and userinfo is refused for
  HTTP's reason — the role is its own field. The port is the **last** `:`, so an IPv6 literal
  reads either way (`::1:5432/db` or `[::1]:5432/db`); `engine::db` unwraps the brackets before
  the driver sees the host, which takes the address itself.

  The **role** is checked on the same terms (`PgStore::check_user`), and for a sharper reason: the
  driver's parameters are interpolated into a connection string with no quoting, so a space or an
  `=` in the user fails as a connection string the parser cannot read rather than as "that user is
  wrong". `CREATE ROLE "read only"` is legal Postgres and simply cannot be dialled through this
  stack, so it is refused by name. It is also half of `ConnectionDef::url()` — the connection's
  identity, and the input its keystore slot derives from.

The checks are deliberately not exhaustive — each provider reserves further names no local check
can settle — they catch what is *statically* wrong so the user is told at the field instead of by
a signing error.

**A database's catalog name has a rule of its own**, `check_catalog_name`, called by the engine's
registration and the editor's blocker alike: a bare SQL identifier (leading letter or `_`, then
letters, digits and `_`), not the workspace's own catalog (`strata`), and not another connection's
— folded, because unquoted identifiers are. The two callers ask different sets on purpose: the
editor folds against what is *stored*, so it can warn before anything is dialled, and the engine
folds against what is *registered*, because a connection that failed to connect reserves nothing.

## Client options

`client_config` is `object_store`'s own `ClientConfigKey` map, offered on every provider because
all three stores are built on one HTTP client: timeouts, connection pooling, proxy settings, HTTP
version and keep-alive, user agent, certificate trust — 16 keys, enumerated in
`engine::store::CLIENT_KEYS` with a description each (the editor offers them with autocomplete).
`check_client_config` validates the map in both the editor and `connect`: an unknown name or a
blank value is refused by name rather than silently dropped at build time.

`allow_http` is deliberately **not** among them, because it is already said elsewhere and a
second control for one setting is two controls that can disagree: on S3 it is the endpoint's own
toggle, and on HTTP it is derived from the scheme the user typed.

## The connection editor

A **child window** of the project window (`crates/strata-freya/src/apps/connection/`), one per
def. Its rows, top to bottom — and which rows exist depends on the provider, and only on the
provider (a control that cannot mean anything for the chosen provider is not shipped disabled):

1. **PROVIDER** — segmented pill, S3 / GCS / HTTP / PG (`ProviderId::ALL`, since this is the
   picker that constant is for; the Configure window's LOCATION pill asks the narrower
   `OBJECT_STORES` question).
2. **The address box** — BUCKET for S3/GCS, URL for HTTP. A database has no single address box:
   it splits `ConnectionDef::address` into **URL** (`host:port`) and **DATABASE** (`appdb`),
   because a server and the database on it are two things Postgres names separately. The stored
   def is still one `host:port/database` string, so `parse_pg_address` remains the only parse of
   that grammar.
3. **CATALOG**, **USER**, **PASSWORD**, **SSL MODE** (PG only), in that order. CATALOG is
   `PgStore::catalog`, the prefix Strata queries the connection by (`pg` makes
   `pg.public.orders`) — Strata's name for the connection, not anything the server has, and its
   hint says so. It is the user's choice rather than derived from DATABASE, because two servers'
   `analytics` would derive one prefix. Making it unnecessary for everyday queries is
   **DB-09** (a current database, so unqualified names resolve). The
   certificate path under SSL MODE appears for the two verifying modes and is optional there
   (blank is the machine's own trust store). There is no SCHEMAS row: schema enablement is a
   tree-node gesture (DB-05), where the live enumeration already sits and where one picker serves
   New and Edit alike — a second surface for the same list is two controls that can disagree.
4. **AUTHENTICATION** (S3 and GCS only) — the mode pill plus whatever it refers to. The S3
   profile is picked from a `Select` over the machine's own configuration
   (`Engine::aws_profiles` reads the section headers of `~/.aws/config` and `~/.aws/credentials`
   — names only, nothing from a profile's body).
5. **REGION** and **ENDPOINT** (S3 only). A new connection opens with a **blank** region —
   `us-east-1` is the placeholder, never the value, because a seeded `us-east-1` is exactly the
   silent builder default in the user's handwriting. Blank blocks Save and says why.
6. **CLIENT OPTIONS** (object stores only) — the key/value table, edited as rows and committed as
   a map. A database speaks no HTTP, so the row and its validator are gated together.
7. A standing note saying where credentials actually come from — per provider, and for PG the one
   that says the password is this machine's keystore's and a colleague enters their own.

A field's error lives in the **footer**, not on the field: one value both disables Save and
explains it, so the form cannot hold two accounts of its own validity. Two of those the draft
cannot answer alone, and both read the project's stored defs beside it — the URL clash, and a
catalog name another database connection holds (`check_catalog_name`).

**The PASSWORD row reports this machine, not the def.** The settings window's API-key marker is
honest because it minted the reference when it stored one; a def carries only the *expectation*,
so the row probes the local keystore once at mount and shows one of: no password expected, stored
on this machine, expected but not stored here, still asking, or the keystore's own refusal. The
two clearing gestures are deliberately separate presses — *remove from this machine* deletes the
local entry and leaves `PgPassword::Keystore` standing, while *this connection uses no password*
edits the shared def to `PgPassword::None`. Conflating them means one person casually breaking
every colleague who has a password. There is no mode pill: a password is optional, so absence is
a state rather than a mode.

Save writes the def, persists the project, **deregisters the old URL itself** when the edit moved
the bucket, the provider or a database's user (nothing downstream ever sees the def it replaced),
and asks for a whole-catalog registration pass; the window then watches its own row and closes
when the connection settles. A database connection has one step in front of all of that: whatever
this machine's keystore owes the password, on a worker, so a keystore that refuses writes nothing
— a migration when the identity moved (`secret::migrate_derived`), then the put or the delete.
A **catalog-name** move with an unchanged URL needs nothing of Save's: `db::connect` replaces on
re-connect, and the whole-catalog pass is what re-connects.

## The data-sources tree

One sidebar pane (DB-05), reached from the activity rail (clicking it again collapses the
sidebar). It answers "what data do I have" for the project's catalog and its connections
together — the separate Connections pane it replaced is gone, and so is the rail toggle beside
it.

Top level is **data sources**:

- the **project workspace**, first and open by default, labelled with the project's own name and
  addressed as `strata`. It is not a "files provider": it is the catalog Strata's federating
  engine defines, so file tables, internal tables, views, saved queries — and a **cross-source**
  view joining workspace files onto `pg.…` — all nest under it. Its children are the flat pane's
  TABLES · VIEWS · QUERIES groups verbatim: same rows, same `Reg` status slots, same menus, same
  expansion to columns, and the TABLES `+` still opens Configure on a new table;
- one node per **database connection**, opening onto its enabled schemas, then Tables and Views
  groups split by the listing's own `relkind`, then its relations. All of it is
  `Engine::db_listing`'s scoped-and-tagged answer — read from the connect-time enumeration, not
  the network — so collapse and re-open cost nothing and ↻, which re-connects, is the refresh. A
  schema the def enables and the server does not have renders as its own failed node naming that
  fact. A relation is a leaf: its columns are an introspection, and the surface that reads them
  (and can select one) is DB-07's inspector;
- one node per **object-store connection**, opening onto the workspace defs that read through it
  as **links** — pressing one opens the def's ancestors and brings its own row into view, rather
  than offering a second editable copy of it.

Every connection node carries a **provider badge** (`Provider::to_string()` — `S3` / `GCS` /
`HTTP` / `PG`), its address, a **status glyph** (nothing when connected, a spinner once the wait
outlasts the progress hold, or a warning triangle carrying the engine's own refusal, clipped to
what a tooltip holds and naming Problems for the rest), and a trailing **⋮** menu (also
right-click): **Edit**, **Schemas…** on a database, **Forget**. Pressing the row opens it; its
actions are the menu.

**Schemas…** is a picker over the same `db_listing` answer, so the tree, the picker and
completion cannot disagree about what a connection shows. Its write is display-only, so it edits
the def **in place** (`ProjectState::update_connection_def`) and keeps the row's registration —
going through `upsert_connection` would leave a `Reg::Loading` that only a whole-catalog re-scan
could answer. A connection that is not live has no enumeration to offer: the picker then lists
the def's own schemas and says so.

The header's `+`, the empty state's row and a node's Edit all open the editor window. **Forget's
consequence differs by kind**, and the confirm carries which kind it is rather than looking it
up: an object store's readers are the tables whose sources name it and the views behind those; a
database's are the views whose plans scan through its catalog (`ViewInfo::remote_deps`), since no
def can name a database. Confirming removes the def, deregisters the store or catalog, and — for
a database — deletes the derived keystore entry the def expected.

One filter spans the tree. A node survives it if its own name matches or any descendant's does;
the workspace's three groups are the stated exception, staying put with their counts following
the filter, because a count of `0` is what says the filter found nothing there.

## Registration order

Connections are the **first phase** of the project registration pass
(`strata_core::register::register_pass`): every connection registers before any table, because a
table's source path cannot resolve to an object store that is not registered yet — an ordering
bug there would look exactly like a broken table — and because a view over `pg.public.orders`
cannot plan before that catalog exists. A whole-catalog ↻ re-connects everything; a
single table's Refresh does not re-connect anything.

## Database connections

A **PG** connection registers a DataFusion **catalog** rather than an object store, so the editor
can `SELECT … FROM pg.public.orders JOIN events …` — cross-joining file-based tables onto live
PostgreSQL, with filters, projections and whole same-source subplans pushed down to the server.
Built on `datafusion-table-providers-postgres` and `datafusion-federation`, both pinned in
lockstep with the `datafusion` version (`crates/strata-core/Cargo.toml` says why).

**The whole database comes through, and nothing is declared per table.** Connect enumerates every
schema the role can see and every relation in them — one round trip against `pg_class`, filtered
to `relkind IN ('r','p','v','m','f')` and to what the role may `USAGE`/`SELECT`, system schemas
excluded — and registers a catalog whose table providers are built lazily on first use and then
cached. There are no per-table defs and no manual adds. The line is *discovery gets catalogs,
declaration gets defs*: a bucket cannot say what its tables are — someone must declare globs, a
format and its options, and that declaration can fail, which is what the `Reg` rows exist to show
— while a database answers for itself. Pinning one remote relation into the workspace is a
**view** (`CREATE VIEW orders AS SELECT * FROM pg.public.orders`), which needs no new machinery.

`pg_class` rather than the provider crate's own `pg_tables` listing: remote views, materialized
views, partitioned tables and foreign tables must show and resolve, or the catalog lies about what
is queryable. That is one of three reasons the catalog/schema provider is ours rather than the
crate's `DatabaseCatalogProvider`; the others are that it snapshots the listing at construction (a
↻ could not refresh it) and that it skips the federation wrapper, silently forfeiting the pushdown
this exists for.

**The def:**

| Field | What it is |
|---|---|
| `catalog` | How queries address the database — the catalog half of `catalog.schema.table`. |
| `user` | The role, and half the connection's identity. |
| `sslmode` | libpq's own vocabulary, in libpq's spellings. Defaults to `prefer`, as libpq does. |
| `sslrootcert` | A root-certificate **file path**, read only by the two verifying modes. |
| `password` | `none` or `keystore` — the **expectation**, never a reference. |
| `schemas` | The schemas this connection *shows*. Defaults to `["public"]`. |

**The password.** This is where W7's "Strata never stores, prompts for or reads a secret" was
deliberately **rewritten** rather than routed around: that rule was a consequence of the OS
keystore not existing when W7 was built, and of object stores happening to have host-side
credential chains where a database does not. The password is captured exactly as an assistant
provider key is (`strata_core::secret`) and read **per pool connection**, never cached.

The reference is **derived**, not minted: `SecretRef::derived("pg-password", def.url())`, a
`Uuid::new_v5` over a fixed namespace. A minted id in a committed, shared `project.json` would be
rewritten by every colleague who entered their own password — two machines ping-ponging one id
through git forever. A derived one addresses the same slot on every machine while each machine's
keystore holds its own entry, and the def therefore stores only `PgPassword::Keystore`: storing a
derivable value beside the fields it derives from is two statements of one fact that can disagree.
The consequences are carried honestly — an identity edit **migrates** the entry
(`secret::migrate_derived`), a Forget deletes it without needing a stored ref, and on a machine
with no entry the row settles failed naming the fix ("No password is stored on this machine for
'…'"), the same shape as an expired SSO session.

**Connecting is the probe, with nothing extra.** Building the pool resolves the host, opens a TCP
connection, authenticates, builds the pool and runs `SELECT 1`; any of them failing is the whole
answer. There is no separate reachability step, because unlike a bucket — whose description can be
well-formed and wrong in a way only the bucket knows — a database either let us in or did not. A
reconnect **replaces**: whatever that URL last registered comes out, under the name it went in
under, so the editor's rename (same URL, new catalog name) is handled by the registration rather
than by a surface remembering.

**Schema visibility scopes display, never resolution.** `schemas` is DataGrip's "N of M schemas"
choice. The engine registers every schema regardless — the providers are lazy, so that costs
nothing — which means a query naming a schema that is not enabled still resolves and runs.
`Engine::db_listing` is the one read every surface shares, and it answers **scoped and tagged**
(`Live | EnabledButMissing | NotEnabled`), so nothing re-derives visibility. It reads the
connect-time enumeration, which is why a ↻ *is* the refresh.

**What is pushed down, so nobody re-measures it.** A single-table filter, projection and `LIMIT`
push down even without federation (the scan unparses them; anything unsupported falls back and
re-applies locally). A same-connection join, aggregate or TopK federates into **one** remote
statement. A pg × parquet join is ambiguous at the join node: the largest single-provider subtree
under the pg side still federates, and the join itself runs locally. A federated subplan that
unparses to SQL the server rejects fails **loudly at execute time** — there is no silent local
fallback, and the results pane's error path is the surface.

`jsonb` and other exotic types arrive as `Utf8` JSON text (`UnsupportedTypeAction::String`), which
the app's own Postgres-style accessors already read. The crate's default would refuse the whole
relation for one such column. This is representation honesty rather than silent corruption: the
value is intact, only the type is wider.

**Read-only in v1.** Nothing writes to a database: `INSERT` gates on whether the target is a table
Strata owns, and the schema provider refuses a registration in its own words underneath that.

Verified end to end against a real PostgreSQL in
`crates/strata-core/tests/postgres_federation.rs`.

## Tables over a connection

`TableDef::connection` holds the chosen connection's `url()` — a *reference*, never a copy of the
bucket, provider or auth — and it is the one field that says a table is remote. Exactly when it
is set, the table's sources are **bucket-relative** (`events/2024/**/*.parquet`), stored as
typed. `strata_core::project::resolve_source` is the single place the two halves compose: given
the connection it prepends the URL, and without one it joins onto the project folder. One
function taking the connection, rather than a local rule with a remote one beside it, so a
bucket-relative source can never be silently resolved against the local disk.

In the Configure window, **LOCATION** is an explicit Local / Remote toggle — never inferred from
a path's scheme. Remote mode shows a single bucket-relative SOURCE PATH (rendered with the
non-editable bucket prefix), a TYPE segmented control that *filters* a CONNECTION dropdown (a
filter, never the table's provider), and a **New connection…** entry that opens the editor. A def
naming a connection the project no longer has keeps naming it, and Save is blocked with:

> 's3://gone' is not a connection in this project. Choose one, or add it back.

Rewriting it to local disk would silently re-point the table at a relative path on the user's own
machine.

The two locations hold **separate** paths in the draft (`local_sources` / `remote_source`) and the
toggle moves none between them: the disk's list and the bucket's one path are written against
different roots, so a flip shows the other arm's own answer — empty until it is typed — and a flip
back finds the first arm as it was left. Nothing is seeded either way; an empty box blocks Save
with the same "A table needs at least one source path".

### The typed form (ED-10)

A `CREATE EXTERNAL TABLE` typed into the editor reaches the same def, and its `LOCATION` is where
the two halves meet from the other direction:

```sql
CREATE EXTERNAL TABLE events STORED AS PARQUET
  LOCATION 's3://acme-lake/events/2024/'
```

lands `connection: Some("s3://acme-lake")` and the bucket-relative source `events/2024/` —
`project::split_remote`, which is `resolve_source` read backwards and asserted to round-trip. The
URL has to be a connection **this project has**, and is refused by name otherwise, on the terms the
Configure footer is blocked on:

> 's3://acme-lake' is not a connection in this project. Add it in Connections

A statement cannot mint one. A connection carries a provider, a region and where its credentials
come from — none of which the statement says, and one of which it must never carry — and it also
carries a *status*, which comes from a probe rather than from a sentence. Refusing here is also
what keeps DataFusion's "No suitable object store found" off a table row, which is the whole point
of registering connections first.

Membership is `Engine::connections`: the URLs `connect` was handed, noted **whatever the outcome**
and removed by `disconnect`. A connection whose region is blank or whose SSO session expired is
still one the user may point a table at — the fix comes afterwards — so asking DataFusion's
object-store registry instead would have answered *no* for exactly the rows the user is on their
way to repair.

The lookup **resolves** rather than merely tests: it falls back to a case-insensitive match,
because `Url::parse` lower-cases a scheme and a host on the way into the registry (so
`S3://acme-lake/events/` names a store that is registered), and it answers with the *connection's*
spelling, which is the string the def then stores — the same string the Configure picker,
`resolve_source` and the Forget confirm all address it by.

This is not the LOCATION toggle read differently. That toggle is an explicit choice precisely so a
typed **path** is never re-read as remote; in a statement the scheme is the only thing said about
where the files are. And the statement's `OPTIONS` cannot carry any of the connection's settings:
`aws.*`, `gcp.*`, client timeouts and the rest are refused toward this pane, on the key alone,
without the value ever being read (`STATEMENTS_SPEC.md` §6.7).

## Hive-partitioned lakes over a bucket

Partition detection is format-agnostic and works over any registered store:
`engine::catalog::detect_partitions` reads `key=` levels from glob segments in the path, or finds
them by **listing** — `list_with_delimiter` through the session's registered object store, the
same call for a local disk and a bucket. Partition columns register typed, rows come back
carrying their folder's values, and a filter on a partition column takes DataFusion's pruning
path through the same store.

The whole arm is proven against a real MinIO in
`crates/strata-core/tests/object_store_minio.rs` — deliberately not `#[ignore]`d, because it is
the only thing that would catch a regression in the S3 credential bridge (see CLAUDE.md for the
container-runtime requirement).

## How a query reaches a bucket

```mermaid
flowchart LR
    subgraph project [".strata/project.json"]
        C["ConnectionDef<br/>s3://acme-lake · region · profile"]
        T["TableDef events<br/>connection = s3://acme-lake<br/>source = events/2024/"]
    end
    C -->|"register pass, phase 1:<br/>probe chain, register store"| R["object-store registry<br/>key = s3://acme-lake"]
    T -->|"resolve_source"| U["s3://acme-lake/events/2024/"]
    U --> S["ListingTable scan"]
    R --> S
    S -->|"per request"| P["SdkCredentials →<br/>host's own credential chain"]
```
