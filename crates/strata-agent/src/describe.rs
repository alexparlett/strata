//! `describe_table`'s bounded schema projection (AA-07) — the walk, the path drill-down and
//! the name search over a def's `ColumnInfo` tree.
//!
//! A schema is not small — the reference fixture infers to 19 top-level columns carrying 241,425
//! nested fields at depth 13 — so the discipline is the value encoder's, restated for a schema
//! tree: a byte budget decides whether an answer is cut, width sampling decays with depth when it
//! is, and every elided set is replaced by a stated count. The convention the whole answer keeps:
//! **a describe answer with no counting fields in it is a complete answer.**
//!
//! Most of that size is one pathology: `engine::json_poly` infers every JSON object as a Struct,
//! so an object keyed by data — the UUID-keyed map — becomes thousands of same-shaped schema
//! fields. Past the budget those siblings **collapse** ([`slots`]): one representative shape
//! under the placeholder name `<key>`, carrying how many keys share it and a few of their real
//! names. It is JSON Schema's `additionalProperties` said in this tool's own vocabulary, and it
//! is a *cutting* strategy — an answer that fits complete is never collapsed, because there the
//! names are the information.
//!
//! The constants are this module's own rather than the value encoder's: same discipline, different
//! budgets, and a shared constant would couple two surfaces that tune independently.
//!
//! In `strata-agent` rather than `strata-engine` because its output is [`ColumnWire`], a wire
//! shape the engine must not know; beside `wire.rs` rather than in it because the walk plus the search plus the
//! budget loop is an algorithm with its own tests.

use std::cmp::Reverse;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use serde_json::to_string;
use strata_model::ColumnInfo;

use crate::error::AgentError;
use crate::host::Described;
use crate::wire::{
    name_matches, windowed, ColumnWire, DescribeResult, DescribeTableParams, EntryKindWire,
    MatchWire, PartitionWire, StatWire, StateWire,
};

/// The deepest rung of the sampling ladder ([`walk`]) — engaged only once the complete
/// rendering is past the budget. A fixed depth alone was measured and rejected: at depth 3
/// with the width decay below, the fixture's 19,311-key struct emits ~26 KB from one column,
/// and even depth 1 grazes the cap on UUID-named keys — which is why the ladder retries
/// shallower and depth 0 is the unmeasured floor.
///
/// It reaches past that 3 now because two things changed together. The collapse frees the
/// width the keys were spending, and every rung is capped at [`NODE_CAP`] nodes as it is
/// built — so a rung too deep to fit costs a bounded count instead of a quarter-million-node
/// tree, and the ladder can afford to ask. Five is where `contentBlocks` → the key shape →
/// `variants` → its element → `eligibilityRule` lands: the view the field feedback said would
/// have told it the whole structure at once.
const SCHEMA_DEPTH: usize = 5;

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
/// prove a rendering cannot fit **before** the tree is built.
const NODE_FLOOR: usize = 55;

/// The most nodes any rendering could fit into [`SCHEMA_BUDGET`]. Past it the answer is over
/// budget whatever its names are, so counting to here and stopping is never a rung refused
/// that would have fit — it is only the build that is bounded. The complete rung proves it
/// before building at all ([`plausibly_complete`]); a sampled rung, whose width and depth
/// only the walk knows, counts as it goes.
const NODE_CAP: usize = SCHEMA_BUDGET / NODE_FLOOR;

/// The fewest same-shaped sibling containers that collapse into one representative. Low
/// enough to catch a small keyed map, high enough that an ordinary record whose fields happen
/// to share a shape — a `created`/`updated` pair of timestamps, three same-shaped addresses —
/// is still printed field by field.
const COLLAPSE_MIN: usize = 8;

/// How many of a collapsed set's real keys the answer names. Enough to show what they look
/// like and to hand 'path' something it accepts; the rest are reached by `matching`, and
/// naming more would spend on data keys the budget the collapse just freed for shape.
const KEY_EXAMPLES: usize = 3;

/// The name a collapsed key set renders under. Not a field any file named — a file *could*
/// name one this, which is why `keys_total` and not the spelling is what marks the entry —
/// and not a path segment: 'path' takes one of the real keys beside it.
const KEY_PLACEHOLDER: &str = "<key>";

/// Source paths a describe answer lists before the count stands in. Far looser than
/// `list_tables`' three — this answer is where that elision points — but not unbounded: a
/// def registered over thousands of parts would otherwise spend the whole result cap on
/// paths, and no parameter of this tool reaches them.
const SOURCES_FULL: usize = 100;

/// Project a [`Described`] into the wire answer, walked as `params` asks.
///
/// The `Remote` arm adds one fact, the connection, and takes its *kind* from the server's own
/// answer: a remote view is a view, and saying "table" about one would be the very thing that
/// arm exists to stop the tool doing — claiming something it was not told.
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
        Described::Remote {
            name,
            connection,
            view,
            columns,
        } => Ok(DescribeResult {
            kind: Some(match view {
                true => EntryKindWire::View,
                false => EntryKindWire::Table,
            }),
            connection: Some(connection),
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
        connection: None,
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

    let walked = walk(forest, params.page);
    let elided = (forest.len() > walked.columns.len()).then_some(forest.len());

    Ok(match node {
        None => DescribeResult {
            columns: walked.columns,
            columns_total: elided,
            page: walked.page,
            page_size: walked.page_size,
            ..answer
        },
        Some(node) => DescribeResult {
            columns: vec![ColumnWire {
                name: node.name.clone(),
                dtype: node.dtype.clone(),
                kind: node.kind.into(),
                nullable: node.nullable,
                children_total: elided,
                children: walked.columns,
                keys_total: None,
                key_examples: Vec::new(),
                stats: node.stats.iter().map(StatWire::from).collect(),
            }],
            columns_total: None,
            page: walked.page,
            page_size: walked.page_size,
            ..answer
        },
    })
}

/// One page of the walk root's children, rendered — and how it was paged.
///
/// The count the caller is owed is not in here: it is `forest.len()` against the entries
/// shown, which is the one rule both arms of [`schema_view`] apply. A collapsed entry stands
/// for many children and so shows fewer than the forest holds, exactly as an elided page does
/// — which is why the same comparison covers both.
struct Walked {
    columns: Vec<ColumnWire>,
    page: Option<usize>,
    page_size: Option<usize>,
}

/// The forest, paged and rendered as deep as the budget allows.
///
/// The first rung is the **whole** subtree of one plain page — the budget, not a depth,
/// decides whether an answer is cut, so a small schema stays complete however deep it nests
/// and carries no totals at all. That rung is guarded by a node count against [`NODE_CAP`]:
/// a window whose nodes already outnumber it provably cannot fit, and building a
/// quarter-million-node tree just to measure it was this module's own founding defect.
///
/// That rung is measured over **one page**, so it is offered only where a page can be the
/// whole answer. A forest longer than one page is already being cut, and a collapse available
/// in it says more than any page of it does — 19,311 UUID keys holding a two-field record fit
/// a page perfectly well, and answering with 50 of their names, 387 pages deep, is the
/// pathology rather than a complete answer. So a forest that is both **paged and collapsible**
/// skips the complete rung outright, and the cutting rule reads over the forest rather than
/// over the window that happened to be asked for.
///
/// The collapse comes first in that cutting: [`slots`] over the **whole** forest, so a set's
/// count is the set's and not this page's, and then the ladder — attempted depth with width
/// sampling, retried shallower, and depth 0, the shown level with every child elided to a
/// count, accepted unmeasured so there is always something to show. Paging is over slots from
/// there on, which is what makes one collapsed page the whole answer rather than the first of
/// 387.
///
/// A sampled rung is built against [`NODE_CAP`] and abandoned the moment it passes it, which
/// is what lets the ladder start as deep as it does: an overrunning rung is one that could not
/// have fitted anyway, so nothing is lost but the building of it.
fn walk(forest: &[ColumnInfo], page: Option<usize>) -> Walked {
    let sets = slots(forest);
    let plain = windowed(forest.iter().collect(), page, SCHEMA_PAGE);
    let cut = plain.page.is_some() && sets.len() < forest.len();
    if !cut && plausibly_complete(&plain.shown) {
        let full: Vec<ColumnWire> = plain.shown.iter().map(|c| ColumnWire::from(*c)).collect();
        if fits(&full) {
            return Walked {
                columns: full,
                page: plain.page,
                page_size: plain.page_size,
            };
        }
    }
    let w = windowed(sets, page, SCHEMA_PAGE);
    let rung = |depth| -> Option<Vec<ColumnWire>> {
        let mut nodes = Nodes::default();
        let rendered: Vec<ColumnWire> = w
            .shown
            .iter()
            .map(|s| render(s, 1, depth, &mut nodes))
            .collect();
        (!nodes.past_cap()).then_some(rendered)
    };
    Walked {
        columns: (1..=SCHEMA_DEPTH)
            .rev()
            .find_map(|depth| rung(depth).filter(|r| fits(r)))
            .unwrap_or_else(|| {
                let mut nodes = Nodes::default();
                w.shown
                    .iter()
                    .map(|s| render(s, 1, 0, &mut nodes))
                    .collect()
            }),
        page: w.page,
        page_size: w.page_size,
    }
}

/// The nodes a sampled rung has emitted, counted against [`NODE_CAP`].
#[derive(Default)]
struct Nodes(usize);

impl Nodes {
    /// Charge for one emitted node.
    fn spend(&mut self) {
        self.0 += 1;
    }

    /// Whether the rung is already too big to fit — which is both the reason to stop opening
    /// children and the reason to throw the rung away.
    fn past_cap(&self) -> bool {
        self.0 > NODE_CAP
    }
}

/// Whether the window's node count leaves the complete rendering any chance of fitting.
fn plausibly_complete(window: &[&ColumnInfo]) -> bool {
    let mut allowance = NODE_CAP;
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

/// One slot with `depth` levels below it: a child as itself, or a collapsed key set as its
/// representative wearing the placeholder name and the two fields that say what it stands for.
///
/// The representative renders exactly as a real child does — the collapse decides *which*
/// subtree is shown, never how deep, so the budget it frees is spent by the ladder above on
/// depth for the shape that remains.
fn render(slot: &Slot, level: usize, depth: usize, nodes: &mut Nodes) -> ColumnWire {
    match slot {
        Slot::One(col) => subtree(col, level, depth, nodes),
        Slot::Keys(keys) => ColumnWire {
            name: KEY_PLACEHOLDER.to_string(),
            keys_total: Some(keys.len()),
            key_examples: keys
                .iter()
                .take(KEY_EXAMPLES)
                .map(|k| k.name.clone())
                .collect(),
            ..subtree(keys[0], level, depth, nodes)
        },
    }
}

/// One column with `depth` levels of children below it, each level collapsed then sampled,
/// with its total stated where it shows fewer entries than the column has children — which a
/// collapsed set always does, since it takes [`COLLAPSE_MIN`] children to make one.
///
/// It stops opening children once the rung is past [`NODE_CAP`], because from there the rung
/// is being thrown away and every node built is waste.
fn subtree(col: &ColumnInfo, level: usize, depth: usize, nodes: &mut Nodes) -> ColumnWire {
    nodes.spend();
    let total = col.children.len();
    let children: Vec<ColumnWire> = if depth == 0 || nodes.past_cap() {
        Vec::new()
    } else {
        slots(&col.children)
            .iter()
            .take(schema_items(level))
            .map(|s| render(s, level + 1, depth - 1, nodes))
            .collect()
    };
    ColumnWire {
        name: col.name.clone(),
        dtype: col.dtype.clone(),
        kind: col.kind.into(),
        nullable: col.nullable,
        children_total: (total > children.len()).then_some(total),
        children,
        keys_total: None,
        key_examples: Vec::new(),
        stats: col.stats.iter().map(StatWire::from).collect(),
    }
}

/// One entry of a rendered child list: a child in its own right, or a set of same-shaped
/// sibling containers standing under the first of them.
enum Slot<'c> {
    One(&'c ColumnInfo),
    Keys(Vec<&'c ColumnInfo>),
}

/// A container's children as the entries an answer shows: every set of [`COLLAPSE_MIN`] or
/// more identically-shaped siblings as one [`Slot::Keys`], largest set first, then everything
/// else in document order. Collapsed first because the width sample above cuts from the end,
/// and the shape a thousand keys share is worth more of that width than any one of them.
///
/// Two exclusions, both because the collapse trades names for shape and there has to be a
/// shape to get: a **leaf** never joins a set (its name is the only thing it carries — that
/// is what `children_total` already elides better), and a set is only ever recognised among
/// enough siblings to have one.
fn slots(children: &[ColumnInfo]) -> Vec<Slot<'_>> {
    if children.len() < COLLAPSE_MIN {
        return children.iter().map(Slot::One).collect();
    }
    let mut sets: Vec<Vec<usize>> = Vec::new();
    let mut by_key: HashMap<u64, Vec<usize>> = HashMap::new();
    for (i, child) in children.iter().enumerate() {
        if child.children.is_empty() {
            continue;
        }
        let candidates = by_key.entry(shallow(child)).or_default();
        let found = candidates
            .iter()
            .copied()
            .find(|&s| same_shape(&children[sets[s][0]], child));
        match found {
            Some(s) => sets[s].push(i),
            None => {
                sets.push(vec![i]);
                candidates.push(sets.len() - 1);
            }
        }
    }
    let mut collapsed: Vec<usize> = (0..sets.len())
        .filter(|&s| sets[s].len() >= COLLAPSE_MIN)
        .collect();
    if collapsed.is_empty() {
        return children.iter().map(Slot::One).collect();
    }
    collapsed.sort_by_key(|&s| Reverse(sets[s].len()));

    let mut taken = vec![false; children.len()];
    let mut out: Vec<Slot<'_>> = Vec::new();
    for s in collapsed {
        for &i in &sets[s] {
            taken[i] = true;
        }
        out.push(Slot::Keys(sets[s].iter().map(|&i| &children[i]).collect()));
    }
    out.extend(
        children
            .iter()
            .enumerate()
            .filter(|(i, _)| !taken[*i])
            .map(|(_, c)| Slot::One(c)),
    );
    out
}

/// Whether two columns are the same shape — everything but the name, read exhaustively so a
/// field added to [`ColumnInfo`] cannot slip into a set unexamined. Names *below* the top
/// count, and must: the representative prints them, and a set whose members disagreed about
/// them would be one answer speaking for keys it does not describe.
fn same_shape(a: &ColumnInfo, b: &ColumnInfo) -> bool {
    let ColumnInfo {
        name: _,
        dtype,
        kind,
        role,
        nullable,
        children,
        stats,
    } = a;
    *dtype == b.dtype
        && *kind == b.kind
        && *role == b.role
        && *nullable == b.nullable
        && *children == b.children
        && *stats == b.stats
}

/// A cheap bucket key for [`slots`]: this column's own type facts plus its children's names
/// and types, one level down. Deliberately **not** the whole subtree — a digest that decided
/// membership would have to be trusted, where [`same_shape`] can simply check, and hashing a
/// quarter-million fields per rung is the cost this module exists to avoid.
fn shallow(col: &ColumnInfo) -> u64 {
    let mut hash = DefaultHasher::new();
    col.dtype.hash(&mut hash);
    col.nullable.hash(&mut hash);
    col.children.len().hash(&mut hash);
    for child in &col.children {
        child.name.hash(&mut hash);
        child.dtype.hash(&mut hash);
    }
    hash.finish()
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
///
/// A path carrying the collapse placeholder gets its own recovery, because it is the one
/// spelling this tool prints and does not accept back: the answer that printed it named real
/// keys beside it, and that is the whole repair.
fn no_such_column(table: &str, path: &[String]) -> AgentError {
    let shown = to_string(path).unwrap_or_default();
    let recovery = match path.iter().any(|segment| segment == KEY_PLACEHOLDER) {
        true => format!(
            "'{KEY_PLACEHOLDER}' stands for a set of same-shaped keys, not a field: put one \
             of the 'key_examples' listed beside it in its place."
        ),
        false => "Call describe_table without 'path' to see the schema, or with 'matching' \
                  to find a field by name."
            .to_string(),
    };
    AgentError::NotFound(format!("No column {shown} in '{table}'. {recovery}"))
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
///
/// It collapses too, and that is where a needle like 'eligibilityRule' stops answering with
/// 2,134 paths that differ in one segment: a field found under a collapsed set is **one row**
/// through the placeholder, carrying how many real fields it stands for. So a row and a hit
/// are no longer the same thing — the rows are what pages, `matched_total` is still every
/// field matched.
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
        rows: 0,
        hits: 0,
        out: Vec::new(),
    };
    let mut trail: Vec<&str> = prefix.iter().map(String::as_str).collect();
    search(forest, &mut trail, &needle.to_lowercase(), &mut window, 1);
    let more = window.out.len() < window.rows;
    DescribeResult {
        matches: window.out,
        matched_total: Some(window.hits),
        page: more.then_some(at),
        page_size: more.then_some(MATCH_PAGE),
        ..answer
    }
}

/// One page of matches being collected: every row counted for the window, every field counted
/// for the total, paths built only inside the window.
struct Matches {
    skip: usize,
    rows: usize,
    hits: usize,
    out: Vec<MatchWire>,
}

impl Matches {
    /// Record one row standing for `keys` real fields.
    fn hit(&mut self, trail: &[&str], col: &ColumnInfo, keys: usize) {
        if self.rows >= self.skip && self.out.len() < MATCH_PAGE {
            self.out.push(MatchWire {
                path: trail.iter().map(|s| (*s).to_string()).collect(),
                dtype: col.dtype.clone(),
                kind: col.kind.into(),
                matched_keys: (keys > 1).then_some(keys),
            });
        }
        self.rows += 1;
        self.hits += keys;
    }
}

/// Depth-first over the same slots the walk shows — collapsed sets first, then everything
/// else in document order — deterministic, so a page of matches is a stable window. The paths
/// carry the caller's own prefix, so a match under 'path' pastes straight back.
///
/// A collapsed set is searched in two halves, because only one of them is shared. The **keys**
/// vary, so each is still tested by name and answers as itself — a caller searching for a key
/// wants that key back, spelled as the file spells it. Everything **below** them is identical
/// by construction, so it is searched once through the placeholder and every hit there stands
/// for the whole set; `keys` carries that multiplier down, so nested sets multiply.
fn search<'c>(
    forest: &'c [ColumnInfo],
    trail: &mut Vec<&'c str>,
    needle: &str,
    window: &mut Matches,
    keys: usize,
) {
    for slot in slots(forest) {
        match slot {
            Slot::One(col) => {
                trail.push(&col.name);
                if name_matches(&col.name, needle) {
                    window.hit(trail, col, keys);
                }
                search(&col.children, trail, needle, window, keys);
                trail.pop();
            }
            Slot::Keys(set) => {
                for &key in &set {
                    trail.push(&key.name);
                    if name_matches(&key.name, needle) {
                        window.hit(trail, key, keys);
                    }
                    trail.pop();
                }
                trail.push(KEY_PLACEHOLDER);
                search(&set[0].children, trail, needle, window, keys * set.len());
                trail.pop();
            }
        }
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
            "keys_total",
            "matched_keys",
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

    /// The keys of the config shape, built as the fixture has them: one struct column whose
    /// children are thousands of UUIDs carrying one repeated record.
    fn keyed(name: &str, keys: usize, shape: &dyn Fn(usize) -> Vec<ColumnInfo>) -> ColumnInfo {
        col(
            name,
            Kind::Struct,
            (0..keys)
                .map(|i| {
                    col(
                        &format!("00000000-0000-0000-0000-{i:012}"),
                        Kind::Struct,
                        shape(i),
                    )
                })
                .collect(),
        )
    }

    /// **The config shape.** One struct column of thousands of same-shaped UUID keys, past
    /// the budget — proven by the node gate before any tree is built. The answer is the one
    /// view the field feedback asked for: a single representative shape, the key count, a few
    /// real keys to descend by, and the record itself rendered under it.
    #[test]
    fn keyed_siblings_collapse_to_one_counted_shape() {
        let blocks = keyed("contentBlocks", 2000, &|_| {
            (0..12).map(|j| leaf(&format!("field_{j}"))).collect()
        });
        let result = describe_result(table(vec![blocks, leaf("channel")]), &ask()).unwrap();
        let root = &result.columns[0];
        assert_eq!(root.children.len(), 1, "one shape, not a page of keys");
        assert_eq!(root.children_total, Some(2000), "elided keys are counted");

        let shape = &root.children[0];
        assert_eq!(shape.name, "<key>");
        assert_eq!(shape.keys_total, Some(2000));
        assert_eq!(
            shape.key_examples,
            vec![
                "00000000-0000-0000-0000-000000000000",
                "00000000-0000-0000-0000-000000000001",
                "00000000-0000-0000-0000-000000000002",
            ],
            "and real keys to descend by"
        );
        assert_eq!(
            shape.children[0].name, "field_0",
            "the record it stands for"
        );
        assert_eq!(shape.children_total, Some(12));

        let bytes = to_string(&result.columns).unwrap().len();
        assert!(bytes <= SCHEMA_BUDGET, "{bytes} > {SCHEMA_BUDGET}");
        assert_eq!(result.columns[1].name, "channel");
    }

    /// **The view the feedback asked for**, at the fixture's own width: describing the table
    /// answers `contentBlocks` → the shape its 19,311 keys share → `variants` → its element →
    /// `eligibilityRule`, in one call, inside the budget. That is the collapse and the ladder
    /// together — the keys stop spending the width, and the depth the width was buying goes
    /// to the shape instead.
    #[test]
    fn the_whole_structure_arrives_in_one_answer() {
        let blocks = keyed("contentBlocks", 19_311, &|_| {
            vec![
                leaf("name"),
                col(
                    "variants",
                    Kind::List,
                    vec![col(
                        "element",
                        Kind::Struct,
                        vec![leaf("eligibilityRule"), leaf("weight")],
                    )],
                ),
            ]
        });
        let result = describe_result(table(vec![blocks, leaf("channel")]), &ask()).unwrap();
        let shape = &result.columns[0].children[0];
        assert_eq!(shape.keys_total, Some(19_311));
        let variants = shape
            .children
            .iter()
            .find(|c| c.name == "variants")
            .unwrap();
        let element = &variants.children[0];
        assert_eq!(element.name, "element");
        let leaves: Vec<&str> = element.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(leaves, vec!["eligibilityRule", "weight"]);

        let bytes = to_string(&result.columns).unwrap().len();
        assert!(bytes <= SCHEMA_BUDGET, "{bytes} > {SCHEMA_BUDGET}");
    }

    /// The same width with no shape to share: the collapse finds nothing, and the answer is
    /// the sampled window it always was, every elision a stated count.
    #[test]
    fn siblings_of_distinct_shapes_are_sampled_not_collapsed() {
        let blocks = keyed("contentBlocks", 2000, &|i| {
            (0..12).map(|j| leaf(&format!("field_{i}_{j}"))).collect()
        });
        let result = describe_result(table(vec![blocks]), &ask()).unwrap();
        let root = &result.columns[0];
        assert_eq!(root.children_total, Some(2000));
        assert!(root.children.len() < 2000);
        assert!(
            root.children.iter().all(|c| c.keys_total.is_none()),
            "nothing shares a shape, so nothing collapses"
        );
        assert_eq!(
            root.children[0].name,
            "00000000-0000-0000-0000-000000000000"
        );
        let bytes = to_string(&result.columns).unwrap().len();
        assert!(bytes <= SCHEMA_BUDGET, "{bytes} > {SCHEMA_BUDGET}");
    }

    /// The collapse is a *cutting* strategy. A schema of same-shaped siblings that fits comes
    /// back complete, names and all — because there the names are the information, and an
    /// answer with no counting fields in it must stay a complete answer.
    #[test]
    fn same_shaped_siblings_that_fit_are_never_collapsed() {
        let blocks = keyed("contentBlocks", 10, &|_| vec![leaf("id"), leaf("label")]);
        let result = describe_result(table(vec![blocks]), &ask()).unwrap();
        let json = to_string(&result).unwrap();
        assert!(!json.contains("<key>"), "{json}");
        assert!(!json.contains("keys_total"), "{json}");
        assert_eq!(result.columns[0].children.len(), 10);
    }

    /// A run of leaves never collapses, however wide: a leaf carries nothing but its name, so
    /// a shape standing for two hundred of them would trade the whole answer for nothing.
    /// `children_total` already elides them, and better.
    #[test]
    fn a_wide_run_of_leaves_is_never_collapsed() {
        let flat = col(
            "row",
            Kind::Struct,
            (0..400).map(|i| leaf(&format!("f{i:03}"))).collect(),
        );
        let result = describe_result(table(vec![flat]), &ask()).unwrap();
        let root = &result.columns[0];
        assert_eq!(root.children_total, Some(400));
        assert_eq!(root.children[0].name, "f000", "names, not a placeholder");
        assert!(
            root.children.iter().all(|c| c.keys_total.is_none()),
            "a leaf never joins a set"
        );
    }

    /// Mixed children collapse per shape, largest set first — the width sample cuts from the
    /// end, and a set of three hundred deserves that width more than any one key does. What
    /// is left over follows in document order, still named.
    #[test]
    fn mixed_shapes_collapse_per_set_largest_first() {
        let mut children: Vec<ColumnInfo> = Vec::new();
        children.push(leaf("version"));
        children.extend((0..100).map(|i| col(&format!("b{i}"), Kind::Struct, vec![leaf("p")])));
        children.extend((0..300).map(|i| {
            col(
                &format!("a{i}"),
                Kind::Struct,
                vec![leaf("x"), leaf("y"), leaf("z")],
            )
        }));
        children.push(col("odd", Kind::Struct, vec![leaf("only")]));

        let result =
            describe_result(table(vec![col("mixed", Kind::Struct, children)]), &ask()).unwrap();
        let shown = &result.columns[0].children;
        assert_eq!(shown[0].keys_total, Some(300), "the largest set leads");
        assert_eq!(shown[0].children[0].name, "x");
        assert_eq!(shown[1].keys_total, Some(100));
        assert_eq!(shown[1].children[0].name, "p");
        let rest: Vec<&str> = shown[2..].iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            rest,
            vec!["version", "odd"],
            "too few to be a set, so still named, in document order"
        );
    }

    /// Describing the keyed struct itself — the drill-down the feedback took — answers the
    /// same one shape, and the count is the *struct's*, never this page's.
    #[test]
    fn a_keyed_struct_addressed_by_path_answers_one_shape() {
        let blocks = keyed("contentBlocks", 2000, &|_| {
            (0..12).map(|j| leaf(&format!("field_{j}"))).collect()
        });
        let result = describe_result(
            table(vec![blocks]),
            &DescribeTableParams {
                path: Some(vec!["contentBlocks".into()]),
                ..ask()
            },
        )
        .unwrap();
        let root = &result.columns[0];
        assert_eq!(root.name, "contentBlocks");
        assert_eq!(root.children.len(), 1);
        assert_eq!(root.children[0].keys_total, Some(2000));
        assert_eq!(root.children_total, Some(2000));
        assert_eq!(result.page, None, "one shape is the whole answer");
    }

    /// A page of a keyed struct is not a complete answer, however comfortably it fits. With a
    /// small record under each key, 50 UUID names render well inside the budget — and
    /// answering with them, 387 pages deep, is the pathology rather than completeness. The
    /// cutting rule reads over the forest, so this collapses exactly as it does when the same
    /// struct is reached from its parent.
    #[test]
    fn a_page_of_a_keyed_struct_is_not_a_complete_answer() {
        let blocks = keyed("contentBlocks", 19_311, &|_| {
            vec![leaf("name"), leaf("enabled")]
        });
        let result = describe_result(
            table(vec![blocks]),
            &DescribeTableParams {
                path: Some(vec!["contentBlocks".into()]),
                ..ask()
            },
        )
        .unwrap();
        let root = &result.columns[0];
        assert_eq!(root.children.len(), 1, "one shape, not 50 of 19,311 names");
        assert_eq!(root.children[0].keys_total, Some(19_311));
        assert_eq!(root.children_total, Some(19_311));
        assert_eq!(result.page, None, "and no 387 pages behind it");
    }

    /// The placeholder is the one spelling this tool prints and will not accept back, so the
    /// refusal names the repair the answer already carried.
    #[test]
    fn the_placeholder_is_refused_as_a_path_segment_with_its_repair() {
        let blocks = keyed("contentBlocks", 2000, &|_| vec![leaf("id")]);
        let Err(AgentError::NotFound(message)) = describe_result(
            table(vec![blocks]),
            &DescribeTableParams {
                path: Some(vec!["contentBlocks".into(), "<key>".into()]),
                ..ask()
            },
        ) else {
            panic!("expected a not-found refusal");
        };
        assert!(message.contains("key_examples"), "{message}");
    }

    /// And a real key resolves straight through the collapsed level — which is what the
    /// examples beside the placeholder are for.
    #[test]
    fn a_real_key_resolves_through_a_collapsed_level() {
        let blocks = keyed("contentBlocks", 2000, &|_| vec![leaf("eligibilityRule")]);
        let result = describe_result(
            table(vec![blocks]),
            &DescribeTableParams {
                path: Some(vec![
                    "contentBlocks".into(),
                    "00000000-0000-0000-0000-000000001234".into(),
                ]),
                ..ask()
            },
        )
        .unwrap();
        assert_eq!(
            result.columns[0].name,
            "00000000-0000-0000-0000-000000001234"
        );
        assert_eq!(result.columns[0].children[0].name, "eligibilityRule");
    }

    /// A tree so wide at every level that even one sampled level is past the budget lands
    /// on the floor: every child elided to a count, the shown level always rendered. Every
    /// field name here is distinct, so no two subtrees share a shape and the collapse has
    /// nothing to offer — the floor is what is left.
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
                                vec![leaf(&format!("x_{i}_{j}"))],
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

    /// **A relation in a database connection's catalog** (DB-03): its columns, the connection
    /// it is in, and the server's own word for what it is — and none of the def facts, because
    /// there is no def. A `not found` here would be false about something the agent can query.
    ///
    /// The second half is the kind: the server's word, not a guess, so a remote view is a view.
    #[test]
    fn a_remote_relation_describes_as_itself() {
        let described = Described::Remote {
            name: "pg.public.orders".into(),
            connection: "pg".into(),
            view: false,
            columns: vec![leaf("id"), leaf("total")],
        };
        let result = describe_result(described, &ask()).unwrap();
        assert_eq!(result.name, "pg.public.orders");
        assert_eq!(result.connection.as_deref(), Some("pg"));
        assert!(matches!(result.kind, Some(EntryKindWire::Table)));
        assert_eq!(result.columns.len(), 2);
        assert!(
            result.format.is_none() && result.sources.is_empty() && result.rows.is_none(),
            "a remote relation has no def to report facts from"
        );

        let remote_view = Described::Remote {
            name: "pg.public.big_orders".into(),
            connection: "pg".into(),
            view: true,
            columns: vec![leaf("id")],
        };
        let result = describe_result(remote_view, &ask()).unwrap();
        assert!(matches!(result.kind, Some(EntryKindWire::View)));
        assert!(
            result.sql.is_none(),
            "and its definition is the server's, not something this app holds"
        );
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

    /// The search collapses too, and this is the row the feedback wanted: one path through
    /// the placeholder, the ×N stated on it, and `matched_total` still every field it stands
    /// for — not 2,000 UUID paths, 25 to a page.
    #[test]
    fn a_field_under_a_collapsed_set_answers_as_one_counted_row() {
        let blocks = keyed("contentBlocks", 2000, &|_| {
            vec![col(
                "variants",
                Kind::List,
                vec![col(
                    "element",
                    Kind::Struct,
                    vec![leaf("eligibilityRule"), leaf("weight")],
                )],
            )]
        });
        let result = describe_result(
            table(vec![blocks]),
            &DescribeTableParams {
                matching: Some("eligibilityRule".into()),
                ..ask()
            },
        )
        .unwrap();
        assert_eq!(result.matches.len(), 1, "one shape, not one row per key");
        assert_eq!(
            result.matches[0].path,
            vec![
                "contentBlocks".to_string(),
                "<key>".to_string(),
                "variants".to_string(),
                "element".to_string(),
                "eligibilityRule".to_string(),
            ]
        );
        assert_eq!(result.matches[0].matched_keys, Some(2000));
        assert_eq!(
            result.matched_total,
            Some(2000),
            "every field it stands for"
        );
        assert_eq!(result.page, None, "one row is the whole answer");
    }

    /// The keys of a collapsed set are the half that varies, so a needle over *them* is
    /// answered key by key, each spelled as the file spells it — a caller searching for a key
    /// is searching for exactly that.
    #[test]
    fn a_search_for_the_keys_themselves_still_names_them() {
        let blocks = keyed("contentBlocks", 2000, &|_| vec![leaf("id")]);
        let result = describe_result(
            table(vec![blocks]),
            &DescribeTableParams {
                matching: Some("000000000042".into()),
                ..ask()
            },
        )
        .unwrap();
        assert_eq!(result.matched_total, Some(1));
        assert_eq!(
            result.matches[0].path,
            vec![
                "contentBlocks".to_string(),
                "00000000-0000-0000-0000-000000000042".to_string(),
            ]
        );
        assert_eq!(
            result.matches[0].matched_keys, None,
            "one key is one field, not a set"
        );
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
