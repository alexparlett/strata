//! **Settings ▸ Engine ▸ Properties** (P4-07, DEV_TASKS W2) — the DataFusion `ConfigOptions`
//! this app's engines are built with, edited as a free-form key/value table.
//!
//! The other four categories are a list of named settings; this one is a list the *user* names,
//! which is what makes it the one pane with a frame of its own: a toolbar, a grid that takes the
//! height the pane has left, and an inspector under it. Hence [`Pane::filled`] and
//! [`Pane::trailing`] rather than the shared breadcrumb-then-scroll frame — widened on `Pane` so
//! the other four keep one frame between them.
//!
//! **It edits rows, and commits a map.** The setting is a `BTreeMap` of non-default overrides,
//! which cannot hold the row you have not named yet or the duplicate you are halfway through
//! fixing, so the editing model is [`PropRows`] and every edit projects back into
//! `SettingsCtx::draft`. That keeps the window's one commit path intact: Apply still merges the
//! draft field-by-field, and `dirty()` still answers by comparing the draft to its seed.
//!
//! **Applying is the engine's business, not this pane's.** Nothing here talks to an engine —
//! there isn't one in this window. Apply writes the config, and each project window's
//! `use_engine_config` picks the change up: the `ConfigOptions` half lands on the live session,
//! and a changed `datafusion.runtime.*` raises that window's restart confirm, because the
//! `RuntimeEnv` is fixed when the engine is built.

mod inspector;
mod model;
mod table;

use freya::prelude::*;

use crate::apps::settings::views::engine::inspector::Inspector;
use crate::apps::settings::views::engine::table::PropTable;
use crate::apps::settings::views::Pane;
use crate::apps::settings::{SettingsCtx, SettingsThemePartial, SettingsThemePreference};
use crate::components::icon::{Icon, IconName};
use crate::components::typography::{Control, Prose};

pub use model::PropRows;

/// The gap under the blurb, and between the toolbar and the grid (canvas `var(--sp-5)` /
/// `var(--sp-3)`).
const BLURB_GAP: f32 = 16.;
const TOOLBAR_GAP: f32 = 8.;
/// The toolbar's icon buttons — the app's standard 28×28 cluster (AGENTS.md §3).
const TOOL_SIZE: f32 = 28.;

/// What the pane says about itself, once, above the table.
const BLURB: &str = "DataFusion options applied to every engine this app starts. Enter any \
                     datafusion.* property; names autocomplete. Runtime properties \
                     (datafusion.runtime.*) take effect when the engine restarts.";

#[derive(PartialEq)]
pub struct EnginePane;

impl Component for EnginePane {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(
            &None::<SettingsThemePartial>,
            SettingsThemePreference,
            "settings"
        );
        let ctx = use_consume::<SettingsCtx>();
        let mut rows = ctx.engine;

        // The rows are the editing model; the draft is what Apply commits. Projecting on every
        // edit is what keeps the window's single commit path honest — no second write path, and
        // `dirty()` keeps answering the only question it knows how to.
        use_side_effect(move || {
            let next = rows.read().to_map();
            if ctx.draft.peek().engine != next {
                ctx.edit(|settings| settings.engine = next);
            }
        });

        // Revert is offered only when there is something to revert *to* or *from*: the seed's
        // overrides, against the rows as they stand.
        let saved = ctx.seed_engine();
        let revertable = rows.read().to_map() != saved || !rows.read().is_empty();
        let selected = rows.read().selected.is_some();
        let theme_colors = use_theme();
        let colors = theme_colors.read().colors().clone();

        Pane::new(
            rect()
                .expanded()
                .vertical()
                .content(Content::Flex)
                .child(
                    Prose::new(BLURB)
                        .color(theme.hint_color)
                        .max_width(Size::px(620.))
                        .wrap(),
                )
                .child(rect().height(Size::px(BLURB_GAP)))
                .child(
                    rect()
                        .width(Size::fill())
                        .horizontal()
                        .spacing(TOOLBAR_GAP)
                        .child(Tool {
                            icon: IconName::Plus,
                            color: colors.primary,
                            enabled: true,
                            action: EventHandler::new(move |_: Event<PressEventData>| {
                                rows.write().add();
                            }),
                        })
                        .child(Tool {
                            icon: IconName::Minus,
                            color: colors.error,
                            enabled: selected,
                            action: EventHandler::new(move |_: Event<PressEventData>| {
                                rows.write().remove_selected();
                            }),
                        })
                        .child(Tool {
                            icon: IconName::Copy,
                            color: colors.text_secondary,
                            enabled: selected,
                            action: EventHandler::new(move |_: Event<PressEventData>| {
                                rows.write().duplicate_selected();
                            }),
                        })
                        .child(Tool {
                            icon: IconName::Clipboard,
                            color: colors.text_secondary,
                            enabled: true,
                            action: EventHandler::new(move |_: Event<PressEventData>| {
                                if let Ok(text) = Clipboard::get() {
                                    rows.write().paste(&text);
                                }
                            }),
                        }),
                )
                .child(rect().height(Size::px(TOOLBAR_GAP)))
                .child(PropTable { rows })
                .child(Inspector { rows }),
        )
        .filled()
        .maybe_trailing(revertable.then(|| {
            Button::new()
                .height(Size::px(26.))
                .on_press(move |_: Event<PressEventData>| rows.write().revert(&saved))
                .child(
                    rect()
                        .horizontal()
                        .cross_align(Alignment::Center)
                        .spacing(6.)
                        .child(Icon::new(IconName::Reload).size(12.))
                        .child(Control::new("Revert changes")),
                )
        }))
    }
}

/// A toolbar button: the standard 28×28 ghost square, with its action behind an enabled gate.
///
/// Gated by wrapping the handler rather than by `interactive(false)`, which would suppress
/// `pointer_leave` and leave a disabled-while-hovered button painting a stale hover forever
/// (AGENTS.md §3).
#[derive(PartialEq)]
struct Tool {
    icon: IconName,
    /// The action's own tone — add is the accent, remove is destructive, the rest recede.
    color: Color,
    enabled: bool,
    action: EventHandler<Event<PressEventData>>,
}

impl Component for Tool {
    fn render(&self) -> impl IntoElement {
        let action = self.action.clone();

        Button::new()
            .flat()
            .enabled(self.enabled)
            .width(Size::px(TOOL_SIZE))
            .height(Size::px(TOOL_SIZE))
            .on_press(move |e: Event<PressEventData>| action.call(e))
            .child(Icon::new(self.icon).size(15.).color(self.color))
    }
}
