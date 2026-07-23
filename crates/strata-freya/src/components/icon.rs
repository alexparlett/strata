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
    ChevronDown,
    ChevronUp,
    ChevronLeft,
    ChevronRight,
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
            IconName::ChevronDown => CHEVRON_DOWN,
            IconName::ChevronUp => CHEVRON_UP,
            IconName::ChevronLeft => CHEVRON_LEFT,
            IconName::ChevronRight => CHEVRON_RIGHT,
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
const CHEVRON_DOWN: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="m6 9 6 6 6-6"/></svg>"#;
// Header sort chevron, ascending (Rz6).
const CHEVRON_UP: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="m6 15 6-6 6 6"/></svg>"#;
// Status-bar pager prev/next.
const CHEVRON_LEFT: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="m15 6-6 6 6 6"/></svg>"#;
const CHEVRON_RIGHT: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="m9 6 6 6-6 6"/></svg>"#;
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
