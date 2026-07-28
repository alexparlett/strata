//! Named icons, rendered from the design's own SVGs. `Icon::new(IconName::…).color(c).size(s)`.
//! One place to add/replace an icon — the toolbar/strip/etc. reference them by name, never by
//! inline SVG.

use freya::prelude::*;

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
    Lines,
    Copy,
    Connections,
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
}

impl IconName {
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
            IconName::Lines => LINES,
            IconName::Copy => COPY,
            IconName::Connections => CONNECTIONS,
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
// Plus's opposite — Settings ▸ Engine's **Remove property**. A minus and not a bin: the row is
// one of a list you are editing, not a stored thing being destroyed.
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
// Ragged text lines — the plan view's Raw/Tree toggle.
const LINES: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M4 7h16M4 12h10M4 17h13"/></svg>"#;
// Two overlapped sheets — the record view's Copy row as JSON / CSV buttons.
const COPY: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="11" height="11" rx="2"/><path d="M5 15V5a2 2 0 0 1 2-2h10"/></svg>"#;
// The activity rail's Connections pane (cloud + up-arrow — an object store the project reads from).
const CONNECTIONS: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M17.5 19a4.5 4.5 0 0 0 .5-8.97 6 6 0 0 0-11.64-1.6A4 4 0 0 0 6 16.5"/><path d="M12 12v6"/><path d="m9 15 3-3 3 3"/></svg>"#;
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
