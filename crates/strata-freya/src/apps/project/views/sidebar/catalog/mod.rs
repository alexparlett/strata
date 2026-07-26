//! The **catalog** sidebar pane (P3-02): TABLES · VIEWS · QUERIES, each a collapsible section of
//! rows that expand to their columns, over a filter that spans all three.
//!
//! ## Where the data comes from
//!
//! The [`ProjectState`] store — the project file's defs plus what engine registration *learned*
//! about each ([`Reg`](crate::apps::project::state::Reg)). **Not** an introspection query
//! against DataFusion, which would be wrong in both directions: result snapshots are
//! registered as real tables (`__snap_*`) and would show up, and a def whose registration
//! *failed* has no engine presence at all yet is exactly the row
//! the catalog must keep showing. Saved queries aren't a DataFusion concept either. The store is
//! also the ⌘S save-target store, so a second cached copy would be two sources of truth.
//!
//! ## Subscriptions
//!
//! Each section subscribes to its own [`ProjChan`], so a table registration landing wakes the
//! TABLES section alone — not the views or saved queries. That is what the store's per-section
//! channels were built for.
//!
//! ## Local UI state
//!
//! Filter text, which sections are collapsed, which entries are open, and which nested columns are
//! expanded are all **pane-local** — none of it is project data, none of it persists.

mod columns;
mod entry;
#[cfg(test)]
mod interaction;
mod menu;
mod section;

use std::collections::HashSet;

use freya::components::{define_theme, get_theme, ScrollView};
use freya::prelude::*;
use freya::radio::use_radio;
use strata_model::CatalogKind;

use self::entry::{EntryRow, SavedQueryRow};
use self::section::CatalogSection;
use crate::apps::project::state::{ProjChan, ProjectState};
use crate::components::typography::Caption;

define_theme!(
    %[component]
    pub Catalog {
        %[fields]
        label_color: Color,
        chevron_color: Color,
        name_color: Color,
        column_color: Color,
        meta_color: Color,
        rail_fill: Color,
        table_color: Color,
        view_color: Color,
        query_color: Color,
        part_color: Color,
        part_background: Color,
        warn_color: Color,
    }
);

/// Does `name` survive the filter? Case-insensitive substring over the **def name** — the filter
/// spans the three sections, not the column trees inside them.
fn matches(name: &str, filter: &str) -> bool {
    filter.is_empty() || name.to_lowercase().contains(&filter.to_lowercase())
}

/// The catalog tree — the sidebar body under the filter row. `filter` is owned by the sidebar
/// shell (it lives in the header row beside the refresh button) and read here.
#[derive(PartialEq)]
pub struct Catalog {
    pub filter: State<String>,
    pub theme: Option<CatalogThemePartial>,
}

impl Catalog {
    pub fn new(filter: State<String>) -> Self {
        Self {
            filter,
            theme: None,
        }
    }
}

impl Component for Catalog {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(&self.theme, CatalogThemePreference, "catalog");
        let filter = self.filter.read().clone();

        // Which entries are expanded to their columns, keyed `"{kind}::{name}"`, and which nested
        // columns are open, keyed `"{owner}::{a.b}"`. Both pane-local: expansion is a way of
        // looking, not project data.
        let open_entries = use_state(HashSet::<String>::new);
        let expanded_cols = use_state(HashSet::<String>::new);

        // `ScrollView` takes no padding of its own, so the scroll body's inset lives on a wrapper
        // inside it — which is also what keeps the scrollbar flush to the panel edge.
        let body = rect()
            .width(Size::fill())
            .vertical()
            .padding(Gaps::new(8., 8., 12., 8.))
            .child(TablesSection::new(
                filter.clone(),
                open_entries,
                expanded_cols,
                theme.clone(),
            ))
            .child(ViewsSection::new(
                filter.clone(),
                open_entries,
                expanded_cols,
                theme.clone(),
            ))
            .child(QueriesSection::new(filter, theme));

        rect().expanded().child(ScrollView::new().child(body))
    }
}

/// The TABLES section. Subscribes to [`ProjChan::Tables`] only.
#[derive(PartialEq)]
struct TablesSection {
    filter: String,
    open_entries: State<HashSet<String>>,
    expanded_cols: State<HashSet<String>>,
    theme: CatalogTheme,
}

impl TablesSection {
    fn new(
        filter: String,
        open_entries: State<HashSet<String>>,
        expanded_cols: State<HashSet<String>>,
        theme: CatalogTheme,
    ) -> Self {
        Self {
            filter,
            open_entries,
            expanded_cols,
            theme,
        }
    }
}

impl Component for TablesSection {
    fn render(&self) -> impl IntoElement {
        let radio = use_radio::<ProjectState, ProjChan>(ProjChan::Tables);
        // Names only, cloned out, so the store's read guard drops before any element is built.
        let names: Vec<String> = radio
            .read()
            .tables
            .iter()
            .map(|t| t.def.name.clone())
            .filter(|n| matches(n, &self.filter))
            .collect();

        // TABLES leads the pane, so it drops the inter-section gap the others carry.
        CatalogSection::new("TABLES", names.len(), self.theme.clone())
            .first()
            .children(names.into_iter().map(|name| {
                EntryRow::new(
                    CatalogKind::Table,
                    name,
                    self.open_entries,
                    self.expanded_cols,
                    self.theme.clone(),
                )
                .into()
            }))
    }
}

/// The VIEWS section. Subscribes to [`ProjChan::Views`] only.
#[derive(PartialEq)]
struct ViewsSection {
    filter: String,
    open_entries: State<HashSet<String>>,
    expanded_cols: State<HashSet<String>>,
    theme: CatalogTheme,
}

impl ViewsSection {
    fn new(
        filter: String,
        open_entries: State<HashSet<String>>,
        expanded_cols: State<HashSet<String>>,
        theme: CatalogTheme,
    ) -> Self {
        Self {
            filter,
            open_entries,
            expanded_cols,
            theme,
        }
    }
}

impl Component for ViewsSection {
    fn render(&self) -> impl IntoElement {
        let radio = use_radio::<ProjectState, ProjChan>(ProjChan::Views);
        let names: Vec<String> = radio
            .read()
            .views
            .iter()
            .map(|v| v.def.name.clone())
            .filter(|n| matches(n, &self.filter))
            .collect();

        CatalogSection::new("VIEWS", names.len(), self.theme.clone()).children(
            names.into_iter().map(|name| {
                EntryRow::new(
                    CatalogKind::View,
                    name,
                    self.open_entries,
                    self.expanded_cols,
                    self.theme.clone(),
                )
                .into()
            }),
        )
    }
}

/// The QUERIES section. Subscribes to [`ProjChan::Queries`] only. Saved queries are addressed by
/// `id` — the name is only a label — so the rows carry both.
#[derive(PartialEq)]
struct QueriesSection {
    filter: String,
    theme: CatalogTheme,
}

impl QueriesSection {
    fn new(filter: String, theme: CatalogTheme) -> Self {
        Self { filter, theme }
    }
}

impl Component for QueriesSection {
    fn render(&self) -> impl IntoElement {
        let radio = use_radio::<ProjectState, ProjChan>(ProjChan::Queries);
        let queries: Vec<(uuid::Uuid, String)> = radio
            .read()
            .saved_queries
            .iter()
            .map(|q| (q.id, q.name.clone()))
            .filter(|(_, n)| matches(n, &self.filter))
            .collect();

        // The empty state is about the *section*, not the filter: with a filter typed, an empty
        // result is a non-match, and "no saved queries yet" would be a lie.
        let empty_note = (queries.is_empty() && self.filter.is_empty()).then(|| {
            rect()
                .padding(Gaps::new(4., 8., 12., 8.))
                .child(Caption::new("No saved queries yet").color(self.theme.meta_color))
        });

        CatalogSection::new("QUERIES", queries.len(), self.theme.clone())
            .children(
                queries
                    .into_iter()
                    .map(|(id, name)| SavedQueryRow::new(id, name, self.theme.clone()).into()),
            )
            .maybe_child(empty_note)
    }
}
