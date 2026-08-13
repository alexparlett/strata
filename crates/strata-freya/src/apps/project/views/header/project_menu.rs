//! The header's **project switcher**: a ghost trigger naming the open project, and the dropdown
//! behind it — **Open…**, the projects that have a window right now, then the recents (design
//! `Strata.dc.html` "project switcher dropdown"; the Dioxus `project_menu_body` is the parity
//! target, which showed only *this* window's project where the canvas shows the whole open set).
//!
//! The **data is real**: this window's project comes from its [`ProjectState`] store, the open
//! set and the recents from the app-global [`AppConfig`] — so a
//! project opening in another window shows up here without any cross-window plumbing of ours.
//!
//! **Acting** on a row goes through the window's [`OpenCtx`] — the same path ⌘O, File ▸
//! Open… and the menubar's Open Recent take, so which window an open lands in is one
//! decision in one place: [`OpenPref`] (this window / a new one / ask). A project that
//! already has a window is *focused* rather than opened twice, and the row for **this**
//! window's project is inert because you are already looking at it.
//!
//! [`OpenPref`]: strata_core::config::OpenPref

use freya::prelude::*;
use freya::radio::use_radio;
use strata_core::config::AppConfig;
use strata_core::util::folder_name;

use crate::apps::project::state::{ProjChan, ProjectState};
use crate::components::avatar::Avatar;
use crate::components::divider::Divider;
use crate::components::icon::{Icon, IconName};
use crate::components::metrics::{MENU_ROW_CHROME, SP_1, SP_3, SP_4};
use crate::components::typography::{Body, Control, Eyebrow, Path, Prose};
use crate::platform::{self, OpenCtx};
use crate::state::AppCtx;
use crate::state::{use_config, ConfigChan};
use crate::theme::{use_roles, Role, RoleColors};

/// The dropdown's width — the comp's 328px card, so a long project path has room to read.
const MENU_WIDTH: f32 = 328.;

/// One switcher row: a project's display name and the folder it lives in.
#[derive(Clone, PartialEq)]
struct ProjectRow {
    name: String,
    path: String,
}

impl ProjectRow {
    /// The row for an **open** project, whose path is all the open-set carries. A project that
    /// loaded was pushed to the recents then (`use_promote_recent`), so the name is normally
    /// there; the fallback to the folder name is what covers the two windows that claim the
    /// open-set without having earned a recent — one still loading, and one showing a load
    /// fault.
    fn for_open(config: &AppConfig, path: &str) -> Self {
        let name = config
            .recent_projects
            .iter()
            .find(|r| r.path == path)
            .map(|r| r.name.clone())
            .unwrap_or_else(|| folder_name(std::path::Path::new(path)));
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
        let app = consume_context::<AppCtx>();
        let opener = consume_context::<OpenCtx>();
        let platform_handle = use_hook(Platform::get);
        let roles = use_roles();

        let project = use_radio::<ProjectState, ProjChan>(ProjChan::Meta);
        let (active_name, active_path) = {
            let p = project.read();
            (p.name.clone(), p.root.to_string_lossy().into_owned())
        };

        let config = use_config(ConfigChan::Recents);
        let open_set = use_config(ConfigChan::Open);
        let _ = open_set.read();
        let (open_rows, recent_rows) = {
            let cfg = config.read();
            let open_rows: Vec<ProjectRow> = cfg
                .open_projects
                .iter()
                .map(|path| ProjectRow::for_open(&cfg, path))
                .collect();
            let recent_rows: Vec<ProjectRow> = cfg
                .recent_projects
                .iter()
                .filter(|r| !cfg.open_projects.contains(&r.path))
                .map(|r| ProjectRow {
                    name: r.name.clone(),
                    path: r.path.clone(),
                })
                .collect();
            (open_rows, recent_rows)
        };

        let menu = Menu::new()
            .min_width(Size::px(MENU_WIDTH))
            .on_close(move |()| open.set(false))
            .child(
                MenuButton::new()
                    .on_press({
                        let app = app.clone();
                        let platform_handle = platform_handle.clone();
                        move |_| {
                            opener.pick(platform_handle.clone(), app.clone());
                            open.set(false);
                        }
                    })
                    .child(
                        rect()
                            .horizontal()
                            .cross_align(Alignment::Center)
                            .spacing(SP_3)
                            .child(Icon::new(IconName::Folder).size(14.))
                            .child(Prose::new("Open…")),
                    ),
            )
            .child(Divider::menu())
            .child(section_label(
                "OPEN PROJECTS",
                roles.get(Role::TextPlaceholder),
            ));
        let menu = open_rows.iter().fold(menu, |menu, row| {
            let current = row.path == active_path;
            menu.child(project_row(
                row,
                true,
                current,
                roles,
                open,
                &app,
                opener,
                &platform_handle,
            ))
        });
        let menu = if recent_rows.is_empty() {
            menu
        } else {
            recent_rows.iter().fold(
                menu.child(Divider::menu()).child(section_label(
                    "RECENT PROJECTS",
                    roles.get(Role::TextPlaceholder),
                )),
                |menu, row| {
                    menu.child(project_row(
                        row,
                        false,
                        false,
                        roles,
                        open,
                        &app,
                        opener,
                        &platform_handle,
                    ))
                },
            )
        };

        let trigger = Button::new()
            .flat()
            .height(Size::px(30.))
            .on_pointer_down(|e: Event<PointerEventData>| e.stop_propagation())
            .on_press(move |_| open.toggle())
            .child(
                rect()
                    .horizontal()
                    .cross_align(Alignment::Center)
                    .spacing(SP_3)
                    .child(
                        Icon::new(IconName::Folder)
                            .color(roles.get(Role::Accent))
                            .size(14.),
                    )
                    .child(
                        Control::new(active_name)
                            .color(roles.get(Role::Text))
                            .max_width(Size::px(220.))
                            .text_overflow(TextOverflow::Ellipsis),
                    )
                    .child(Icon::new(IconName::ChevronDown).size(12.)),
            );

        Attached::new(
            TooltipContainer::new(Tooltip::new_text("Switch project"))
                .position(AttachedPosition::Bottom)
                .child(trigger),
        )
        .bottom()
        .align_start()
        .offset(4.)
        .maybe_child(open().then_some(menu))
    }
}

/// A section heading (`OPEN PROJECTS` / `RECENT PROJECTS`) — the scale's Eyebrow role, which is
/// the comp's tracked 10px mono label exactly.
fn section_label(text: &str, color: Color) -> impl IntoElement {
    rect()
        .padding(Gaps::new(SP_3, SP_4, SP_3, SP_4))
        .child(Eyebrow::new(text).color(color))
}

/// One project row: initials avatar · name over path. The row for the window's *own* project is
/// `selected` (the comp's accent-tinted current row) and does nothing on press — it's already
/// here; every other row opens (or focuses) through the shared open path.
#[allow(clippy::too_many_arguments)]
fn project_row(
    row: &ProjectRow,
    is_open: bool,
    current: bool,
    roles: RoleColors,
    mut open: State<bool>,
    app: &AppCtx,
    opener: OpenCtx,
    platform_handle: &Platform,
) -> MenuItem {
    let path = row.path.clone();
    let app = app.clone();
    let platform_handle = platform_handle.clone();
    MenuItem::new()
        .selected(current)
        .padding(Gaps::new(SP_3, SP_4, SP_3, SP_4))
        .on_press(move |_| {
            if !current {
                if let Some(root) = platform::resolve_recent(app.config, &path) {
                    opener.request(platform_handle.clone(), app.clone(), root);
                }
            }
            open.set(false);
        })
        .child(
            rect()
                .horizontal()
                .cross_align(Alignment::Center)
                .content(Content::Flex)
                .spacing(SP_4)
                .width(Size::fill())
                .max_width(Size::px(MENU_WIDTH - MENU_ROW_CHROME))
                .child(Avatar::new(row.name.as_str()).active(is_open))
                .child(
                    rect()
                        .vertical()
                        .width(Size::flex(1.))
                        .spacing(SP_1)
                        .child(
                            Body::new(row.name.as_str())
                                .color(roles.get(Role::Text))
                                .width(Size::fill())
                                .text_overflow(TextOverflow::Ellipsis),
                        )
                        .child(
                            Path::new(row.path.as_str())
                                .color(roles.get(Role::TextPlaceholder))
                                .width(Size::fill())
                                .text_overflow(TextOverflow::Ellipsis),
                        ),
                ),
        )
}
