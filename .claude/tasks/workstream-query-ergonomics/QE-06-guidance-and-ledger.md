# QE-06 · Deep-JSON guidance + the upstream ledger

**Workstream:** Query ergonomics · **Status:** ✅ · **Depends on:** best after QE-01 (its
guidance names `to_json`); can land before it minus that line

## What landed (2026-08-14)

Step 4 came first, and it is why the other three read differently from the plan: the
reproductions were run against this build before a line of guidance was written, and **three
of the five inherited workarounds were wrong**. The ledger entries carry the corrections
(items 3, 4, 5, 7); the two that matter most are that `r['p']` does **not** rescue a
FROM-clause `UNNEST` alias — nothing addresses that alias, and the fix is to unnest in a
subquery's select list — and that `x || ''` is not "the only spelling that works" but the only
one that *strips the metadata*, which has to go on every branch that calls a json function
because the mismatch is between branches. `string_agg`'s own error names a working ordering.
Guidance therefore ships the select-list rewrite, not the bracket spelling.

`system.md` gained one *Large JSON schemas* section (casing, the struct family, the two
`UNNEST` rewrites, the recursive-CTE spelling, materialise-instead-of-fan-out). The `SET` line
is phrased as a card to **offer**: `Capability::Agent` refuses `SET`, so the assistant telling
the user to run one is the honest form. `describe_table`'s description gained the casing
sentence. `ENGINE.md` points at the ledger from the built-ins paragraph and states where the
survivors go when this folder is deleted.

## Goal

The workarounds and knobs that already exist become findable at the moment they're needed —
in the assistant's system prompt, the agent tool descriptions, and the docs — instead of
being rediscovered per user. Covers feedback items 9 (the casing trap has a knob already),
3 (materialise instead of fanning out), and the workaround spellings for upstream items
4, 5, 8. The upstream ledger itself is already written (workstream README); this task points
running-code surfaces at it and keeps the claims honest.

## Current state (verified 2026-08-13)

- Item 9 is a knob, not a gap: `datafusion.sql_parser.enable_ident_normalization`
  (`engine/config.rs:321`, default true, desc "Lower-case unquoted identifiers.") — offered
  in Settings ▸ Engine ▸ Properties **and** settable by typed `SET` (absent from
  `refuse_reserved_key`'s six refusing keys, `ddl/session.rs:539-556`); the language service
  follows it (`sql/resolve.rs:89-95`). The trap reads as arbitrary ("Field contentvariants
  not found") because all-lowercase paths work fine. Quoting (`n."contentVariants"`) is the
  zero-config answer.
- Item 3's real workaround exists since ED: an internal table (`CREATE TABLE t AS …`)
  materialises once into `.strata/tables/<slug>/`; the 96-branch UNION then reads one spool
  instead of re-scanning the source per branch. The **agent** cannot run CTAS
  (read-only, settled) — but the assistant can *offer* it (`offer_sql` validates under the
  editor's capability), which is the sanctioned path for "materialise this for me".
- The assistant's system prompt (`crates/strata-agent/src/assistant/system.md`) already
  carries the bounded-schema and session-cap notes (:33-37, :41-44) — guidance lives there,
  per-surface, not in a lore file.

## Build

1. `assistant/system.md` gains a short "large JSON schemas" section: quote mixed-case field
   names (or `SET datafusion.sql_parser.enable_ident_normalization = false` for the
   session); enumerate object-keyed structs with `struct_keys`/`struct_entries`, falling
   back to `to_json` + the json_* family where the shapes don't unify (QE-01); prefer
   offering a CTAS materialisation over building wide self-UNIONs; the
   `x || ''` spelling where a json function's output must unify in a recursive CTE; bracket
   access (`r['p']`) where dot access fails on an unnest alias. Terse, imperative,
   IDE-voiced — it is a prompt, not documentation.
2. Tool descriptions: `validate`'s description already routes squiggle-shaped problems; add
   the one-line casing hint to `describe_table`'s (field names are answered exactly as the
   file spells them; unquoted SQL identifiers lowercase by default).
3. Docs: `docs/reference/ENGINE.md`'s function-registry section points at
   `workstream-query-ergonomics/README.md`'s ledger while the workstream is open; when the
   workstream closes, the ledger's surviving upstream items move to a "known DataFusion 54
   limits" note wherever SETTLED_TASKS records the workstream (do not leave the only copy in
   a folder that gets deleted).
4. Verify each claimed workaround once against the current build (the ledger inherited them
   from field reports): the `SET` route for ident normalization, quoting, `x || ''`,
   bracket access on an unnest alias, CTAS-then-query. A workaround that doesn't reproduce
   is corrected in the ledger, not shipped as guidance.

## Acceptance

- Every guidance line traces to a verified reproduction; none contradicts a settled rule
  (the agent stays read-only; the assistant offers writes, never runs them).
- system.md stays lean — this adds one section, not a manual; if it can't be said in a
  sentence, it links nothing and gets cut (the model can rediscover exotic cases).
- Full check green (system.md changes ride through the assistant's existing snapshot tests
  if any pin the prompt).

## Files

`crates/strata-agent/src/assistant/system.md` · `crates/strata-agent/src/tools.rs` (tool
descriptions) · `docs/reference/ENGINE.md` · `.claude/tasks/workstream-query-ergonomics/README.md`
(ledger corrections, if any).
