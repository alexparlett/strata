# Connections + remote object-store sources (S21)

Spec for the v10 Connections feature: reading parquet/csv/etc. from remote object
stores (S3 + S3-compatible, GCS, HTTP; **Azure optional** — see Provider scope) via
**project-scoped connections**, with **no app-managed secrets**. Design source: v10
`Strata.dc.html` + `FEATURES.md` §6/§15b + CHANGELOG.

## Direction (decided)

- Connections live in a **project-scoped sidebar pane**, not in Settings.
- **The app never stores or prompts for secrets.** Credentials resolve at query
  time from the standard cloud provider chains.
- For AWS we **wrap the `aws-config` default provider chain in an
  `object_store::CredentialProvider`** (the datafusion-cli pattern) — this is the
  chosen approach, not the env-only fallback.

## Provider scope

We register object stores **ourselves** (`ctx.register_object_store(url, store)` per
bucket), so the supported set is what the **`object_store`** crate implements + which
feature we enable — *not* what `datafusion-cli` happens to auto-register.

- **datafusion-cli's** built-in remote schemes are **`s3://` · `oss://` · `cos://` ·
  `gs://` · `http(s)://`** — **not** `az://`. (OSS / COS are the S3 builder + a custom
  endpoint — i.e. **S3-compatible**, not separate stores.)
- **`object_store`** additionally ships an **Azure** store behind its **`azure`
  feature** — so Azure *is* reachable, but we enable the feature and write the
  registration ourselves (same shape as S3 / GCS); nothing hands it to us for free.

**v1 baseline:** **S3** (+ S3-compatible — OSS / COS / Cloudflare R2 / MinIO via a
custom **endpoint**), **GCS**, **HTTP(S)**. **Azure (`az://` / `abfs://`) is a
fast-follow** (flip the `object_store` `azure` feature + register). The v10 design
still lists `az://`, so either we build Azure or the design drops it — **decision
pending**.

## 1. Connections pane

- Left **activity rail** top group = **Catalog** | **Connections** (`sidebarPane`
  state; clicking the active pane collapses the sidebar — VS Code model).
- Lists each saved object store: provider icon · bucket · **status dot** — green
  *Connected* (env / anonymous / profile resolves) vs amber *Needs credentials*.
- Row ⋮ / right-click → shared catalog row-menu (`kind:"conn"`): **Edit
  connection** / **Forget connection** (Forget routes through remove-confirm).
- Empty state: icon + one-line explainer + **Add connection**.
- **Add/Edit modal:** Bucket URL (Add-only, parsed live → provider label;
  read-only subtitle on Edit), **Authentication** segmented (see §2), **Region**,
  and a **profile picker** for the profile mode (names read from `~/.aws/config`).
  **No secret-entry fields.** Primary button disabled until the URL resolves.
- Keyed by **scheme+authority (bucket)** in the `connections` map — the *same map*
  the Configure-table auth panel reads, so connecting a bucket flips referencing
  tables *Needs credentials → Connected* live.

## 2. Auth model — no app-managed secrets

Authentication modes (per connection):

- **Ambient** — the standard default provider chain (see §3).
- **Named profile** — a profile from `~/.aws/{config,credentials}` (AWS only).
- **Anonymous** — unsigned / public buckets (`aws.SKIP_SIGNATURE`); HTTP(S) never
  needs credentials.

A connection stores only **non-secret metadata**: bucket · provider · region ·
auth-mode · profile name. Keychain is **not** part of the AWS chain — an optional
later integration, not baseline.

> Diverges from the v10 design's Access-Key-ID/Secret form — that form should drop
> the secret fields.

## 3. Credential mechanics (researched)

- **DataFusion core resolves nothing.** The embedder builds an `object_store` and
  calls `ctx.register_object_store(&Url::parse("s3://<bucket>")?, Arc::new(store))`
  **per bucket** — else *"No suitable object store found"*.
- **`object_store` alone is env-only.** `AmazonS3Builder::from_env()` reads `AWS_*`
  env vars + IMDS / ECS / web-identity. It does **not** read `~/.aws` **profiles**
  or do **SSO**; `AWS_PROFILE` alone is ignored.
- **The full "normal AWS" chain** (env → profile → SSO → IMDS →
  `credential_process`) is the **`aws-config`** SDK crate.
- **The bridge (our direction):** wrap `aws-config`'s resolved credentials in an
  `object_store::CredentialProvider` and feed the `AmazonS3Builder` — the pattern
  `datafusion-cli` uses (precedence: explicit keys → `aws-config`). Needs
  `aws-config` + `aws-credential-types`; vendor datafusion-cli's ~200-line bridge
  (it's a binary crate, not a stable API to depend on).
- **Region must be set explicitly** (arrow-rs#2795 — not reliably auto-derived), so
  the connection's Region field is load-bearing.
- **GCS / Azure** resolve via `from_env` (service-account / ADC path;
  `AZURE_*` / managed identity) — no extra SDK.

## 4. Remote sources in Configure-table (FEATURES §6)

- Source paths may be `s3://` · `gs://`·`gcs://` · `az://`·`abfs://` · `http(s)://`
  through one `ListingTableUrl` — globs / dirs / Hive partitioning work identically
  to local paths.
- Remote panel names the derived bucket/provider + live connection status.
- **Public-bucket** toggle → `aws.SKIP_SIGNATURE true`.
- **One table = one object store**: the store is derived from the *first* path;
  mixing buckets/providers or local+remote is flagged inline and blocks Register
  (*"UNION them in a view"*).
- A cloud path with no connection and not public blocks Register with an inline
  *connect-this-bucket* prompt (distinct from a generic error).

## 5. Persistence

Connections carry **no secrets**, so the definition (bucket · provider · region ·
auth-mode · profile name) persists in the project's `.strata/` and reloads on open
— hydrating the Connections pane + the Configure-table bucket matcher; saved on
add / edit / forget.

- Bucket/provider is a shareable **def** → committed `project.json`.
- auth-mode + profile name are **per-machine** → may belong in the gitignored
  `session.json` instead (**open question**).
- Either way, **no key/secret ever touches disk via the app**.

## References

- DataFusion CLI data sources: <https://datafusion.apache.org/user-guide/cli/datasources.html>
- DataFusion `query_aws_s3` example: <https://github.com/apache/datafusion/blob/main/datafusion-examples/examples/external_dependency/query_aws_s3.rs>
- `aws-config` crate: <https://docs.rs/aws-config/latest/aws_config/>
- `object_store::aws::AmazonS3Builder`: <https://docs.rs/object_store/latest/object_store/aws/struct.AmazonS3Builder.html>
