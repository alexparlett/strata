//! The launcher's right pane: the filter box and the ghost **OPEN** action over the
//! scrolling **PINNED** / **RECENT** groups.
//!
//! The list is derived, not stored: it reads the app-global config's recents (and the
//! open-set, for the accent avatar) and re-groups on every keystroke through
//! [`ProjectList::build`]. There is no launcher-local copy of the recents to keep fresh,
//! which is the whole point — a pin written here, or a project opened in another window,
//! shows up because the *store* changed.

use freya::prelude::*;

use crate::apps::launcher::model::{ProjectList, ProjectRow};
use crate::apps::launcher::views::open::pick_and_open;
use crate::apps::launcher::views::row::ProjectRowView;
use crate::apps::launcher::{LauncherThemePartial, LauncherThemePreference};
use crate::components::icon::{Icon, IconName};
use crate::components::metrics::{SP_3, SP_4, SP_5, SP_6};
use crate::components::typography::{Control, Eyebrow, InputTypography, Prose};
use crate::state::{use_config, AppCtx, ConfigChan};
use crate::theme::{use_roles, Role};

/// The filter box's cap (canvas `max-width: 420px`), so it doesn't stretch to the pane.
const SEARCH_MAX_WIDTH: f32 = 420.;

#[derive(PartialEq)]
pub struct ProjectsPane {
    pub app: AppCtx,
}

impl Component for ProjectsPane {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(
            &None::<LauncherThemePartial>,
            LauncherThemePreference,
            "launcher"
        );
        let roles = use_roles();
        let query = use_state(String::new);

        // Two subscriptions, one read: the recents move when a project is opened, pinned or
        // removed, and the open-set moves when any window opens or closes — both change what
        // this list paints. Both handles must be **read**, not merely taken: a radio
        // subscribes in `read()`, so an unread handle is inert and its channel never wakes
        // this pane (the launcher would keep painting a just-closed project as open).
        let config = use_config(ConfigChan::Recents);
        let open_set = use_config(ConfigChan::Open);
        let list = ProjectList::build(&open_set.read(), &query.read());
        let _ = config.read();

        let app = self.app.clone();
        // Keyed by path: pinning or removing re-groups the list, and unkeyed children are
        // paired by index, which would leave per-row hover state on whichever project slid
        // into that position.
        let row = |row: &ProjectRow| {
            ProjectRowView {
                row: row.clone(),
                app: app.clone(),
                key: DiffKey::None,
            }
            .key(row.path.as_str())
        };

        // PINNED first (only when something is pinned), then RECENT — which heads the
        // recents whether or not anything is pinned above it (V26).
        let groups = rect().width(Size::fill()).vertical();
        let groups = if list.pinned.is_empty() {
            groups
        } else {
            list.pinned
                .iter()
                .fold(
                    groups.child(group_label("PINNED", theme.label_color)),
                    |el, r| el.child(row(r)),
                )
                // The canvas's 10px gap between the groups.
                .child(rect().width(Size::fill()).height(Size::px(10.)))
        };
        let groups = if list.recent.is_empty() {
            groups
        } else {
            list.recent.iter().fold(
                groups.child(group_label("RECENT", theme.label_color)),
                |el, r| el.child(row(r)),
            )
        };

        // Two empty states, because they mean different things: nothing matched what you
        // typed, versus you have no projects yet.
        let empty = list.is_empty().then(|| {
            let q = query.read().trim().to_string();
            let copy = if q.is_empty() {
                "No recent projects — choose one with OPEN.".to_string()
            } else {
                format!("No projects match \u{201c}{q}\u{201d}.")
            };
            rect()
                .width(Size::fill())
                .padding(Gaps::new(SP_6, SP_4, SP_6, SP_4))
                .child(
                    Prose::new(copy)
                        .color(roles.get(Role::TextPlaceholder))
                        .wrap(),
                )
        });

        let open_app = self.app.clone();
        let toolbar = rect()
            .width(Size::fill())
            .horizontal()
            .cross_align(Alignment::Center)
            .content(Content::Flex)
            .spacing(SP_5)
            .padding(Gaps::new(SP_5, SP_6, SP_4, SP_6))
            .child(
                rect()
                    .width(Size::flex(1.))
                    .max_width(Size::px(SEARCH_MAX_WIDTH))
                    .child(
                        InputTypography::body(
                            Input::new(query)
                                .leading(
                                    Icon::new(IconName::Search)
                                        .color(roles.get(Role::TextPlaceholder))
                                        .size(14.),
                                )
                                .placeholder("Search projects")
                                .width(Size::fill()),
                        )
                        .width(Size::fill()),
                    ),
            )
            // The canvas's second flex child: the filter stops growing at its cap, so this
            // absorbs the rest and pins OPEN to the right edge.
            .child(rect().height(Size::px(1.)).width(Size::flex(1.)))
            .child(
                Button::new()
                    .flat()
                    // The canvas's ghost action: the whole control is one tone at rest and
                    // one on hover. Colouring only the label would leave the glyph on the
                    // flat-button ramp, so they'd disagree at rest *and* diverge on hover.
                    .theme_colors(
                        ButtonColorsThemePartial::default()
                            .color(theme.title_color)
                            .hover_color(roles.get(Role::Accent)),
                    )
                    .on_press(move |_| pick_and_open(open_app.clone()))
                    .child(
                        rect()
                            .horizontal()
                            .cross_align(Alignment::Center)
                            .spacing(SP_3)
                            .child(Icon::new(IconName::Folder).size(15.))
                            // The canvas's 12.5px UI text — the Control role. (`Eyebrow` is
                            // the 10px mono group label the PINNED / RECENT headings wear.)
                            .child(Control::new("OPEN")),
                    ),
            );

        rect()
            .width(Size::flex(1.))
            .height(Size::fill())
            .vertical()
            .content(Content::Flex)
            .child(toolbar)
            .child(
                ScrollView::new()
                    .width(Size::fill())
                    .height(Size::flex(1.))
                    .child(
                        rect()
                            .width(Size::fill())
                            .vertical()
                            .padding(Gaps::new(0., SP_4, SP_4, SP_4))
                            .child(groups)
                            .maybe_child(empty),
                    ),
            )
    }
}

/// A group heading (`PINNED` / `RECENT`) — the scale's Eyebrow role, which is the canvas's
/// tracked 10px mono label exactly.
fn group_label(text: &str, color: Color) -> impl IntoElement {
    rect()
        .padding(Gaps::new(SP_3, SP_4, SP_3, SP_4))
        .child(Eyebrow::new(text).color(color))
}
