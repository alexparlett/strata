# P3-07 · Registration failure messages

**Phase:** 3 · **Status:** ✅ · **DEV_TASKS:** D9 · **Depends on:** P3-04

## Goal
Every registration failure the catalog can show says what actually went wrong, in the app's own
register. This is the whole of what's left of D9.

## Current state

### Why this task changed shape
It was *"PART badges · nested JSON · shape detection"*. Two of those three shipped with **P3-02**,
and the third dissolved on inspection.

- **PART chips — done.** `flatten_cols` sets `is_part` from the owner's partition columns, top level
  only (`catalog/columns.rs`, with a unit test that a nested field of the same name is *not*
  flagged); the row renders `Badge::tag("PART", …)` (`catalog/entry.rs`), and
  `catalog/interaction.rs` asserts it. The chain is exercised end to end by the sample project:
  `sample/.strata/project.json` declares `events` with
  `partition_cols: [["year","Utf8"],["month","Utf8"]]` over a real Hive tree
  (`sample/events/year=2024/month=01|02/data.parquet`). DataFusion appends the partition fields to
  the table schema under **exactly** the def's names (`datafusion-datasource/src/table_schema.rs`,
  `TableSchema::new`), so both sides of the name compare come from the same string in
  `project.json` — no case folding needed here, unlike view deps, which come back from the planner.
- **Nested columns — done.** `flatten_cols` descends struct/list/map to any depth, chevrons and all,
  with per-`Kind` dots and `ColRef { kind, owner, path }` identity. The one nested gap left — a
  *nested* column populating the inspector — is **P3-08**'s, and its file already claims it.
- **"Parseable-JSON echo for nested columns"** had no referent in `strata-core`, `strata-dioxus`, or
  the design bundle. It was a garbled compression of the handoff `FEATURES.md` §7 (below), which is
  about *registration*, not nested columns. Nothing to build.

### The pre-flight consistency report is not being built
`FEATURES.md` §7 asked for **JSON shape detection** (NDJSON vs whole-document) and a
**schema-consistency check** *before* committing to one table (`100 files · schema consistent ✓` /
`97 match · 3 have an extra column`). Both are dropped, for two reasons:

1. **The register already is the check.** DataFusion fails registration for every shape it can't
   read; the row lands `Reg::Failed(err)` and **P3-04** renders the reason as a triangle with a
   tooltip and an `a11y_alt`. There is nothing to detect ahead of time, and a pre-flight pass would
   mean listing every file and inferring each one's schema separately — real cost, to answer a
   question the register answers for free.
2. **§7 contradicts §6 of the same document.** §6 walked the up-front counting back on purpose:
   source paths show their **shape** (`file` / `directory` / `glob`), *"**not** a file count"*, the
   summary reads `N path(s) ready · files resolved at query time`, and the fabricated stage stepper
   was deleted. `DEV_TASKS` D7 records the same ("Counts/stepper already honest"). §7 is the older
   claim that survived in the file.

### What is actually broken: the messages
Measured against the real engine (`Engine::register`, DataFusion 54, `tests/fixtures/loadfix`):

| source | today's message |
|---|---|
| pretty-printed / multi-line JSON objects | `Arrow error: Json error: Not valid JSON: EOF while parsing an object at line 1 column 1` |
| top-level array document `[{…},{…}]` | `Arrow error: Json error: Expected JSON record to be an object, found Array [Object {"a": Number(1)}, …]` |
| `.csv` path under a `parquet`-typed table | `Error during planning: No files found at file:///…/regions.csv. Cannot infer schema from an empty location; either add data files or declare an explicit schema for the table.` |

Each is wrong in its own way. The first says "not valid JSON" about a file that **is** valid JSON
(the reader is line-delimited) and points at a line:column that means nothing. The second
**interpolates the parsed document into the error string** — two elements in the probe, the whole
value in general, and that string is rendered as a tooltip *and* an a11y label. The third claims the
location is empty when the file plainly exists; it just didn't match the format's extension filter.

## Build
Map registration failures to honest messages at the **one** place registration returns `Err` —
`register_external` in `strata-core/src/engine/catalog.rs` — so every caller inherits them: the
whole-catalog ↻ (P3-03), a row's Refresh (P3-06), project open, and the config modal when it lands
(P4-11 / P4-12). Not in the UI, and not per-caller.

1. **Recognise the JSON read shapes.** A file that parses as JSON but isn't one record per line
   (pretty-printed objects, a top-level array, a single document) says so, and says what the reader
   wants. Both Arrow spellings above are the same story to a user.
2. **Never embed file contents.** No parsed value, no row, no document in a message, and bound the
   length of anything passed through — `Reg::Failed(String)` reaches a tooltip and an a11y label.
3. **Format/path mismatch is its own message.** Name the extension the format looks for rather than
   claiming an empty location (`register_external` filters `.parquet` / `.csv` / `.json` /
   `.arrow`). This is the `FEATURES.md` §6 error case.
4. **Anything unrecognised passes through unchanged.** The map translates known failures, it never
   swallows — an unfamiliar DataFusion error reaching the user verbatim is the correct outcome, not
   a gap to paper over.
5. **Register per AGENTS §3:** terse plain sentences, single-quoted identifiers, no em dashes,
   backticks, ellipsis or hedges. Point at the surface that fixes it where there is one.

## Acceptance
- [x] Each of the three measured cases yields a specific message naming the real cause; an
      unrecognised failure passes through unchanged.
- [x] No message embeds parsed file contents, and message length is bounded.
- [x] Unit tests in `strata-core` over the mapping (15, in `engine/catalog.rs`), each keyed to a
      **measured** engine string so a DataFusion upgrade that rewords a failure fails the test
      rather than silently reverting that arm to pass-through. Every fake error is built through
      `listing_url`, because DataFusion names the failing source as the URL it built — hand-writing
      a bare path would bypass the recovery lookup and test the fallback instead.

## As built
`register_error` + `json_shape_error` / `no_files_error` in `engine/catalog.rs`, applied to all three
fallible steps of `register_external`. `listing_url` was factored out of the URL loop so the messages
resolve a path exactly the way registration does — that's what lets a multi-path table name the
source that actually failed instead of the first one.

The delivered messages:

| case | message |
|---|---|
| pretty-printed / multi-line / truncated JSON | `Cannot read 'signups' as JSON: a record does not end on its line. JSON sources must be newline-delimited, one record per line.` |
| top-level array | `Cannot read 'signups' as JSON: the source is a JSON array. JSON sources must be…` |
| other top-level value | `Cannot read 'nums' as JSON: a top-level Number is not a record. JSON sources must be…` |
| genuine JSON syntax error | `Cannot read 'bad' as JSON: key must be a string at line 1 column 9` |
| path absent | `No source at '/x/nope.parquet'.` |
| file, wrong extension | `Table 'regions' reads .parquet files, and '/x/regions.csv' is not one.` |
| directory, nothing readable | `No .parquet files under '/x/mixed'.` |
| directory, files filtered by partitions | `No .csv files under '/x/lake' match the partition columns 'year'.` |
| glob / object store | `No files matched '/x/**/*.parquet'.` |
| anything else | DataFusion's own text, capped at 240 chars |

### What building it settled
- **A truncated file and a pretty-printed one are indistinguishable from the error alone** — both
  come back `Not valid JSON: EOF while parsing an object`, differing only in a column number. They
  share one message, and it is true of both: a record doesn't end on its line. A *syntax* error
  (`key must be a string at…`) is a different arm and keeps Arrow's line:column, because rewriting
  that into "must be newline-delimited" would be the same confident-wrong diagnosis this task exists
  to delete.
- **DataFusion pairs partition columns to directory levels positionally, not by name.** Declaring
  `event_id` over a `year=2024/month=01/` tree registers fine, as does declaring two columns over a
  flat directory or one over a two-level tree. So partition *depth* and *name* mismatches are not
  reachable failures. The case that **is** reachable — and the reason the partition arm exists — is
  an **unkeyed** directory: files under `2024/` where a Hive partition needs `year=2024/`. Every file
  is filtered out and DataFusion calls the location empty with the data sitting right there.
- **"No files found" covers five causes**, four of which are not an empty location: absent path,
  extension mismatch, unreadable directory, partition filtering, and genuine emptiness. Telling them
  apart needs a local `exists()`/`is_file()` check plus a bounded directory walk (`holds_ext`, first
  hit wins, 4096 entries) — cheap, and only ever on the failure path. Globs and `://` paths can't be
  resolved on disk and get only what's certain: nothing matched.
- **The walk is gated on partitions, and its budget is tri-state.** With no partition columns nothing
  was filtered, so DataFusion's empty listing is trustworthy and the directory can be called empty
  without looking — which also keeps the walk off the common failure. And `holds_ext` returns
  `Option<bool>`, not `bool`: a lake big enough to exhaust the budget is indistinguishable from an
  empty directory if the answer collapses to `false`, and the caller turns that into a claim about
  the user's data. "I stopped looking" must not become "there is nothing here" — the exact
  false-absence this task removed from DataFusion's own wording.

## Freya / references
- `strata-core/src/engine/catalog.rs` (`register_external`); `Reg::Failed` + P3-04's status slot are
  the consumers that exist today.
- Handoff `FEATURES.md` §6 (the error cases, and the honesty rules that killed §7's pre-flight).
- **Wire into P4-11 / P4-12:** the config modal shows these same messages on a failed Register. It
  must not grow a second set — noted in P4-11's file.
