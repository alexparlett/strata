# P2-06 · Running state (spinner + elapsed + cancel)

**Phase:** 2 — Workbench · **Status:** ✅ **built** · **Depends on:** P2-02 · **Related:** P2-15 (Cancel)

> **Built to the comp** (`Strata.dc.html` running state): 30px `CircularLoader` (accent, already
> themed) · "Running query…" (Body role) · a live mono elapsed readout (Path role, ticking off an
> `async_io::Timer` loop — restarts per press since the body is keyed on the run nonce) · the
> error-tinted **Cancel · Esc** control — its own `cancel_button` theme component, authored per
> theme in `themes/*.json` with values that track `run_button`'s `running_*` set (the same cancel
> dress as P2-15's Run→Cancel flip; keep them in step when retuning either), schema regenerated.
> Esc is wired via `on_global_key_down` scoped to the running body's mount. One API correction:
> freya-query has **no** `query.cancel()` — Cancel is `engine.cancel(ws, tag)` (tag-guarded, S14)
> plus clearing the workbench `request` slot, which unmounts the body back to the empty state; the
> superseded entry settles `Err("cancelled")` unobserved. The datagrid's page-fetch state no longer
> borrows `Running` (a page read isn't a cancellable run) — it shows a bare centred spinner.

## Goal
A real "query running" body: spinner, elapsed time, and a Cancel affordance.

## Acceptance
- [x] Running a query shows the spinner + a live elapsed timer.
- [x] Cancel stops the run and returns to the previous/empty state.

## Freya / references
- Freya `CircularLoader` / `ProgressBar` / `Skeleton` (plan §5 DS map). Design: `Results.dc.html` running state.
