# Connections + remote object-store sources (S21)

Spec for the Connections feature (**v11**): reading parquet/csv/etc. from remote object stores (**S3, GCS, HTTP** —
Azure dropped) via **project-scoped connections**, with **no app-managed secrets**. Design source: v11 `Strata.dc.html`

+ `FEATURES.md` §6/§15b + CHANGELOG.

## Direction (decided)

- Connections live in a **project-scoped sidebar pane**, not in Settings.
- **The app never stores or prompts for secrets** — no keys, no inline credentials. Access + region resolve at query
  time from the host's standard provider chains (AWS/GCS config files, env vars, instance/pod roles).
- For AWS we **wrap the `aws-config` default provider chain in an
  `object_store::CredentialProvider`** (the datafusion-cli pattern) — the chosen approach, not the env-only fallback.
- **Provider is an explicit picker** (S3 / GCS / HTTP), *not* inferred from a typed URL scheme.

## Provider scope

Registered ourselves (`ctx.register_object_store(url, store)` per bucket), so the set is what **`object_store`**
implements + which feature we enable.

- **v11 supported providers: S3, GCS, HTTP** only. **Azure was dropped** in v11 — no
  `az://` / `abfs://`, no Azure store/feature.
- **S3-compatible** stores (Cloudflare R2 / MinIO / Alibaba OSS / Tencent COS) ride the **S3** provider via a custom
  **Endpoint** (+ an **Allow HTTP** toggle) — not separate providers.
- (datafusion-cli's built-in remote schemes are `s3`/`oss`/`cos`/`gs`/`http(s)`; we register stores ourselves
  regardless, so this only informs the S3-compatible path.)

## 1. Connections pane (§15b)

- Left **activity rail** top group = **Catalog** | **Connections** (`sidebarPane`; clicking the active pane collapses
  the sidebar — VS Code model).
- Each row: a **provider badge** (labeled `S3` / `GCS` / `HTTP` — rounded-rect outline in `currentColor`/accent, not one
  shared cloud glyph) · bucket · **status dot** — green *Connected* (the chain resolves: Ambient / picked profile / SA
  file / Anonymous; **HTTP is always public → Connected**) vs amber *Needs credentials* (the chain yields nothing, e.g.
  profile mode with no profile chosen).
- **Edit is menu-only:** the row is not clickable (cursor `default`); Edit / Forget come from the ⋮ / right-click
  catalog row-menu (`kind:"conn"`; Forget → remove-confirm).
- Empty state: icon + one-line explainer + **Add connection**.
- **Add / Edit dialog — as built (Connections 03), a child *window* rather than a dialog**
  (`data-winframe="conn"`, like Configure and Export). Departures from the canvas, each a state
  removed rather than a field renamed:
    - **HTTP is one box holding a whole URL** (`http://aserver:8484`), scheme included — no
      prefix chip and no scheme picker, because `http://` and `https://` are two different
      origins and only the person typing knows which their server speaks. A **path is a
      validation error naming the part to drop**, never trimmed off: the registry keys on scheme
      and authority, so a connection carrying one would go in under a key nothing looks up.
    - **A bucket name is checked against its provider's own published rules** before Save and
      before `connect` — the same call, so the two cannot disagree. S3's are AWS's four (3-63
      characters, lowercase/digits/dots/hyphens, alphanumeric at both ends, no `..`); GCS's are
      Google's, which are **not** the same (underscores allowed, a dotted name to 222 with each
      part to 63, no dotted-decimal IP, no `goog` prefix, no `google`).
    - **Client options** are a table of `object_store` `ClientConfigKey` rows (timeouts, proxy,
      HTTP version, user agent), edited as rows and committed as a map, offered on every
      provider because all three stores are built on one HTTP client. `allow_http` is **not**
      among them: on HTTP it is derived from the scheme typed, and on S3 it is the endpoint's own
      toggle — and a plain-`http` endpoint without that toggle is now **refused by name**, because
      reqwest is built `https_only` and otherwise fails every request with a bare
      "builder error".
    - **A new S3 connection opens with a blank region, not `us-east-1`.** That seed is exactly the
      arrow-rs#2795 default in a user's handwriting — the builder assumes `us-east-1` silently, the
      credential probe still passes, and the connection registers green over the wrong region.
      Blank blocks Save and says why; `us-east-1` stays the placeholder.
    - **A field's error lives in the footer, not on the field.** One value both disables Save and
      explains it, so the form cannot hold two accounts of its own validity. The label still
      carries `REQUIRED`.
    - **HTTP shows the URL box and nothing else** — no auth pill, no region, no endpoint: a
      control that cannot mean anything for the chosen provider is not shipped disabled.
  Save writes the def, persists it, deregisters the old URL when an edit **moved** the bucket or
  the provider, and asks for a whole-catalog pass; the window then watches its own row and closes
  on `Ready`.
- **Add / Edit dialog:**
    - **PROVIDER** segmented picker (S3 / GCS / HTTP) — explicit; switching provider sanitises the auth mode to one
      valid for it.
    - **BUCKET** (REQUIRED) — scheme-qualified (e.g. `s3://acme-lake`). A non-editable **scheme-prefix chip** shows
      **only for HTTP** (`https://`); S3/GCS hide it since the provider picker already states the scheme.
    - Per-provider auth control + fields (see §2 / §6). **No key/secret fields anywhere.**
    - Save/validation is **per-provider** (e.g. S3 Region required).
- Keyed by **scheme+authority (bucket)** in the `connections` map — the same map the Configure-table connection dropdown
  reads.

## 2. Auth model — no app-managed secrets

The app stores only **non-secret metadata** per connection. Credentials resolve at query time from the standard provider
chains; the app never takes or stores keys.

**Auth is provider-specific** (`connAuthOptions(provider)`) — see §6:

- **S3** — Ambient / Named profile / Anonymous.
- **GCS** — Ambient (ADC) / Service-account **file path** / Anonymous.
- **HTTP** — none (always anonymous).

## 3. Credential mechanics (researched)

- **DataFusion core resolves nothing.** The embedder builds an `object_store` and calls
  `ctx.register_object_store(&Url::parse("s3://<bucket>")?, Arc::new(store))`
  **per bucket** — else *"No suitable object store found"*.
- **`object_store` alone is env-only.** `AmazonS3Builder::from_env()` reads `AWS_*`
  env vars + IMDS / ECS / web-identity. It does **not** read `~/.aws` **profiles** or do **SSO**; `AWS_PROFILE` alone is
  ignored.
- **The full "normal AWS" chain** (env → profile → SSO → IMDS → `credential_process`)
  is the **`aws-config`** SDK crate.
- **The bridge (our direction):** wrap `aws-config`'s resolved credentials in an
  `object_store::CredentialProvider` and feed `AmazonS3Builder` — the datafusion-cli pattern. Needs `aws-config` +
  `aws-credential-types`; vendor datafusion-cli's
  ~200-line bridge (it's a binary crate, not a stable API).
  **Built** as `strata_core::engine::store::SdkCredentials` — and it resolves **per request**, not once at build,
  because that is the whole reason to wrap the SDK's provider instead of copying a key out of it: SSO / assumed-role
  / IMDS credentials expire in minutes and the SDK's provider is what knows how to refresh them.
- **Ambient and Named profile are two different providers, not one chain with a setting.** `aws-config`'s
  `ConfigLoader::profile_name` configures the default chain's *Profile arm*; it does not move that arm to the front,
  and `DefaultCredentialsChain::build` is unconditionally `Environment → Profile → WebIdentity → ECS → IMDS`. Built
  that way (as it was first time), a Strata launched from a shell exporting `AWS_ACCESS_KEY_ID` signs as the
  *environment* identity while the pane shows the profile the user chose — Ambient and Profile become the same
  connection wherever ambient credentials exist, and a misspelled profile name still shows green. So **Ambient** is
  `aws_config::defaults(...)` (the whole chain, whatever answers) and **Named profile** is
  `ProfileFileCredentialsProvider` alone (that profile's own mechanism — `source_profile`, `role_arn`,
  `sso_session`, `credential_process` — and no fallback to anyone else's identity).
- **Region must be set explicitly** (arrow-rs#2795 — not reliably auto-derived), so the S3 connection's Region field is
  load-bearing. `AmazonS3Builder` silently defaults it to `us-east-1`, so `engine::store` **refuses** a blank one
  rather than letting that default stand.
- **GCS** resolves via `from_env` / a service-account file (ADC path) — no extra SDK. One consequence worth stating:
  an ambient GCS connection with no credentials at all still *builds* (the builder installs the GCE metadata
  provider without asking anything), so it is the one arm whose status cannot be known without a request to the
  bucket — and that request is not worth making.
- **The status dot is the connect outcome, not a separate probe.** `engine::store::connect` resolves the credential
  chain once and throws the answer away, *before* registering: green is a connection that registered, amber is the
  `Err` it reported, with what to fix. Without that probe a credential-less connection registers happily and the
  diagnosis lands on every table over the bucket instead — one opaque signing error each, in the wrong place.
  Registration is therefore all-or-nothing: a connection is never both refused and live.

## 4. Configure-table: local vs object store (FEATURES §6) — as built (Connections 04)

- A **LOCATION** segmented control at the top — **Local** / **Remote** (the canvas's *Local disk* / *Object store*) — makes the choice **explicit** (not
  inferred from the first path's scheme). Both modes share name, format, and Hive partitioning.
- **Local:** the multi-path source list + per-row **Browse** (unchanged).
- **Object store:** a **single SOURCE PATH** (no add/remove, **no Browse** — object-store paths are text-only), entered
  **relative to the connection's bucket**
  (rendered with a non-editable bucket prefix). Plus a **TYPE** segmented (S3/GCS/HTTP) filtering a **CONNECTION**
  dropdown (the same `Select` the FORMAT control is) with a **New connection…** entry; switching provider auto-selects
  its first connection, empty-provider shows an inline hint.
- **Removed** vs earlier drafts: the inline Manage/auth form (auth lives solely on the connection now), the
  **Public-bucket** toggle, the Disconnect action, and the **first-path-wins store-mismatch guard** (a table's store is
  the selected connection by construction).
- Validation blocks Register when object-store mode has **no connection** selected, and keeps the **S3 region** check
  via the connection.

**What the def stores, and where the two halves meet.** `TableDef::connection` is the chosen connection's
`url()` — a *reference*, never a copy of the bucket, the provider or the auth — and it is the one field that says a
table is remote: its sources are bucket-relative exactly when it is `Some`, stored as typed (never `relativize`d, which
measures against the project folder). `strata_core::project::resolve_source` takes the connection and is the single
place the two are composed; `register::table_spec` calls it, and so does the window's own Hive detection, so a remote
path is never resolved by the local rule. The engine half needs nothing: the store went in under that URL in
`register_pass`'s first phase.

**Four departures from the canvas:**

- **The toggle's second answer is `Remote`, not `Object store`.** That is the implementation's word — the thing
  DataFusion registers and this app calls a connection — and a reader who has never met it cannot tell which of the two
  answers is theirs. `Remote` is the question the row is actually asking; TYPE, CONNECTION and a bucket-relative path
  explain themselves from there. The concept keeps its name everywhere it is not a label (the pane, the spec, the code).
- **The LOCATION and TYPE pills are text-only** — no leading glyphs — because the connection editor's PROVIDER pill next
  door is, and the two windows' pills should read as one control.
- **New connection… does not open an editor**; it sets the project window's `ConnectionRequest`, the same slot the
  pane's `+`, its empty-state CTA and a row's *Edit connection* set. The editor is that window's child, so it survives a
  Configure window closed while it is up, and the connection it saves appears in this picker without a reopen. It opens
  on the editor's **own** default provider rather than the TYPE currently picked: the target is that window's identity,
  and a provider seed would make two *New connection* windows possible at once.
- **A def naming a connection this project no longer has keeps naming it**, and Save is blocked with
  `'s3://gone' is not a connection in this project.` Rewriting it to "local disk" would silently re-point the table at a
  relative path on the user's own machine — the same treatment a format with no reader gets.

**A forget now has a consequence**, since a table's sources can name a connection: the confirm lists the tables whose
def reads through it and the views behind those, in the sentence a table drop already uses.

**Hive partitioning works over a bucket, and needed nothing.** DataFusion's partitioning is entirely at the
`ObjectStore` level (`list_partitions` over `list_with_delimiter`'s common prefixes, `parse_partitions_for_path` over
the object path), and `engine::catalog::detect_partitions` already listed through the session's registered store rather
than `read_dir`. The MinIO test proves the whole arm: the keys are **found** by listing the bucket, the folder levels
register as the typed columns the def declares, every partition's rows come back carrying its folder's values, and a
filter on a partition column takes the pruning path through the same store. The one thing that changed is the
**failure** message: a partitioned source whose location came back empty is now *listed* — `std::fs` for a local
directory as before, `ObjectStore::list` for a bucket (`store_holds_ext`, the client `detect_partitions` already uses)
— so a remote lake under plain `2024/` folders gets the same "No .csv files under 'x' match the partition columns
'year'." a local one does, and a prefix that really is empty is not blamed on the columns. One bounded listing, only
on a failure, only for a partitioned source; a glob brings none, because a pattern is not a place to list.

## 5. Persistence — as built (Connections 01)

Connections carry **no secrets**. The per-provider **non-secret def** persists in the project's `.strata/` and reloads
on open (hydrating the pane + the Configure-table connection list); saved on add / edit / forget.

**Settled: the whole def rides the committed `project.json`**, beside the tables and views —
`ProjectDefs::connections`, sorted by bucket like every other section. The open question below (split the
per-machine `profile` / `saPath` into the gitignored `session.json`) is **closed against splitting**: a def carrying
only a profile *name* and a key *file path* holds nothing a colleague may not have, and a catalog whose tables live
in a bucket is not shareable if the bucket isn't. **No key/secret ever touches disk via the app** either way.

The shape is `strata_model::ConnectionDef` — a **bucket plus a tagged provider**, where the provider *is* its own
settings (the same argument as `SourceFormat`: a region means nothing to the HTTP store, and a def carrying every
provider's fields has states where they disagree):

```json
{ "address": "acme-lake",
  "provider": { "provider": "s3", "region": "eu-west-2",
                "auth": { "mode": "profile", "name": "analytics" },
                "endpoint": "", "allow_http": false },
  "client_config": { "timeout": "30s" } }
{ "address": "lake",                "provider": { "provider": "gcs", "auth": { "mode": "service-account", "path": "…" } } }
{ "address": "http://aserver:8484", "provider": { "provider": "http" } }
```

The field is **`address`, not `bucket`** (`serde(alias = "bucket")` for the name, and
`ConnectionDef::migrated` for the value — an HTTP connection written under the older shape stored
the authority alone and derived `https`, so `load_defs` prepends it rather than letting a
scheme-less URL be refused on the next open): S3 and GCS address a bucket whose scheme their provider states, while an
HTTP connection addresses a whole origin URL, and one field cannot be named for only one of them.
`client_config` is absent unless set.

Two deliberate differences from the v11 canvas's flat object, both of them states being removed rather than fields
being renamed:

- **An S3 / GCS address is the bucket alone**, not the scheme-qualified string. The scheme comes from the provider
  (`ConnectionDef::url()` → `s3://acme-lake`), so an `s3://` bucket under a GCS provider cannot be written down;
  the form strips a pasted prefix. **An HTTP address is the whole URL**, because its scheme is not the provider's
  to state. `url()` is the registry key either way.
- **`profile` / `saPath` live inside `auth`**, not beside it — `{"mode":"profile","name":…}`. A profile named on an
  Ambient connection is not a state worth having.

Every provider's settings are `#[serde(default)]`, so a def written before a setting existed still loads.

## 6. Provider auth options

Provider is chosen by the **PROVIDER picker**; the field set + auth control change per provider. Only secret-free
options are offered.

### S3 — `s3://` (+ S3-compatible via endpoint)

- **Fields:** Bucket · **Region — required** (arrow-rs#2795) · optional **Endpoint** + **Allow HTTP** toggle
  (S3-compatible: R2 / MinIO / OSS / COS).
- **Auth:** **Ambient** (env → `~/.aws` profiles → SSO → web-identity → ECS → IMDS) · **Named profile** (dropdown from
  `~/.aws/config`) · **Anonymous** (`skip_signature`).
- **Bridge:** `aws-config` needed **only** for profile / SSO; env / IMDS / ECS / anonymous work with `object_store`
  alone. **Excluded:** any key / secret / token.

### GCS — `gs://`

- **Fields:** Bucket.
- **Auth:** **Ambient / ADC** (`GOOGLE_APPLICATION_CREDENTIALS` → gcloud ADC → GCE/GKE metadata) · **Service-account
  file** (a **path**, not inline JSON) · **Anonymous**.
- Native to `object_store`; no extra SDK. **Excluded:** inline SA JSON key, bearer token.

### HTTP (S) — `http(s)://`

- No auth control, no fields beyond the bucket/URL — always anonymous ("public URL").

| Provider          | Required non-secret fields                | Secret-free auth modes                         | Stored def                                          | Extra dep                       |
|-------------------|-------------------------------------------|------------------------------------------------|-----------------------------------------------------|---------------------------------|
| S3 (+ compatible) | Region (+ Endpoint/Allow-HTTP for compat) | Ambient · Named profile · Anonymous            | `{provider,region,auth,profile,endpoint,allowHttp}` | `aws-config` (profile/SSO only) |
| GCS               | —                                         | Ambient/ADC · Service-account file · Anonymous | `{provider,auth,saPath}`                            | none                            |
| HTTP(S)           | —                                         | (none — anonymous)                             | `{provider,auth:"anonymous"}`                       | none                            |

## 7. Design alignment

The v10 "design changes required" (drop the Access-key form, per-provider auth, no-secrets copy, required Region,
status-dot legend, provider set) were **all incorporated in v11** — plus Azure dropped, an explicit provider picker,
single-path object-store tables, and a custom connection dropdown. So the spec above *is* the v11 design; no outstanding
design asks.

## References

- DataFusion CLI data sources: <https://datafusion.apache.org/user-guide/cli/datasources.html>
- DataFusion `query_aws_s3`
  example: <https://github.com/apache/datafusion/blob/main/datafusion-examples/examples/external_dependency/query_aws_s3.rs>
- `aws-config` crate: <https://docs.rs/aws-config/latest/aws_config/>
-
`object_store::aws::AmazonS3Builder`: <https://docs.rs/object_store/latest/object_store/aws/struct.AmazonS3Builder.html>
