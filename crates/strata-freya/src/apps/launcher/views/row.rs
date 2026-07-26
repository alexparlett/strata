//! One project row: the initials tile, the name over its folder, and the three hover
//! actions — **Pin · Reveal · Remove** (V26's three; there is no open-in-new-window, since
//! every open already goes to its own window).
//!
//! Pin and Remove write the app-global config through `write_config`, which persists — so
//! there is no load/save dance at the call site and no launcher-local copy of the recents
//! to keep in step. Pin state reads from the PINNED grouping and the action's accent tint;
//! the canvas carries no inline badge.

use freya::prelude::*;

use crate::apps::launcher::model::ProjectRow;
use crate::apps::launcher::views::open::open_and_close;
use crate::apps::launcher::{LauncherThemePartial, LauncherThemePreference};
use crate::components::avatar::Avatar;
use crate::components::icon::{Icon, IconName};
use crate::components::typography::{Body, Path as PathText};
use crate::state::{use_config_station, write_config, AppCtx, ConfigChan};

#[derive(PartialEq)]
pub struct ProjectRowView {
    pub row: ProjectRow,
    pub app: AppCtx,
    /// Reconciliation key — the project's path. Without it the rows pair by index, and a
    /// pin (which moves a row between the two groups) would leave this row's hover state on
    /// whichever project slid into its place.
    pub key: DiffKey,
}

impl KeyExt for ProjectRowView {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for ProjectRowView {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(
            &None::<LauncherThemePartial>,
            LauncherThemePreference,
            "launcher"
        );
        let colors = use_theme().read().colors().clone();
        let mut hovered = use_state(|| false);

        let ProjectRow {
            name,
            path,
            pinned,
            open,
        } = self.row.clone();
        // The station, never a subscribing radio: a pin / remove is a write, and the row
        // already re-renders because the list above it re-derives.
        let config = use_config_station();
        let app = self.app.clone();

        // Pin / unpin, and drop from the list. Both are single edits on the `Recents`
        // channel: the list above re-derives, every other window's switcher with it.
        let pin_path = path.clone();
        let on_pin = move || {
            write_config(config, &[ConfigChan::Recents], |cfg| {
                cfg.set_pinned(&pin_path, !pinned)
            });
        };
        let remove_path = path.clone();
        let on_remove = move || {
            write_config(config, &[ConfigChan::Recents], |cfg| {
                cfg.remove_recent(&remove_path)
            });
        };
        let reveal_path = path.clone();
        let on_reveal = move || reveal(&reveal_path);
        let open_path = path.clone();
        let on_open =
            move |_: Event<PressEventData>| open_and_close(app.clone(), open_path.clone());

        rect()
            .width(Size::fill())
            .horizontal()
            .cross_align(Alignment::Center)
            .content(Content::Flex)
            .spacing(12.)
            .padding(Gaps::new(8., 12., 8., 12.))
            .corner_radius(10.)
            .background(if hovered() {
                theme.row_hover_background
            } else {
                Color::TRANSPARENT
            })
            // `over`/`out`, not `enter`/`leave`: the latter are exclusive to the deepest
            // listening node, so moving onto one of the row's own action buttons would fire
            // the row's leave and drop its tint out from under the cursor. This is the pair
            // Freya's own `Button` uses for its hover fill.
            .on_pointer_over(move |_| hovered.set(true))
            .on_pointer_out(move |_| hovered.set_if_modified(false))
            .on_press(on_open)
            // The accent tile marks a project that already has a window — pressing the row
            // focuses it rather than opening a second.
            .child(Avatar::new(name.as_str()).active(open).size(32.))
            .child(
                rect()
                    .vertical()
                    .width(Size::flex(1.))
                    .spacing(2.)
                    .child(
                        Body::new(name.as_str())
                            .color(colors.text_primary)
                            .width(Size::fill())
                            .text_overflow(TextOverflow::Ellipsis),
                    )
                    .child(
                        PathText::new(path.as_str())
                            .color(colors.text_placeholder)
                            .width(Size::fill())
                            .text_overflow(TextOverflow::Ellipsis),
                    ),
            )
            .child(
                rect()
                    .horizontal()
                    .cross_align(Alignment::Center)
                    .spacing(2.)
                    .child(RowAction {
                        icon: IconName::Pin,
                        title: if pinned { "Unpin" } else { "Pin" }.into(),
                        // A pinned row's pin wears the accent; everything else is recessive
                        // until hovered.
                        color: if pinned {
                            colors.primary
                        } else {
                            colors.text_placeholder
                        },
                        hover_background: colors.active,
                        hover_color: colors.text_primary,
                        on_press: EventHandler::new(move |_: Event<PressEventData>| on_pin()),
                    })
                    .child(RowAction {
                        icon: IconName::Folder,
                        title: "Reveal on disk".into(),
                        color: colors.text_placeholder,
                        hover_background: colors.active,
                        hover_color: colors.text_primary,
                        on_press: EventHandler::new(move |_: Event<PressEventData>| on_reveal()),
                    })
                    .child(RowAction {
                        icon: IconName::Close,
                        title: "Remove from list".into(),
                        color: colors.text_placeholder,
                        hover_background: theme.remove_hover_background,
                        hover_color: colors.error,
                        on_press: EventHandler::new(move |_: Event<PressEventData>| on_remove()),
                    }),
            )
    }
}

/// One 28×28 ghost action on a row. Each stops the press propagating, so it never also
/// opens the project underneath it.
#[derive(PartialEq)]
struct RowAction {
    icon: IconName,
    title: String,
    color: Color,
    hover_background: Color,
    hover_color: Color,
    on_press: EventHandler<Event<PressEventData>>,
}

impl Component for RowAction {
    fn render(&self) -> impl IntoElement {
        let mut hovered = use_state(|| false);
        let on_press = self.on_press.clone();

        TooltipContainer::new(Tooltip::new(self.title.clone()))
            .position(AttachedPosition::Bottom)
            .child(
                rect()
                    .width(Size::px(28.))
                    .height(Size::px(28.))
                    .corner_radius(6.)
                    .center()
                    .background(if hovered() {
                        self.hover_background
                    } else {
                        Color::TRANSPARENT
                    })
                    .color(if hovered() {
                        self.hover_color
                    } else {
                        self.color
                    })
                    .on_pointer_enter(move |_| hovered.set(true))
                    .on_pointer_leave(move |_| hovered.set(false))
                    .on_press(move |e: Event<PressEventData>| {
                        e.stop_propagation();
                        on_press.call(e);
                    })
                    .child(Icon::new(self.icon).size(15.)),
            )
    }
}

/// Show a project's folder in the OS file manager.
fn reveal(path: &str) {
    #[cfg(target_os = "macos")]
    let command = ("open", path);
    #[cfg(target_os = "windows")]
    let command = ("explorer", path);
    #[cfg(all(unix, not(target_os = "macos")))]
    let command = ("xdg-open", path);

    if let Err(e) = std::process::Command::new(command.0).arg(command.1).spawn() {
        tracing::error!("reveal `{path}`: {e}");
    }
}
