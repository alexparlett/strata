//! The header's **project switcher**: a ghost trigger naming the open project, and the dropdown
//! behind it — **Open…**, the projects that have a window right now, then the recents (design
//! `Strata.dc.html` "project switcher dropdown"; the Dioxus `project_menu_body` is the parity
//! target, which showed only *this* window's project where the canvas shows the whole open set).
//!
//! The **data is real**: this window's project comes from its [`ProjectState`] store, the open
//! set and the recents from the app-global [`AppConfig`](strata_core::config::AppConfig) — so a
//! project opening in another window shows up here without any cross-window plumbing of ours.
//!
//! **Acting** on a row is deliberately not wired: opening a project is one mechanism (folder
//! pick → `.strata/` load → this-window-or-new-window per [`OpenPref`], plus the re-open-in-place
//! guard), owned by **P4-13** with **P4-01**'s window model. There is nothing to call yet, so the
//! rows log and close; wire them at that one seam rather than folding a header-local open path in
//! here.
//!
//! [`OpenPref`]: strata_core::config::OpenPref

use freya::prelude::*;
use freya::radio::use_radio;
use strata_core::config::AppConfig;

use freya::components::use_theme;

use crate::apps::project::state::{ProjChan, ProjectState};
use crate::components::avatar::Avatar;
use crate::components::divider::Divider;
use crate::components::icon::{Icon, IconName};
use crate::components::typography::{Body, Control, Eyebrow, Path, Prose};
use crate::state::{use_config, ConfigChan};

/// The dropdown's width — the comp's 328px card, so a long project path has room to read.
const MENU_WIDTH: f32 = 328.;

/// The horizontal chrome around a project row: the `menu_container` card padding (4 × 2) plus the
/// row's own (12 × 2). A `Menu` only takes a **min** width and its container hugs its children, so
/// a full path would otherwise stretch the card to the whole window — capping the row at
/// `MENU_WIDTH - MENU_ROW_CHROME` is what fixes the card at [`MENU_WIDTH`] *and* gives the name /
/// path a bounded box to ellipsize in (the same recipe as the tab menus' `HINT_MENU_WIDTH`).
const MENU_ROW_CHROME: f32 = 32.;

/// One switcher row: a project's display name and the folder it lives in.
#[derive(Clone, PartialEq)]
struct ProjectRow {
    name: String,
    path: String,
}

impl ProjectRow {
    /// The row for an **open** project, whose path is all the open-set carries. Every open
    /// project was pushed to the recents when its window mounted (`use_open_project`), so the
    /// name is normally there; a missing one degrades to the folder name.
    fn for_open(config: &AppConfig, path: &str) -> Self {
        let name = config
            .recent_projects
            .iter()
            .find(|r| r.path == path)
            .map(|r| r.name.clone())
            .unwrap_or_else(|| {
                std::path::Path::new(path)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.to_string())
            });
        Self {
            name,
            path: path.to_string(),
        }
    }
}

#[derive(PartialEq)]
pub struct ProjectMenu;

impl Component for ProjectMenu {
    fn render(&self) -> impl IntoElement {
        let mut open = use_state(|| false);
        // Everything the dropdown paints is a root colour — the accent, and the text ramp — so
        // it reads the sheet through the normal hook rather than inventing header-only theme
        // fields for colours the palette already names. The rows' tiles are `Avatar`'s theme and
        // the separators are `Divider::menu`'s.
        let colors = use_theme().read().colors.clone();

        // This window's project — `ProjChan::Meta` is exactly "the identity changed".
        let project = use_radio::<ProjectState, ProjChan>(ProjChan::Meta);
        let (active_name, active_path) = {
            let p = project.read();
            (p.name.clone(), p.root.to_string_lossy().into_owned())
        };

        // Two subscriptions, one read: a window opening or closing anywhere moves the open set,
        // and opening a project also re-orders the recents. Both change what this menu lists.
        let config = use_config(ConfigChan::Recents);
        let _open_set = use_config(ConfigChan::Open);
        let (open_rows, recent_rows) = {
            let cfg = config.read();
            let open_rows: Vec<ProjectRow> = cfg
                .open_projects
                .iter()
                .map(|path| ProjectRow::for_open(&cfg, path))
                .collect();
            // A project that's open is listed above; the recents section is what's *only* recent.
            let recent_rows: Vec<ProjectRow> = cfg
                .recent_projects
                .iter()
                .filter(|r| !cfg.open_projects.iter().any(|p| *p == r.path))
                .map(|r| ProjectRow {
                    name: r.name.clone(),
                    path: r.path.clone(),
                })
                .collect();
            (open_rows, recent_rows)
        };

        // Built by folding the two lists in — the `Menu` is never held in a mutable variable.
        let menu = Menu::new()
            .min_width(Size::px(MENU_WIDTH))
            .on_close(move |_| open.set(false))
            .child(
                MenuButton::new()
                    .on_press(move |_| {
                        tracing::debug!("project switcher: open-project not built yet (P4-13)");
                        open.set(false);
                    })
                    .child(
                        rect()
                            .horizontal()
                            .cross_align(Alignment::Center)
                            .spacing(8.)
                            .child(Icon::new(IconName::Folder).size(14.))
                            .child(Prose::new("Open…")),
                    ),
            )
            .child(Divider::menu())
            .child(section_label("OPEN PROJECTS", colors.text_placeholder));
        let menu = open_rows.iter().fold(menu, |menu, row| {
            let current = row.path == active_path;
            menu.child(project_row(row, true, current, &colors, open))
        });
        let menu = if recent_rows.is_empty() {
            menu
        } else {
            recent_rows.iter().fold(
                menu.child(Divider::menu())
                    .child(section_label("RECENT PROJECTS", colors.text_placeholder)),
                |menu, row| menu.child(project_row(row, false, false, &colors, open)),
            )
        };

        // The comp's ghost trigger: transparent until hover (the `flat_button` dress), folder
        // glyph in the accent, the project name, and the ⌄ affordance. The pointer-down stop is
        // what keeps a press on it from dragging the window (the bar is the drag region).
        let trigger = Button::new()
            .flat()
            .height(Size::px(30.))
            .on_pointer_down(|e: Event<PointerEventData>| e.stop_propagation())
            .on_press(move |_| open.toggle())
            .child(
                rect()
                    .horizontal()
                    .cross_align(Alignment::Center)
                    .spacing(8.)
                    .child(Icon::new(IconName::Folder).color(colors.primary).size(14.))
                    .child(
                        Control::new(active_name)
                            .color(colors.text_primary)
                            .max_width(Size::px(220.))
                            .text_overflow(TextOverflow::Ellipsis),
                    )
                    .child(Icon::new(IconName::ChevronDown).size(12.)),
            );

        Attached::new(
            TooltipContainer::new(Tooltip::new("Switch project"))
                .position(AttachedPosition::Bottom)
                .child(trigger),
        )
        .bottom()
        .align_start()
        // A few pixels off the trigger, so the card reads as its own surface rather than growing
        // out of the button.
        .offset(4.)
        .maybe_child(open().then(|| menu))
    }
}

/// A section heading (`OPEN PROJECTS` / `RECENT PROJECTS`) — the scale's Eyebrow role, which is
/// the comp's tracked 10px mono label exactly.
fn section_label(text: &str, color: Color) -> impl IntoElement {
    rect()
        .padding(Gaps::new(8., 12., 8., 12.))
        .child(Eyebrow::new(text).color(color))
}

/// One project row: initials avatar · name over path. The row for the window's *own* project is
/// `selected` (the comp's accent-tinted current row) and does nothing on press — it's already
/// here; every other row is where P4-13's open path will hang.
fn project_row(
    row: &ProjectRow,
    is_open: bool,
    current: bool,
    colors: &ColorsSheet,
    mut open: State<bool>,
) -> MenuItem {
    let path = row.path.clone();
    MenuItem::new()
        .selected(current)
        .padding(Gaps::new(8., 12., 8., 12.))
        .on_press(move |_| {
            if !current {
                tracing::debug!("project switcher: opening `{path}` not built yet (P4-13)");
            }
            open.set(false);
        })
        .child(
            rect()
                .horizontal()
                .cross_align(Alignment::Center)
                .content(Content::Flex)
                .spacing(12.)
                .width(Size::fill())
                .max_width(Size::px(MENU_WIDTH - MENU_ROW_CHROME))
                .child(Avatar::new(row.name.as_str()).active(is_open))
                .child(
                    rect()
                        .vertical()
                        .width(Size::flex(1.))
                        .spacing(2.)
                        .child(
                            Body::new(row.name.as_str())
                                .color(colors.text_primary)
                                .width(Size::fill())
                                .text_overflow(TextOverflow::Ellipsis),
                        )
                        .child(
                            Path::new(row.path.as_str())
                                .color(colors.text_placeholder)
                                .width(Size::fill())
                                .text_overflow(TextOverflow::Ellipsis),
                        ),
                ),
        )
}
