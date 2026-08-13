//! `describe_table`'s bounded schema projection (AA-07) — the walk, the path drill-down and
//! the name search over a def's `ColumnInfo` tree.
//!
//! A schema is not small — the reference fixture infers to 19 top-level columns carrying 241,425
//! nested fields at depth 13 — so the discipline is the value encoder's, restated for a schema
//! tree: a byte budget decides whether an answer is cut, width sampling decays with depth when it
//! is, and every elided set is replaced by a stated count. The convention the whole answer keeps:
//! **a describe answer with no counting fields in it is a complete answer.**
//!
//! The constants are this module's own rather than the value encoder's: same discipline, different
//! budgets, and a shared constant would couple two surfaces that tune independently.
//!
//! In `strata-agent` rather than `strata-core` because its output is [`ColumnWire`], a wire shape
//! core must not know; beside `wire.rs` rather than in it because the walk plus the search plus the
//! budget loop is an algorithm with its own tests.

use serde_json::to_string;
use strata_model::ColumnInfo;

use crate::error::AgentError;
use crate::host::Described;
use crate::wire::{
    name_matches, windowed, ColumnWire, DescribeResult, DescribeTableParams, EntryKindWire,
    MatchWire, PartitionWire, StatWire, StateWire,
};

/// The deepest rung of the sampling ladder ([`bounded_forest`]) — engaged only once the
/// complete rendering is past the budget. A fixed depth alone was measured and rejected: at
/// depth 3 with the width decay below, the fixture's 19,311-key struct emits ~26 KB from one
/// column, and even depth 1 grazes the cap on UUID-named keys — which is why the ladder
/// retries shallower and depth 0 is the unmeasured floor.
const SCHEMA_DEPTH: usize = 3;

/// How many children a container shows at `level` levels below the walk root — 15, then 7,
/// then 3. The value encoder's decay, restated: depth exists to show shape, and a deep
/// level's job is done by a sample. The `min` guards the shift the way `items_at` does,
/// so a raised `SCHEMA_DEPTH` cannot turn the decay into a shift overflow.
fn schema_items(level: usize) -> usize {
    (30 >> level.min(usize::BITS as usize - 1)).max(3)
}

/// The walk root's direct children shown per page. A page rather than a sample, because the
/// top level is what the caller is here to read and 'page' reaches the rest; 50 stat-heavy
/// parquet columns measure ~14-17 KB, inside the budget with the facts beside them.
pub const SCHEMA_PAGE: usize = 50;

/// The match-list page — half [`SCHEMA_PAGE`], because a match carries its whole path: at
/// the fixture's worst (13 segments of UUID-length names, ~550 bytes a row) 25 rows are
/// ~14 KB, inside the budget, where 50 were measured past the whole result cap.
pub const MATCH_PAGE: usize = 25;

/// Bytes the columns portion may serialize to. Short of the assistant's whole 24,000-byte
/// result cap because the envelope also carries the table's facts — and, for a view, its
/// whole SQL. Measured with the same `serde_json::to_string` the dispatch encodes with, so
/// the measurement is the truth.
const SCHEMA_BUDGET: usize = 16_384;

/// The fewest bytes one rendered column can serialize to (`{"name":..,"dtype":..,
/// "kind":..,"nullable":..}` with one-character names) — what lets a window's node count
/// prove the complete rung cannot fit **before** the tree is built.
const NODE_FLOOR: usize = 55;

/// Source paths a describe answer lists before the count stands in. Far looser than
/// `list_tables`' three — this answer is where that elision points — but not unbounded: a
/// def registered over thousands of parts would otherwise spend the whole result cap on
/// paths, and no parameter of this tool reaches them.
const SOURCES_FULL: usize = 100;

/// Project a [`Described`] into the wire answer, walked as `params` asks.
///
/// Fallible and parameterized, which is why this is a function rather than the `From` it
/// replaced: a 'path' that resolves nowhere is a refusal. The Failed and Pending arms ignore
/// the walk parameters entirely — the state is the answer, and a path refusal on a table
/// that exists but has no schema yet would be a lie.
pub fn describe_result(
    described: Described,
    params: &DescribeTableParams,
) -> Result<DescribeResult, AgentError> {
    match described {
        Described::Table {
            name,
            format,
            sources,
            partitions,
            rows,
            columns,
        } => {
            let (sources, sources_total) = bounded_sources(sources);
            Ok(DescribeResult {
                kind: Some(EntryKindWire::Table),
                format: Some(format),
                sources,
                sources_total,
                partitions: partitions
                    .into_iter()
                    .map(|(name, dtype)| PartitionWire { name, dtype })
                    .collect(),
                rows,
                ..schema_view(name, &columns, params)?
            })
        }
        Described::View {
            name,
            sql,
            columns,
            reads,
        } => Ok(DescribeResult {
            kind: Some(EntryKindWire::View),
            sql: Some(sql),
            reads,
            ..schema_view(name, &columns, params)?
        }),
        Described::Failed { name, error } => Ok(DescribeResult {
            error: Some(error),
            ..blank(name, StateWire::Failed)
        }),
        Described::Pending { name } => Ok(blank(name, StateWire::Pending)),
    }
}

/// The all-empty answer every arm builds on — a failed def has no schema, and the
/// projection must not invent one.
fn blank(name: String, state: StateWire) -> DescribeResult {
    DescribeResult {
        name,
        state,
        kind: None,
        error: None,
        format: None,
        sources: Vec::new(),
        sources_total: None,
        sql: None,
        partitions: Vec::new(),
        rows: None,
        columns: Vec::new(),
        columns_total: None,
        reads: Vec::new(),
        matches: Vec::new(),
        matched_total: None,
        page: None,
        page_size: None,
    }
}

/// The source list, elided past [`SOURCES_FULL`] with its total stated.
fn bounded_sources(mut sources: Vec<String>) -> (Vec<String>, Option<usize>) {
    let total = sources.len();
    sources.truncate(SOURCES_FULL);
    (sources, (total > SOURCES_FULL).then_some(total))
}

/// Resolve the walk root, then search or walk under it. Answers as a ready
/// [`DescribeResult`] carrying only the schema portion; the caller's arm lays its facts
/// over it by struct update.
fn schema_view(
    name: String,
    columns: &[ColumnInfo],
    params: &DescribeTableParams,
) -> Result<DescribeResult, AgentError> {
    let path = params.path.as_deref().unwrap_or(&[]);
    let node = match path {
        [] => None,
        path => Some(resolve(columns, path).ok_or_else(|| no_such_column(&name, path))?),
    };
    let forest = node.map_or(columns, |n| n.children.as_slice());
    let answer = blank(name, StateWire::Ready);
    if let Some(needle) = params.matching.as_deref() {
        return Ok(searched(forest, path, needle, params.page, answer));
    }

    let w = windowed(forest.iter().collect(), params.page, SCHEMA_PAGE);
    let rendered = bounded_forest(&w.shown);

    Ok(match node {
        None => DescribeResult {
            columns: rendered,
            columns_total: w.page.map(|_| w.total),
            page: w.page,
            page_size: w.page_size,
            ..answer
        },
        Some(node) => DescribeResult {
            columns: vec![ColumnWire {
                name: node.name.clone(),
                dtype: node.dtype.clone(),
                kind: node.kind.into(),
                nullable: node.nullable,
                children_total: w.page.map(|_| w.total),
                children: rendered,
                stats: node.stats.iter().map(StatWire::from).collect(),
            }],
            columns_total: None,
            page: w.page,
            page_size: w.page_size,
            ..answer
        },
    })
}

/// The windowed forest, rendered as deep as the budget allows.
///
/// The first rung is the **whole** subtree — the budget, not a depth, decides whether an
/// answer is cut, so a small schema stays complete however deep it nests and carries no
/// totals at all. That rung is guarded by a node count against [`NODE_FLOOR`]: a window
/// whose nodes already outnumber the budget at the floor provably cannot fit, and building
/// a quarter-million-node tree just to measure it was this module's own founding defect.
/// Only past the budget does the ladder engage: attempted depth with width sampling,
/// retried shallower, and depth 0 — the shown level with every child elided to a count —
/// accepted unmeasured, so there is always something to show.
fn bounded_forest(window: &[&ColumnInfo]) -> Vec<ColumnWire> {
    if plausibly_complete(window) {
        let full: Vec<ColumnWire> = window.iter().map(|c| ColumnWire::from(*c)).collect();
        if fits(&full) {
            return full;
        }
    }
    for depth in (1..=SCHEMA_DEPTH).rev() {
        let rendered: Vec<ColumnWire> = window.iter().map(|c| subtree(c, 1, depth)).collect();
        if fits(&rendered) {
            return rendered;
        }
    }
    window.iter().map(|c| subtree(c, 1, 0)).collect()
}

/// Whether the window's node count leaves the complete rendering any chance of fitting.
fn plausibly_complete(window: &[&ColumnInfo]) -> bool {
    let mut allowance = SCHEMA_BUDGET / NODE_FLOOR;
    window.iter().all(|c| counted_within(c, &mut allowance))
}

/// Count `col` and its subtree against `allowance`, giving up the moment it runs out.
fn counted_within(col: &ColumnInfo, allowance: &mut usize) -> bool {
    if *allowance == 0 {
        return false;
    }
    *allowance -= 1;
    col.children.iter().all(|c| counted_within(c, allowance))
}

/// One column with `depth` levels of children below it, each level a sample with its total
/// stated where it shows fewer.
fn subtree(col: &ColumnInfo, level: usize, depth: usize) -> ColumnWire {
    let total = col.children.len();
    let children: Vec<ColumnWire> = if depth == 0 {
        Vec::new()
    } else {
        col.children
            .iter()
            .take(schema_items(level))
            .map(|c| subtree(c, level + 1, depth - 1))
            .collect()
    };
    ColumnWire {
        name: col.name.clone(),
        dtype: col.dtype.clone(),
        kind: col.kind.into(),
        nullable: col.nullable,
        children_total: (total > children.len()).then_some(total),
        children,
        stats: col.stats.iter().map(StatWire::from).collect(),
    }
}

/// Whether a rendering serializes inside the budget.
fn fits(rendered: &[ColumnWire]) -> bool {
    to_string(rendered).is_ok_and(|s| s.len() <= SCHEMA_BUDGET)
}

/// Walk a path of names into the tree — the inspector's own resolve, restated over the same
/// `ColumnInfo` tree the walk prints. That sameness is what closes the vocabulary: a
/// segment is a name a previous answer showed, exactly as the file spells it (a List child
/// is named whatever the file's schema says — 'item', 'element' — and a Map's synthetic
/// entries level was already skipped when the tree was built). Duplicate sibling names
/// resolve to the first, as the inspector does.
fn resolve<'c>(columns: &'c [ColumnInfo], path: &[String]) -> Option<&'c ColumnInfo> {
    let (first, rest) = path.split_first()?;
    let node = columns.iter().find(|c| &c.name == first)?;
    if rest.is_empty() {
        Some(node)
    } else {
        resolve(&node.children, rest)
    }
}

/// The refusal for a path that resolves nowhere. The path is rendered as the JSON array the
/// parameter accepts back — never dot-joined, because names come from the user's files and
/// may contain dots — and the recovery is this tool's own, not a listing tool's.
fn no_such_column(table: &str, path: &[String]) -> AgentError {
    let shown = to_string(path).unwrap_or_default();
    AgentError::NotFound(format!(
        "No column {shown} in '{table}'. Call describe_table without 'path' to see the \
         schema, or with 'matching' to find a field by name."
    ))
}

/// The 'matching' answer: every field whose name contains the needle, as paths, one
/// [`MATCH_PAGE`] window at a time — with the total stated even at zero, so an empty answer
/// cannot read as an unsearched one.
///
/// The search streams: it counts every hit but materializes a path only for the hits inside
/// the window, because a broad needle over the fixture matches ~19k fields and cloning a
/// 13-segment path for each of them, to answer with 25, is the same build-and-discard the
/// walk's node gate exists to prevent. An empty needle is legal and matches everything —
/// bounded like any other broad needle.
fn searched(
    forest: &[ColumnInfo],
    prefix: &[String],
    needle: &str,
    page: Option<usize>,
    answer: DescribeResult,
) -> DescribeResult {
    let at = page.unwrap_or(1).max(1);
    let mut window = Matches {
        skip: (at - 1).saturating_mul(MATCH_PAGE),
        hits: 0,
        out: Vec::new(),
    };
    let mut trail: Vec<&str> = prefix.iter().map(String::as_str).collect();
    search(forest, &mut trail, &needle.to_lowercase(), &mut window);
    let more = window.out.len() < window.hits;
    DescribeResult {
        matches: window.out,
        matched_total: Some(window.hits),
        page: more.then_some(at),
        page_size: more.then_some(MATCH_PAGE),
        ..answer
    }
}

/// One page of matches being collected: every hit counted, paths built only in the window.
struct Matches {
    skip: usize,
    hits: usize,
    out: Vec<MatchWire>,
}

/// Depth-first, document order — deterministic, so a page of matches is a stable window.
/// The paths carry the caller's own prefix, so a match under 'path' pastes straight back.
fn search<'c>(
    forest: &'c [ColumnInfo],
    trail: &mut Vec<&'c str>,
    needle: &str,
    window: &mut Matches,
) {
    for col in forest {
        trail.push(&col.name);
        if name_matches(&col.name, needle) {
            if window.hits >= window.skip && window.out.len() < MATCH_PAGE {
                window.out.push(MatchWire {
                    path: trail.iter().map(|s| (*s).to_string()).collect(),
                    dtype: col.dtype.clone(),
                    kind: col.kind.into(),
                });
            }
            window.hits += 1;
        }
        search(&col.children, trail, needle, window);
        trail.pop();
    }
}

#[cfg(test)]
mod tests {
    use serde_json::to_value;
    use strata_model::{ChartRole, Kind, Stat, StatKey};

    use super::*;

    /// Fixtures are synthetic on purpose: the reference fixture is untracked, 62 MB and
    /// absent from worktrees. The trees here are `ColumnInfo` exactly as `column_info`
    /// builds it — a List's one child named whatever the file's schema says, a Map's
    /// entries level already skipped — because the walk's contract is over that tree.
    fn col(name: &str, kind: Kind, children: Vec<ColumnInfo>) -> ColumnInfo {
        ColumnInfo {
            name: name.into(),
            dtype: match kind {
                Kind::Struct => "Struct".into(),
                Kind::List => "List".into(),
                _ => "Utf8".into(),
            },
            kind,
            role: ChartRole::Other,
            nullable: true,
            children,
            stats: Vec::new(),
        }
    }

    fn leaf(name: &str) -> ColumnInfo {
        col(name, Kind::Str, Vec::new())
    }

    fn table(columns: Vec<ColumnInfo>) -> Described {
        Described::Table {
            name: "config".into(),
            format: "json".into(),
            sources: vec!["config.json".into()],
            partitions: Vec::new(),
            rows: Some(1),
            columns,
        }
    }

    fn ask() -> DescribeTableParams {
        DescribeTableParams {
            name: "config".into(),
            ..DescribeTableParams::default()
        }
    }

    /// **A describe answer with no counting fields in it is a complete answer** — and the
    /// budget, not a depth, decides. A schema nested past `SCHEMA_DEPTH` but well inside
    /// the budget comes back whole, with not one counting field anywhere in the JSON.
    #[test]
    fn a_small_schema_is_complete_however_deep_with_no_totals() {
        let deep = col(
            "a",
            Kind::Struct,
            vec![col(
                "b",
                Kind::Struct,
                vec![col(
                    "c",
                    Kind::Struct,
                    vec![col(
                        "d",
                        Kind::Struct,
                        vec![col("e", Kind::Struct, vec![leaf("f")])],
                    )],
                )],
            )],
        );
        let result = describe_result(table(vec![deep, leaf("id")]), &ask()).unwrap();
        let json = to_string(&result).unwrap();
        assert!(json.contains("\"f\""), "{json}");
        for absent in [
            "children_total",
            "columns_total",
            "matched_total",
            "sources_total",
            "page",
        ] {
            assert!(
                !json.contains(absent),
                "{absent} in a complete answer: {json}"
            );
        }
    }

    /// A page the caller asked for is echoed only when the answer really is one window of
    /// more — the caller's own request must not forge the "more exists" signal.
    #[test]
    fn a_requested_page_of_a_complete_schema_stays_total_free() {
        let result = describe_result(
            table(vec![leaf("id"), leaf("name")]),
            &DescribeTableParams {
                page: Some(1),
                ..ask()
            },
        )
        .unwrap();
        assert_eq!(result.page, None);
        assert_eq!(result.page_size, None);
        assert_eq!(result.columns_total, None);
        assert_eq!(result.columns.len(), 2);
    }

    #[test]
    fn a_wide_flat_schema_pages_with_its_total() {
        let columns: Vec<ColumnInfo> = (0..60).map(|i| leaf(&format!("col_{i:02}"))).collect();
        let first = describe_result(table(columns.clone()), &ask()).unwrap();
        assert_eq!(first.columns.len(), SCHEMA_PAGE);
        assert_eq!(first.columns_total, Some(60));
        assert_eq!(first.page, Some(1));
        assert_eq!(first.page_size, Some(SCHEMA_PAGE));

        let second = describe_result(
            table(columns),
            &DescribeTableParams {
                page: Some(2),
                ..ask()
            },
        )
        .unwrap();
        assert_eq!(second.columns.len(), 10);
        assert_eq!(second.columns[0].name, "col_50");
        assert_eq!(second.page, Some(2));
    }

    #[test]
    fn a_page_past_the_end_is_empty_with_totals_not_a_fault() {
        let columns: Vec<ColumnInfo> = (0..60).map(|i| leaf(&format!("c{i}"))).collect();
        let past = describe_result(
            table(columns),
            &DescribeTableParams {
                page: Some(9),
                ..ask()
            },
        )
        .unwrap();
        assert!(past.columns.is_empty());
        assert_eq!(past.columns_total, Some(60));
        assert_eq!(past.page, Some(9));
    }

    /// A page number is wire input, so the window arithmetic must not trust it: the
    /// largest expressible page is an empty window with totals, never a panic or a
    /// silently wrapped skip.
    #[test]
    fn the_largest_expressible_page_is_empty_not_wrapped() {
        let columns: Vec<ColumnInfo> = (0..60).map(|i| leaf(&format!("c{i}"))).collect();
        let past = describe_result(
            table(columns.clone()),
            &DescribeTableParams {
                page: Some(usize::MAX),
                ..ask()
            },
        )
        .unwrap();
        assert!(past.columns.is_empty());
        assert_eq!(past.columns_total, Some(60));

        let matches = describe_result(
            table(columns),
            &DescribeTableParams {
                matching: Some("c".into()),
                page: Some(usize::MAX),
                ..ask()
            },
        )
        .unwrap();
        assert!(matches.matches.is_empty());
        assert_eq!(matches.matched_total, Some(60));
    }

    /// The config shape: one struct column of thousands of keys. The complete rendering is
    /// past the budget — proven by the node gate before any tree is built — so the ladder
    /// engages: a sampled window of children, each elision a stated count, and the whole
    /// answer still inside the budget.
    #[test]
    fn an_oversized_tree_is_sampled_with_every_elision_counted() {
        let blocks = col(
            "contentBlocks",
            Kind::Struct,
            (0..2000)
                .map(|i| {
                    col(
                        &format!("00000000-0000-0000-0000-{i:012}"),
                        Kind::Struct,
                        (0..12).map(|j| leaf(&format!("field_{j}"))).collect(),
                    )
                })
                .collect(),
        );
        let result = describe_result(table(vec![blocks, leaf("channel")]), &ask()).unwrap();
        let root = &result.columns[0];
        assert_eq!(root.children_total, Some(2000), "elided keys are counted");
        assert!(root.children.len() < 2000);
        let bytes = to_string(&result.columns).unwrap().len();
        assert!(bytes <= SCHEMA_BUDGET, "{bytes} > {SCHEMA_BUDGET}");
        assert_eq!(result.columns[1].name, "channel");
    }

    /// A tree so wide at every level that even one sampled level is past the budget lands
    /// on the floor: every child elided to a count, the shown level always rendered.
    #[test]
    fn the_floor_always_renders_the_shown_level() {
        let columns: Vec<ColumnInfo> = (0..50)
            .map(|i| {
                col(
                    &format!("00000000-0000-0000-{i:04}-000000000000"),
                    Kind::Struct,
                    (0..300)
                        .map(|j| {
                            col(
                                &format!("00000000-1111-0000-{j:04}-000000000000"),
                                Kind::Struct,
                                vec![leaf("x")],
                            )
                        })
                        .collect(),
                )
            })
            .collect();
        let result = describe_result(table(columns), &ask()).unwrap();
        assert_eq!(result.columns.len(), 50);
        for column in &result.columns {
            assert!(column.children.is_empty(), "the floor elides every child");
            assert_eq!(column.children_total, Some(300));
        }
    }

    /// The source list is the one envelope fact that scales with the user's data, and it
    /// is bounded with its total stated — `list_tables` sends callers here for the full
    /// list, so this answer must not itself be unreturnable.
    #[test]
    fn a_sharded_tables_source_list_is_bounded_with_its_total() {
        let described = Described::Table {
            name: "shards".into(),
            format: "parquet".into(),
            sources: (0..500).map(|i| format!("part-{i:04}.parquet")).collect(),
            partitions: Vec::new(),
            rows: None,
            columns: vec![leaf("id")],
        };
        let result = describe_result(described, &ask()).unwrap();
        assert_eq!(result.sources.len(), SOURCES_FULL);
        assert_eq!(result.sources_total, Some(500));
    }

    /// A path answers as the node itself — so a leaf answers as the leaf, stats and all,
    /// never as an empty list.
    #[test]
    fn a_path_lands_on_the_node_itself_and_a_leaf_answers_as_itself() {
        let mut price = leaf("price");
        price.stats = vec![Stat {
            key: StatKey::Min,
            text: "0".into(),
            exact: true,
        }];
        let columns = vec![col(
            "pricing",
            Kind::Struct,
            vec![col("tiers", Kind::Struct, vec![price])],
        )];

        let node = describe_result(
            table(columns.clone()),
            &DescribeTableParams {
                path: Some(vec!["pricing".into(), "tiers".into()]),
                ..ask()
            },
        )
        .unwrap();
        assert_eq!(node.columns.len(), 1);
        assert_eq!(node.columns[0].name, "tiers");
        assert_eq!(node.columns[0].children[0].name, "price");

        let leaf_answer = describe_result(
            table(columns),
            &DescribeTableParams {
                path: Some(vec!["pricing".into(), "tiers".into(), "price".into()]),
                ..ask()
            },
        )
        .unwrap();
        let shown = &leaf_answer.columns[0];
        assert_eq!(shown.name, "price");
        assert!(shown.children.is_empty());
        assert_eq!(shown.children_total, None, "a leaf has nothing elided");
        assert_eq!(shown.stats.len(), 1);
    }

    /// A List child is named whatever the file's schema says — the walk prints it and the
    /// resolver accepts it back, so the vocabulary closes without a documented constant.
    #[test]
    fn a_path_through_a_list_uses_the_files_own_element_name() {
        let columns = vec![col(
            "events",
            Kind::List,
            vec![col("element", Kind::Struct, vec![leaf("ts")])],
        )];
        let result = describe_result(
            table(columns),
            &DescribeTableParams {
                path: Some(vec!["events".into(), "element".into(), "ts".into()]),
                ..ask()
            },
        )
        .unwrap();
        assert_eq!(result.columns[0].name, "ts");
    }

    /// The refusal renders the path as the JSON array the parameter accepts back — never
    /// dot-joined, because a name may contain dots — and names this tool's own recovery.
    #[test]
    fn a_path_that_resolves_nowhere_is_refused_with_the_path_as_json() {
        let Err(AgentError::NotFound(message)) = describe_result(
            table(vec![leaf("id")]),
            &DescribeTableParams {
                path: Some(vec!["nba".into(), "a.b".into()]),
                ..ask()
            },
        ) else {
            panic!("expected a not-found refusal");
        };
        assert!(message.contains(r#"["nba","a.b"]"#), "{message}");
        assert!(message.contains("'matching'"), "{message}");
        assert!(!message.contains("nba.a.b"), "{message}");
    }

    /// 'matching' searches the whole tree, answers with paths that paste straight back into
    /// 'path', and states its total even at zero — an empty answer must not read as an
    /// unsearched one.
    #[test]
    fn matching_answers_paths_with_the_total_stated_even_at_zero() {
        let columns = vec![
            col(
                "nba",
                Kind::Struct,
                vec![col(
                    "actions",
                    Kind::Struct,
                    vec![leaf("priority"), leaf("PriorityGroup")],
                )],
            ),
            leaf("priority_flag"),
        ];
        let found = describe_result(
            table(columns.clone()),
            &DescribeTableParams {
                matching: Some("priority".into()),
                ..ask()
            },
        )
        .unwrap();
        assert_eq!(found.matched_total, Some(3), "case-insensitive, whole tree");
        let paths: Vec<Vec<String>> = found.matches.iter().map(|m| m.path.clone()).collect();
        assert!(paths.contains(&vec![
            "nba".to_string(),
            "actions".to_string(),
            "priority".to_string()
        ]));
        assert!(paths.contains(&vec!["priority_flag".to_string()]));
        assert!(found.columns.is_empty(), "a search answers with matches");
        assert_eq!(found.page, None, "three matches are a complete answer");

        let nothing = describe_result(
            table(columns),
            &DescribeTableParams {
                matching: Some("zzz".into()),
                ..ask()
            },
        )
        .unwrap();
        assert_eq!(nothing.matched_total, Some(0));
        assert!(nothing.matches.is_empty());
    }

    /// Under 'path' the search is scoped, and the reported paths still carry the prefix —
    /// a match anywhere is pasteable anywhere.
    #[test]
    fn matching_under_a_path_is_scoped_and_keeps_the_prefix() {
        let columns = vec![
            col("pricing", Kind::Struct, vec![leaf("amount")]),
            col("nba", Kind::Struct, vec![leaf("amount")]),
        ];
        let result = describe_result(
            table(columns),
            &DescribeTableParams {
                path: Some(vec!["pricing".into()]),
                matching: Some("amount".into()),
                ..ask()
            },
        )
        .unwrap();
        assert_eq!(
            result.matched_total,
            Some(1),
            "the other subtree is out of scope"
        );
        assert_eq!(
            result.matches[0].path,
            vec!["pricing".to_string(), "amount".to_string()]
        );
    }

    /// A match page fits the budget by construction, even at the fixture's worst shape —
    /// 13-segment paths of UUID-length names — because a match carries its whole path and
    /// the page is sized for that.
    #[test]
    fn a_match_page_fits_the_budget_at_the_fixtures_worst_shape() {
        let mut level: Vec<ColumnInfo> = (0..60)
            .map(|i| leaf(&format!("00000000-aaaa-0000-{i:04}-000000000000")))
            .collect();
        for d in 0..12 {
            level = vec![col(
                &format!("00000000-bbbb-0000-{d:04}-000000000000"),
                Kind::Struct,
                level,
            )];
        }
        let result = describe_result(
            table(level),
            &DescribeTableParams {
                matching: Some("aaaa".into()),
                ..ask()
            },
        )
        .unwrap();
        assert_eq!(result.matches.len(), MATCH_PAGE);
        assert_eq!(result.matched_total, Some(60));
        assert_eq!(result.page, Some(1));
        assert_eq!(result.page_size, Some(MATCH_PAGE));
        let bytes = to_string(&result.matches).unwrap().len();
        assert!(bytes <= SCHEMA_BUDGET, "{bytes} > {SCHEMA_BUDGET}");
    }

    #[test]
    fn matching_pages_its_matches() {
        let columns = vec![col(
            "blocks",
            Kind::Struct,
            (0..70).map(|i| leaf(&format!("key_{i:02}"))).collect(),
        )];
        let second = describe_result(
            table(columns),
            &DescribeTableParams {
                matching: Some("key".into()),
                page: Some(2),
                ..ask()
            },
        )
        .unwrap();
        assert_eq!(second.matched_total, Some(70));
        assert_eq!(second.matches.len(), MATCH_PAGE);
        assert_eq!(second.matches[0].path, vec!["blocks", "key_25"]);
        assert_eq!(second.page, Some(2));
    }

    /// Moved from `wire.rs` with the projection: a failed def has no schema, and the
    /// flattening must not invent one — nor may any of the new counting fields appear.
    #[test]
    fn a_failed_description_carries_only_its_name_state_and_error() {
        let wire = describe_result(
            Described::Failed {
                name: "orders".into(),
                error: "No source paths".into(),
            },
            &ask(),
        )
        .unwrap();
        assert_eq!(
            to_value(&wire).unwrap(),
            serde_json::json!({
                "name": "orders",
                "state": "failed",
                "error": "No source paths",
            })
        );
    }

    /// The state is the answer: a 'path' into a def with no schema yet is not a missing
    /// column, and refusing it would be a lie.
    #[test]
    fn walk_params_are_ignored_for_a_pending_def() {
        let wire = describe_result(
            Described::Pending {
                name: "orders".into(),
            },
            &DescribeTableParams {
                path: Some(vec!["anything".into()]),
                ..ask()
            },
        )
        .unwrap();
        assert!(matches!(wire.state, StateWire::Pending));
        assert!(wire.columns.is_empty());
    }
}
