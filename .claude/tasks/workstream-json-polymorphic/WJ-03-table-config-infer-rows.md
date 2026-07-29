# WJ-03 · Table Config silently caps JSON schema inference

**Workstream:** JSON · **Status:** ✅ · **DEV_TASKS:** — · **Depends on:** WJ-02

## The bug
`crates/strata-freya/src/apps/configure/model.rs:396` writes `infer_rows` **unconditionally**:

```rust
infer_rows: Some(self.json_infer_rows.max(1) as usize),
```

seeded from `DEFAULT_INFER_ROWS = 1000` (`model.rs:31`) whenever the def has `None`
(`model.rs:217`). So the reader's deliberate **no-default-cap** design
(`json_poly::format::infer_schema`) is unreachable for any table ever saved through the UI.

## Why it matters
`infer_rows: None` means "read every record", and WJ-02 chose that default precisely to close a
trap: a conflict that first appears at record 1001 must fail at **registration**, not mid-query.
With a silent cap of 1000 the trap is back —

1. user opens Table Config on an uncapped JSON table just to look at the shape;
2. the field displays `1000` for a def that was actually unbounded;
3. Save writes `Some(1000)`;
4. inference now sees 1000 clean records and types `content` as `Struct`;
5. registration succeeds, the catalog row is green, and **every** `SELECT` fails at scan with
   arrow's `expected string got {...}` — the case `normalize::fit`'s doc calls "reachable only when
   a sampled inference missed a conflict".

Nothing the user did looks like a change. They opened a dialog and pressed Save.

## The fix — shipped
**0 is the "scan every record" sentinel**, and the pane writes `None` for it.

- Seed: `o.infer_rows.unwrap_or(0)`, so an unset def opens showing 0 rather than a fabricated 1000.
- Commit: `(self.json_infer_rows > 0).then_some(..)`.
- The control's `min` drops from 1 to 0, and the hint says what 0 does.

A sentinel rather than threading `Option` through the draft because `Control::Num` is a plain
number field and JSON had a spare value: `Some(0)` is refused by the engine outright (it would
infer a schema with no columns), so 0 was unreachable as a *quantity*. CSV already spends its 0 the
same way, on "read every column as text", so the pane keeps one convention rather than two.

The engine's `Some(0)` refusal stays as the guard for a hand-edited `project.json`; the two
compose — the UI can no longer send it, and the engine still declines it.

## Related, same file — done
`strata-model::JsonRead::infer_rows`'s doc now describes `PolyJsonFormat` (`None` = scan
everything, `Some(0)` refused) instead of `JsonFormat`.

**CSV is deliberately untouched.** `csv_infer_rows` writes `Some(1000)` for an unset def too, but
CSV's engine default *is* 1000, so that round trip preserves behaviour rather than silently
capping. A fidelity nit, not the bug this task is about.

## Acceptance
- Opening Table Config on a JSON def with `infer_rows: None` and pressing Save leaves it `None`. ✅
  (`an_unbounded_json_def_survives_a_no_op_save`)
- The pane can express "all records" and shows it for an unset def. ✅
- `JsonRead::infer_rows`'s doc describes `PolyJsonFormat`'s behaviour. ✅
- The stale test is replaced (`json_zero_infer_rows_means_scan_everything`). ✅
