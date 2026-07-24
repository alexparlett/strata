# `loadfix` — project-load test fixture

A self-contained, throwaway project folder for `tests/project_load.rs`. It stands in for
the app's real `sample/` project so that project-load acceptance tests never depend on the
live sample data — in particular, it can carry a **deliberately-broken source** without
wedging a malformed file into a project people actually use.

All data is plain text (CSV + JSON) so the fixture is diffable and needs no binary blobs:

- `users.csv`, `regions.csv` — flat CSV tables.
- `events/year=…/month=…/data.csv` — a **Hive-partitioned** CSV table (two partition
  columns, `year` and `month`), so registration exercises partition discovery.
- `signups.json` — the **deliberate dud**: pretty-printed (multi-line) JSON, which
  DataFusion's JSON format (NDJSON reader) can't parse. Registering it fails, which is the
  point — the test asserts exactly this one table lands in the Failed state while the rest
  of the project still loads. Do **not** "fix" it to valid NDJSON.
- `.strata/project.json` — the catalog defs (tables, views, saved queries).

The views (`active_users`, `revenue_by_region`) join across the tables so the test can
plan them and query through one.
