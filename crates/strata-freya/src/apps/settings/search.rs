//! The Settings window's **search index** (P4-09) — what the nav's search box filters, and what
//! picking a result points at.
//!
//! Three kinds of thing are findable, because the window holds three kinds of thing:
//!
//! - a **named setting** on one of the panes ([`Anchor`]), which is a row of a
//!   [`Form`](crate::components::form::Form);
//! - a **DataFusion property** ([`ENGINE_KEYS`]), which is a row the user has to name in the
//!   Engine pane's grid;
//! - a **page** that is its own answer, having no named settings yet (Keymap, until P4-08).
//!
//! **The category is never spelled out here.** A hit resolves its page's name through
//! [`category`] — the same nav tree the rail draws and the breadcrumb reads — so a setting cannot
//! be filed under one name in the results list and another over its own pane. That is the whole
//! reason [`super::model`] is data rather than a component.
//!
//! **An [`Anchor`] is a variant, not a string.** The index and the pane that draws the row have to
//! agree on the name search jumps to, and a typo in either would be a jump that navigates and then
//! singles nothing out — silent, and only visible by trying it. So the table below generates the
//! enum, the list of all of them, and each one's label, subtext and keywords together, and the pane
//! builds its row from the same entry ([`Anchor::row`]): there is one spelling of a setting's name
//! in the app, and the compiler holds the panes to it.
//!
//! **The engine's properties are the catalogue, not a chosen few.** They are indexed straight off
//! `ENGINE_KEYS` with their descriptions as the search terms, so every documented `datafusion.*`
//! key is findable by what it does ("memory", "parallelism", "spill") — the tunables that were
//! otherwise reachable only by typing into a grid on a page you had to know to visit.
//!
//! **Following a hit is navigation and nothing else.** It routes, and singles the setting out where
//! there is something to single out — a row of a pane, or a property's row in the grid *if it is
//! overridden*. It never writes: a property nobody has set gets no row made for it, because a row in
//! that grid is an override (see [`PropRows::reveal`](super::views::PropRows::reveal)).

use strata_core::engine::config::{EngineKey, ENGINE_KEYS};

use crate::apps::settings::{category, Route};
use crate::components::form::Row;

/// How many hits the list offers at once (canvas: 8). A search that answers with more than a
/// glance's worth of rows is a search that hasn't narrowed anything.
pub const MAX_RESULTS: usize = 8;

/// Generate the settings index: the [`Anchor`] enum, every anchor in nav order, and each one's
/// page, name, subtext and extra search terms.
///
/// One table, so the enum can neither gain a variant the index doesn't know nor lose one the panes
/// still draw — and `route`/`label`/`hint` are exhaustive matches, so a new setting that forgets
/// any of them is a build error rather than a row search cannot find.
macro_rules! settings_index {
    ($(
        $Variant:ident => $route:expr, $label:literal, $hint:literal, $keywords:literal
    );* $(;)?) => {
        /// One named setting, wherever it lives: the identity its pane's row carries
        /// ([`Row::anchor`]) and search jumps to.
        #[derive(Clone, Copy, PartialEq, Eq, Debug)]
        pub enum Anchor {
            $( #[doc = $label] $Variant, )*
        }

        impl Anchor {
            /// Every named setting: pages in the order the nav lists them ([`CATEGORIES`](super::CATEGORIES)),
            /// and within a page the order its pane lays them out. That ordering is what survives the
            /// [`MAX_RESULTS`] cap, so a broad query offers them the way the rail reads.
            pub const ALL: &'static [Anchor] = &[ $( Anchor::$Variant, )* ];

            /// The page this setting is on.
            pub fn route(self) -> Route {
                match self { $( Anchor::$Variant => $route, )* }
            }

            /// The setting's name — its row's title, and its hit's label.
            pub fn label(self) -> &'static str {
                match self { $( Anchor::$Variant => $label, )* }
            }

            /// What the row says under its title. Empty for a setting whose title says it all.
            pub fn hint(self) -> &'static str {
                match self { $( Anchor::$Variant => $hint, )* }
            }

            /// Words that should find this setting but appear in neither its name nor its subtext.
            fn keywords(self) -> &'static str {
                match self { $( Anchor::$Variant => $keywords, )* }
            }

            /// The anchor id the row carries. The variant's own name, so it is impossible to type
            /// a different one on either side.
            pub fn id(self) -> &'static str {
                match self { $( Anchor::$Variant => stringify!($Variant), )* }
            }
        }
    };
}

settings_index! {
    SyncOs => Route::Theme,
        "Sync with OS",
        "Matches your system light/dark appearance automatically.",
        "appearance os automatic";

    Theme => Route::Theme,
        "Theme",
        "",
        "appearance colour color midnight daylight dark light scheme accent";

    Reopen => Route::System,
        "Reopen projects on startup",
        "Reopens the projects that had a window when you last quit.",
        "launch session restore";

    DefaultDir => Route::System,
        "Default project directory",
        "Where the folder picker starts when you open a project. Leave blank to use the last \
         location.",
        "path browse";

    OpenPref => Route::System,
        "Opening a project",
        "Where a project opens when the window you open it from already has one.",
        "new this ask behaviour behavior";

    ConfirmClose => Route::System,
        "Confirm before closing a tab or window with a running query",
        "Asks only while a query is still running; closing is silent otherwise.",
        "safety warn quit exit prompt";

    CheckUpdates => Route::System,
        "Check for updates on startup",
        "",
        "update upgrade version release download automatic launch";

    HistoryLimit => Route::System,
        "Query history limit",
        "How many past runs the History drawer keeps, newest first. Lowering it drops the older \
         runs from a project's saved history.",
        "recent cap entries";

    Density => Route::DataDisplay,
        "Row density",
        "Controls row height in the results grid and catalog.",
        "comfortable compact spacing";

    Zebra => Route::DataDisplay,
        "Alternating row colors",
        "Shades every other row in the results grid for easier scanning.",
        "zebra stripe striping banding colours";

    ColumnWidth => Route::DataDisplay,
        "Default column width",
        "Starting width for result-grid columns before you resize them. Drag a column's edge to \
         override it for that column, or double-click the edge to auto-fit.",
        "size px";

    RowLimit => Route::DataDisplay,
        "Default row limit",
        "New queries are generated with this LIMIT so a stray SELECT * cannot pull a whole file \
         into memory. Set to 0 for no limit.",
        "cap rows";

    AiProvider => Route::Chat,
        "New chat provider",
        "Which provider a new chat starts on. Only enabled providers are offered.",
        "ai assistant default brain llm";

    AiModel => Route::Chat,
        "New chat model",
        "Which model a new chat starts on, chosen from the ones the provider reports. Each chat \
         can change its own afterwards.",
        "ai assistant default llm name";

    AiEffort => Route::Chat,
        "New chat reasoning effort",
        "How hard a reasoning model thinks by default. Models that do not reason have no \
         setting.",
        "ai assistant default thinking budget low medium high";

    AiChatLimit => Route::Chat,
        "Conversation limit",
        "How many conversations a project keeps, newest first. Lowering it deletes the older \
         conversations from that project's saved chats.",
        "ai assistant chat history cap retention transcript";

    AgentEnabled => Route::Mcp,
        "Enable agent access",
        "Runs a local MCP server on 127.0.0.1 so agents can query the projects you have open. \
         Off by default, and never reachable from outside this machine.",
        "mcp agent access claude ai assistant server";

    AgentPort => Route::Mcp,
        "Port",
        "The loopback port the server listens on. Changing it restarts the server when you \
         apply, and clients pointed at the old port stop resolving.",
        "mcp agent access localhost 127.0.0.1 loopback address";

    AgentToken => Route::Mcp,
        "Token",
        "The bearer token every client has to present. Regenerating replaces it when you apply, \
         and clients still using the old one stop working.",
        "mcp agent access secret bearer authorization credential regenerate";
}

impl Anchor {
    /// This setting's form row: its title, its subtext and the anchor a reveal finds it by — so a
    /// pane supplies nothing but the control.
    ///
    /// The row is built here rather than in the pane because the *name* of a setting has to be one
    /// string: the results list titles a hit with it and the pane heads its row with it, and two
    /// copies would eventually read differently.
    pub fn row(self) -> Row {
        let row = Row::new(self.label()).anchor(self.id());
        match self.hint() {
            "" => row,
            hint => row.hint(hint),
        }
    }
}

/// A page that is its own answer — it holds no named settings, so there is nothing on it to single
/// out and the hit only navigates.
#[derive(PartialEq, Eq, Debug)]
pub struct Page {
    route: Route,
    label: &'static str,
    keywords: &'static str,
}

/// Keymap is the only one, and only until P4-08 indexes its shortcuts as settings of their own.
///
/// It is here rather than left out because the search box **replaces** the category rail while it
/// has a query: a search for "shortcut" that answered "no settings match" while a Keymap row sat
/// hidden behind it would be the field lying about the window.
const PAGES: &[Page] = &[
    Page {
        route: Route::Keymap,
        label: "Keyboard shortcuts",
        keywords: "keymap keybinding rebind shortcut keys chord",
    },
    // A page rather than an `Anchor`, because its rows are **providers**, not named settings:
    // there is nothing on it a `Reveal` could single out, and an anchor no row carries is a hit
    // that navigates and then silently does nothing — unlike every other setting hit.
    Page {
        route: Route::Providers,
        label: "Providers",
        keywords: "ai assistant anthropic openai gemini deepseek groq xai ollama api key \
                   endpoint llm model credential keychain",
    },
];

/// One result: what it is called, where it lives, and what picking it does.
///
/// `Copy`, because every variant is a name or a reference into a static table — a hit is a pointer
/// at an indexed setting, never a copy of one.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Hit {
    /// A named setting on a pane.
    Setting(Anchor),
    /// A `datafusion.*` property, set as a row of the Engine pane's grid.
    Property(&'static EngineKey),
    /// A page with nothing on it to single out.
    Page(&'static Page),
}

impl Hit {
    /// The page this hit is on.
    pub fn route(&self) -> Route {
        match self {
            Hit::Setting(anchor) => anchor.route(),
            Hit::Property(_) => Route::Engine,
            Hit::Page(page) => page.route.clone(),
        }
    }

    /// This hit's stable identity — what the results list keys its rows on, so retyping a query
    /// re-associates a row with its hit rather than shifting hover state along the list.
    pub fn id(&self) -> &'static str {
        match self {
            Hit::Setting(anchor) => anchor.id(),
            Hit::Property(entry) => entry.key,
            Hit::Page(page) => page.label,
        }
    }

    /// What the results list titles this hit with.
    ///
    /// A property's name is its key's last segment, spelled as words — the whole key is too long
    /// for a 244px rail and would truncate to its namespace, which is the one part every property
    /// shares. Its namespace is set under it as the [`location`](Self::location) instead, which is
    /// what tells two keys with the same last segment apart.
    pub fn label(&self) -> String {
        match self {
            Hit::Setting(anchor) => anchor.label().to_string(),
            Hit::Property(entry) => property_label(entry.key),
            Hit::Page(page) => page.label.to_string(),
        }
    }

    /// Where the setting lives, under its label: the page's own breadcrumb, or — for a property,
    /// which can only ever be on the one page — the namespace that tells two similarly named keys
    /// apart.
    pub fn location(&self) -> String {
        if let Hit::Property(entry) = self {
            return namespace(entry.key).to_string();
        }
        // Every route has a category (`model`'s test pins that), so the fallback is unreachable
        // rather than a case worth dressing.
        let Some(category) = category(&self.route()) else {
            return String::new();
        };
        match category.breadcrumb() {
            (Some(group), label) => format!("{group} \u{203a} {label}"),
            (None, label) => label.to_string(),
        }
    }

    /// Everything a query is matched against, lowercased: what the hit is called, what it says
    /// about itself, its extra terms, and where it lives — so "data display" finds every setting on
    /// that page and "engine" every property.
    fn haystack(&self) -> String {
        let own = match self {
            Hit::Setting(anchor) => {
                format!("{} {} {}", anchor.label(), anchor.hint(), anchor.keywords())
            }
            // The key in full as well as the label, so both `batch_size` and "batch size" match.
            Hit::Property(entry) => format!("{} {}", entry.key, entry.desc),
            Hit::Page(page) => format!("{} {}", page.label, page.keywords),
        };
        format!("{own} {}", self.location()).to_lowercase()
    }
}

/// The whole index, in the order results are offered: the named settings first — they are what the
/// window is *for* — then the pages, then the engine catalogue.
fn index() -> impl Iterator<Item = Hit> {
    Anchor::ALL
        .iter()
        .copied()
        .map(Hit::Setting)
        .chain(PAGES.iter().map(Hit::Page))
        .chain(ENGINE_KEYS.iter().map(Hit::Property))
}

/// The hits for `query`: every indexed setting matching **all** of its words, capped at
/// [`MAX_RESULTS`]. An empty query is not a search, so it matches nothing rather than everything.
///
/// Every word has to match somewhere in the entry rather than as one phrase, so "row limit" and
/// "limit row" find the same setting and neither needs to be typed the way the label spells it.
pub fn search(query: &str) -> Vec<Hit> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Vec::new();
    }
    let terms: Vec<&str> = query.split_whitespace().collect();
    index()
        .filter(|hit| {
            let haystack = hit.haystack();
            terms.iter().all(|term| haystack.contains(term))
        })
        .take(MAX_RESULTS)
        .collect()
}

/// A property key's last segment as words: `datafusion.execution.batch_size` → `Batch size`.
fn property_label(key: &str) -> String {
    let last = key.rsplit('.').next().unwrap_or(key).replace('_', " ");
    let mut chars = last.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => last,
    }
}

/// A property key without its last segment: `datafusion.execution.batch_size` →
/// `datafusion.execution`.
fn namespace(key: &str) -> &str {
    match key.rsplit_once('.') {
        Some((namespace, _)) => namespace,
        None => key,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Assert on the exact target rather than on a count: a match that happens to be at the top for
    /// the wrong reason would pass a count assertion.
    fn finds(query: &str, hit: Hit) -> bool {
        search(query).contains(&hit)
    }

    /// Every indexed page resolves through the nav tree, which is what the results list reads a
    /// hit's category from. A route with no category would show a hit filed under nothing.
    #[test]
    fn every_hit_has_a_category() {
        for hit in index() {
            assert!(
                category(&hit.route()).is_some(),
                "{:?} is on a page the nav tree doesn't name",
                hit.route()
            );
            assert!(!hit.location().is_empty(), "{hit:?} has no location");
        }
    }

    /// Two settings with the same name would be two rows a reader cannot tell apart — the same
    /// rule the History drawer's collapse key holds.
    #[test]
    fn no_two_named_settings_share_a_label() {
        let mut labels: Vec<&str> = Anchor::ALL.iter().map(|a| a.label()).collect();
        labels.sort_unstable();
        let count = labels.len();
        labels.dedup();
        assert_eq!(count, labels.len(), "duplicate setting label in the index");
    }

    /// A setting is found by its own name, in any word order and any case.
    #[test]
    fn a_setting_is_found_by_its_name() {
        assert!(finds("row limit", Hit::Setting(Anchor::RowLimit)));
        assert!(finds("limit row", Hit::Setting(Anchor::RowLimit)));
        assert!(finds("ROW LIMIT", Hit::Setting(Anchor::RowLimit)));
    }

    /// …and by what its own subtext or its extra terms say, which is the point of indexing more
    /// than the label: nothing on the Data display pane is called "zebra".
    #[test]
    fn a_setting_is_found_by_what_it_says_about_itself() {
        assert!(finds("zebra", Hit::Setting(Anchor::Zebra)));
        assert!(finds("scanning", Hit::Setting(Anchor::Zebra)));
    }

    /// The MCP rows are found by the vocabulary a reader arrives with, which is the whole reason
    /// they carry keywords: two of the three are named after things every other page also has
    /// ("Port", "Token" — well, would).
    ///
    /// **"Agent access" still finds them**, though nothing is called that any more. AS-03 renamed
    /// the page to MCP when it gained two siblings, and a rename that silently drops the term
    /// people already know is a search that got worse — so the old name is a keyword now.
    #[test]
    fn the_agent_rows_are_found_by_what_the_feature_is_called() {
        let hits = search("mcp");
        assert!(hits.contains(&Hit::Setting(Anchor::AgentEnabled)));
        assert!(hits.contains(&Hit::Setting(Anchor::AgentPort)));
        assert!(hits.contains(&Hit::Setting(Anchor::AgentToken)));
        // …and by the page they are on, like every other pane's settings.
        assert!(finds(
            "agent access token",
            Hit::Setting(Anchor::AgentToken)
        ));
    }

    /// A page's own name finds every setting on it, because the breadcrumb is part of the haystack.
    #[test]
    fn a_page_name_finds_its_settings() {
        let hits = search("data display");
        assert!(hits.contains(&Hit::Setting(Anchor::Zebra)));
        assert!(hits.contains(&Hit::Setting(Anchor::Density)));
    }

    /// An engine property is found by what it *does*, not only by its key — the catalogue's
    /// descriptions are indexed for exactly this.
    #[test]
    fn an_engine_property_is_found_by_its_description() {
        let memory = ENGINE_KEYS
            .iter()
            .find(|e| e.key == "datafusion.runtime.memory_limit")
            .expect("the catalogue documents the memory limit");
        assert!(finds("memory limit", Hit::Property(memory)));
        assert!(finds("datafusion.runtime.memory", Hit::Property(memory)));
    }

    /// A property's label is readable and its location tells two keys in different namespaces
    /// apart, which is what lets the label be the short name at all.
    #[test]
    fn a_property_reads_as_a_name_over_its_namespace() {
        let batch = ENGINE_KEYS
            .iter()
            .find(|e| e.key == "datafusion.execution.batch_size")
            .expect("the catalogue documents the batch size");
        let hit = Hit::Property(batch);
        assert_eq!(hit.label(), "Batch size");
        assert_eq!(hit.location(), "datafusion.execution");
    }

    /// The Keymap page is findable even though nothing on it is a setting yet — the rail is hidden
    /// while searching, so a miss here would hide the page entirely.
    #[test]
    fn the_keymap_page_is_findable() {
        let hits = search("shortcut");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].route(), Route::Keymap);
        assert_eq!(hits[0].label(), "Keyboard shortcuts");
    }

    #[test]
    fn an_empty_query_matches_nothing_and_a_miss_matches_nothing() {
        assert!(search("").is_empty());
        assert!(search("   ").is_empty());
        assert!(search("kubernetes").is_empty());
    }

    /// A broad query is capped rather than filling the rail — and the cap keeps the named settings,
    /// which is why they are indexed first.
    #[test]
    fn a_broad_query_is_capped_and_offers_the_named_settings_first() {
        let hits = search("a");
        assert_eq!(hits.len(), MAX_RESULTS);
        assert!(matches!(hits[0], Hit::Setting(_)));
    }
}
