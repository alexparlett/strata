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
- **Region must be set explicitly** (arrow-rs#2795 — not reliably auto-derived), so the S3 connection's Region field is
  load-bearing.
- **GCS** resolves via `from_env` / a service-account file (ADC path) — no extra SDK.

## 4. Configure-table: local vs object store (FEATURES §6)

- A **LOCATION** segmented control at the top — **Local disk** / **Object store** — makes the choice **explicit** (not
  inferred from the first path's scheme). Both modes share name, format, and Hive partitioning.
- **Local disk:** the multi-path source list + per-row **Browse** (unchanged).
- **Object store:** a **single SOURCE PATH** (no add/remove, **no Browse** — object-store paths are text-only), entered
  **relative to the connection's bucket**
  (rendered with a non-editable bucket-prefix chip). Plus a **TYPE** segmented (S3/GCS/HTTP) filtering a **CONNECTION**
  custom dropdown (matches the FORMAT control) with a **＋ New connection…** entry; switching provider auto-selects its
  first connection, empty-provider shows an inline hint.
- **Removed** vs earlier drafts: the inline Manage/auth form (auth lives solely on the connection now), the
  **Public-bucket** toggle, the Disconnect action, and the **first-path-wins store-mismatch guard** (a table's store is
  the selected connection by construction).
- Validation blocks Register when object-store mode has **no connection** selected, and keeps the **S3 region** check
  via the connection.

## 5. Persistence

Connections carry **no secrets**. The per-provider **non-secret def** persists in the project's `.strata/` and reloads
on open (hydrating the pane + the Configure-table connection list); saved on add / edit / forget:

- **S3** — `{ provider, region, auth, profile, endpoint, allowHttp }`
- **GCS** — `{ provider, auth, saPath }`  (auth ∈ ambient / sa-file / anonymous)
- **HTTP** — `{ provider, auth: "anonymous" }`

Open question: bucket/provider is a shareable **def** (→ committed `project.json`), while a per-machine `profile` /
`saPath` may fit the gitignored `session.json`. Either way **no key/secret ever touches disk via the app**.

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
