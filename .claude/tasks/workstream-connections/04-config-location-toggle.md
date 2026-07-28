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
- The ⓘ resolution tooltip already has the sentence for this ("Object-store paths are relative to
  the selected connection's bucket"); P4-11 ships it, so nothing changes there.

## Acceptance
- [ ] A table can be registered over a remote connection (paths resolve against its object store).
- [ ] With Object store selected and no connection for the provider, Save is blocked and the empty
      line explains why.

## Freya / references
- Design: `Configure.dc.html` LOCATION / TYPE / CONNECTION blocks + the `remote` branches of
  `SW.cfgView`. Core `register_external` + `object_store`. DEV_TASKS U14/W7.
- **P4-11** owns the window itself (`apps/configure/`, `platform/configure.rs`), the path list, the
  import options and the Save path — this task adds a section and a branch, and changes nothing
  about how Save works.
