# AA-07 · Tools that page or filter, and truncation that names a real recovery

**Workstream:** Agent access · **Status:** ⬜ · **Depends on:** AA-02 (built), AS-02 (built)

## Goal

Four of the ten tools answer with a list that has no bound but the user's data, and the
assistant's loop caps a tool result at 24,000 bytes — so it cuts them positionally and tells the
model to recover in a way three of the four cannot. Give the list-shaped tools a way to answer
the question the model actually asked, and make a truncation name a recovery that exists.

## Current state, measured

Against the shipped registry and realistic data, serialized exactly as `dispatch::encode` does,
against `turn::MAX_TOOL_RESULT` (24,000):

| tool | overflows | measured |
|---|---|---|
| `list_functions` | **always** | 63,729 B — **2.66x**, and fixed: it does not depend on the user's data at all |
| `run` / `read_page` | past ~330 rows | 100 rows x 8 cols = 7,151 B (0.30x); **`MAX_PAGE_SIZE` (10,000) = 811,756 B, 33.8x** |
| `describe_table` | past ~90 columns | 100 cols with statistics = 27,321 B; 400 cols = 109,221 B (4.55x); 1,000 cols = 11.4x |
| `list_tables` | past ~170 tables | 200 tables = 27,993 B (1.17x); 1,000 tables = 140,793 B (5.87x) |

`list_projects`, `validate` and the three session tools are bounded small and are not in scope.

Where `list_functions`' bytes sit, since it is the one that overflows for everybody: descriptions
33,994 (53%), signatures 10,339, JSON structure ~15,500, names 3,070, returns 872. **Dropping
descriptions entirely still lands at ~25,000** — over the cap. Enumerating 319 functions does not
fit at any useful level of detail, which is why this is a shape change and not a trim.

## The two defects

**A truncation names a recovery the tool does not have.** `turn::bounded` ends every cut result
with "Read the rest with read_page, or run a narrower query." That is true for `run` and false
for the other three: there is no snapshot behind a function list, a catalog listing or a table
description, so `read_page` answers not-found and the model has spent a round learning nothing.

**A cut list is a prefix, not a sample.** All three truncate positionally, so a 400-column table
describes as columns 1-90 and the model reasons about that as the schema. The `truncated: true`
flag `bounded` now emits is the only thing standing between that and a confidently wrong answer.

## What to build

**One narrowing mechanism across the list-shaped tools, not three ad-hoc ones** (AGENTS.md §1:
generic capability, not hardcoded subsets). The recommended shape, to be confirmed against the
code rather than settled here:

- An optional `matching` substring on `list_functions`, `list_tables` and `describe_table` (over
  column names for the last), plus optional paging where a filtered answer can still be large.
- **Every narrowed answer states its total.** A model cannot tell "the 12 date functions" from
  "12 of the 40 date functions" unless the answer says so, and a filter that silently truncates
  is the same defect one level in.
- `describe_table` is the awkward one: it is one object with a list inside, not a list, and its
  columns nest (`ColumnWire::children`), so a page boundary can fall inside a struct column.
  Decide against the code and name the reason where it lands.

**`run`'s half is the assistant's, not the vocabulary's.** `MAX_PAGE_SIZE` (10,000 rows) is
right for an MCP client, which decides for itself what to do with 811 KB, and 34x wrong for a
conversation that must carry the result forever. So the ceiling belongs on the assistant's
`Scope` — the assistant asks for less — rather than on the tool, or every MCP client is degraded
to solve an assistant problem. The shipped default (`row_limit` 100) is already comfortable; it
is the model *asking* for more that overflows.

**Then `turn::bounded`'s wording is this task's to make true**, or to make unnecessary. Once each
tool has a narrowing, the cut note should name that tool's own recovery. A single sentence that
is right for one tool in four is what this task exists to remove.

## Acceptance

- The unfiltered call to each of the three list tools returns a **complete** answer under the
  cap, or an answer that states what it left out and how to reach it — never a positional prefix
  whose only tell is a flag.
- A `matching` call returns the matches with full detail, and the total it matched against.
- A conversation can ask "does `date_trunc` exist and what does it take" and get an answer, in
  one round, on a project with the full 319-function registry.
- A 400-column parquet table can be described usefully by an assistant turn.
- Every truncation note names a recovery that tool actually offers; `read_page` is named only
  where a snapshot exists.
- The assistant cannot ask for a page it cannot hold; an MCP client still can (`MAX_PAGE_SIZE`
  unchanged on the wire).
- `tests/facade.rs`'s manifest assertions still pass — a new optional parameter is not a new
  tool, and the ten stay ten.

## What is NOT this task

- **Raising `MAX_TOOL_RESULT`.** A 63 KB result is re-sent every round *and every later turn*,
  and a `Conversation` cannot be trimmed. The cap is the point.
- Profiling, statistics computation, or anything that scans — `describe_table` reports what
  registration read for free and that does not change.
- A second results pipeline. `run`/`read_page` already page a snapshot; nothing here duplicates
  that.
- The chat pane's rendering of any of this (AS-04).

## Notes

- Both halves were found by the second adversarial pass over AS-02 (2026-08-10), which fixed the
  cut result's *shape* (it used to be sliced mid-object into unparseable JSON) without noticing
  that three of the four tools it cuts have nowhere to go afterwards.
- The measurements above are reproducible: build the wire values at scale and
  `serde_json::to_string` them, which is what `dispatch::encode` does.
