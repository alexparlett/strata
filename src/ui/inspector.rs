//! Right column inspector: the column's facts, one completeness bar, nested fields, and
//! the profile action (D4).
//!
//! **One statistics box, whatever a fact cost to get.** Facts arrive from two places —
//! the source's own metadata (a Parquet footer, free at registration) and a full scan
//! ([`crate::profile`]) — but they're the same facts, so they merge into one list
//! rather than being sorted into tiers the reader has to care about. What matters is
//! that the list is *dynamic*: a row per fact that genuinely exists (a footer carries
//! nulls/min/max, CSV carries nothing, a scan fills the gaps), never a grid of blanks
//! and never a fabricated number. Facts are matched by [`StatKey`], so none appears
//! twice, and inexact ones are marked `~`.
//!
//! Nulls, presence and completeness are **one bar**, not three renderings of the same
//! number.
//!
//! These stats used to be computed from `crate::runs` — i.e. whatever rows sat on the
//! current page of the current tab's query. They described one page of one query while
//! presenting as facts about the column, so they're gone.

use dioxus::prelude::*;

use crate::action::panel::Resizer;
use crate::action::{dispatch, Action};
use crate::engine::{ColumnInfo, Stat, StatKey};
use crate::profile::TableProfile;
use crate::project::ProjectStoreExt;
use crate::ui::components::{
    Button, ButtonVariant, Dot, Eyebrow, Icon, IconButton, IconButtonVariant, Meta, MonoValue,
    Path, Prose, Readout, Tooltip,
};
use crate::ui::icons::{IconName, IconSize};
use crate::util::Kind;

/// The inspector's label for a fact.
fn label(key: StatKey) -> &'static str {
    match key {
        StatKey::Nulls => "Nulls",
        StatKey::Min => "Min",
        StatKey::Max => "Max",
        StatKey::Distinct => "Distinct",
        StatKey::Mean => "Mean",
        StatKey::Median => "Median",
    }
}

/// A fact's value. Inexact ones are marked `~`: a Parquet footer truncates long strings
/// and binary, so what it stored is a bound, not the value — showing it bare would be
/// exactly the fabrication this panel exists to avoid.
fn value(s: &Stat) -> String {
    if s.exact {
        s.text.clone()
    } else {
        format!("~{}", s.text)
    }
}

/// Resolve a column path (`["address", "city"]`) by walking `children`.
///
/// Resolving only the first segment was the old bug: a nested `address.city` was looked
/// up among the top-level columns, so it either found nothing — a blank panel — or, far
/// worse, found an unrelated top-level `city` and showed *its* facts as this column's.
fn resolve<'a>(cols: &'a [ColumnInfo], path: &[String]) -> Option<&'a ColumnInfo> {
    let (first, rest) = path.split_first()?;
    let col = cols.iter().find(|c| &c.name == first)?;
    if rest.is_empty() {
        Some(col)
    } else {
        resolve(&col.children, rest)
    }
}

/// Every fact known about the column — free and scanned — as one list.
///
/// Where a number came from doesn't change what it is, so they merge. A free fact wins
/// a tie: it's already on hand, and by definition says the same thing. Fixed order, so
/// the box doesn't reshuffle when a scan lands. `Nulls` is deliberately absent — it's
/// the completeness bar, and drawing it as a row too would be the third rendering of
/// one number.
fn merged_facts(free: &[Stat], profile: &Option<TableProfile>, path: &[String]) -> Vec<Stat> {
    const ORDER: [StatKey; 5] = [
        StatKey::Distinct,
        StatKey::Min,
        StatKey::Max,
        StatKey::Mean,
        StatKey::Median,
    ];
    let scanned = profile_stats(profile, path);
    ORDER
        .iter()
        .filter_map(|k| {
            free.iter()
                .find(|s| s.key == *k)
                .or_else(|| scanned.and_then(|v| v.iter().find(|s| s.key == *k)))
                .cloned()
        })
        .collect()
}

/// The column's null count, from wherever it's honestly known — the footer first, then
/// a scan.
fn null_count(free: &[Stat], profile: &Option<TableProfile>, path: &[String]) -> Option<u64> {
    free.iter()
        .find(|s| s.key == StatKey::Nulls)
        .or_else(|| {
            profile_stats(profile, path).and_then(|v| v.iter().find(|s| s.key == StatKey::Nulls))
        })
        .and_then(|s| s.text.parse::<u64>().ok())
}

#[component]
pub fn Inspector() -> Element {
    let store = crate::project::store();
    let tables_lens = store.tables();
    let views_lens = store.views();
    let tables = tables_lens.read();
    let views = views_lens.read();
    // The inspector owns its own width — a local reactive signal, not global state.
    let width = use_signal(|| 292.0);

    let Some((table, path)) = crate::inspector::selected() else {
        return rsx! {
            Resizer { axis_x: true, sign: -1.0, min: 220.0, max: 560.0, size: width }
            aside { class: "ps-inspector", style: "width:{width}px;",
                div { class: "insp-head",
                    Eyebrow { class: "sec-label", "COLUMN INSPECTOR" }
                    IconButton { icon: IconName::Close, icon_size: IconSize::Xs, variant: IconButtonVariant::Ghost, title: "Close inspector",
                        onclick: move |_| dispatch(Action::CloseInspector), }
                }
                Prose { style: "padding:var(--sp-6) var(--sp-4);color:var(--dim2);", "Select a column to inspect." }
            }
        };
    };

    // The column's catalog row. A view resolves too, but has no files under it — no
    // footers to read and nothing to scan, so it gets neither tier.
    let row = tables.iter().find(|t| t.name == table);
    let colinfo = row
        .and_then(|t| resolve(&t.columns, &path).cloned())
        .or_else(|| {
            views
                .iter()
                .find(|v| v.name == table)
                .and_then(|v| resolve(&v.columns, &path).cloned())
        });
    // The leaf's own name — the path is how it's found, not what it's called.
    let colname = path.last().cloned().unwrap_or_default();
    // A nested *field* (a struct's child), which is a position, not a type: a top-level
    // struct column is not one.
    let is_child = path.len() > 1;
    let kind = colinfo.as_ref().map(|c| c.kind).unwrap_or(Kind::Str);
    let dtype = colinfo.as_ref().map(|c| c.dtype.clone()).unwrap_or_default();
    let free: Vec<Stat> = colinfo.as_ref().map(|c| c.stats.clone()).unwrap_or_default();
    let children = colinfo.map(|c| c.children).unwrap_or_default();
    let src_fmt = row
        .map(|t| t.format.to_uppercase())
        .unwrap_or_else(|| "VIEW".to_string());
    let rows = row.and_then(|t| t.rows);
    let profile = row.and_then(|t| t.profile.clone());
    let profiling = row.map(|t| t.profiling).unwrap_or(false);
    let profilable = row.is_some();

    let dot = kind.dot_color();
    let tcls = kind.text_class();
    let nested = kind.is_nested();

    let facts = merged_facts(&free, &profile, &path);
    let has_sql = profile.as_ref().is_some_and(|p| !p.sql.is_empty());
    let (tsql, tref) = (table.clone(), table.clone());
    // One bar for nulls / presence / completeness — the same number three ways was the
    // duplication. It needs a null count from somewhere real (footer, else scan);
    // without one there's nothing honest to draw, so it doesn't appear.
    let total = rows.or(profile.as_ref().map(|p| p.rows));
    let fill = match (null_count(&free, &profile, &path), total) {
        (Some(n), Some(t)) if t > 0 => {
            let pct = 100.0 - (n as f64 / t as f64 * 100.0);
            Some((
                pct,
                format!("{pct:.0}%"),
                format!("{n} of {t} rows are null — the bar is the share with a value."),
            ))
        }
        _ => None,
    };

    rsx! {
        Resizer { axis_x: true, sign: -1.0, min: 220.0, max: 560.0, size: width }
        aside { class: "ps-inspector ps-scroll", style: "width:{width}px;",
            div { class: "insp-head",
                Eyebrow { class: "sec-label", "COLUMN INSPECTOR" }
                IconButton { icon: IconName::Close, icon_size: IconSize::Xs, variant: IconButtonVariant::Ghost, title: "Close inspector",
                    onclick: move |_| dispatch(Action::CloseInspector), }
            }

            div { class: "insp-title",
                div { class: "row", style: "gap:var(--sp-3);",
                    Dot { color: "{dot}", square: true, size: 8 }
                    MonoValue { class: "insp-name", "{colname}" }
                }
                div { class: "row", style: "gap:var(--sp-3);margin-top:var(--sp-3);",
                    Meta { class: "{tcls} insp-dtype", "{dtype}" }
                    Meta { class: "insp-srcfmt", "{src_fmt}" }
                    Path { "from {table}" }
                }
            }

            // The section owns the scan's controls: they act on everything below them,
            // so they sit above it. The offer/spinner stays under the stats (see
            // `profile_slot`) — it's the thing you press to fill them in.
            div { class: "insp-stats-head",
                Eyebrow { class: "sec-label", "STATISTICS" }
                // Not on a nested field: it carries no scanned facts, so reporting a
                // scan's age over its box — and offering to re-run one — would be
                // describing something that isn't there. Same rule as the offer below.
                if let Some(age) = profile.as_ref().map(age_label).filter(|_| !profiling && !is_child) {
                    span { class: "prof-ctl",
                        Meta { class: "prof-age", "scanned {age}" }
                        // Only when there's a query to show — an un-renderable plan
                        // leaves `sql` empty rather than opening a blank tab.
                        if has_sql {
                            IconButton { icon: IconName::Brackets, icon_size: IconSize::Sm, variant: IconButtonVariant::Ghost,
                                title: "View as query",
                                onclick: move |_| dispatch(Action::OpenProfileSql(tsql.clone())) }
                        }
                        // An explicit re-run of something already chosen — no confirm.
                        IconButton { icon: IconName::Refresh, icon_size: IconSize::Sm, variant: IconButtonVariant::Ghost,
                            title: "Re-scan",
                            onclick: move |_| dispatch(Action::ProfileTable(tref.clone())) }
                    }
                }
            }

            // One statistics box: every fact, free or scanned. Type is always known; the
            // rest appear only where they exist, which is the entire point.
            div { class: "insp-facts",
                {fact_row("Type", &dtype, "var(--accent)")}
                if let Some(n) = rows {
                    {fact_row("Rows", &n.to_string(), "var(--text)")}
                }
                for s in facts.iter() {
                    {fact_row(label(s.key), &value(s), "var(--text)")}
                }
            }

            if let Some((w, pct, detail)) = fill {
                div { class: "insp-section",
                    div { class: "row", style: "justify-content:space-between;margin-bottom:var(--sp-3);",
                        // The (i) is the affordance — a tooltip nobody knows is there
                        // may as well not exist. Hovering either part shows it.
                        Tooltip {
                            message: detail,
                            span { class: "insp-hint",
                                Meta { "Completeness" }
                                Icon { name: IconName::Info, size: IconSize::Xs }
                            }
                        }
                        Meta { style: "color:var(--text2);", "{pct}" }
                    }
                    div { class: "fill-track", div { class: "fill-bar", style: "width:{w}%;" } }
                }
            }

            // The profile action sits below the statistics it fills in — button,
            // spinner and age/re-scan all occupy this one slot.
            if profilable {
                {profile_slot(&table, is_child, &profile, profiling)}
            }

            if nested {
                div { class: "insp-note", Path { "Nested column — expand values in the results grid (click a cell) or use get_field / unnest to project fields." } }
                if !children.is_empty() {
                    div { class: "insp-section",
                        Eyebrow { class: "sec-label", style: "margin-bottom:var(--sp-3);", "NESTED FIELDS" }
                        div { class: "nested-box",
                            {nested_rows(&children, 0)}
                        }
                    }
                }
            }
        }
    }
}

/// The facts a profile holds for the column at `path`.
///
/// Only top-level columns are profiled, and the map is keyed by their names — so this
/// must refuse a nested path outright. Looking up by leaf name would let
/// `address.city` collect an unrelated top-level `city`'s facts: the same confusion of
/// name for identity that the path fixes.
fn profile_stats<'a>(profile: &'a Option<TableProfile>, path: &[String]) -> Option<&'a Vec<Stat>> {
    let [name] = path else { return None };
    profile.as_ref()?.cols.get(name)
}

/// The profile *offer* (D4) — one slot below the statistics it fills in.
///
/// Only two states live here: the button, and the spinner that replaces it. They're the
/// same thing — the action you pressed, and it still running — so they share a place.
/// Once a scan exists there's nothing to offer, and its controls (age, view-as-query,
/// re-scan) live in the STATISTICS header instead, since they act on everything below.
///
/// Offered for **any top-level column**, whatever its type: the action profiles the
/// *table*, so hiding it because a struct happens to be selected would be arbitrary.
/// What it isn't offered for is a nested *field* — a struct's child — which is a
/// position (`is_child`), not a type. `kind.is_nested()` is the wrong test: it asks
/// whether the column *contains* fields, not whether it *is* one.
fn profile_slot(
    table: &str,
    is_child: bool,
    profile: &Option<TableProfile>,
    profiling: bool,
) -> Element {
    let (t2, t3) = (table.to_string(), table.to_string());

    // A nested field is never the place to launch a table-wide scan from.
    if is_child {
        return rsx! {};
    }
    // Nothing left to offer once a scan exists — the header carries its controls.
    if !profiling && profile.is_some() {
        return rsx! {};
    }

    rsx! {
        div { class: "insp-section",
            if profiling {
                div { class: "prof-running",
                    Icon { name: IconName::Spinner, size: IconSize::Sm }
                    Readout { class: "prof-running-txt", "Profiling…" }
                    // A full scan can run for minutes — it has to be stoppable.
                    IconButton { icon: IconName::Close, icon_size: IconSize::Xs, variant: IconButtonVariant::Ghost,
                        title: "Stop",
                        onclick: move |_| dispatch(Action::CancelProfileTable(t3.clone())) }
                }
            } else {
                div { class: "prof-empty",
                    Prose { class: "prof-empty-note",
                        "Reads every file to compute distinct counts, means and distributions. The result is cached until the table changes."
                    }
                    // Confirms, like the context menu's Profile: the same action from
                    // two places shouldn't warn from only one of them.
                    Button { variant: ButtonVariant::Secondary, small: true,
                        icon: IconName::Chart, icon_size: IconSize::Sm,
                        onclick: move |_| dispatch(Action::AskProfileTable(t2.clone())),
                        "Profile table"
                    }
                }
            }
        }
    }
}

/// Nested fields, indented by depth. Display only — profiling never descends here.
fn nested_rows(fields: &[ColumnInfo], depth: usize) -> Element {
    let indent = depth * 10;
    rsx! {
        for f in fields.iter() {
            div { class: "nested-field", style: "padding-left:{indent}px;",
                Dot { color: "{f.kind.dot_color()}", square: true, size: 6 }
                Readout { class: "fname", "{f.name}" }
                Meta { class: "ftype {f.kind.text_class()}", "{f.dtype}" }
            }
            if !f.children.is_empty() {
                {nested_rows(&f.children, depth + 1)}
            }
        }
    }
}

/// "just now" / "4m ago" / "2h ago" — how stale the profile is.
fn age_label(p: &TableProfile) -> String {
    let secs = p.at.elapsed().map(|d| d.as_secs()).unwrap_or(0);
    match secs {
        0..=59 => "just now".to_string(),
        60..=3599 => format!("{}m ago", secs / 60),
        3600..=86399 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86400),
    }
}

fn fact_row(label: &str, value: &str, color: &str) -> Element {
    rsx! {
        div { class: "insp-fact",
            Meta { class: "k", "{label}" }
            MonoValue { class: "v", style: "color:{color};", "{value}" }
        }
    }
}
