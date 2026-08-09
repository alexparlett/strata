# The engineering bar, in full

AGENTS §1 with its reasoning. [AGENTS.md](../../AGENTS.md) §1 carries the one-line form of each;
the bolded lead sentence there is verbatim the bolded lead here, so it greps.

- **Generic capability, not hardcoded subsets.** Build the real, general mechanism, not a tactical
  stub that happens to pass the current case.
- **Real end-states, not placeholders.** No TODO scaffolding left as the deliverable. (The one
  sanctioned exception is a deliberately **inert control** whose capability another task owns — §5.)
- **Native Rust tooling, not stray scripts.** Schema/codegen/tests live in the crate (e.g. the
  `schema_in_sync` test), not one-off Python.
- **Verify from source before agreeing.** If Alex asserts an API or behaviour, check it in the fork
  (`crates/freya/`) or the crate before confirming; correct it if it's wrong. Same bar for your own
  claims: don't enshrine or restate an API you haven't just looked at.
- **Framework-native idiom — never pattern-carrying.** Find the Freya/freya-query native shape
  first (fork examples, Valin) and build to that: no adapters, echo fields, parallel ids, or
  compatibility shims to keep old shapes alive ("I don't want to keep a pattern that worked with
  dioxus for the sake of it"). Prefer widening a native id over introducing a mapping. The Dioxus
  app this rule guarded against has been deleted; its patterns (`GlobalStore`, `dispatch`/`action`,
  the `Command`/`Event` protocol) stay gone.
- **Model impossible states out of existence; fail loud on the rest.**
  - A project can't exist without a folder, so `ProjectState.root` is `PathBuf` (not `Option`), has
    no `Default`, and is only built full from load/scaffold. Don't thread `Option`s or blank
    fallbacks to paper over failures.
  - Expected absences get defaults (missing session file → one blank tab; missing `.strata/` →
    scaffold). Unrecoverable faults (unopenable project dir, unparseable defs, unreadable
    session) are **surfaced**, never a silent blank/rootless fallback: pre-launch resolution
    reports and skips, and a fault at mount renders `ProjectLoadFailed` — the fault dialog
    offering Try again (a generation bump that re-runs the load) and Close window through
    `close_this_window` (P4-01 item 5; the fallible IO runs once in `ProjectRoot` and decides
    which arm the subtree is — `ProjectLoaded` or the fault — so no store is ever built from
    anything but a successful load).
  - Never shape a production signature (or add an `Option`) to satisfy a test — build the test's
    store inline instead. Pull deps like the project root from context
    (`use_radio_station::<ProjectState>`), not params-for-tests.
- **No over-engineering.** Private/internal app: use `pub` freely, don't hand-annotate visibility
  per field on struct-literal-built components (the linter widens them back anyway).
- **A path is qualified in the `use` and nowhere else.** Import the **item** and refer to it by its
  bare name; a qualified path in a signature, a body or a match arm is the smell, and so is
  importing a *module* to qualify through — `use crate::platform::{self, WindowKind}` plus
  `platform::open_export(…)` is the same rule broken, one step shorter. Not tidiness: the import
  block is the one place a reader checks what a file actually depends on, and a path spelled inline
  is a dependency that isn't listed there — which is how one item ends up reached three different
  ways in a single file (`crate::components::form::form_theme()` beside a `use` of the same module
  elsewhere). It also removes a class of bad call site outright: `platform::open_export(platform
  .clone(), launch)` reads as one name meaning two things, because the module and the local
  `Platform` are both spelled `platform`; `open_export(platform.clone(), launch)` cannot. The
  anchor is unchanged and unlegislated — `use super::` for a sibling, `use crate::` across the tree,
  both are in use here. Three things are **not** covered, because they are not shortenable code:
  a visibility modifier (`pub(in crate::apps::project::views::workbench)`), a rustdoc intra-doc link
  (`[`Subtree`](crate::platform::owner::Subtree)` — the full path *is* the link target), and the
  handful of `std` aliases whose module segment is what disambiguates them (`io::Result`,
  `fmt::Result`, `fs::write`; a bare `use std::io::Result` shadows the prelude). On a genuine
  collision between two of our own names, alias with `as` — never fall back to a reach through the
  crate root.
- **Valin-shaped.** Follow [`marc2332/valin`](https://github.com/marc2332/valin) (the Freya
  author's own IDE) for module layout, per-window data scoping, and stateful tabs.

