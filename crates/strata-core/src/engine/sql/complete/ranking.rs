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
pub(super) fn column_ord(affinity_miss: Option<bool>, cross_miss: Option<bool>, written: bool) -> usize {
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
pub(super) fn other_side_columns(ca: &CaretAnalysis, catalog: &Catalog, owner: &str) -> Vec<String> {
    let mut out = Vec::new();
    for rel in &ca.in_scope {
        if rel.eq_ignore_ascii_case(owner) {
            continue;
        }
        if let Some(inline) = ca.inline_relation(rel) {
            out.extend(inline.columns.iter().cloned());
        } else if let Some(t) = catalog.table(rel) {
            out.extend(t.columns.iter().map(|c| c.name.clone()));
        }
    }
    out
}

/// One pooled candidate before rank: the completion, its context tier, a sub-tier
/// `ord` (curated declaration order within a tier — statement/follow keyword lists
/// carry a deliberate priority), and whether it belongs to the demoted keyword tail.
pub(super) struct Cand {
    pub(super) c: Completion,
    pub(super) ctx: u8,
    pub(super) ord: u8,
    pub(super) tail: bool,
}

impl Cand {
    pub(super) fn new(c: Completion, ctx: u8) -> Self {
        Cand {
            c,
            ctx,
            ord: 0,
            tail: false,
        }
    }

    pub(super) fn ordered(c: Completion, ctx: u8, ord: usize) -> Self {
        Cand {
            c,
            ctx,
            ord: ord.min(u8::MAX as usize) as u8,
            tail: false,
        }
    }
}

/// Filter, rank, dedupe, truncate. Sort key: match tier → context tier → curated
/// order → label length → alphabetical. Tail keywords need a ≥2-char prefix match
/// to appear — unless the ask was manual (⌃/⌘Space lifts the gate: an explicit
/// trigger deserves the full vocabulary).
pub(super) fn rank(pool: Vec<Cand>, partial: &str, manual: bool) -> Vec<Completion> {
    let mut ranked: Vec<(u8, u8, u8, Completion)> = Vec::new();
    for cand in pool {
        let Some(mt) = match_tier(&cand.c.label, partial) else {
            continue;
        };
        if cand.tail && !manual && !(mt <= 1 && partial.len() >= 2) {
            continue;
        }
        ranked.push((mt, cand.ctx, cand.ord, cand.c));
    }
    ranked.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(a.1.cmp(&b.1))
            .then(a.2.cmp(&b.2))
            .then(a.3.label.len().cmp(&b.3.label.len()))
            .then_with(|| {
                a.3.label
                    .to_ascii_lowercase()
                    .cmp(&b.3.label.to_ascii_lowercase())
            })
    });
    let mut seen: HashSet<(CompletionKind, String)> = HashSet::new();
    ranked
        .into_iter()
        .map(|(_, _, _, c)| c)
        .filter(|c| seen.insert((c.kind, c.label.to_ascii_lowercase())))
        .take(RESULT_CAP)
        .collect()
}

/// The offer's visible universe — everything past this many never renders (the
/// popup shows ~7 rows and scrolls); `FALLBACK_COLUMN_CAP` sizes its pool against
/// this.
pub(super) const RESULT_CAP: usize = 50;
