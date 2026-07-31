# docs/reference — the detail behind CLAUDE.md and AGENTS.md

[CLAUDE.md](../../CLAUDE.md) is the map and [AGENTS.md](../../AGENTS.md) is the rules, both kept
short because they are loaded into every session. This directory holds what they used to carry
inline: the module map, and the reasoning behind each rule. It is loaded **on demand** — read the
file that covers the area you are working in before you start.

| File | What it holds | Read it when |
|---|---|---|
| [MODULE_MAP.md](MODULE_MAP.md) | the annotated `strata-freya` tree | locating code, or placing something new |
| [INVARIANTS.md](INVARIANTS.md) | AGENTS §2 in full | touching engine, query, snapshot, catalog, history, windows, config, agent access |
| [FREYA_UI.md](FREYA_UI.md) | AGENTS §3–§4 in full | any Freya UI work, or deciding where state lives |
| [ENGINE.md](ENGINE.md) | the DataFusion boundary, SQL/DDL policy, the function registry | working at the engine seam |
| [WORKFLOW.md](WORKFLOW.md) | AGENTS §6–§7 in full | editing the fork, or anything git / CI / release |
| [BAR.md](BAR.md) | AGENTS §1 in full | when a rule of thumb needs its reasoning |
| [SETTLED_TASKS.md](SETTLED_TASKS.md) | what each finished task settled, and the corrections it carried | a surface's shape looks arbitrary, or you are about to re-litigate a decision |

**The one-liner and the full entry share a bolded lead sentence**, verbatim — so a rule in AGENTS.md
greps straight to its reasoning here. Keep that true: when a review settles or overturns a
convention, update both in the same change.
