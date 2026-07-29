# `loadfix` — project-load test fixture

A self-contained, throwaway project folder for `tests/project_load.rs`. It stands in for the app's real `sample/`
project so that project-load acceptance tests never depend on the live sample data — in particular, it can carry a
**deliberately-broken source** without wedging a malformed file into a project people actually use.

All data is plain text (CSV + JSON) so the fixture is diffable and needs no binary blobs:

- `users.csv`, `regions.csv` — flat CSV tables.
- `events/year=…/month=…/data.csv` — a **Hive-partitioned** CSV table (two partition columns, `year` and `month`), so
  registration exercises partition discovery.
- `signups.json` — the **deliberate dud**: the second record is missing its closing brace, so the file is not valid
  JSON at all. Registering it fails, which is the point — the test asserts exactly this one table lands in the Failed
  state while the rest of the project still loads. Do **not** "fix" it.

  It is still *pretty-printed* (multi-line), and that alone **used to be** what broke it: arrow's line-based NDJSON
  reader could not parse a record spanning several lines. That stopped being a dud when `engine::json_poly` replaced
  arrow's JSON reader — it reads records with serde's `StreamDeserializer`, which takes whitespace-separated values
  rather than one per line, so multi-line records now read correctly. A capability gain with nothing regressed, but
  it left this fixture readable, and the Failed path needs a source that genuinely cannot be read. Hence the missing
  brace: a syntax error no reader can rescue, on a file whose *shape* is now perfectly fine.
- `.strata/project.json` — the catalog defs (tables, views, saved queries).

The views (`active_users`, `revenue_by_region`) join across the tables so the test can plan them and query through one.
