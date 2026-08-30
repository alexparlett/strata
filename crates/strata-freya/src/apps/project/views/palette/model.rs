//! The palette's **index**: everything it can offer, and what a query narrows that to.
//!
//! Five kinds of thing, because the window holds five: the [`Action`]s the command registry
//! declares, and then the project's tables, views, saved queries and columns. The first is code,
//! the rest are the catalog — so an [`Entry`] is either a pointer into a static table or a name
//! read off the store, never a copy of a def.
//!
//! **Group order is fixed and empty groups vanish** — a result list whose sections move about as
//! you type is one you cannot aim at.
//!
//! **An empty query hides COLUMNS.** Every other group is bounded by the project and worth offering
//! cold; columns are thousands of rows that answer nothing until you have typed something.
//!
//! **A word matches anywhere**, so "sales limit" and "limit sales" find the same row — the rule the
//! Settings search box already holds ([`crate::apps::settings::search`]), because a user who has
//! learned one of this app's search boxes has learned the other.
//!
//! **The cap is per group, not overall — and only the catalog groups have one.** A global cap lets
//! a common substring fill the list with columns and push the table you were after off the bottom.
//! ACTIONS is uncapped because it is a fixed set defined in code rather than an unbounded
//! project-scoped list: capping it once **hid a command**, when the registry grew to nine against a
//! cap of eight and Settings… silently stopped being offered.
//!
//! **Only top-level columns.** `ColumnInfo::children` is a tree carrying 241,425 nested fields in
//! this repo's own reference fixture, so walking it to build a search index would be that same
//! unbounded materialization paid on every open. Views' columns are indexed as well as tables'.

use strata_engine::Registrations;
use strata_model::{CatalogKind, ColRef, Kind, SavedQuery};
use uuid::Uuid;

use crate::apps::project::commands::Action;
use crate::apps::project::state::ProjectState;

/// How many rows a **catalog** group offers at once. A section longer than a glance is a section
/// that has not narrowed anything — and the whole point of typing more is to shorten it.
///
/// [`Group::Actions`] is exempt; see [`Group::cap`].
pub const MAX_PER_GROUP: usize = 8;

/// The palette's sections, in the order it offers them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Group {
    Actions,
    Tables,
    Views,
    SavedQueries,
    Columns,
}

impl Group {
    /// The order the list reads in — also the order [`Index::search`] returns.
    pub const ALL: &'static [Group] = &[
        Group::Actions,
        Group::Tables,
        Group::Views,
        Group::SavedQueries,
        Group::Columns,
    ];

    /// How many rows this group may offer, or `None` for "all of them".
    ///
    /// **Only the catalog groups are capped.** The cap exists because a project's tables, views,
    /// saved queries and especially columns are unbounded — a common substring would otherwise
    /// fill the list with columns and push the table you were after off the bottom.
    /// [`Actions`](Self::Actions) is none of that: it is the command registry, a fixed list
    /// defined in code, and capping it means the palette silently *hides a command* — which is
    /// the exact opposite of what the surface is for. It hid the ninth one (Settings…, the last
    /// declared) for as long as the cap applied here.
    pub fn cap(self) -> Option<usize> {
        match self {
            Group::Actions => None,
            _ => Some(MAX_PER_GROUP),
        }
    }

    /// The section heading.
    pub fn title(self) -> &'static str {
        match self {
            Group::Actions => "ACTIONS",
            Group::Tables => "TABLES",
            Group::Views => "VIEWS",
            Group::SavedQueries => "SAVED QUERIES",
            Group::Columns => "COLUMNS",
        }
    }
}

/// One offerable thing.
#[derive(Clone, PartialEq, Debug)]
pub enum Entry {
    /// A command the registry declares — a pointer into
    /// [`ROUTES`](crate::apps::project::commands::ROUTES), not a copy of one.
    Action(Action),
    /// A registered table: open it in a tab.
    Table { name: String, meta: String },
    /// A saved view: open its rows in a tab.
    View { name: String, meta: String },
    /// A saved query, addressed by id — its name is only a label.
    Query { id: Uuid, name: String },
    /// A top-level column of a table or view: select it and reveal the inspector.
    Column {
        col: ColRef,
        dtype: String,
        kind: Kind,
        /// A partition key, which is a fact about the table rather than about the file.
        part: bool,
    },
}

impl Entry {
    pub fn group(&self) -> Group {
        match self {
            Entry::Action(_) => Group::Actions,
            Entry::Table { .. } => Group::Tables,
            Entry::View { .. } => Group::Views,
            Entry::Query { .. } => Group::SavedQueries,
            Entry::Column { .. } => Group::Columns,
        }
    }

    /// What the row is called. A column is `table.column`, because its own name is not unique
    /// across the catalog and "id" on its own names nothing.
    pub fn label(&self) -> String {
        match self {
            Entry::Action(action) => action.label().to_string(),
            Entry::Table { name, .. } | Entry::View { name, .. } => name.clone(),
            Entry::Query { name, .. } => name.clone(),
            Entry::Column { col, .. } => {
                format!("{}.{}", col.owner.label(), col.path.join("."))
            }
        }
    }

    /// The mono detail after the label — the row's second fact, or empty where the label says
    /// it all.
    pub fn sub(&self) -> String {
        match self {
            Entry::Action(action) => action.sub().to_string(),
            Entry::Table { meta, .. } | Entry::View { meta, .. } => meta.clone(),
            Entry::Query { .. } => "saved query".to_string(),
            Entry::Column { dtype, part, .. } => match part {
                true => format!("{dtype} · partition"),
                false => dtype.clone(),
            },
        }
    }

    /// This entry's stable identity — what the list keys its rows on, so retyping re-associates
    /// a row with its entry rather than shifting hover state along the list.
    pub fn id(&self) -> String {
        match self {
            Entry::Action(action) => format!("cmd:{}", action.id()),
            Entry::Table { name, .. } => format!("table:{name}"),
            Entry::View { name, .. } => format!("view:{name}"),
            Entry::Query { id, .. } => format!("query:{id}"),
            Entry::Column { col, .. } => {
                format!("col:{:?}:{}", col.owner, col.path.join("."))
            }
        }
    }

    /// Everything a query is matched against, lowercased: what it is called, what it says about
    /// itself, and — for a command — the words that should find it but appear in neither.
    ///
    /// Computed **once**, into [`Index`], never per keystroke: it allocates four strings an entry
    /// and there is one entry per column in the project, so re-deriving it on every character
    /// would be tens of thousands of allocations a keystroke on the thread that draws every
    /// window.
    fn haystack(&self) -> String {
        let extra = match self {
            Entry::Action(action) => action.keywords(),
            _ => "",
        };
        format!("{} {} {extra}", self.label(), self.sub()).to_lowercase()
    }
}

/// The palette's snapshot: everything it can offer, with the text each entry is matched against
/// already lowercased. Built once when the card opens; [`Index::search`] reads it per keystroke and
/// allocates nothing but the rows that matched.
pub struct Index {
    entries: Vec<Entry>,
    /// `entries[i]`'s searchable text — see [`Entry::haystack`].
    haystacks: Vec<String>,
}

/// The rows a query narrowed the index to, **flat**, plus where each heading falls in them.
///
/// One list rather than a list per group and a flattened copy beside it: ↑↓ step along the flat
/// order and the headings are not stops, so the groups only need to say where they begin.
pub struct Results {
    /// Every matched row, in reading order.
    pub rows: Vec<Entry>,
    /// Each group's heading and the index in [`rows`](Self::rows) it sits above.
    pub groups: Vec<(Group, usize)>,
}

/// Everything the palette can offer for this project, in group order.
///
/// A **snapshot**: built once when the overlay opens and filtered per keystroke, the same trade
/// the catalog row menus make (`views::sidebar::catalog::menu`). A palette shows what was true
/// when it opened, and acting on it dismisses it.
fn entries(project: &ProjectState, registrations: &Registrations) -> Vec<Entry> {
    let answers = &registrations.workspace;
    let mut entries: Vec<Entry> = Action::ALL.iter().copied().map(Entry::Action).collect();

    for table in &project.tables {
        entries.push(Entry::Table {
            name: table.def.name.clone(),
            meta: table.meta_label(answers.status(&table.def.name)),
        });
    }
    for view in &project.views {
        let cols = view.info.as_ref().map(|v| v.columns.len());
        entries.push(Entry::View {
            name: view.def.name.clone(),
            meta: match cols {
                Some(n) => format!("{n} cols · view"),
                None => "view".to_string(),
            },
        });
    }
    for SavedQuery { id, name, .. } in &project.saved_queries {
        entries.push(Entry::Query {
            id: *id,
            name: name.clone(),
        });
    }

    for table in &project.tables {
        let Some(meta) = &table.meta else {
            continue;
        };
        for column in &meta.columns {
            entries.push(Entry::Column {
                col: ColRef::entry(
                    CatalogKind::Table,
                    table.def.name.clone(),
                    vec![column.name.clone()],
                ),
                dtype: column.dtype.clone(),
                kind: column.kind,
                part: table
                    .def
                    .partition_cols
                    .iter()
                    .any(|(name, _)| *name == column.name),
            });
        }
    }
    for view in &project.views {
        let Some(info) = &view.info else {
            continue;
        };
        for column in &info.columns {
            entries.push(Entry::Column {
                col: ColRef::entry(
                    CatalogKind::View,
                    view.def.name.clone(),
                    vec![column.name.clone()],
                ),
                dtype: column.dtype.clone(),
                kind: column.kind,
                part: false,
            });
        }
    }
    entries
}

impl Index {
    /// Build the snapshot for this project.
    pub fn new(project: &ProjectState, registrations: &Registrations) -> Self {
        let entries = entries(project, registrations);
        let haystacks = entries.iter().map(Entry::haystack).collect();
        Self { entries, haystacks }
    }

    /// What `query` narrows the index to: sections in [`Group::ALL`] order, each capped by its
    /// own [`Group::cap`], empty ones dropped, flattened into one list of rows.
    ///
    /// An empty query is not a search — it offers everything except [`Group::Columns`] (see the
    /// module doc). A non-empty one matches every one of its words somewhere in the entry.
    ///
    /// Each surviving entry is cloned **once**; nothing else here allocates per keystroke,
    /// because the text being searched was lowercased when the index was built.
    pub fn search(&self, query: &str) -> Results {
        let query = query.trim().to_lowercase();
        let terms: Vec<&str> = query.split_whitespace().collect();
        let matches = |i: usize, entry: &Entry| {
            if terms.is_empty() {
                return entry.group() != Group::Columns;
            }
            terms.iter().all(|term| self.haystacks[i].contains(term))
        };

        let mut results = Results {
            rows: Vec::new(),
            groups: Vec::new(),
        };
        for group in Group::ALL {
            let start = results.rows.len();
            results.rows.extend(
                self.entries
                    .iter()
                    .enumerate()
                    .filter(|(i, entry)| entry.group() == *group && matches(*i, entry))
                    .take(group.cap().unwrap_or(usize::MAX))
                    .map(|(_, entry)| entry.clone()),
            );
            if results.rows.len() > start {
                results.groups.push((*group, start));
            }
        }
        results
    }
}

impl Results {
    /// Whether nothing matched — the empty state.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The heading that sits above row `index`, if one does.
    pub fn heading(&self, index: usize) -> Option<Group> {
        self.groups
            .iter()
            .find(|(_, start)| *start == index)
            .map(|(group, _)| *group)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use datafusion::arrow::datatypes::{DataType, Field};
    use strata_arrow::column_info;
    use strata_engine::{Answers, CatalogGen, RegStatus, TableMeta};
    use strata_model::{ColumnInfo, SourceFormat, TableDef, TableOrigin, ViewDef};

    use super::*;
    use crate::apps::project::state::{TableRow, ViewInfo, ViewRow};

    fn column(name: &str, dtype: DataType) -> ColumnInfo {
        column_info(&Field::new(name, dtype, true))
    }

    fn table(name: &str, partition_cols: &[&str], meta: Option<TableMeta>) -> TableRow {
        TableRow {
            def: TableDef {
                name: name.to_string(),
                format: SourceFormat::Parquet,
                source: None,
                paths: vec![format!("{name}/")],
                partition_cols: partition_cols
                    .iter()
                    .map(|c| (c.to_string(), "Utf8".to_string()))
                    .collect(),
                origin: TableOrigin::External,
            },
            meta,
            profile: None,
        }
    }

    /// What the engine answered about [`store`]'s defs — the half of a row that says whether it
    /// registered.
    fn answered() -> Registrations {
        Registrations {
            workspace: Answers::recorded(
                [
                    ("orders".to_string(), RegStatus::Ready),
                    (
                        "broken".to_string(),
                        RegStatus::Failed {
                            reason: "No files found".into(),
                        },
                    ),
                    ("revenue".to_string(), RegStatus::Ready),
                ],
                CatalogGen::default(),
            ),
            ..Default::default()
        }
    }

    /// A store built inline — never a production signature bent to be testable.
    /// One registered table with a partition key, one view over it, one saved query, and one
    /// table the engine refused.
    fn store() -> ProjectState {
        ProjectState {
            name: "sales".to_string(),
            root: PathBuf::from("/data/sales"),
            sources: Vec::new(),
            tables: vec![
                table(
                    "orders",
                    &["country"],
                    Some(TableMeta {
                        columns: vec![
                            column("order_id", DataType::Int64),
                            column("country", DataType::Utf8),
                        ],
                        rows: Some(1_000),
                    }),
                ),
                table("broken", &[], None),
            ],
            views: vec![ViewRow {
                def: ViewDef {
                    name: "revenue".to_string(),
                    sql: "SELECT 1".to_string(),
                },
                info: Some(ViewInfo {
                    columns: vec![column("total", DataType::Float64)],
                    deps: vec!["orders".to_string()],
                    remote_deps: Vec::new(),
                    view_deps: Vec::new(),
                }),
                profile: None,
            }],
            saved_queries: vec![SavedQuery {
                id: Uuid::nil(),
                name: "top countries".to_string(),
                sql: "SELECT 1".to_string(),
                meta: String::new(),
            }],
        }
    }

    /// The labels a group's rows carry, read back out of the flat result list.
    fn labels(results: &Results, group: Group) -> Vec<String> {
        results
            .rows
            .iter()
            .filter(|entry| entry.group() == group)
            .map(Entry::label)
            .collect()
    }

    /// The groups a search produced, in the order they read.
    fn present(results: &Results) -> Vec<Group> {
        results.groups.iter().map(|(group, _)| *group).collect()
    }

    /// The one thing a cold palette must get right: it offers the project, and it does **not**
    /// offer every column in it.
    #[test]
    fn an_empty_query_offers_everything_but_the_columns() {
        let groups = Index::new(&store(), &answered()).search("");
        assert_eq!(
            present(&groups),
            [
                Group::Actions,
                Group::Tables,
                Group::Views,
                Group::SavedQueries
            ]
        );
        assert_eq!(labels(&groups, Group::Tables), ["orders", "broken"]);
    }

    /// **Every** command is offered, not the first [`MAX_PER_GROUP`] of them.
    ///
    /// The regression this pins is one the test above could not see, because it asserted which
    /// groups were present and never how many rows ACTIONS held: the registry grew to nine
    /// commands against a cap of eight, so the ninth — Settings…, the last declared — was
    /// silently dropped from a cold palette and only reappeared once a query narrowed ACTIONS
    /// below the cap. A palette that hides a command is worse than no palette, and the number of
    /// commands is expected to keep growing, so this asserts against `Action::ALL` rather than
    /// against a figure that would need updating with it.
    #[test]
    fn every_command_is_offered_however_many_there_are() {
        let groups = Index::new(&store(), &answered()).search("");
        let offered = labels(&groups, Group::Actions);
        let declared: Vec<String> = Action::ALL.iter().map(|a| a.label().to_string()).collect();
        assert_eq!(offered, declared);
        assert!(
            declared.len() > MAX_PER_GROUP,
            "the registry has shrunk below the catalog cap, so this no longer proves anything — \
             the ACTIONS group must stay uncapped regardless"
        );
    }

    /// …and typing brings them in.
    #[test]
    fn a_column_is_found_by_its_own_name() {
        let groups = Index::new(&store(), &answered()).search("order_id");
        assert_eq!(labels(&groups, Group::Columns), ["orders.order_id"]);
    }

    /// A view's columns are indexed too — the sidebar lists them, so the palette must find them.
    #[test]
    fn a_views_columns_are_indexed() {
        let groups = Index::new(&store(), &answered()).search("total");
        assert_eq!(labels(&groups, Group::Columns), ["revenue.total"]);
    }

    /// A def the engine refused keeps its row — that is what the catalog is for — but has no
    /// schema behind it, so it contributes no columns.
    #[test]
    fn a_failed_table_is_listed_but_has_no_columns() {
        let all = entries(&store(), &answered());
        assert!(all.iter().any(|e| e.label() == "broken"));
        assert!(!all.iter().any(|e| e.label().starts_with("broken.")));
    }

    /// Every word has to match, in any order and any case — the settings box's rule.
    #[test]
    fn words_match_in_any_order() {
        let index = Index::new(&store(), &answered());
        for query in ["top countries", "countries top", "TOP Countries"] {
            assert_eq!(
                labels(&index.search(query), Group::SavedQueries),
                ["top countries"],
                "{query}"
            );
        }
    }

    /// A command is found by what it *does*, not only by its name — nothing in the registry is
    /// called "execute".
    #[test]
    fn a_command_is_found_by_its_keywords() {
        let groups = Index::new(&store(), &answered()).search("execute");
        assert_eq!(labels(&groups, Group::Actions), ["Run query"]);
    }

    /// Group order is the list's order, whatever matched — and a miss is an empty list rather
    /// than a list of empty sections.
    #[test]
    fn groups_keep_their_order_and_empty_ones_vanish() {
        let index = Index::new(&store(), &answered());
        let groups = index.search("o");
        let order = present(&groups);
        let mut expected = order.clone();
        expected.sort_by_key(|g| Group::ALL.iter().position(|x| x == g));
        assert_eq!(order, expected);
        assert!(groups
            .groups
            .iter()
            .all(|(_, start)| *start < groups.rows.len()));

        let miss = index.search("kubernetes");
        assert!(miss.is_empty() && miss.groups.is_empty());
    }

    /// The cap is per group: a query matching many columns must not cost the table row its place.
    #[test]
    fn the_cap_is_per_group() {
        let mut project = store();
        let Some(meta) = &mut project.tables[0].meta else {
            unreachable!("the fixture's first table is registered")
        };
        meta.columns = (0..20)
            .map(|i| column(&format!("c{i}"), DataType::Int64))
            .collect();

        let groups = Index::new(&project, &answered()).search("c");
        assert_eq!(labels(&groups, Group::Columns).len(), MAX_PER_GROUP);
        assert!(
            labels(&groups, Group::Tables).contains(&"orders".to_string()),
            "a flood of columns must not push the table out"
        );
    }

    /// Two rows a reader cannot tell apart would be two rows keyed the same — the History
    /// drawer's collapse rule, applied to a list that is rebuilt on every keystroke.
    #[test]
    fn every_entry_has_its_own_id() {
        let mut ids: Vec<String> = entries(&store(), &answered())
            .iter()
            .map(Entry::id)
            .collect();
        let count = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(count, ids.len(), "duplicate entry id");
    }

    /// The flat list is what ↑↓ walk: every matched row, in the order they are drawn, headings
    /// excluded.
    #[test]
    fn rows_read_in_group_order_and_headings_point_into_them() {
        let groups = Index::new(&store(), &answered()).search("");
        assert_eq!(groups.rows[0], Entry::Action(Action::ALL[0]));
        assert_eq!(groups.heading(0), Some(Group::Actions));
        for (index, entry) in groups.rows.iter().enumerate() {
            let heading = groups
                .groups
                .iter()
                .rfind(|(_, start)| *start <= index)
                .expect("every row sits under a heading");
            assert_eq!(heading.0, entry.group());
        }
    }

    /// A view's row says how wide it is, which is the only fact a view has before it is scanned
    /// — it has no files under it to report anything else for free.
    #[test]
    fn a_view_row_reports_its_column_count() {
        let groups = Index::new(&store(), &answered()).search("revenue");
        assert_eq!(
            groups
                .rows
                .iter()
                .find(|entry| entry.group() == Group::Views)
                .map(Entry::sub),
            Some("1 cols · view".to_string())
        );
    }
}
