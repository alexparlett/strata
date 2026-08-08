# Connections 03 · Editor forms (S3 / GCS / HTTP)

**Workstream:** Connections (W7) · **Status:** ✅ · **Depends on:** 01 · **Unblocks:** 04

## Goal
Per-provider connection editor forms.

## What landed

- **A window, not a modal** — `apps/connection/`, the canvas's 480 × 588 frame with its own
  traffic lights, drag bar and footer (`Connections.dc.html` is a `data-winframe`, like Configure
  and Export). Configure's shape throughout: a child of the project window that asked, pinned
  above it (`platform/connection.rs`), closing with the **project subtree** rather than the window
  id, single-instance per target, and writing that window's store through `persisted_defs` rather
  than holding an engine of its own.
- **The three parked affordances are live**, and nothing at those call sites changed but the
  handler: the pane header's `+`, the empty state's CTA and the row menu's *Edit connection* set
  `ConnectionRequest` and stop. `ConnectionLauncher` at the project root opens the window —
  `ConfigureLauncher`'s shape, for its reason (a menu is built inside an event handler, where no
  hook may run).
- **A command-palette row**, *New connection…*, so the header `+` folding under panel pressure
  loses nothing. One method on the router (P6-01), whose body is the same slot write.
- **`ProjectState::upsert_connection`** — replaces on `ConnectionDef::url()`, inserts at the
  address-sorted slot, resets the row to `Loading`. The two keys are the point, and both are
  tested: an address-keyed replace would let saving `gs://lake` take out the `s3://lake` it sorts
  beside.
- **`strata_core::engine::store::aws_profiles`** + `Engine::aws_profiles` — the profile *names*
  `~/.aws/config` and `~/.aws/credentials` define, parsed by `aws-config` (the `[profile x]` vs
  `[x]` split, `AWS_CONFIG_FILE`, and the two files merging are not the ini rules they look like).
  Nothing from inside a profile is read. New direct dep: `aws-types`, for the `Fs`/`Env` shims
  `aws_config::profile::load` takes and does not re-export — already in the graph.
- **`strata_model::ProviderId`** — the provider discriminant with no settings attached, so a
  picker has something to offer. `Provider::id()` projects onto it and `Display` delegates, which
  keeps the product's name (`GCS`) written down once across the badge and the picker. It carries
  no scheme: two providers state one and HTTP's is inside its address.
- **`components::form::PathField`** replaces `DirectoryField` — the same box and the same report
  contract over a folder **or one file** (`PathField::file(value, &["json"])`), because the two
  differ in the picker call and nothing else. The GCS service-account row is its first file user.

## Decisions this task settled

- **Save asks for a whole-catalog pass.** `plan_scan` puts connections in `ScanScope::All` alone,
  and its own doc names the re-connect case — a corrected region, an `aws sso login` — as "exactly
  what ↻ is for". That case *is* this window, so Save is the ↻ the user would otherwise press with
  the def written first. It is also the honest width: every table over the bucket was registered
  against the store this save replaces, and no per-connection dependency index exists to narrow it.
  No new `ScanScope` variant was added.
- **The window watches its own row** (`use_watch_connection`), the Configure window's
  reconciliation over shared state rather than a second registration path: `Ready` closes it,
  `Failed` keeps it open with the engine's own sentence. Worth staying open for here more than
  anywhere else — "The AWS profile 'analytics' resolved no credentials" describes the very field
  still on screen, and the pane's row can only give it on a hover.
- **A moved identity deregisters the old URL, in the footer.** A changed address *or* a changed
  provider changes `url()`; `engine::store::connect` only ever sees the def it is given and
  `register_pass` is additive, so nothing else would ever take that store out. Same
  `Engine::disconnect` Forget makes, and the move is logged like Configure logs a rename.
- **A new S3 connection opens with a blank region, not the canvas's `us-east-1`.** That seed is
  the arrow-rs#2795 default wearing a user's handwriting: `AmazonS3Builder` assumes `us-east-1`
  silently, the credential probe still passes, and the connection registers green over the wrong
  region. Blank blocks Save and says why; `us-east-1` stays the box's placeholder.
- **Field errors are the footer's, not the field's.** The canvas reddens the region box and writes
  a line under it; here `blocker()` is both the note beside the buttons and the value that
  disables Save, so a form cannot hold two accounts of its own validity. `Row::required()` still
  marks the label, because that is a fact about the field rather than a verdict on its contents.
- **The draft holds every provider's fields flat and projects the one in play** — the Configure
  and Export windows' split over `SourceFormat`. Spec §1 asks for provider switching to *sanitise*
  the auth mode; the types already make an invalid pair unwritable (`S3Auth` and `GcsAuth` are
  different types), so what the draft has to do instead is **not forget**: flipping to GCS and
  back must still have your region.
- **The Named-profile picker is a real discovery, and says which of its three states it is in.**
  A hardcoded list is the stub §1 forbids, and an empty dropdown cannot tell "no profiles" from
  "not read yet" — so the row reads *reading*, *none defined here (use Ambient)*, or the list.
- **HTTP shows the address box and nothing else.** No auth row, no region, no endpoint: a
  control that cannot mean anything for the chosen provider is not a control, which is the call
  the Configure window made about its own one-option LOCATION toggle.

## Follow-on, same task

Four changes after the first review pass, all of them tightening what a connection may *be*:

- **`ConnectionDef::bucket` is now `address`** (`serde(alias = "bucket")`, so committed projects
  still load). The three providers do not address the same thing, and one field named for one of
  them was the reason HTTP kept needing special cases.
- **HTTP is one input holding a whole URL** — `http://aserver:8484`, scheme included. No prefix
  chip, no scheme picker, no `HttpStore`: `http` and `https` are two different origins, and only
  the person typing knows which their server speaks. A **path is a validation error naming the
  part to drop** rather than something trimmed off, because the registry keys on scheme and
  authority. `allow_http` is **derived from the typed scheme** in `engine::store::build` —
  `ClientOptions` builds reqwest `https_only(!allow_http)`, so without it every plain-`http`
  request failed before leaving the process with a "builder error" naming nothing. The MinIO
  integration test caught that; nothing else would have.
- **Bucket names are validated against each provider's published rules**, in `Provider::
  check_address` — one copy, called by `engine::store::connect` *and* the editor, so the two
  cannot disagree. S3's are AWS's four; GCS's are Google's and genuinely differ (underscores,
  a dotted name to 222 with each part to 63, no dotted-decimal IP, no `goog`/`google`). What no
  local check can settle is left to the store and named as such: S3's reserved prefixes and
  suffixes, GCS's "close misspellings" of `google`.
- **Client options** — `object_store`'s `ClientConfigKey` map (`ConnectionDef::client_config`),
  edited as a table and committed as a map. On the def rather than in a provider because all
  three stores are built on one HTTP client; offered from `CLIENT_KEYS` (a written-down table,
  since the enum cannot list itself — pinned by a test that parses every entry) and refused by
  `check_client_config`, again one call for both sides. `ConfigRows` is deliberately **not**
  Settings' `PropRows`, which is welded to `ENGINE_KEYS`, a selection, an autocomplete and an
  inspector pane; what is shared is the rule, not the code.

**Two things the review pass caught, both of them silent failures.** A stored HTTP connection
written under the older shape (`bucket`, the authority alone, `https` derived) read as a URL with
no scheme after the rename and was refused on the next open — asking the user for something they
never had to type. `serde(alias)` migrates the field *name*; `ConnectionDef::migrated`, applied in
`project::load_defs` (the one path defs come off disk), migrates the *value*. And a plain-`http`
**S3-compatible endpoint** without Allow HTTP is now refused by name: `ClientOptions` builds its
client `https_only(!allow_http)`, so `object_store` failed every request with a bare
"HTTP error: builder error" naming neither the host nor the control to change. That is the same
trap the HTTP arm derives away; S3 has a toggle of its own, so it says which one.

**The MinIO integration test now proves external tables over two providers.** The same container
serves S3 (signed, a prefix listing) and HTTP (anonymous, one object, world-readable bucket
policy), each through connection → registered store → `register_external` → a query returning
rows, and each with its own orphan check. The HTTP connection carries client options, so
`with_config` is proved against a store that is then read through. **GCS remains a known gap**
and the file says why: `object_store`'s GCS client needs the XML list API no emulator serves, and
MinIO refuses its empty bearer header — GCS coverage needs a real bucket.

## What this leaves 04

- The **Configure LOCATION toggle** now has a way to make a connection: its canvas's
  **＋ New connection…** entry in the connection dropdown is one `ConnectionRequest` write, the
  same slot the pane's three triggers use — but from the *Configure* window, which is a different
  window and therefore needs the slot passed as a launch value rather than consumed from context.
  Worth deciding there rather than pre-building it here.
- `ProviderId` is the filter the connection dropdown wants (`ProviderId::ALL` for its TYPE
  segmented, `def.provider.id()` per row), and `ProviderId::label()` is the name that picker and
  the pane's badge have to agree on.
- The pane's ⓘ deliberately still does not mention Configure — add that sentence with the control
  (`ConnectionsHint`).

## Acceptance
- [x] Each provider's form validates + saves a connection; no secret is stored inline.
- [x] The pane's three parked affordances open it, and none is left `enabled(false)`.

## Freya / references
- Design: `Connections.dc.html` (+ the conn VM in `strata-windows.js`). `docs/CONNECTIONS_SPEC.md`
  §1/§6. DEV_TASKS W7. Module map: `docs/reference/MODULE_MAP.md` (`src/apps/connection/`).
