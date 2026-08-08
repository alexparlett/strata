# Connections 04 · Config LOCATION toggle + object-store branch

**Workstream:** Connections (W7) · **Status:** ✅ · **DEV_TASKS:** U14 · **Depends on:** 01, P4-11

## Goal
Register tables over a remote connection from the Configure-table window.

## What was built

- **`TableDef::connection`** (`strata-model`) — the chosen connection's `ConnectionDef::url()`, and
  the **one** field that says a table is remote: its sources are bucket-relative exactly when it is
  `Some`. A reference, never a copy of the bucket, provider or auth. `#[serde(default,
  skip_serializing_if)]`, so a project file gains nothing until a table points at a bucket and
  every def written before it still loads.
- **`project::resolve_source(root, connection, source)`** — one function, taking the connection, so
  a caller cannot reach for the wrong rule. That is stronger than the "must not touch a remote
  path" note below: `s3://` is not an absolute *path*, so the local rule would silently answer
  `<project>/events/2024/` and register a missing folder on the user's own disk. `table_spec` and
  the Configure draft's `resolved_sources` both go through it; `relativize` is skipped on the way
  back out, because a bucket-relative path has nothing to do with the project folder.
- **The LOCATION section** (`apps/configure/views/location.rs`): the Local · Remote
  pill, and behind the second answer a TYPE pill (`ProviderId::ALL`) filtering a CONNECTION
  `Select` over `connections_for(&connections, provider)`, with *New connection…* and the
  "No {provider} connections yet. Add one to continue." line (two sentences, not the canvas's
  dash: the Connections pane's own empty state reads this way, and shipped text carries no
  em-dashes). `ObjectStore` is always mounted and draws nothing on the local disk —
  `views::hive`'s shape, so the form's row count never changes.
- **The source list goes singular on a connection** (`views/paths.rs`): `SOURCE PATH`, no toolbar,
  one row wearing the bucket as a non-editable prefix, a bucket-relative placeholder, and a hint
  that says the trailing slash out loud (nothing browses a bucket, and `events/2024` without one is
  a request for a single object of that name).
- **Save is blocked** while Remote has no connection (`ConfigureDraft::blocker`), and while
  the def names a connection this project no longer has (`views::footer::missing_connection`).
- **Forget's consequence line** (`dialogs/drop_confirm.rs`): the tables whose def reads through the
  connection (`ProjectState::tables_over`) *and* the views behind them (`views_over`), in
  `forget_consequence`'s own sentence — a connection has no readers in the SQL namespace, so the
  shared `consequence` could not have said it.

## Decisions worth keeping

- **The toggle's answers are `Local` and `Remote`**, where the canvas says *Local disk* / *Object
  store*. "Object store" is the implementation's word and a reader who has never met it cannot tell
  which answer is theirs; one word each also makes the pair read as the choice it is rather than as
  a place beside a technology. The concept keeps its name everywhere it is not a label.
- **The LOCATION and TYPE pills are text-only**, dropping the canvas's leading glyphs, because the
  connection editor's PROVIDER pill next door is text-only and the two windows' pills should read
  as one control. `SegmentedToggle` was not grown a third content shape for one call site.
- **New connection… sets `ConnectionRequest` and stops** — the slot the pane's `+`, its CTA and a
  row's *Edit connection* all set. The editor is the *project* window's child, so it survives a
  Configure window closed while it is up, and what it saves lands in the store this picker already
  reads. It opens on the editor's own default provider rather than the TYPE picked here: the target
  is that window's identity, and a provider seed would make two *New connection* windows possible.
- **A def over a forgotten connection keeps naming it.** Quietly rewriting it to the local disk
  would re-point the table at a relative path on the user's machine; the footer says so and blocks
  Save instead, which is what a format with no reader gets.
- **The ⓘ hint is per-mode.** P4-11 shipped only the local half ("Paths are absolute"), so the
  object-store sentence the canvas has was written here after all.
- **Switching to Remote keeps the first non-blank path** and clears the detected partition
  columns, as every other path mutator does — they describe a location the draft no longer names.

## Acceptance
- [x] A table can be registered over a remote connection (paths resolve against its object store).
- [x] With Remote selected and no connection for the provider, Save is blocked and the empty
      line explains why.

## Tests

`apps/configure/model.rs` covers the draft (the def's URL + relative path, the single-path switch,
the provider filter, a remembered connection that is not a location, a def over a forgotten
connection); `apps/configure/interaction.rs` drives the body and footer (the empty-provider line
and its footer twin, a Save that writes the def, the forgotten-connection block);
`register.rs` pins `table_spec`'s composition and `project.rs` `resolve_source`'s;
`drop_confirm.rs` pins the forget consequence.

**Acceptance 1 is proved against a real server.** `strata-core/tests/object_store_minio.rs` now
builds its tables as **defs naming the connection** and composes them through `table_spec`, so the
container test's chain is connection → registered store → a def as Table Config writes one →
listing, inference, and a query that returns rows. It carries the whole HTTP arm the same way.

**Not driven from a test: a `Select`'s own menu.** Freya's `Select` closes itself when the focused
accessibility node is not its own, and a press in the testing runner never lands that focus, so the
list opens and shuts inside one update. That is the harness rather than this picker (it is the
format picker's control), so choosing a connection and *New connection…* are covered by the model
tests and the one-line handlers instead. Fixing it would be a fork-side change to `Select` or to
`freya-testing`'s focus handling — worth doing if another surface needs the same coverage.

## Hive partitioning over a bucket

**It works, and nothing had to be built for it** — verified against DataFusion 54's source and
then against MinIO. Both halves of DataFusion's partitioning are at the `ObjectStore` trait level:
`catalog-listing::helpers::list_partitions` walks `list_with_delimiter`'s common prefixes, and
`parse_partitions_for_path` reads the `key=value` segments off the object path relative to the
table root. Our half was already store-agnostic too — `engine::catalog::detect_partitions` lists
through the session's registered store rather than `read_dir` (its own doc says that is why it
lives in the engine), and `register_external` passes `with_table_partition_cols` whatever the def
declares.

Two things worth knowing:

- **A trailing `/` is advice, not a rule, on a bucket.** `listing_url` only adds one for a local
  directory (`Path::is_dir`, which a bucket prefix answers `false` to), but DataFusion's
  `list_prefixed_files` does `head` first and **retries as a collection on `NotFound`**, so a
  slash-less remote prefix still lists. The SOURCE PATH hint still says to write one, because that
  is a round trip saved and the shape the user means.
- **The partition diagnosis now asks the store**, so a bucket gets the same three answers a local
  directory does. A source that holds files under `2024/` where the def asks for `year=` earns
  "No .csv files under 'x' match the partition columns 'year'." — locally off a bounded
  `std::fs` walk (`holds_ext`), remotely off a bounded `ObjectStore::list`
  (`store_holds_ext`), which is the same client `detect_partitions` lists through. The listing is
  `async`, so it happens in `register_external`'s failure arm and its answer is *handed* to the
  (still sync) message mapper: `register_error(spec, ext, raw, holds)`. It costs one listing, only
  on a failure, only for a **partitioned** source whose location came back empty — a glob still
  brings none, because a pattern is not a place to list. `None` means unsettled and counts as
  "do not claim emptiness", exactly as an exhausted local walk does.

The MinIO test carries the whole arm: `detect_partitions` finds `["year", "month"]` by listing the
bucket, the table registers with the folder levels as `Int32` columns, a read returns every
partition's rows carrying its folder's values, and a `WHERE year = 2025` read exercises
`pruned_partition_list` — the other path through the store, which lists levels and drops whole
prefixes before opening a file. Then the two diagnosis cases, and the second one is what makes the
first mean anything: an unkeyed `flat/2024/` lake blames the columns, and a prefix with **nothing**
under it does not — that second assertion is the only one that fails if the listing is removed,
since an unsettled answer is deliberately indistinguishable from "files are there". Verified by
mutation: stubbing `holds_under_partitions` to `None` turns the empty-prefix message into the
partition complaint and the test red.

## Known seam, found while building this

**An edit that moves a connection's address or provider orphans every table over it, silently.**
Identity is the `url()`, so changing either half writes a new one; the editor already deregisters
the old store (Connections 03), but the table defs go on naming a URL nothing answers to, and their
rows fail the next pass with "no suitable object store found". Forget now warns about exactly this
population (`tables_over` / `views_over`) — the editor's Save does not, and there are two honest
answers: warn on the same terms, or re-point the defs in the same write. Neither is this task's
(it changes nothing about how either window saves), and both need the other window's owner to
decide. Worth a task when connections next get attention.

## References
- Design: `Configure.dc.html` LOCATION / TYPE / CONNECTION blocks + the `remote` branches of
  `SW.cfgView`. Spec: `docs/CONNECTIONS_SPEC.md` §4 (rewritten as-built). DEV_TASKS U14/W7.
- **P4-11** owns the window itself (`apps/configure/`, `platform/configure.rs`), the path list, the
  import options and the Save path — this task added a section and a branch, and changed nothing
  about how Save works.
