//! The rank pipeline: context tiers, the composed per-candidate forces
//! (type affinity, cross-side join keys, written-demotion), and the final
//! filter/sort/dedupe/truncate over the pooled candidates.

use std::collections::HashSet;

use crate::engine::sql::context::CaretAnalysis;
use crate::engine::sql::fuzzy::match_tier;
use crate::engine::sql::symbols::Catalog;
use strata_model::Kind;

use super::{Completion, CompletionKind};

/// Context tiers (lower ranks first): what the clause position is *for*.
pub(super) const T_PRIMARY: u8 = 0;
pub(super) const T_SECONDARY: u8 = 1;
pub(super) const T_FUNCTION: u8 = 2;
pub(super) const T_KEYWORD: u8 = 3;
/// The demoted `ALL_KEYWORDS` tail — additionally gated to ≥2-char prefix matches.
pub(super) const T_TAIL: u8 = 4;

/// The composed column sub-rank — the ranking forces acting *within* a tier,
/// strongest first: **comparison type-affinity** (`a.int = b.string` is legal but
/// rarely meant — same type family floats), **cross-side key likelihood** at ON
/// positions (a name present on both sides of a join is the probable equi-key),
/// and the **written-demotion** (an item already referenced in the caret's own
/// clause list is the less likely next pick). Every force is a demotion bit —
/// candidates are only ever reordered, never removed. `None` = signal absent.
pub(super) fn column_ord(
    affinity_miss: Option<bool>,
    cross_miss: Option<bool>,
    written: bool,
) -> usize {
    (affinity_miss == Some(true)) as usize * 4
        + (cross_miss == Some(true)) as usize * 2
        + written as usize
}

/// The type family of the comparand (`e.user_id = |` → Num), when resolvable:
/// qualified refs resolve through the alias map to a catalog table (inline
/// relations carry no dtypes); bare refs through the first in-scope relation
/// owning the column.
pub(super) fn comparand_kind(ca: &CaretAnalysis, catalog: &Catalog) -> Option<Kind> {
    let (qualifier, column) = ca.comparand.as_ref()?;
    let dtype_of = |rel: &str| -> Option<String> {
        let resolved = ca
            .aliases
            .iter()
            .find(|(a, _)| a.eq_ignore_ascii_case(rel))
            .map(|(_, t)| t.as_str())
            .unwrap_or(rel);
        catalog
            .table(resolved)
            .and_then(|t| t.column(column))
            .map(|c| c.dtype.clone())
    };
    let dtype = match qualifier {
        Some(q) => dtype_of(q),
        None => ca.in_scope.iter().find_map(|r| dtype_of(r)),
    }?;
    Some(Kind::from_arrow(&dtype))
}

/// Column names offered by the in-scope relations **other than** `owner` — the
/// candidate join keys at an ON position.
///
/// A **set**, keyed on the folded name, because the caller asks it one question per
/// candidate column: over a join of wide relations a linear scan made the ON offer
/// quadratic in total width (2 x 1000 columns cost ~1M comparisons per keystroke).
pub(super) fn other_side_columns(
    ca: &CaretAnalysis,
    catalog: &Catalog,
    owner: &str,
) -> HashSet<String> {
    let mut out = HashSet::new();
    for rel in &ca.in_scope {
        if rel.eq_ignore_ascii_case(owner) {
            continue;
        }
        if let Some(inline) = ca.inline_relation(rel) {
            out.extend(inline.columns.iter().map(|c| c.to_ascii_lowercase()));
        } else if let Some(t) = catalog.table(rel) {
            out.extend(t.columns.iter().map(|c| c.name.to_ascii_lowercase()));
        }
    }
    out
}

/// The folded form of a name list, for a membership test asked once per candidate.
///
/// Every one of these was a linear scan once, which is only free while the list is
/// short — and none of them are bounded: a clause region grows with the query, a join's
/// other side with the relation's width.
pub(super) fn folded_set(names: &[String]) -> HashSet<String> {
    names.iter().map(|n| n.to_ascii_lowercase()).collect()
}

/// One pooled candidate: the completion, its **match tier** against the partial, its
/// context tier, and a sub-tier `ord` (curated declaration order within a tier —
/// statement/follow keyword lists carry a deliberate priority).
pub(super) struct Cand {
    pub(super) c: Completion,
    pub(super) mt: u8,
    pub(super) ctx: u8,
    pub(super) ord: u8,
}

/// The candidate pool: **the match happens at the push, not at the rank.**
///
/// Every pool used to be materialized whole and filtered afterwards, so a keystroke
/// built 1600-2700 `Completion`s — three or four string allocations each — no matter
/// how few the partial could possibly match, and the demoted `ALL_KEYWORDS` tail
/// (~1200 of them) was built in full at every operand position only to be dropped by
/// the tail gate. Taking the label first and the `Completion` only on a hit is the rule
/// the all-columns fallback already followed; this is that rule made structural, so no
/// pool can forget it. It also removes the second match: the tier computed here is the
/// one [`rank`] sorts on.
pub(super) struct Pool<'a> {
    cands: Vec<Cand>,
    partial: &'a str,
    manual: bool,
}

impl<'a> Pool<'a> {
    pub(super) fn new(partial: &'a str, manual: bool) -> Self {
        Pool {
            cands: Vec::new(),
            partial,
            manual,
        }
    }

    /// Push at `ctx`, ordered first within its tier.
    pub(super) fn push(&mut self, label: &str, ctx: u8, make: impl FnOnce() -> Completion) {
        self.add(label, ctx, 0, false, make);
    }

    /// Push at `ctx` with a curated sub-order.
    pub(super) fn ordered(
        &mut self,
        label: &str,
        ctx: u8,
        ord: usize,
        make: impl FnOnce() -> Completion,
    ) {
        self.add(label, ctx, ord, false, make);
    }

    /// Push a candidate that may belong to the demoted keyword tail — which needs a
    /// ≥2-char prefix match to appear at all, unless the ask was manual (⌃/⌘Space lifts
    /// the gate: an explicit trigger deserves the full vocabulary).
    pub(super) fn keyword(
        &mut self,
        label: &str,
        ctx: u8,
        tail: bool,
        make: impl FnOnce() -> Completion,
    ) {
        self.add(label, ctx, 0, tail, make);
    }

    /// Whether `label` can match at all — the cheap pre-check for a caller that has
    /// per-candidate work of its own to do *before* it can name the tier to push at.
    pub(super) fn admits(&self, label: &str) -> bool {
        match_tier(label, self.partial).is_some()
    }

    /// Whether a **tail** candidate could appear at all from here. The tail gate is a
    /// property of the ask, not of the candidate, so a caller with a whole demoted
    /// vocabulary to walk can ask once and skip it entirely rather than per entry.
    pub(super) fn tail_possible(&self) -> bool {
        self.manual || self.partial.len() >= 2
    }

    fn add(
        &mut self,
        label: &str,
        ctx: u8,
        ord: usize,
        tail: bool,
        make: impl FnOnce() -> Completion,
    ) {
        let Some(mt) = match_tier(label, self.partial) else {
            return;
        };
        if tail && !self.manual && !(mt <= 1 && self.partial.len() >= 2) {
            return;
        }
        let c = make();
        // The gate label **is** the completion's label — the whole equivalence with
        // filtering after the fact rests on that, at every push site, and a site that
        // gated on something else would silently offer a different set.
        debug_assert_eq!(
            c.label, label,
            "a pool gate must match on the label it pushes"
        );
        self.cands.push(Cand {
            c,
            mt,
            ctx,
            ord: ord.min(u8::MAX as usize) as u8,
        });
    }
}

/// Rank, dedupe, truncate. Sort key: match tier → context tier → curated order → label
/// length → alphabetical. Filtering already happened at the push ([`Pool`]).
pub(super) fn rank(pool: Pool) -> Vec<Completion> {
    let mut ranked: Vec<(u8, u8, u8, Completion)> = pool
        .cands
        .into_iter()
        .map(|c| (c.mt, c.ctx, c.ord, c.c))
        .collect();
    // The alphabetical tie-break compares ASCII-lowercased bytes **lazily** — the same
    // order as comparing two `to_ascii_lowercase` copies, without building them. Inside
    // a comparator those copies were two allocations per *comparison*, so an
    // empty-partial offer (where nothing filters and the pool sorts entire) paid them
    // O(n log n) times.
    ranked.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(a.1.cmp(&b.1))
            .then(a.2.cmp(&b.2))
            .then(a.3.label.len().cmp(&b.3.label.len()))
            .then_with(|| lower_bytes(&a.3.label).cmp(lower_bytes(&b.3.label)))
    });
    let mut seen: HashSet<(CompletionKind, String)> = HashSet::new();
    ranked
        .into_iter()
        .map(|(_, _, _, c)| c)
        .filter(|c| seen.insert((c.kind, c.label.to_ascii_lowercase())))
        .take(RESULT_CAP)
        .collect()
}

/// The label's ASCII-lowercased bytes, as an iterator — the allocation-free form of the
/// key the alphabetical tie-break sorts on.
fn lower_bytes(label: &str) -> impl Iterator<Item = u8> + '_ {
    label.bytes().map(|b| b.to_ascii_lowercase())
}

/// The offer's visible universe — everything past this many never renders (the
/// popup shows ~7 rows and scrolls); `FALLBACK_COLUMN_CAP` sizes its pool against
/// this.
pub(super) const RESULT_CAP: usize = 50;
