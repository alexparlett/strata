# Connections 04 · Config LOCATION toggle + object-store branch

**Workstream:** Connections (W7) · **Status:** ⬜ · **DEV_TASKS:** U14 · **Depends on:** 01, P4-11

## Goal
Register tables over a remote connection from the Configure-table window.

## Current state
Not built. P4-11 builds the Configure window **local-disk only and leaves no hook** — a toggle with
one option is a control that cannot be operated, so the whole LOCATION section is absent rather than
present-and-disabled. This task adds it, which means adding the branch as well as the switch.

## Build
Everything below is drawn on `Configure.dc.html` and lives in its VM (`SW.cfgView`) already; P4-11
built only the local arm of each.

- The **LOCATION** segmented control at the top of the body: **Local disk** vs **Object store**.
- The remote arm, which is a second row: a provider pill (S3 · GCS · HTTP), a CONNECTION picker
  (`REQUIRED`, with a "New connection…" item that opens the connection editor, and the
  "No {provider} connections yet — add one to continue." empty line), and Save blocked while no
  connection is chosen.
- The path list becomes **single-path** on a connection (`multiPath` / `singlePath` in the VM), with
  the connection's bucket rendered as a non-editable prefix on the box, the label reading
  `SOURCE PATH`, and the placeholder changing to a bucket-relative one. Switching to remote keeps
  the first non-blank local path.
- Resolve paths against the connection's object store and `register_external` over the remote store.
  Connections 01 did the engine half: the store is already registered under
  `ConnectionDef::url()` by the time any table registers (connections are `register_pass`'s first
  phase), so this is **path composition only** — `format!("{}/{path}", conn.url())` into
  `TableSpec::paths`, with nothing new on the engine. Note that `project::resolve_source` must not
  touch a remote path: it joins relative entries onto the project folder, which would turn
  `s3://…`-relative text into a local path.
- The ⓘ resolution tooltip already has the sentence for this ("Object-store paths are relative to
  the selected connection's bucket"); P4-11 ships it, so nothing changes there.

## What Connections 02 handed over

- **The provider's name is `impl Display for Provider`** (`strata-model`) — `S3` / `GCS` / `HTTP`,
  which the pane's badge already reads. This picker is the second surface that has to agree, so
  read it there rather than typing the three strings again. Not `Provider::scheme`, which is the
  URL's word (`gs`, `https`) and belongs to the registry.
- **Forget's confirm makes no consequence claim**, because today nothing can read a bucket: the
  body is "Removes the object store from this project. Nothing in the bucket is deleted." The
  moment a table's sources can name a connection, that stops being the whole truth — give
  `DropTarget::Connection` a **consequence line** listing the tables over the forgotten bucket, the
  way a table drop lists the views over it (`consequence` + `dependent_views` in
  `dialogs/drop_confirm.rs`, and the same shape `registration_faults` already uses).

## Acceptance
- [ ] A table can be registered over a remote connection (paths resolve against its object store).
- [ ] With Object store selected and no connection for the provider, Save is blocked and the empty
      line explains why.

## Freya / references
- Design: `Configure.dc.html` LOCATION / TYPE / CONNECTION blocks + the `remote` branches of
  `SW.cfgView`. Core `register_external` + `object_store`. DEV_TASKS U14/W7.
- The CONNECTION picker's list is `ProjectState::connections` filtered by `Provider` variant, read
  off the Configure window's shared store. The provider's **label** (`S3` / `GCS` / `HTTP`) is a
  name two surfaces have to agree on — this picker and 02's row badge — so it belongs in one place;
  Connections 01 deliberately left it unwritten rather than shipping an accessor nothing called. The
  **S3 region check** the spec keeps (§4) is the connection's, not the table's — a connection with
  no region never registers a store, so a table over it fails on its own row.
- **P4-11** owns the window itself (`apps/configure/`, `platform/configure.rs`), the path list, the
  import options and the Save path — this task adds a section and a branch, and changes nothing
  about how Save works.
