//! Named icons, rendered from the design's own SVGs. `Icon::new(IconName::…).color(c).size(s)`.
//! One place to add/replace an icon — the toolbar/strip/etc. reference them by name, never by
//! inline SVG.

use freya::prelude::*;
use strata_model::CatalogKind;

/// The app's icon set (grown as views need them). Each maps to the design comp's SVG.
#[derive(PartialEq, Clone, Copy)]
pub enum IconName {
    Play,
    Explain,
    Analyze,
    Format,
    Trash,
    Eye,
    Save,
    Stop,
    Rows,
    Plus,
    /// Remove, paired with [`Plus`](Self::Plus) — Settings ▸ Engine's **Remove property** and
    /// the Configure window's source-path toolbar. Both arrived at it independently, which is
    /// the argument for one glyph: they are the same gesture on two lists.
    Minus,
    Clipboard,
    ChevronDown,
    ChevronUp,
    ChevronLeft,
    ChevronRight,
    /// Double chevrons — the drawer header's expand / restore toggle.
    ChevronsUp,
    ChevronsDown,
    Dots,
    Search,
    Close,
    Database,
    Reopen,
    Reload,
    Download,
    Alert,
    Warning,
    LogOut,
    Clock,
    First,
    Last,
    Grid,
    Chart,
    /// The six chart **marks**, for the Chart body's type picker (Rz2). Distinct from
    /// [`Chart`](Self::Chart), which is the results toolbar's Table/Chart toggle: these are
    /// six glyphs that have to read as a set, and the toggle's is one of a different pair.
    MarkBar,
    MarkLine,
    MarkArea,
    MarkScatter,
    MarkHistogram,
    MarkPie,
    Lines,
    Copy,
    Connections,
    /// The activity rail's Agents pane (AA-03b).
    Agent,
    /// The **right** rail's two panes (AS-04): the column inspector's split panel, and the
    /// assistant's speech bubble. Both from the canvas's `data-rg="rightrail"` buttons.
    Inspector,
    Chat,
    Problems,
    Brackets,
    Folder,
    Gear,
    /// Edit — the catalog menus' "Edit query" / "Rename".
    Pencil,
    Pin,
    /// A query file — the Problems drawer's group header (the tab a problem belongs to).
    File,
    /// A tick — the Problems drawer's no-problems state.
    Check,
    /// Circled `i` — an informational diagnostic, beside `Alert` (error) and `Warning`.
    Info,
    /// The app mark — the only multi-colour icon (it paints its own fills, so
    /// [`Icon::color`] doesn't apply to it).
    StrataLogo,
    // -- Provider marks (AS-03) --------------------------------------------------------------
    //
    // The brands Settings ▸ AI ▸ Providers lists, one row each. **Not from the design comp**
    // like every glyph above: they are third-party marks, carried to identify the service a row
    // connects to. They live here rather than in a module of their own only because the render
    // is identical — an inline SVG through `SvgViewer`, sized and coloured by `Icon`.
    //
    // Every one is **monochrome on `currentColor`**, so a mark flips between Midnight and
    // Daylight exactly as the UI glyphs do; that is what lets the two sets share a component at
    // all, and it is why `StrataLogo` above stays the only icon here painting its own fills.
    ProviderAnthropic,
    ProviderOpenAi,
    ProviderGemini,
    ProviderDeepSeek,
    ProviderGroq,
    ProviderXai,
    ProviderOllama,
}

impl IconName {
    /// The glyph for a catalog row's kind — the sidebar's rows and the command palette's, which
    /// list the same things and so must mark them the same way.
    ///
    /// The mapping only, never the colour: the sidebar tints from the `catalog` theme and the
    /// palette from its own, exactly as [`kind_color`](super::type_palette::kind_color) is one
    /// mapping whose consumers each choose where to paint it.
    pub fn for_catalog(kind: CatalogKind) -> Self {
        match kind {
            CatalogKind::Table => IconName::Database,
            CatalogKind::View => IconName::Eye,
            CatalogKind::Query => IconName::Brackets,
        }
    }

    fn svg(self) -> &'static str {
        match self {
            IconName::Play => PLAY,
            IconName::Explain => EXPLAIN,
            IconName::Analyze => ANALYZE,
            IconName::Format => FORMAT,
            IconName::Trash => TRASH,
            IconName::Eye => EYE,
            IconName::Save => SAVE,
            IconName::Stop => STOP,
            IconName::Rows => ROWS,
            IconName::Plus => PLUS,
            IconName::Minus => MINUS,
            IconName::Clipboard => CLIPBOARD,
            IconName::ChevronDown => CHEVRON_DOWN,
            IconName::ChevronUp => CHEVRON_UP,
            IconName::ChevronLeft => CHEVRON_LEFT,
            IconName::ChevronRight => CHEVRON_RIGHT,
            IconName::ChevronsUp => CHEVRONS_UP,
            IconName::ChevronsDown => CHEVRONS_DOWN,
            IconName::Dots => DOTS,
            IconName::Search => SEARCH,
            IconName::Close => CLOSE,
            IconName::Database => DATABASE,
            IconName::Reopen => REOPEN,
            IconName::Reload => RELOAD,
            IconName::Download => DOWNLOAD,
            IconName::Alert => ALERT,
            IconName::Warning => WARNING,
            IconName::LogOut => LOG_OUT,
            IconName::Clock => CLOCK,
            IconName::First => FIRST,
            IconName::Last => LAST,
            IconName::Grid => GRID,
            IconName::Chart => CHART,
            IconName::MarkBar => MARK_BAR,
            IconName::MarkLine => MARK_LINE,
            IconName::MarkArea => MARK_AREA,
            IconName::MarkScatter => MARK_SCATTER,
            IconName::MarkHistogram => MARK_HISTOGRAM,
            IconName::MarkPie => MARK_PIE,
            IconName::Lines => LINES,
            IconName::Copy => COPY,
            IconName::Connections => CONNECTIONS,
            IconName::Agent => AGENT,
            IconName::Inspector => INSPECTOR,
            IconName::Chat => CHAT,
            IconName::Problems => PROBLEMS,
            IconName::Brackets => BRACKETS,
            IconName::Folder => FOLDER,
            IconName::Gear => GEAR,
            IconName::Pencil => PENCIL,
            IconName::Pin => PIN,
            IconName::File => FILE,
            IconName::Check => CHECK,
            IconName::Info => INFO,
            IconName::StrataLogo => STRATA_LOGO,
            IconName::ProviderAnthropic => PROVIDER_ANTHROPIC,
            IconName::ProviderOpenAi => PROVIDER_OPENAI,
            IconName::ProviderGemini => PROVIDER_GEMINI,
            IconName::ProviderDeepSeek => PROVIDER_DEEPSEEK,
            IconName::ProviderGroq => PROVIDER_GROQ,
            IconName::ProviderXai => PROVIDER_XAI,
            IconName::ProviderOllama => PROVIDER_OLLAMA,
        }
    }
}

/// A single icon. By default it **inherits the ambient `color`** (the SVG's `currentColor` resolves
/// to the parent's text colour) — so an icon inside a `Button` follows that button's colour,
/// including its hover colour, with no wiring. Call [`Icon::color`] only to pin an explicit tint
/// (e.g. a standalone icon not sitting in a coloured container).
#[derive(PartialEq)]
pub struct Icon {
    name: IconName,
    color: Option<Color>,
    size: f32,
}

impl Icon {
    pub fn new(name: IconName) -> Self {
        Self {
            name,
            color: None,
            size: 16.,
        }
    }

    /// Pin an explicit tint. Omit to inherit the parent's `color` (the hover-reactive default).
    pub fn color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }

    pub fn size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }
}

impl Component for Icon {
    fn render(&self) -> impl IntoElement {
        let svg = SvgViewer::new(self.name.svg().as_bytes())
            .width(Size::px(self.size))
            .height(Size::px(self.size))
            .show_loader(false);
        // No explicit colour → let `currentColor` inherit the parent's text colour (so hover on a
        // themed parent flows through). An explicit colour pins it.
        match self.color {
            Some(color) => svg.color(color),
            None => svg,
        }
    }
}

// The design comp's inline SVGs (stroke="currentColor" so `Icon::color` tints them).
const PLAY: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor"><path d="M8 5v14l11-7z"/></svg>"#;
const EXPLAIN: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M8 6h13M8 12h13M8 18h13M3 6h.01M3 12h.01M3 18h.01"/></svg>"#;
const ANALYZE: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M9 2h6"/><circle cx="12" cy="13" r="8"/><path d="M12 9v4l2.5 2"/></svg>"#;
const FORMAT: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M4 6h16M4 12h10M4 18h13"/></svg>"#;
const TRASH: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M3 6h18M8 6V4a1 1 0 0 1 1-1h6a1 1 0 0 1 1 1v2m3 0v14a1 1 0 0 1-1 1H6a1 1 0 0 1-1-1V6"/></svg>"#;
const EYE: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12z"/><circle cx="12" cy="12" r="2.5"/></svg>"#;
const SAVE: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M6 3h10l4 4v13a1 1 0 0 1-1 1H6a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1z"/><path d="M8 3v5h7V3M8 21v-7h8v7"/></svg>"#;
const STOP: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor"><rect x="6" y="6" width="12" height="12" rx="2"/></svg>"#;
const ROWS: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="4" width="18" height="16" rx="2"/><path d="M3 9h18M3 14h18"/></svg>"#;
// New query (+), tab-list chevron, and tab-actions overflow dots — from the strip's right cluster.
const PLUS: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><path d="M12 5v14M5 12h14"/></svg>"#;
// Plus's opposite, the same stroke one path short — Settings ▸ Engine's **Remove property** and
// the Configure window's **Remove path**. A minus and not a bin: in both cases the row is one of
// a list you are editing, not a stored thing being destroyed.
const MINUS: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><path d="M5 12h14"/></svg>"#;
// A clipboard — Settings ▸ Engine's **Paste properties**.
const CLIPBOARD: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><rect x="8" y="2" width="8" height="4" rx="1"/><path d="M16 4h2a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h2"/></svg>"#;
const CHEVRON_DOWN: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="m6 9 6 6 6-6"/></svg>"#;
// Header sort chevron, ascending (Rz6).
const CHEVRON_UP: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="m6 15 6-6 6 6"/></svg>"#;
// Status-bar pager prev/next.
const CHEVRON_LEFT: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="m15 6-6 6 6 6"/></svg>"#;
const CHEVRON_RIGHT: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="m9 6 6 6-6 6"/></svg>"#;
// Drawer header expand (up) / restore (down) — the canvas's `logExpandIcon` pair.
const CHEVRONS_UP: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m17 11-5-5-5 5M17 18l-5-5-5 5"/></svg>"#;
const CHEVRONS_DOWN: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m7 6 5 5 5-5M7 13l5 5 5-5"/></svg>"#;
const DOTS: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor"><circle cx="12" cy="5" r="1.6"/><circle cx="12" cy="12" r="1.6"/><circle cx="12" cy="19" r="1.6"/></svg>"#;
const SEARCH: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><circle cx="11" cy="11" r="7"/><path d="m21 21-4.3-4.3"/></svg>"#;
const CLOSE: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M18 6 6 18M6 6l12 12"/></svg>"#;
// Empty-state hero (database cylinder) + reopen-closed (arrow curving back).
const DATABASE: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><ellipse cx="12" cy="5" rx="8" ry="3"/><path d="M4 5v6c0 1.66 3.58 3 8 3s8-1.34 8-3V5M4 11v6c0 1.66 3.58 3 8 3s8-1.34 8-3v-6"/></svg>"#;
const REOPEN: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M9 14l-4-4 4-4"/><path d="M5 10h11a4 4 0 0 1 0 8h-1"/></svg>"#;
// Two circular arrows — the results **Reload** (re-run) button.
const RELOAD: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M21 12a9 9 0 1 1-2.64-6.36"/><path d="M21 3v6h-6"/></svg>"#;
// Down arrow into a tray — the results **Download** (export) button.
const DOWNLOAD: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M12 3v12M8 11l4 4 4-4M5 21h14"/></svg>"#;
// Results error state (circle + exclamation).
// The close-confirm dialog's warning triangle + the project variant's exit arrow (T2 comp).
const WARNING: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M10.29 3.86 1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z"/><line x1="12" y1="9" x2="12" y2="13"/><line x1="12" y1="17" x2="12.01" y2="17"/></svg>"#;
const LOG_OUT: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4"/><polyline points="16 17 21 12 16 7"/><line x1="21" y1="12" x2="9" y2="12"/></svg>"#;
const ALERT: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><path d="M12 7.5v5.5M12 16.5h.01"/></svg>"#;
// Status-bar snapshot chip (clock face) + pager first/last (chevron against a stop bar).
const CLOCK: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><path d="M12 8v4l2.5 1.5"/></svg>"#;
const FIRST: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M17 6l-6 6 6 6M8 6v12"/></svg>"#;
// The results Table/Chart segmented toggle (bordered grid vs bar chart).
const GRID: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="4" width="18" height="16" rx="2"/><path d="M3 10h18M9 4v16"/></svg>"#;
const CHART: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M4 20V10M10 20V4M16 20v-7M22 20H2"/></svg>"#;
const LAST: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M7 6l6 6-6 6M16 6v12"/></svg>"#;
// The Chart body's six mark tiles (design `chartTypes`).
const MARK_BAR: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M4 20V10h4v10zM10 20V4h4v16zM16 20v-7h4v7z"/></svg>"#;
const MARK_LINE: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M3 17l5-6 4 3 8-9"/></svg>"#;
const MARK_AREA: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M3 17l5-6 4 3 8-9v11H3z"/></svg>"#;
const MARK_SCATTER: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M6 18h.01M10 13h.01M14 15h.01M18 8h.01M8 9h.01"/></svg>"#;
const MARK_HISTOGRAM: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M4 20V12h3v8zM9 20V7h3v13zM14 20V10h3v10zM19 20v-5h1v5z"/></svg>"#;
const MARK_PIE: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M12 3v9l7.5 4.5A9 9 0 1 0 12 3z"/></svg>"#;
// Ragged text lines — the plan view's Raw/Tree toggle.
const LINES: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M4 7h16M4 12h10M4 17h13"/></svg>"#;
// Two overlapped sheets — the record view's Copy row as JSON / CSV buttons.
const COPY: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="11" height="11" rx="2"/><path d="M5 15V5a2 2 0 0 1 2-2h10"/></svg>"#;
// The activity rail's Connections pane (cloud + up-arrow — an object store the project reads from).
const CONNECTIONS: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M17.5 19a4.5 4.5 0 0 0 .5-8.97 6 6 0 0 0-11.64-1.6A4 4 0 0 0 6 16.5"/><path d="M12 12v6"/><path d="m9 15 3-3 3 3"/></svg>"#;
// The activity rail's Agents pane (canvas `Strata.dc.html` `data-pane="agents"`).
const AGENT: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><rect x="4" y="8" width="16" height="12" rx="2.5"/><path d="M12 8V4.5"/><circle cx="12" cy="3" r="1.4"/><path d="M9 13v1.5M15 13v1.5"/></svg>"#;
// The right rail's two panes (canvas `data-rg="rightrail"`): a panel split with a detail column,
// and a speech bubble.
const INSPECTOR: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="4" width="18" height="16" rx="2"/><path d="M14 4v16"/><path d="M17 9h1M17 13h1"/></svg>"#;
const CHAT: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M21 11.5a8.38 8.38 0 0 1-.9 3.8 8.5 8.5 0 0 1-7.6 4.7 8.38 8.38 0 0 1-3.8-.9L3 21l1.9-5.7a8.38 8.38 0 0 1-.9-3.8 8.5 8.5 0 0 1 4.7-7.6 8.38 8.38 0 0 1 3.8-.9h.5a8.48 8.48 0 0 1 8 8z"/></svg>"#;
// The activity rail's Problems drawer (octagon + exclamation).
const PROBLEMS: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M8.5 2.5h7L21.5 8.5v7L15.5 21.5h-7L2.5 15.5v-7z"/><path d="M12 8v4.5"/><path d="M12 16h.01"/></svg>"#;
// Facing angle brackets — a saved SQL snippet in the catalog (design `kindIcon("query")`).
const BRACKETS: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="m8 8-4 4 4 4M16 8l4 4-4 4"/></svg>"#;
// The header's project switcher (folder) and settings (gear).
const FOLDER: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M3 7a2 2 0 0 1 2-2h4l2 2h6a2 2 0 0 1 2 2v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/></svg>"#;
const GEAR: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/></svg>"#;
// The catalog row menus' edit glyph — "Edit query" on a view, "Rename" on a saved query.
const PENCIL: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M12 20h9M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4z"/></svg>"#;
// The launcher row's pin / unpin action (a drawing pin, head down).
const PIN: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M9 4v5.5L6.5 13h11L15 9.5V4"/><path d="M8 4h8M12 13v7"/></svg>"#;
// A dog-eared page carrying a prompt — the Problems group header's glyph (design `problemGroups`).
const FILE: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z"/><path d="M14 3v5h5"/><path d="m9.5 13 1.5 1.5-1.5 1.5"/><path d="M13 16.5h2"/></svg>"#;
// The Problems drawer's clean state (design `problemsEmpty`).
const CHECK: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M20 6 9 17l-5-5"/></svg>"#;
const INFO: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><path d="M12 11.5V17"/><path d="M12 7.5h.01"/></svg>"#;
// The app mark: the dock icon's sedimentary bands (`design-handoff/.../icons/strata.svg`, scaled
// from its 1024 viewBox to 24). It paints its **own** fills — brand colours, not `currentColor` —
// and is drawn square: the rounded tile is the caller's `corner_radius` + `Overflow::Clip`, so it
// needs no `clipPath` (which Skia's SVG support is shaky on).
const STRATA_LOGO: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><rect x="-1" y="-1" width="26" height="26" fill="#0b1017"/><polygon points="-1,-2.88 25,-9.6 25,-4.75 -1,1.97" fill="#0f2536"/><polygon points="-1,1.92 25,-4.8 25,0.05 -1,6.77" fill="#1a4a6e"/><polygon points="-1,6.72 25,0 25,4.85 -1,11.57" fill="#2b7fd0"/><polygon points="-1,11.52 25,4.8 25,9.65 -1,16.37" fill="#4cc6ff"/><polygon points="-1,16.32 25,9.6 25,14.45 -1,21.17" fill="#8fe0ff"/></svg>"##;

// -- Provider marks ---------------------------------------------------------------------------
//
// Third-party brand marks, **all seven from one set** — lobehub/lobe-icons (MIT), which exists
// for exactly this list. Simple Icons covers four of them and was tried first; taking the other
// three from elsewhere would mix two drawing grids and two optical weights in one vertical list,
// which reads as wrong long before anyone works out why. One source is also one license and one
// place to check when a brand restyles.
//
// Embedded verbatim but for two normalizations, both about fitting the app's icon pipeline
// rather than about the artwork:
//
// - `width`/`height="1em"` and the inline `style` are **stripped**. `SvgViewer` is handed a
//   pixel size already, and an `em` on the root fights it.
// - `role` and `<title>` are dropped: `Icon` is decorative here — the row's own text names the
//   provider, and a nested title would say it twice.
//
// `fill="currentColor"` and `fill-rule="evenodd"` are the set's own and stay. The first is what
// makes a mark flip with the theme; the second is what keeps the holes in OpenAI's knot and
// Groq's ring from filling solid.
const PROVIDER_ANTHROPIC: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor" fill-rule="evenodd"><path d="M13.827 3.52h3.603L24 20h-3.603l-6.57-16.48zm-7.258 0h3.767L16.906 20h-3.674l-1.343-3.461H5.017l-1.344 3.46H0L6.57 3.522zm4.132 9.959L8.453 7.687 6.205 13.48H10.7z"/></svg>"#;
const PROVIDER_OPENAI: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor" fill-rule="evenodd"><path d="M9.205 8.658v-2.26c0-.19.072-.333.238-.428l4.543-2.616c.619-.357 1.356-.523 2.117-.523 2.854 0 4.662 2.212 4.662 4.566 0 .167 0 .357-.024.547l-4.71-2.759a.797.797 0 00-.856 0l-5.97 3.473zm10.609 8.8V12.06c0-.333-.143-.57-.429-.737l-5.97-3.473 1.95-1.118a.433.433 0 01.476 0l4.543 2.617c1.309.76 2.189 2.378 2.189 3.948 0 1.808-1.07 3.473-2.76 4.163zM7.802 12.703l-1.95-1.142c-.167-.095-.239-.238-.239-.428V5.899c0-2.545 1.95-4.472 4.591-4.472 1 0 1.927.333 2.712.928L8.23 5.067c-.285.166-.428.404-.428.737v6.898zM12 15.128l-2.795-1.57v-3.33L12 8.658l2.795 1.57v3.33L12 15.128zm1.796 7.23c-1 0-1.927-.332-2.712-.927l4.686-2.712c.285-.166.428-.404.428-.737v-6.898l1.974 1.142c.167.095.238.238.238.428v5.233c0 2.545-1.974 4.472-4.614 4.472zm-5.637-5.303l-4.544-2.617c-1.308-.761-2.188-2.378-2.188-3.948A4.482 4.482 0 014.21 6.327v5.423c0 .333.143.571.428.738l5.947 3.449-1.95 1.118a.432.432 0 01-.476 0zm-.262 3.9c-2.688 0-4.662-2.021-4.662-4.519 0-.19.024-.38.047-.57l4.686 2.71c.286.167.571.167.856 0l5.97-3.448v2.26c0 .19-.07.333-.237.428l-4.543 2.616c-.619.357-1.356.523-2.117.523zm5.899 2.83a5.947 5.947 0 005.827-4.756C22.287 18.339 24 15.84 24 13.296c0-1.665-.713-3.282-1.998-4.448.119-.5.19-.999.19-1.498 0-3.401-2.759-5.947-5.946-5.947-.642 0-1.26.095-1.88.31A5.962 5.962 0 0010.205 0a5.947 5.947 0 00-5.827 4.757C1.713 5.447 0 7.945 0 10.49c0 1.666.713 3.283 1.998 4.448-.119.5-.19 1-.19 1.499 0 3.401 2.759 5.946 5.946 5.946.642 0 1.26-.095 1.88-.309a5.96 5.96 0 004.162 1.713z"/></svg>"#;
const PROVIDER_GEMINI: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor" fill-rule="evenodd"><path d="M20.616 10.835a14.147 14.147 0 01-4.45-3.001 14.111 14.111 0 01-3.678-6.452.503.503 0 00-.975 0 14.134 14.134 0 01-3.679 6.452 14.155 14.155 0 01-4.45 3.001c-.65.28-1.318.505-2.002.678a.502.502 0 000 .975c.684.172 1.35.397 2.002.677a14.147 14.147 0 014.45 3.001 14.112 14.112 0 013.679 6.453.502.502 0 00.975 0c.172-.685.397-1.351.677-2.003a14.145 14.145 0 013.001-4.45 14.113 14.113 0 016.453-3.678.503.503 0 000-.975 13.245 13.245 0 01-2.003-.678z"/></svg>"#;
const PROVIDER_DEEPSEEK: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor" fill-rule="evenodd"><path d="M23.748 4.482c-.254-.124-.364.113-.512.234-.051.039-.094.09-.137.136-.372.397-.806.657-1.373.626-.829-.046-1.537.214-2.163.848-.133-.782-.575-1.248-1.247-1.548-.352-.156-.708-.311-.955-.65-.172-.241-.219-.51-.305-.774-.055-.16-.11-.323-.293-.35-.2-.031-.278.136-.356.276-.313.572-.434 1.202-.422 1.84.027 1.436.633 2.58 1.838 3.393.137.093.172.187.129.323-.082.28-.18.552-.266.833-.055.179-.137.217-.329.14a5.526 5.526 0 01-1.736-1.18c-.857-.828-1.631-1.742-2.597-2.458a11.365 11.365 0 00-.689-.471c-.985-.957.13-1.743.388-1.836.27-.098.093-.432-.779-.428-.872.004-1.67.295-2.687.684a3.055 3.055 0 01-.465.137 9.597 9.597 0 00-2.883-.102c-1.885.21-3.39 1.102-4.497 2.623C.082 8.606-.231 10.684.152 12.85c.403 2.284 1.569 4.175 3.36 5.653 1.858 1.533 3.997 2.284 6.438 2.14 1.482-.085 3.133-.284 4.994-1.86.47.234.962.327 1.78.397.63.059 1.236-.03 1.705-.128.735-.156.684-.837.419-.961-2.155-1.004-1.682-.595-2.113-.926 1.096-1.296 2.746-2.642 3.392-7.003.05-.347.007-.565 0-.845-.004-.17.035-.237.23-.256a4.173 4.173 0 001.545-.475c1.396-.763 1.96-2.015 2.093-3.517.02-.23-.004-.467-.247-.588zM11.581 18c-2.089-1.642-3.102-2.183-3.52-2.16-.392.024-.321.471-.235.763.09.288.207.486.371.739.114.167.192.416-.113.603-.673.416-1.842-.14-1.897-.167-1.361-.802-2.5-1.86-3.301-3.307-.774-1.393-1.224-2.887-1.298-4.482-.02-.386.093-.522.477-.592a4.696 4.696 0 011.529-.039c2.132.312 3.946 1.265 5.468 2.774.868.86 1.525 1.887 2.202 2.891.72 1.066 1.494 2.082 2.48 2.914.348.292.625.514.891.677-.802.09-2.14.11-3.054-.614zm1-6.44a.306.306 0 01.415-.287.302.302 0 01.2.288.306.306 0 01-.31.307.303.303 0 01-.304-.308zm3.11 1.596c-.2.081-.399.151-.59.16a1.245 1.245 0 01-.798-.254c-.274-.23-.47-.358-.552-.758a1.73 1.73 0 01.016-.588c.07-.327-.008-.537-.239-.727-.187-.156-.426-.199-.688-.199a.559.559 0 01-.254-.078c-.11-.054-.2-.19-.114-.358.028-.054.16-.186.192-.21.356-.202.767-.136 1.146.016.352.144.618.408 1.001.782.391.451.462.576.685.914.176.265.336.537.445.848.067.195-.019.354-.25.452z"/></svg>"#;
const PROVIDER_GROQ: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor" fill-rule="evenodd"><path d="M12.036 2c-3.853-.035-7 3-7.036 6.781-.035 3.782 3.055 6.872 6.908 6.907h2.42v-2.566h-2.292c-2.407.028-4.38-1.866-4.408-4.23-.029-2.362 1.901-4.298 4.308-4.326h.1c2.407 0 4.358 1.915 4.365 4.278v6.305c0 2.342-1.944 4.25-4.323 4.279a4.375 4.375 0 01-3.033-1.252l-1.851 1.818A7 7 0 0012.029 22h.092c3.803-.056 6.858-3.083 6.879-6.816v-6.5C18.907 4.963 15.817 2 12.036 2z"/></svg>"#;
const PROVIDER_XAI: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor" fill-rule="evenodd"><path d="M6.469 8.776L16.512 23h-4.464L2.005 8.776H6.47zm-.004 7.9l2.233 3.164L6.467 23H2l4.465-6.324zM22 2.582V23h-3.659V7.764L22 2.582zM22 1l-9.952 14.095-2.233-3.163L17.533 1H22z"/></svg>"#;
const PROVIDER_OLLAMA: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor" fill-rule="evenodd"><path d="M7.905 1.09c.216.085.411.225.588.41.295.306.544.744.734 1.263.191.522.315 1.1.362 1.68a5.054 5.054 0 012.049-.636l.051-.004c.87-.07 1.73.087 2.48.474.101.053.2.11.297.17.05-.569.172-1.134.36-1.644.19-.52.439-.957.733-1.264a1.67 1.67 0 01.589-.41c.257-.1.53-.118.796-.042.401.114.745.368 1.016.737.248.337.434.769.561 1.287.23.934.27 2.163.115 3.645l.053.04.026.019c.757.576 1.284 1.397 1.563 2.35.435 1.487.216 3.155-.534 4.088l-.018.021.002.003c.417.762.67 1.567.724 2.4l.002.03c.064 1.065-.2 2.137-.814 3.19l-.007.01.01.024c.472 1.157.62 2.322.438 3.486l-.006.039a.651.651 0 01-.747.536.648.648 0 01-.54-.742c.167-1.033.01-2.069-.48-3.123a.643.643 0 01.04-.617l.004-.006c.604-.924.854-1.83.8-2.72-.046-.779-.325-1.544-.8-2.273a.644.644 0 01.18-.886l.009-.006c.243-.159.467-.565.58-1.12a4.229 4.229 0 00-.095-1.974c-.205-.7-.58-1.284-1.105-1.683-.595-.454-1.383-.673-2.38-.61a.653.653 0 01-.632-.371c-.314-.665-.772-1.141-1.343-1.436a3.288 3.288 0 00-1.772-.332c-1.245.099-2.343.801-2.67 1.686a.652.652 0 01-.61.425c-1.067.002-1.893.252-2.497.703-.522.39-.878.935-1.066 1.588a4.07 4.07 0 00-.068 1.886c.112.558.331 1.02.582 1.269l.008.007c.212.207.257.53.109.785-.36.622-.629 1.549-.673 2.44-.05 1.018.186 1.902.719 2.536l.016.019a.643.643 0 01.095.69c-.576 1.236-.753 2.252-.562 3.052a.652.652 0 01-1.269.298c-.243-1.018-.078-2.184.473-3.498l.014-.035-.008-.012a4.339 4.339 0 01-.598-1.309l-.005-.019a5.764 5.764 0 01-.177-1.785c.044-.91.278-1.842.622-2.59l.012-.026-.002-.002c-.293-.418-.51-.953-.63-1.545l-.005-.024a5.352 5.352 0 01.093-2.49c.262-.915.777-1.701 1.536-2.269.06-.045.123-.09.186-.132-.159-1.493-.119-2.73.112-3.67.127-.518.314-.95.562-1.287.27-.368.614-.622 1.015-.737.266-.076.54-.059.797.042zm4.116 9.09c.936 0 1.8.313 2.446.855.63.527 1.005 1.235 1.005 1.94 0 .888-.406 1.58-1.133 2.022-.62.375-1.451.557-2.403.557-1.009 0-1.871-.259-2.493-.734-.617-.47-.963-1.13-.963-1.845 0-.707.398-1.417 1.056-1.946.668-.537 1.55-.849 2.485-.849zm0 .896a3.07 3.07 0 00-1.916.65c-.461.37-.722.835-.722 1.25 0 .428.21.829.61 1.134.455.347 1.124.548 1.943.548.799 0 1.473-.147 1.932-.426.463-.28.7-.686.7-1.257 0-.423-.246-.89-.683-1.256-.484-.405-1.14-.643-1.864-.643zm.662 1.21l.004.004c.12.151.095.37-.056.49l-.292.23v.446a.375.375 0 01-.376.373.375.375 0 01-.376-.373v-.46l-.271-.218a.347.347 0 01-.052-.49.353.353 0 01.494-.051l.215.172.22-.174a.353.353 0 01.49.051zm-5.04-1.919c.478 0 .867.39.867.871a.87.87 0 01-.868.871.87.87 0 01-.867-.87.87.87 0 01.867-.872zm8.706 0c.48 0 .868.39.868.871a.87.87 0 01-.868.871.87.87 0 01-.867-.87.87.87 0 01.867-.872zM7.44 2.3l-.003.002a.659.659 0 00-.285.238l-.005.006c-.138.189-.258.467-.348.832-.17.692-.216 1.631-.124 2.782.43-.128.899-.208 1.404-.237l.01-.001.019-.034c.046-.082.095-.161.148-.239.123-.771.022-1.692-.253-2.444-.134-.364-.297-.65-.453-.813a.628.628 0 00-.107-.09L7.44 2.3zm9.174.04l-.002.001a.628.628 0 00-.107.09c-.156.163-.32.45-.453.814-.29.794-.387 1.776-.23 2.572l.058.097.008.014h.03a5.184 5.184 0 011.466.212c.086-1.124.038-2.043-.128-2.722-.09-.365-.21-.643-.349-.832l-.004-.006a.659.659 0 00-.285-.239h-.004z"/></svg>"#;
