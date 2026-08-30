# Data sources — reading from remote sources

How Strata reads parquet/CSV/JSON out of S3, GCS and plain HTTP(S), and how it queries a live
`PostgreSQL`. A **data source** is a project-scoped description of one remote source: the name it
is called, the bucket, origin or server it addresses, what serves it, and a *reference* to where
credentials live. Tables read through an object-store source by naming it; a database source
needs no tables at all — see [Database sources](#database-sources).

> The file is still `CONNECTIONS_SPEC.md` because engine code cites the path. The **word** is
> "data source" everywhere the user meets one; "connection" now means what it says — a live
> connection to a server, an MCP client's connection, an HTTP client's pooled one.

Two rules shape everything below:

- **No data source field is a secret value.** A def carries only non-secret metadata — bucket,
  region, endpoint, an auth *mode*, a server and a role — plus at most a named `~/.aws` profile,
  a service-account key **file path**, or the bare statement that this machine's OS keystore
  holds one. There is no key or token field anywhere in the model
  (`crates/strata-model/src/source.rs`), so one cannot be persisted by accident. Object-store
  credentials resolve at query time from the machine's own provider chains, and never touch
  Strata; a database source's secret is held by `strata_core::secret` and read per use — the def
  records **which** of the kind's secret-typed keys are set and **the slot each is filed under**
  (`SourceDef::secrets`; see [Secrets](#how-credentials-resolve)).
- **DataFusion resolves nothing itself.** There is no built-in "read `s3://…`": the embedder
  builds an `object_store` and registers it per bucket, or every scan fails with *"No suitable
  object store found"*. Registering that store is **half** of what an object-store source does
  (`crates/strata-engine/src/sources/store/`); the other half is a catalog — see
  [Store catalogs](#store-catalogs). A database source is the same shape against a different
  registry: it opens a live handle and registers a catalog
  (`crates/strata-engine/src/sources/`).

## Kinds

Every kind is a **registrant** of the one `DataSource` trait — the four shipped are `s3`, `gcs`,
`http` and `postgres`, each reaching the engine through `EngineBuilder::with_source` behind its
own cargo feature. There is no second trait, no second registry and no typed enum: `Provider`,
`ProviderId` and `ConnectionDef` are gone. Which kinds exist is the engine's answer, not the
model's — `Sources::registrants()` lists what is registered, and the editor's picker, a catalog
row's badge and every form read that one list, so a kind an embedder registers is offered on the
same terms as a shipped one. The kind is an **explicit picker**, never inferred from a typed URL
scheme.

What a kind declares about itself — its label, its badge, its `MODE` (does connecting yield a
store or a catalog), whether it is `WRITABLE`, which settings two of it may not share
(`UNIQUE`), and the settings its form draws — is read from the registrant rather than written
down here. The seam is `crates/strata-engine/src/sources/source.rs`.

**Auth is the kind's own business, declared like any other setting.** S3 does not become three
kinds: it declares one `auth` key (`Field::Choice` of ambient · profile · keys · anonymous) and
gates its credential rows on it with `When`, so the form draws what that choice implies and
nothing else.

- **S3-compatible** stores (Cloudflare R2, MinIO, Alibaba OSS, Tencent COS) ride the S3 kind via
  its **Endpoint** setting plus an **Allow plain HTTP** toggle — they are not separate kinds. An
  `http://` endpoint without the toggle is refused by name, because the underlying HTTP client is
  built `https_only` and would otherwise fail every request with a bare "builder error".
- **HTTP** is a public origin: always anonymous, no auth control, no region — the address itself
  is a whole URL, scheme included.

## Name, identity and persistence

**The name is the identity.** `SourceDef::name` is what the user typed and nothing derives it:
it is what the catalog tree shows, what the editor titles, what a table def points at, what a
store source's catalog is registered under, and — for a database source — the catalog half of
`catalog.schema.table`. One field for all of it, so what a user renames is what queries say. It
is not `(kind, address)` and nothing is minted from those: two sources may hold identical
settings and differ only in what they are called.

**Whether two sources may share settings is the kind's rule, not the axis's.**
`SourceKind::UNIQUE` names the settings two of that kind may not both hold, asked through
`Sources::check_unique`. An object store declares `address`, because two names for one bucket
would register one object store twice. A database declares none: four Postgres servers behind
SSM legitimately differ only in credentials, and refusing that would be refusing a real setup.

A **rename** is a store-funnel operation (`ProjectState::rename_source`): the row moves and every
`TableDef::source` pointing at the old name moves with it, in one settle. It owes the keystore
nothing — see [Secrets](#secrets).

The def stores the **address** as an ordinary declared setting in `config`, and what a scheme
would have said is the kind's:

- **S3 / GCS** — the bucket name alone (`acme-lake`). Storing the scheme too would be two
  statements of one fact that can disagree: an `s3://` bucket under a GCS kind would read one way
  and register another. The scheme is `SourceKind::SCHEME`, composed where the store is
  registered and nowhere else.
- **HTTP** — the whole origin (`http://aserver:8484`). `http` and `https` are two different
  origins, so the scheme is part of the address and only the person typing knows which their
  server speaks.
- **A database** — whatever its kind says an address is (`host:port/database` for `PostgreSQL`),
  judged by that kind's own rule through `Sources::check_address`. The model holds no copy of it.

Defs persist in the committed `.strata/project.json` (`ProjectDefs::sources`), beside the tables
and views. Nothing in a def needs gitignoring: a profile *name* and a key *file path* hold nothing
a colleague may not have. The shape is **flat** — one struct for every kind, its settings in an
open map the kind declares and reads:

```json
{ "kind": "s3", "name": "acme_lake",
  "config": { "address": "acme-lake", "region": "eu-west-2",
              "auth": "profile", "profile": "analytics", "timeout": "30s" } }
{ "kind": "gcs",  "name": "lake",    "config": { "address": "lake", "auth": "service-account", "key_file": "…" } }
{ "kind": "http", "name": "aserver", "config": { "address": "http://aserver:8484" } }
{ "kind": "postgres", "name": "warehouse",
  "config": { "address": "db.internal:5432/analytics", "user": "reader", "sslmode": "verify-full" },
  "secrets": { "password": "9f1c…" }, "schemas": ["public"], "read_only": true }
```

`config` keys are the **kind's own vocabulary**, declared by the registrant
(`DataSource::settings`) and read by it; the model never interprets them, the address included.
`secrets` maps each declared secret key this source has set to the keystore slot it is filed
under — the values themselves live in the keystore or arrive through the kind's environment
convention, never here.

## Auth modes

Per kind, and only secret-free options exist, each declared as an ordinary setting:

| Kind | Modes |
|---|---|
| S3 | **Ambient** (the host's whole chain) · **Named profile** (a profile from `~/.aws/config`) · **Keys** (an access key id, with the secret and any session token in the keystore) · **Anonymous** (unsigned, public bucket) |
| GCS | **Ambient** (Application Default Credentials) · **Service-account file** (a key-file *path*, never inline JSON) · **Anonymous** |
| HTTP | none — always anonymous |

## How credentials resolve

Credentials resolve **at query time** from the machine's own chains; the app never copies a key
out of them.

**S3** wraps the AWS SDK's resolved credential provider in an `object_store`
`CredentialProvider` (`engine::sources::store::s3`'s `SdkCredentials`) that re-resolves **per
request**, not once
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
Pinned by test (`sources/store/s3.rs`,
`a_named_profile_signs_as_that_profile_and_not_as_the_environment`).

**GCS** is native `object_store`: Ambient is ADC (`GOOGLE_APPLICATION_CREDENTIALS`, then the
gcloud ADC file, then the GCE/GKE metadata server); Service-account uses
`with_service_account_path` — the key file's path, never its contents. One consequence worth
knowing: the builder installs the GCE metadata arm without a request, so an ambient GCS
source on a machine with no credentials at all still registers cleanly and fails at first
read — the one arm whose status cannot be known without asking the bucket.

**S3 region is required and never defaulted.** `AmazonS3Builder` silently assumes `us-east-1`
when the region is blank (arrow-rs#2795), which resolves to a real endpoint serving a different
bucket's worth of nothing — so the engine refuses a blank region rather than letting the default
stand, and the editor blocks Save on the same terms.

## Connecting is all-or-nothing

Both arms settle through one body (`engine::connect::settle`), which takes the take-back as an
argument: the registries differ — an object store keyed by URL, a catalog keyed by its SQL name —
and the contract does not.

`engine::sources::store::connect` **probes the credential chain before registering**: it resolves the
chain once, throws the answer away, and only then registers the store. On `Err` nothing is
registered — including anything an earlier pass registered under the same URL, which is
deregistered rather than left behind. A data source is never both refused and live, which is what
makes its status a single honest row: without the probe, a credential-less source would
register happily and the diagnosis would land on every table over the bucket as one opaque
signing error each.

One honest limit: the probe checks that the host can *produce* a credential, not that the bucket
*accepts* it. Wrong-but-well-formed credentials connect green and surface at the first read.

## Address rules

Each kind's published naming rules live in **one place** — its own
`DataSource::check_address` — called by both the engine's `connect` and the source editor, so a
name refused at the field is refused by the engine in the same words:

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
  reads through the data source. **Userinfo is refused too** — `https://alice:hunter2@files.example.com`
  is a well-formed origin and the ordinary way a protected file drop is handed around, so it gets
  pasted here; every word of a `SourceDef` rides in the committed, shared `.strata/project.json`,
  and it would be echoed on the data-sources row and in the Forget confirm besides. It is asked of
  the host part only, before the path is trimmed, so an `@` inside a *path* is answered by the
  path's own message instead. No provider Strata supports authenticates this way, so nothing is
  lost by refusing it.
- **PG** — `host:port/database`. The port is required and never defaulted to 5432: a Postgres off
  5432 is the ordinary case for a container, a tunnel or a pooler, and a def reading
  `db.internal/analytics` while it means `:5432` shows one thing and connects to another — the
  same argument that keeps S3's region out of `object_store`'s silent default. A scheme is refused
  (the provider supplies it), a second `/` is refused (one database), and userinfo is refused for
  HTTP's reason — the role is its own declared key. The port is the **last** `:`, so an IPv6
  literal reads either way (`::1:5432/db` or `[::1]:5432/db`); the source unwraps the brackets
  before the driver sees the host, which takes the address itself.

  The **role** is checked on the same terms (`settings::check_user`), and for a sharper reason: the
  driver's parameters are interpolated into a data source string with no quoting, so a space or an
  `=` in the user fails as a data source string the parser cannot read rather than as "that user is
  wrong". `CREATE ROLE "read only"` is legal Postgres and simply cannot be dialled through this
  stack, so it is refused by name.

The checks are deliberately not exhaustive — each provider reserves further names no local check
can settle — they catch what is *statically* wrong so the user is told at the field instead of by
a signing error.

**A database's catalog name has a rule of its own**, `check_catalog_name`, called by the engine's
registration and the editor's blocker alike: a bare SQL identifier (leading letter or `_`, then
letters, digits and `_`), not the workspace's own catalog (`strata`), and not another source's
— folded, because unquoted identifiers are. The two callers ask different sets on purpose: the
editor folds against what is *stored*, so it can warn before anything is dialled, and the engine
folds against what is *registered*, because a data source that failed to connect reserves nothing.

## Client options

`client_config` is `object_store`'s own `ClientConfigKey` map, offered on every provider because
all three stores are built on one HTTP client: timeouts, connection pooling, proxy settings, HTTP
version and keep-alive, user agent, certificate trust — 16 keys, enumerated in
`strata_arrow::client::CLIENT_KEYS` with a description each (the editor offers them with
autocomplete).
`check_client_config` validates the map in both the editor and `connect`: an unknown name or a
blank value is refused by name rather than silently dropped at build time.

`allow_http` is deliberately **not** among them, because it is already said elsewhere and a
second control for one setting is two controls that can disagree: on S3 it is the endpoint's own
toggle, and on HTTP it is derived from the scheme the user typed.

## The data source editor

A **child window** of the project window (`crates/strata-freya/src/apps/source/`), one per def.
**It serves every registered kind and it names none of them**: every row comes from what the
chosen kind *declared*, so registering a `DataSource` puts a working editor in front of it with
no UI code. A control that cannot mean anything for the chosen kind is not shipped disabled — it
is not shipped.

1. **PROVIDER** — a segmented pill, one segment per **registered kind**, badged with its own
   `SourceKind::BADGE`. Pressing one carries the kind *and its declaration*
   (`SourceInfo::settings`) onto the draft, so the rows below and the values written cannot
   describe different kinds. A new source opens on the first registrant, because "none chosen" is
   a state only a build with no kinds can reach.
2. **NAME** — the handle: what every surface calls this source *and* the catalog its relations
   are addressed under (`lake` makes `lake.public.orders`). One field, because they are one
   thing, and nothing mints it — the name **is** the identity. **The editor's own row, not a
   declared one**: the store keys by it, `check_catalog` judges it, and a kind has no opinion
   about any of that.
3. **One row per declared `SourceSetting`**, in the order the kind declared them, under whatever
   `group` headings it asked for. The whole row is declared: `label` is the eyebrow, `hint` the ⓘ
   tooltip, `placeholder` the box's ghost text, `required` the REQUIRED marker, `default` what an
   untouched box shows and writes, `when` **whether the row exists at all**, and `field` the dress
   (`Text` a box, `Choice` a `Select` over the kind's own words, `Path` a file field — the path,
   never the contents — `Flag` a switch, `Secret` the secret dress below). So the entire
   `PostgreSQL` form is `sources/postgres/settings.rs`: its address, USER, PASSWORD, SSL MODE and
   ROOT CERTIFICATE, the CONNECTION and SSL sections they sit in, the certificate appearing only
   under `verify-ca`/`verify-full`, and the sentence explaining each. **S3, GCS and HTTP are the
   same** — `sources/store/{s3,gcs,http}.rs`, with `CLIENT_KEYS` folded in as declarations.
4. **READ ONLY** — the editor's own row too, and for the mirror reason: the sentence beside it is
   Strata's policy about Strata's gate, so a kind declaring it would be every kind copy-pasting
   our words. What *is* the kind's is whether it can be written to at all — `SourceKind::WRITABLE`,
   which is what decides the row exists. It replaced `MODE` as the gate, which was only ever a
   proxy: a catalog you can read and not write is an ordinary source.

**The address is an ordinary declared setting** and lands in `config` like any other. There is no
`Slot` and no typed address field: everything about a source is a property, and which property is
the address is the kind's own business — `SourceKind::SCHEME` and its `check_address` are what
read it. So S3 declares `label: "BUCKET"` with its own hint and placeholder, HTTP declares
`label: "URL"`, and the editor gains nothing either way.

**A `Field::Secret` row is the one thing not fully declared**, and deliberately: a secret has
machine-local state no declaration can carry. The box is drawn beside a sentence about *this
machine* (`SecretRow`) and a **Remove from this machine** press. What the declaration does decide
is whether an absence is a demand: a key the kind marks `required: false` reads
`SecretRow::Optional` — "No password is stored on this machine." — and asks for nothing. See
`.agent/tasks/.../EA-29` for what is still open there.

A `When { key, values }` is how a setting says it only means something once another has a
particular answer. A hidden setting **keeps its value** — moving the deciding key back brings the
box back with what was in it, and the def still carries it, because what a mode reads is the
source's business and not a reason to discard a path — and it is **required of nobody**: a
question that is not asked cannot be unanswered.

The conformance body (`sources/fake.rs`) refuses five declarations, because none of them shows up
as a failure anywhere else — the editor simply draws a form missing a setting the source needs, or
draws one twice. A `When` naming a key its source does not declare hides its row **forever** (the
deciding value can never be typed, because there is no box to type it in); a `When` over no values
is the same as never; a duplicate key gives one setting two rows whose values overwrite each
other; no `Slot::Address`, or two, leaves a data source with nowhere to put its address or a second
that silently wins; and a group interrupted by another group's key prints its heading twice. It
also checks `WRITABLE` against what the connected handle implements, **in both directions** — a
source that claims it cannot be written to and quietly has a writer hides a control that works,
and one that claims it can and has none offers a control that cannot.

A form ends with its last row: **there is no standing note**. A paragraph about where a kind keeps
its credentials is prose only that kind can write, and the editor writing it generically produced
a sentence true of every source and useful about none. What each secret box does with what is
typed into it is the row's own note, which is specific because it reports this machine.

There is no SCHEMAS row: schema enablement is a tree-node gesture (DB-05), where the live
enumeration already sits and where one picker serves New and Edit alike — a second surface for the
same list is two controls that can disagree, and the editor is pre-connect, so it has nothing to
enumerate from.

Last comes a standing note saying where credentials actually come from — per provider, and for a
source off its **declaration**: whether the kind takes any `Field::Secret` at all, because no
sentence here may name a setting only one kind has.

**What the form can be wrong about is the handle, and a shown `required` key being empty.** What a
*value* may be is the kind's own rule, asked by `connect`, whose refusal lands on the data source's
row. A field's error lives in the **footer**, not on the field: one value both disables Save and
explains it, so the form cannot hold two accounts of its own validity. Three of those the draft
cannot answer alone — the kind's address rule (asked of the registry), a name another source
holds, and a catalog name another database source holds (`check_catalog_name`).

**A `Field::Secret` row reports this machine, not the def.** The settings window's API-key marker
is honest because it minted the reference when it stored one; a def carries only the
*expectation* (`SourceDef::secrets`), so each expected key probes the local keystore once at
mount and shows one of: none expected, stored on this machine, expected but not stored here, still
asking, or the keystore's own refusal. Every sentence names the key off its declared label, so a
source with two credentials has two rows that read differently. The two clearing gestures are
deliberately separate presses — *remove from this machine* deletes that one local entry and leaves
the def's expectation standing, while *this data source uses no …* edits the shared def to expect
none. Conflating them means one person casually breaking every colleague who has one. There is no
mode pill: a secret is optional wherever its key says so, so absence is a state rather than a mode.

Save writes the def through the store's own funnel, persists the project, **deregisters what the
old name registered** when the edit moved it (nothing downstream ever sees the def it replaced),
and asks for a whole-catalog registration pass; the window then watches its own row and closes
when the source settles. A **rename** goes through `rename_source`, which moves the tables reading
through it in the same settle. A source with secrets has one step in front of all of that:
whatever this machine's keystore owes, on a worker, so a keystore that refuses writes nothing — a
put or a delete **per declared key**, addressed by the slot the def records
(`put_secret_at`). A **rename plans no keystore work at all**, and neither does a change of kind:
the slot travels in the def. A key the save no longer expects is cleared through the slot the
**previous** def named, which is the only name that entry ever had.

An **address** move with an unchanged name needs nothing further of Save's: connecting replaces on
re-connect, and the whole-catalog pass is what re-connects.

## The data-sources tree

One sidebar pane (DB-05), reached from the activity rail (clicking it again collapses the
sidebar). It answers "what data do I have" for the project's catalog and its data sources
together — the separate Data sources pane it replaced is gone, and so is the rail toggle beside
it.

Top level is **data sources**:

- the **project workspace**, first and open by default, labelled with the project's own name and
  addressed as `strata`. It is not a "files provider": it is the catalog Strata's federating
  engine defines, so file tables, internal tables, views, saved queries — and a **cross-source**
  view joining workspace files onto `pg.…` — all nest under it. Its children are the flat pane's
  TABLES · VIEWS · QUERIES groups verbatim: same rows, same status slots, same menus, same
  expansion to columns, and the TABLES `+` still opens Configure on a new table;
- one node per **database source**, opening onto its enabled schemas, then Tables and Views
  groups split by the listing's own `relkind`, then its relations. All of it is
  `Sources::listing`'s scoped-and-tagged answer — read from the connect-time enumeration, not
  the network — so collapse and re-open cost nothing and ↻, which re-connects, is the refresh. A
  schema the def enables and the server does not have renders as its own failed node naming that
  fact. A relation **opens onto its columns** (DB-07), and it is the one node in the tree whose
  children are not free: they are a round trip through the provider the data source caches per
  relation, so an open relation shows a *Reading columns…* note until the read lands, or the
  reason it did not. Selecting a column points the inspector at it exactly as a workspace column
  does. Because the walk that builds the tree is synchronous — and is the only place the tree's
  shape is decided — it *returns* the relations it drew open and the **pane** holds the one
  subscription for their columns, keyed by those relations and the catalog generation; a row cannot
  hold it, since a virtualized row's scope is a slot. What a relation row *does* is
  [below](#gestures-on-a-remote-relation);
- one node per **object-store source**, opening onto the workspace defs that read through it
  as **links** — pressing one opens the def's ancestors and brings its own row into view, rather
  than offering a second editable copy of it.

The two halves are **joined once, before the walk** (`state::sources::assemble`): the project's
source rows, the engine's one snapshot and the workspace defs that read through each bucket
become a `SourceNode` per source, and the walk below it only decides shape. That is what keeps
the walk a plain function of its inputs — it reaches no engine at all — where it used to make a
listing call and a registry call per row on every filter keystroke, each answering as of its own
moment.

Every source node carries a **provider badge** (the registered kind's own, from the snapshot —
`S3` / `GCS` / `HTTP` / `PG`), its address, a **status glyph** (nothing when connected, a spinner once the wait
outlasts the progress hold, or a warning triangle carrying the engine's own refusal, clipped to
what a tooltip holds and naming Problems for the rest), and a trailing **⋮** menu (also
right-click): **Edit**, **Schemas…** on a database, **Forget**. Pressing the row opens it; its
actions are the menu.

**Schemas…** is a picker over the same `Sources::listing` answer, so the tree, the picker and
completion cannot disagree about what a data source shows — and it is the one surface that sees a
schema the data source does *not* show, taking one back being what it is for. Its write is display-only, so it edits
the def **in place** (`ProjectState::update_source_def`) and asks the engine nothing — going
through `upsert_source` would move the def's identity under a registration that had not been
asked for again. A data source that is not live has no enumeration to offer: the picker then
lists the def's own schemas and says so.

The header's `+`, the empty state's row and a node's Edit all open the editor window. **Forget's
consequence differs by kind**, and the confirm carries which kind it is rather than looking it
up — that decides the *sentence*. What it counts is one engine read, `Sources::dependents(name)`
(EA-18): an object store's readers are the tables whose defs name it and the views behind those; a
database's are the views whose plans scan through its catalog, since no def can name a database.
Both are what registration established — a table's def named its source, a view's plan named
what it scanned — so the engine answers rather than two surfaces re-deriving it, and the answer is
bounded by the last pass: a def nothing has registered yet is not counted, and a view the engine
could not create recorded no plan to have read anything with. Confirming removes the def,
deregisters the store and its catalog, and deletes the keystore entries the
def expected.

One filter spans the tree. A node survives it if its own name matches or any descendant's does,
and a node kept by a descendant's match opens itself, since keeping the container and then hiding
what saved it is worse than not keeping it; the workspace's three groups are the stated
exception, staying put with their counts following the filter, because a count of `0` is what
says the filter found nothing there.

The pane **walks the tree into a flat list of visible rows and virtualizes it** (the fork's
`Tree`), so only the rows on screen are built. That is not an optimisation of the workspace half,
whose row count is bounded by the project file: it is what makes a database node safe to open,
since `RELATIONS_QUERY` carries no `LIMIT` and one schema's relation list is the server's to
decide. A jump from an object-store link is therefore answered by the target's **index** in that
list rather than by its measured rectangle, because the row it names has usually not been built.

### Gestures on a remote relation

Three. The two that compose a statement do so **into a new unrun tab** (DB-06) — the tree is where
work starts, not where it runs; a full read of a remote table is not something to begin by pointing
at a row:

- **Query table / Query view** — `SELECT * FROM <catalog>.<schema>.<relation>` at the row-limit
  setting, on the relation's double-press and on its ⋮. It is the workspace rows' *View table*
  over a three-part name: one funnel (`select_sql`), so the two cannot disagree about the shape or
  the `LIMIT`. A single press still does nothing, and that survived the row gaining children
  (DB-07): the **chevron** opens its columns, which is the *column* row's own arrangement one level
  up rather than a new one.
- **Pin as view…** — `CREATE VIEW <relation> AS SELECT * FROM <catalog>.<schema>.<relation>`, for
  the user to rename and run. This is the workstream's "make it a bare-named def" gesture, and
  composing rather than executing is the point: the name is a guess (frequently the wrong one in a
  workspace that already has an `orders`), and running the statement lands the def through the
  view funnel that already exists. A gesture that created the view itself would have had to invent
  a name, or refuse.

- **Profile table / Profile view** (DB-07) — the same `ProfileActions::ask` every other profile
  gesture goes through, so a first scan raises the cost confirm and a re-scan does not. It is the
  one gesture here that does not compose a statement, because it is not a place to start work: it
  *is* the work, which is why it is the one with a confirm in front of it. The confirm says the
  scan runs on the server, and the numbers it promises are the ones a federated scan can actually
  compute — see [profiling a remote relation](#profiling-a-remote-relation).

The menu carries these three and nothing else. Everything the workspace rows offer beyond them is
about a **def** — Configure edits one, Drop removes one, Refresh re-infers one — and a remote
relation has none; the data source's own row is where its lifecycle lives.

The two names in that `CREATE VIEW` are rendered by **two different renderers**, because they
belong to two different owners: the relation's address goes through
`sql::qualified`/`quote_verbatim`, which preserves the server's spelling segment by segment, and
the view's name through `engine::quote_ident`, which folds — that being the identity the workspace
store will key the def under. `docs/COMPLETION_SPEC.md` §6 states the pair.

## Profiling a remote relation

The column inspector treats a relation inside a database source's catalog exactly as it treats
a workspace table (DB-07): a title, its type, a **source badge** in the source-format slot, and
a STATISTICS zone offering one opt-in full scan through the same `ProfileActions::ask` every other
profile gesture uses. Two things differ, and both are consequences of *where* the work happens.

**The free tier is the schema and nothing else.** There is no footer to read and no file listing to
count, so a remote column shows its type and — until a scan runs — no row count and no completeness
bar. `pg_class.reltuples` is deliberately **not** shown: it is an estimate, its only home would be
the ROWS row, and the completeness bar *divides by* that row count. An estimated denominator under
an exact null count is the "two reads pretending to be one" this panel refuses everywhere else, so
the estimate is refused rather than dressed up. A scan answers both for real.

**The scan runs one statement on the server, so its expression set is its own.** The aggregate
federates whole — `datafusion-federation` sweeps it into a single remote statement or none, with no
per-expression fallback — so every expression in it has to be one the unparser renders *and*
PostgreSQL has. Count, distinct count, min, max and mean all are. The **median** is not:
DataFusion's is `approx_percentile_cont`, a DF-only aggregate, and DataFusion 54's
`PostgreSqlDialect` exposes scalar-function overrides only, so there is nowhere to teach it a
spelling. Leaving it in would not cost a median — it would fail the scan of every remote table with
a numeric column in it. It is therefore **dropped and stated**: the zone carries a footnote saying
medians are not computed on a database, and the cost confirm does not promise one.
`percentile_cont(0.5) WITHIN GROUP (ORDER BY …)` is not substituted for it — that is an ordered-set
aggregate the unparser has no expression to emit, so it would be an assumption rather than a fix.

`engine::profile`'s `Profiled` is the one value that decides this, and it decides the **`FROM`
renderer** with it, because both turn on the same fact: a workspace name renders through the
fold-preserving `engine::quote_ident`, a remote one segment-by-segment through `sql::qualified`, so
"view as query" hands over a statement that runs. Both halves are pinned — `engine::profile`'s unit
tests render every expression through DataFusion's own PostgreSQL dialect (no container needed), and
`tests/postgres_federation.rs` pins that the aggregate federates into one node, that the server runs
it, and that the *unsplit* set does not.

The one structural difference is where the request lives. A remote relation has no `ProjectState`
row, so its `ScanId` goes in a window-side satellite keyed by the relation, never a row minted into
the store; it is dropped when its source is no longer connected, which covers both a Forget and
a ↻ without either being noticed specially.

## Completion over a data source

The editor offers a database's names as you type them (DB-06): a data source's **catalog name** at
any relation-target position, its **enabled schemas** after `catalog.`, and its **relations** after
`catalog.schema.`. The catalog name comes from the def, so a data source that has never answered
still offers the name a query has to say; the schemas and relations come from the same
`Sources::listing` snapshot — the scoped-and-tagged answer the tree and the Schemas… picker read. A non-enabled schema is
absent from the offer and still resolves if typed — visibility, not policy. Nothing on the
completion path touches the network. The full rules, including where it deliberately stops (a
remote relation's columns), are `docs/COMPLETION_SPEC.md` §2, §4 and §10.

## Store catalogs

**An object-store source registers two things, and they are one act**: the object store, under
the URL its kind's `SCHEME` and address compose, and a `StoreCatalogProvider` under the source's
own name. One arm of `connect::Registration`, because a source that put its store on the session
and not its catalog would resolve every path and hold no table. The object store still goes on
**first**: the catalog's tables read through it.

The catalog has one schema, `public`, in the workspace catalog's own shape, and it **enumerates
nothing** — a bucket cannot say what its tables are. What it holds is exactly what the project's
own table defs put there: `register_external` places a table's provider in the catalog its def
names (`providers::def_home`) instead of the workspace, building the same `ListingTable` it always
built. Catalog-is-the-store is intact; the defs answer for membership, not the bucket.

That is what makes **Forget structural**: `Sources::disconnect` deregisters the catalog and its
tables stop resolving with it, rather than a deregistration per table that a failure could
half-finish.

**Placement, not a namespace.** Table names stay unique across the whole project, so
`lake.public.regions` and a bare `regions` are one row reached two ways:

- The **qualified** address always works. `sql::qualify` already searched every registered
  catalog, so a bare name resolves across them with no change there.
- **Programmatic** lookups do not parse SQL, so they go through `providers::def_ref`, which
  resolves a project name to wherever its def was placed. Without it, `ctx.table("regions")` —
  bare resolution being the *default* catalog's — would simply not find a bucket table.
- `resolve_target` answers `Target::Store`, and every arm that manages a target says what it does
  about one. `INSERT`'s internal gate refuses exactly as before; `DROP TABLE` drops the def;
  `Target::workspace` refuses a store catalog in its own words (`in_store`), because a bucket does
  not "describe its own relations" the way a server does and the fix is a LOCATION rather than a
  server.
- `in_workspace` is **untouched**, so the `__snap_` namespace is exactly as wide as it was. The
  *checkability* split is a different predicate: `providers::def_backed` is what `plan_deps` uses,
  so a view over a bucket table records it as a workspace name and is still named when that table
  drops.

## Registration order

Data sources are the **first phase** of the project registration pass
(`strata_engine::register::sync`): every source registers before any table, because a table's
path cannot resolve to an object store that is not registered yet — an ordering bug there would
look exactly like a broken table — because a table's provider is placed in the catalog its source
registered, and because a view over `pg.public.orders` cannot plan before that catalog exists. A
whole-catalog ↻ re-connects everything; a single table's Refresh does not re-connect anything.

## Database sources

A **PG** source registers a DataFusion **catalog** of relations the server enumerates, so the
editor
can `SELECT … FROM pg.public.orders JOIN events …` — cross-joining file-based tables onto live
PostgreSQL, with filters, projections and whole same-source subplans pushed down to the server.
Built on `datafusion-table-providers-postgres` and `datafusion-federation`, both pinned in
lockstep with the `datafusion` version: `TableProvider`, the unparser and the federation plan
node are all DataFusion types, so app and providers must resolve **one** DataFusion and a bump
moves all four pins together.

**The whole database comes through, and nothing is declared per table.** Connect enumerates every
schema the role can see and every relation in them — one round trip against `pg_class`, filtered
to `relkind IN ('r','p','v','m','f')` and to what the role may `USAGE`/`SELECT`, system schemas
excluded — and registers a catalog whose table providers are built lazily on first use and then
cached. There are no per-table defs and no manual adds. The line is *discovery gets catalogs,
declaration gets defs*: a bucket cannot say what its tables are — someone must declare globs, a
format and its options, and that declaration can fail, which is what a table row's status exists
to show — while a database answers for itself. Pinning one remote relation into the workspace is a
**view** (`CREATE VIEW orders AS SELECT * FROM pg.public.orders`), which needs no new machinery.

`pg_class` rather than the provider crate's own `pg_tables` listing: remote views, materialized
views, partitioned tables and foreign tables must show and resolve, or the catalog lies about what
is queryable. That is one of three reasons the catalog/schema provider is ours rather than the
crate's `DatabaseCatalogProvider`; the others are that it snapshots the listing at construction (a
↻ could not refresh it) and that it skips the federation wrapper, silently forfeiting the pushdown
this exists for.

A relation's provider is built **one level below** the crate's `PostgresTableFactory`
(`engine/sources/postgres/mod.rs`, over the assembly in `engine/sources/sql.rs`), which is that
factory's three steps written out — the `SqlTable`, the data source's unparser dialect, the
federation wrapper — plus an executor of ours around the crate's. The executor is what recodes the
error coming back and stamps the data source's identity as federation's fusion key, so two
data sources to one server can never fuse into one statement. What the statement *says* is the
dialect's, not the executor's: it is handed to the `SqlTable` and to the wrapper alike, so the
federated statement and the fallback provider's own scan speak one data source's SQL. The schema
provider is still the one construction site, and the wrapper and the per-relation cache are
unchanged by the move.

**The def:**

| Field | What it is |
|---|---|
| `catalog` | How queries address the database — the catalog half of `catalog.schema.table`. |
| `user` | The role, and half the data source's identity. |
| `sslmode` | libpq's own vocabulary, in libpq's spellings. Defaults to `prefer`, as libpq does. |
| `sslrootcert` | A root-certificate **file path**, read only by the two verifying modes. |
| `password` | `none` or `keystore` — the **expectation**, never a reference. |
| `schemas` | The schemas this data source *shows*. Defaults to `["public"]`. |
| `read_only` | Whether Strata refuses to change this database. Defaults to `true`. |

**The password.** This is where W7's "Strata never stores, prompts for or reads a secret" was
deliberately **rewritten** rather than routed around: that rule was a consequence of the OS
keystore not existing when W7 was built, and of object stores happening to have host-side
credential chains where a database does not. The password is captured exactly as an assistant
provider key is (`strata_core::secret`) and read **per pool connection**, never cached.

The reference is **recorded in the def and written once** — `SourceDef::secrets` is
`{key: SecretRef}`, minted by `secret_slot_or_mint` the first time a secret is filed. One slot per
**key**, so a source declaring two credentials keeps them in two.

It used to be *derived*: `SecretRef::derived("{kind}-{key}", def.named())`, a `Uuid::new_v5` over
a fixed namespace. That derived the slot from two things the user can edit, addressing an entry
that lives on a machine no edit can reach. **Renaming a data source moved the slot for everyone**,
while only the machine doing the renaming could move its own keystore entry to follow: every
colleague who pulled the rename was left with a password under an id nothing would ever name
again. `migrate_secrets` compensated locally and could not possibly compensate there; a change of
kind stranded one even locally, since that hook compared names. And every symptom was
indistinguishable from never having entered a password, so the form could not state the
difference. A recorded ref survives both, and `migrate_secrets` is **gone** rather than fixed.

This does not reintroduce what derivation was for. The objection was to a *minted* ref in a
committed file being rewritten by every colleague who entered their own password — two machines
ping-ponging one id through git. A ref written **once** is never rewritten: a colleague entering
their own password writes their own keystore entry under the id already in the file.

`SecretRef` therefore lives in **`strata-model`** — a def carries one, and a def crate reaches no
platform — while every keystore operation stays in `strata_core::secret` behind the `Keystore`
extension trait. A def written before the slot was recorded still reads: `Secrets` is untagged
over the recorded map and the old key list, `SourceDef::secret_slot` resolves either, and
`load_defs` adopts **once** — minting a ref per key, moving whatever this machine holds under the
old derived slot onto it, and rewriting the file. Best-effort about the keystore on purpose: a
colleague with no entry must still get the recorded ref or it would adopt on every open, and a
locked keychain must not stop a project opening.

**The engine owns the write.** `sources::{put_secret, put_secret_at, forget_secret,
forget_secrets}` are where a secret is stored and dropped; no surface composes a slot. A value may
also arrive from the kind's own environment convention (`PGPASSWORD` for
`PostgreSQL`), declared in its `SecretRequest` and asked through `ChainSecrets`; the app asks the
keystore first, a headless tool the environment alone, and a miss names **both** fixes.

The consequences are carried honestly — a Forget deletes the entries without needing a stored ref,
and on a machine with no entry the row settles failed naming the fix ("No password is stored on this machine for
'…'"), the same shape as an expired SSO session.

**Connecting is the probe, with nothing extra.** Building the pool resolves the host, opens a TCP
connection, authenticates, builds the pool and runs `SELECT 1`; any of them failing is the whole
answer. There is no separate reachability step, because unlike a bucket — whose description can be
well-formed and wrong in a way only the bucket knows — a database either let us in or did not. A
reconnect **replaces**: whatever that URL last registered comes out, under the name it went in
under, so the editor's rename (same address, new name) is handled by the registration rather
than by a surface remembering.

**Schema visibility scopes display and the *implicit* search, never the resolution of a name the
user wrote.** `schemas` is DataGrip's "N of M schemas" choice. The engine registers every schema
regardless — the providers are lazy, so that costs nothing — which means a query naming a schema
that is not enabled still resolves and runs. What it *does* bound, since DB-09, is where an
**unqualified** name is looked for (see *Unqualified names* above): a schema you switched off
neither captures a bare name nor collides with one in a schema you left on.
`Sources::listing` is the one read every surface shares, and it answers **scoped and tagged**
(`Live | EnabledButMissing | NotEnabled`), so nothing re-derives visibility. It reads the
connect-time enumeration, which is why a ↻ *is* the refresh.

**It is one read of everything, not a read per source** (EA-18). `listing()` takes no argument
and answers a `SourcesSnapshot { generation, sources }` covering every data source the engine has
been told about — live or not, so a refused one still has a row to hang its failure on — each
carrying its kind's badge, whether it is registered right now, and what it registered: an object
store, or a catalog with its scoped schemas. The tree, the Schemas… picker, completion's
`database_syms` and the agent's `list_tables` all read that one value, so no two of them can be
answering as of different moments, and none of them asks the registry a second question to badge a
row. The two name reads it derives are deliberately different questions: `database_syms` offers
the catalog of a data source that has never answered, because that is the name a query would have
to write, while `catalog_names` — what an agent is told the databases are — lists only what can be
reached into now.

**What is pushed down, so nobody re-measures it.** A single-table filter, projection and `LIMIT`
push down even without federation (the scan unparses them; anything unsupported falls back and
re-applies locally). A same-source join, aggregate or TopK federates into **one** remote
statement. A pg × parquet join is ambiguous at the join node: the largest single-provider subtree
under the pg side still federates, and the join itself runs locally. A federated subplan that
unparses to SQL the server rejects fails **loudly at execute time** — there is no silent local
fallback, and the results pane's error path is the surface. What that failure *says*, when the
cause is a name only DataFusion has, is the JSON paragraph below.

`jsonb` and other exotic types arrive as `Utf8` JSON text (`UnsupportedTypeAction::String`), which
the app's own Postgres-style accessors already read. The crate's default would refuse the whole
relation for one such column. This is representation honesty rather than silent corruption: the
value is intact, only the type is wider.

**JSON accessors are written as PostgreSQL's own operators, and an accessor that has no faithful
spelling refuses by name** (`engine/sources/postgres/json.rs` — the family table and the refusals;
`dialect.rs` — the data source's unparser dialect, whose
`Dialect::scalar_function_to_sql_overrides` is where a call becomes an operator expression).
`payload ->> 'type'` is planned as a `datafusion-functions-json` UDF call, and a UDF call unparses
*by name* — so without this a federated subplan would carry `json_as_text(payload, 'type')` to a
server that has no such function, and federation has no per-expression fallback to catch it. What
pushes down:

| Typed | Planned as | Sent as |
|---|---|---|
| `x ->> 'k'`, `x -> 'a' ->> 'k'` | `json_as_text(x, …)` | `(x ->> 'k')`, `((x -> 'a') ->> 'k')` |
| `x ? 'k'` | `json_contains(x, …)` | `((x -> 'k') IS NOT NULL)` |

An accessor compared against something needs **parentheses**: `WHERE (payload ->> 'type') =
'click'`, not `WHERE payload ->> 'type' = 'click'`. sqlparser gives every Postgres-style operator
`PgOther` precedence, which is looser than `Eq`, so the bare form binds as
`payload ->> ('type' = 'click')` and fails planning with "expected string or int, got Boolean".
That is the parser's, not federation's — it reads the same way over a local JSON column.

Everything else in the family — `json_get` (bare `->`), `json_get_str`, `json_get_json`,
`json_length`, `json_object_keys` and the typed getters — is **unmapped on purpose**, because each
would answer differently on the server than it does here: `->` returns Arrow's JSON union, which no
PostgreSQL expression produces; `->>` stringifies an object where `json_get_str` is NULL; `->`
hands back normalised `jsonb` where `json_get_json` hands back the source slice. A mapping that was
close enough would make a query's answer depend on where it ran, so an unmapped member is a
refusal naming the function, the data source and the way out (copy the rows in with `CREATE TABLE …
AS SELECT …`); `->`'s refusal also names `->>`, and a mapped accessor called with no key to look
up says *that* rather than that the accessor is unsupported. The same sentence wraps the failures
only the server can raise — a created SQL macro that survived `simplify`, an accessor over a column
that is `text` rather than `json` — recognised by the `SQLSTATE: 42883` the provider crate renders
rather than by PostgreSQL's prose, which has at least three wordings for it. Those keep the
server's own words and add ours on a line of their own. **`json_contains` is not `?`**: Postgres's `?` is also true for a string *array element* and
takes no integer index, where the local function is false for both, so the faithful spelling is the
arrow chain. None of this is reachable from a local JSON column: the dialect belongs to one
source and is reached only when that source's SQL is being written.

Two things read differently in a plan because the spelling is decided at the unparser rather than
after it. A federated node's `base_sql=` already carries the operator, and there is no
`rewritten_executor_sql=` beside it. And a **refused** statement has no `base_sql=` at all —
federation prints that field only when the plan unparses — so an `EXPLAIN` over a query the family
refuses shows the node without its statement; running it is what shows the sentence. A refused
accessor in a `WHERE` also stops claiming pushdown: the fallback provider asks the dialect whether
a filter can be written down, so a filter it cannot write is one DataFusion keeps and evaluates
locally, where before it was written into the scan's SQL and refused by the server.

### Writing into a database (DB-10)

`INSERT INTO pg.public.events SELECT … FROM local_parquet` loads a local result into PostgreSQL,
and `CREATE TABLE pg.public.report AS SELECT …` materializes any result — local, remote, or a
cross-source join — as a real server table. Those two are the statements **DataFusion can plan**
against a remote catalog, so they are planned here and driven; the ones only the server can run are
dispatched as text (below).

**Off unless the data source says otherwise.** `read_only` defaults to `true`, so a stored def that
predates the field — and every data source nobody has opted in — refuses both statements by name,
pointing at the one setting that would allow them ("Turn off 'Read only' in the data source's
settings"). The gate is the **def** rather than a machine-local preference because a data source is
committed and shared: a colleague pulling the project gets the same answer about the same server.
The editor's row is **READ ONLY**, a switch that starts on.

**A write target resolves exactly as a read does.** `INSERT INTO orders` reaches the relation
`SELECT * FROM orders` reads (DB-09's resolution, with the write carve-out removed). Three things
make that safe with no second gate: the data source was opted in, an ambiguous name still refuses by
name so a write never picks between two servers, and the arm is reached with a qualified name — so
one funnel answers whether or not the qualifier was typed. A **create** target is still never
resolved, permanently; `CREATE TABLE pg.public.report AS …` is how the server is addressed.

**The write provider is resolved by the arm and lives nowhere.** `PostgresTableWriter` *wraps* the
federated read provider, so the node a plan sees would be the writer rather than the
`FederatedTableProviderAdaptor` the federation rule downcasts to. Serving writers from the schema
provider would silently forfeit pushdown on **every read** — exactly the failure the
own-provider decision exists to prevent — so the catalog goes on serving read providers, and a
write statement builds a writer over the one it resolved, drives the sink once and drops it. The
drive itself is `sink::append_rows` (the workspace `INSERT` shared it from DB-12 until the EA-08
seam gave that arm its own writer, `InternalTableStore::append` over `sink::insert_stream` — the
same input handling, minus the provider sink). The input plan
is coalesced to a single partition first, because `DataSinkExec` reads partition 0 and nothing
else, and its redundant projection is collapsed first (`sink::collapse_projections`): DataFusion's
`INSERT` planner leaves a renaming projection over the query's own, and the unparser renders that
pair as a derived table while leaving the outer column references carrying the scan's qualifier — so
a remote source would come back as `missing FROM-clause entry`. It has to happen before the
federation rule runs, which is why it is not the executor's `logical_optimizer` hook.

**What each statement does.** An `INSERT` runs in one transaction on one pooled connection, so an
interrupted one rolls back and nothing half-lands; it changes no listing, so it carries no store
effect. A CTAS creates the server table from its input's Arrow schema (`CreateTableBuilder`, under
a `SET LOCAL search_path` of exactly the target schema, since the builder renders an unqualified
name), re-enumerates the database, fills it, and on a failed fill drops the table again and
re-enumerates — never a schema-only husk under a name the user thinks holds data. **A cancel is
the other way out**, and it reaches no error path at all: the guard `write::Created` removes the
table when the future is dropped, which is `arms::tables::Staging`'s rule for the local half.
Whether the relation already exists is asked **inside the create's own transaction** rather than
by a round trip before it, because `CreateTableBuilder` hardcodes `IF NOT EXISTS`: a relation that
appeared in between would be silently adopted and then dropped by the rollback. The successful
CTAS carries `StoreEffect::RemoteRelationsChanged`, whose only job is the catalog generation: a
remote relation has no store row, and the tree, completion and every tab's diagnostics already key
on that number, so the new table shows with no manual ↻.

**Still refused**, each for its own reason: `INSERT OVERWRITE` (a statement that silently empties a
server table is not v1) and `CREATE OR REPLACE TABLE` over a relation that **exists** (it would
drop one; over a free name it simply creates, as the local arm does). A read-only capability is
unchanged: the agent surface refuses both write statements as it always did.

### Statements the server runs (DB-11)

`CREATE VIEW pg.public.active AS …`, `CREATE MATERIALIZED VIEW`, `DROP VIEW`, `DROP TABLE`, a
column-list `CREATE TABLE pg.public.t (payload jsonb, …)` with the server's own types, and
`UPDATE` / `DELETE` with the server's own affected-row count. DataFusion can plan none of them
against a remote catalog, so the mechanism is **dispatch**: the statement the user typed, with the
catalog qualifier spliced out by span, sent over the data source's own pool. The read-only toggle
gates them exactly as it gates the two above, and the full design — the splice, the body check, the
refusals and what each arm reports — is `docs/STATEMENTS_SPEC.md` §6.9.

Two consequences worth knowing at the data source level. A statement that changed what the server
holds **re-enumerates the data source**, so a new view is in the tree with no ↻ and a dropped
relation loses its cached provider rather than going on answering scans. And a body may only name
relations of the data source it runs on: a server-side view cannot read across sources, so a
workspace table, another source's relation, or a name left unqualified is refused **by name**
before anything is sent.

**Still refused**, and this is where the note lives: `ALTER`, `TRUNCATE` and `MERGE` stay
default-deny. `TRUNCATE` is a `WHERE`-less `DELETE` with nothing new to say, `ALTER` is a large
surface with its own listing-refresh questions, and the splice generalizes to either if it is ever
asked for. Registering a table externally over a remote relation is refused for a different
reason — a database describes its own relations — and that is the one sentence
`statements::target::in_database` still carries.

### Unqualified names (DB-09)

`SELECT * FROM orders` reaches a connected database's `orders`. The three-part name is what you
type to reach *across* sources, not what you type to work in one.

There is no current database and no `USE`: **a bare name is resolved against everything
connected, once, before the statement is planned** (`sql::qualify`, run from `sql::parse_one` —
the one parse in front of both the router and the planner).

1. The workspace wins. Its schema holds its tables, its views and the result spool, so nothing
   that resolves today changes meaning, and `__snap_` names stay inside the fence that reserves
   them.
2. Exactly one relation of that name across the connected catalogs: the statement is rewritten to
   the three-part name, every part quoted, in the spellings that reach it — the catalog as the
   source registered it, the schema and the relation as the server spells them. Views and
   materialized views included; the search asks the providers, and the listing is
   `relkind IN ('r','p','v','m','f')`. **The search runs in the schemas each data source shows**
   (`SourceDef::schemas`) — see below.
3. More than one: refused, naming every candidate — `'orders' is ambiguous: 'pg.public.orders',
   'pg.analytics.orders'. Qualify it`. Never a coin flip between two servers.
4. None: left bare, which is the error DataFusion already gives.

**Resolvable positions: everything but a create target.** A CTAS body, an `INSERT`'s source query
and its **target**, a `DROP`'s target, an `UPDATE`/`DELETE`'s target and clauses, a view's body and
a `COPY`'s source all resolve. A CTE name and a registered table function are held back. The one
carve-out:

- **A create target is never resolved.** `CREATE TABLE orders` makes a workspace table while a
  source has an `orders` — the name does not exist yet, so there is nothing to resolve *to*,
  and resolving would read a plainly local intent as "make it on the server", which then fails as
  already existing. Permanent, and the same for `CREATE VIEW`.

A **write** target was once read-but-not-rewritten, refused in the data source's own words rather
than as a name that does not exist. That was what the rule looked like while writing to a database
was impossible at all; DB-10 turned it into a rewrite, so `INSERT INTO orders` now dispatches to
`pg.public.orders` the way `SELECT * FROM orders` reads it, and what refuses a write is the arm.
DB-11 extended the same rule to every target that addresses a relation which already exists — a
`DROP`'s, an `UPDATE`'s, a `DELETE`'s — for the same reason and with the same three safeguards.

**Why the statement and not the session.** DataFusion has one default catalog and one default
schema and no search path, so the other design is to *move* the default. `providers::in_workspace`
answers `true` for every bare name, and four rules turn on that answer: the `__snap_` fence, what
a write may target, and the two halves of a view's recorded dependencies. A moved default makes
all four wrong at once — most sharply the last, where a view whose body says `orders` would be
recorded as reading a *workspace* table it never read, so dropping an unrelated table names it,
its missing-dependency check cries wolf over a relation no def exists for, and forgetting the
source matches nothing. Resolving on the statement leaves all four untouched: the plan carries
the name the read reached, so `PlanDeps` records `pg.public.orders` in its remote half for free.
There is no mode, nothing to display and nothing a restart has to clear.

**The implicit search is scoped to the schemas a data source shows, and only the implicit one.**
`SourceDef::schemas` bounds what an *unqualified* name searches; a name written in full still
resolves into any schema the role can see, which is what "display, never resolution" was always
about. Both halves are one rule: the tree is the statement of what you are working with, so a
schema switched off cannot capture a bare name, and — the case that made this obvious — cannot
collide with a relation in a schema you left on. Without the scoping, `sessions` in a hidden
`analytics` refuses a query about the `sessions` the tree is showing, naming a schema the user
cannot see. The set is one live cell shared between the data source and its catalog provider
(`db::Shown`), written by `connect` and by the Schemas… picker through `Sources::show_schemas` —
the picker does not reconnect, so a copy taken at connect would be stale by the time it was read.

What a bare name means can still change — creating a workspace `orders` takes the name back, and
the same query then reads the project's own table. Completion is the answer to that: at a relation
position it offers each data source's relations too, and **inserts the spelling that resolves** —
bare where the name is unambiguous, three-part where the project's own catalog or a second shown
schema holds it (`complete::bare_relation_item`). The offer is ranked below the project's own
tables and views, which is the precedence rule written into the ranking.

Verified end to end against a real PostgreSQL in
`crates/strata-engine/tests/postgres_federation.rs` — including a bare read of a remote view, a
bare read into a schema the data source does not display, the ambiguity refusal, the refused write,
and the dependency assertion that is the whole risk.

## Tables over a data source

`TableDef::source` holds the chosen data source's **name** — a *reference*, never a copy of the
bucket, provider or auth — and it is the one field that says a table is remote. Exactly when it
is set, the table's sources are **bucket-relative** (`events/2024/**/*.parquet`), stored as
typed. A def written when the field held a URL or an identity migrates on read
(`TableDef::migrated`), because both older spellings carry the address a name is minted from.

Composing the two halves is the **engine's**: `register::table_spec` turns the name into the
address that source's store is registered under and hands
`strata_core::project::resolve_source` the result, so a bucket-relative source can never be
silently resolved against the local disk — and no surface keeps a second copy of the scheme. The
same rule is why `Catalog::detect_partitions` takes a data source name and def-relative paths rather
than composed ones.

In the Configure window, **LOCATION** is an explicit Local / Remote toggle — never inferred from
a path's scheme. Remote mode shows a single bucket-relative SOURCE PATH (rendered with the
non-editable bucket prefix), a TYPE segmented control that *filters* a SOURCE dropdown (a
filter, never the table's provider), and a **New data source…** entry that opens the editor. A def
naming a data source the project no longer has keeps naming it, and Save is blocked with:

> 's3://gone' is not a data source in this project. Choose one, or add it back.

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

lands `source: Some("acme_lake")` and the bucket-relative source `events/2024/` —
`project::split_remote`, which is `resolve_source` read backwards and asserted to round-trip. The
URL has to be a data source **this project has**, and is refused by name otherwise, on the terms the
Configure footer is blocked on:

> 's3://acme-lake' is not a data source in this project. Add it in Data sources

A statement cannot mint one. A data source carries a provider, a region and where its credentials
come from — none of which the statement says, and one of which it must never carry — and it also
carries a *status*, which comes from a probe rather than from a sentence. Refusing here is also
what keeps DataFusion's "No suitable object store found" off a table row, which is the whole point
of registering data sources first.

Membership is the engine's `Data sources` map: the **defs** `connect` was handed, keyed by name and
noted **whatever the outcome**, removed by `disconnect`. A data source whose region is blank or
whose SSO session expired is still one the user may point a table at — the fix comes afterwards —
so asking DataFusion's object-store registry instead would have answered *no* for exactly the rows
the user is on their way to repair. The def rather than the identity alone, because the same map
is what `Sources::listing` walks: an engine that has been told about a data source can say what
kind serves it and what it registers without asking a host for its rows back.

The lookup **resolves** rather than merely tests: it falls back to a case-insensitive match,
because `Url::parse` lower-cases a scheme and a host on the way into the registry (so
`S3://acme-lake/events/` names a store that is registered), and it answers with the *source's*
spelling, which is the string the def then stores — the same string the Configure picker,
`resolve_source` and the Forget confirm all address it by.

This is not the LOCATION toggle read differently. That toggle is an explicit choice precisely so a
typed **path** is never re-read as remote; in a statement the scheme is the only thing said about
where the files are. And the statement's `OPTIONS` cannot carry any of the data source's settings:
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
`crates/strata-engine/tests/object_store_minio.rs` — deliberately not `#[ignore]`d, because it is
the only thing that would catch a regression in the S3 credential bridge (a container runtime —
Docker, colima or Testcontainers Cloud — is required, and without one it fails rather than skips).

### How the two container tests are built

Both integration tests — `object_store_minio.rs` (W7) and `postgres_federation.rs` (DB-02) — drive
a **real server** rather than a mock, because the thing under test is a whole round trip against
one, and a mock is written by the same understanding of the protocol it is meant to check: it can
be shaped to pass without anyone noticing.

For the same reason, **each fixture is seeded by a deliberately different client than the code
under test**. `postgres_federation.rs` seeds with `tokio-postgres`, a layer *below* the pool and
factory it exercises, so the fixture is written with the raw driver and the test and the code
cannot agree on a shared misunderstanding. `object_store_minio.rs` seeds with `aws-sdk-s3`, an
independent implementation — `object_store` cannot create a bucket at all, and having the write
side be someone else's code is what stops fixture and subject agreeing on a wrong reading of the
protocol. Both seeding crates are already in the graph (the Postgres provider's `bb8-postgres`
pins one; the AWS chain carries the other), so neither costs a build.

`testcontainers` is pinned to the major `testcontainers-modules` itself depends on: the two share
a `bollard`, and a newer `testcontainers` resolves a second one that conflicts outright. Its
`properties-config` feature is not optional for us: the runtime is discovered from
`~/.testcontainers.properties` (which a Testcontainers Cloud agent writes) or `DOCKER_HOST`, and
without the feature that file is `#[cfg]`'d out — a perfectly good runtime reads as absent, while
being up, configured and invisible.

## How a query reaches a bucket

```mermaid
flowchart LR
    subgraph project [".strata/project.json"]
        C["SourceDef<br/>kind = s3 · name = lake<br/>address · region · auth"]
        T["TableDef events<br/>source = lake<br/>paths = events/2024/"]
    end
    C -->|"register pass, phase 1:<br/>probe chain, register store"| R["object-store registry<br/>key = s3://acme-lake"]
    C -->|"and its catalog,<br/>under the name"| K["catalog lake<br/>one schema, def-fed"]
    T -->|"resolve_source"| U["s3://acme-lake/events/2024/"]
    T -->|"register_external<br/>placed by def_home"| K
    U --> S["ListingTable scan"]
    R --> S
    K --> S
    S -->|"per request"| P["SdkCredentials →<br/>host's own credential chain"]
```
