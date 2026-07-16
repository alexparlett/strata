//! Command palette (⌘K) — search tables/columns + run commands.
use dioxus::prelude::*;

use crate::action::{dispatch, Action};
use crate::project::ProjectStoreExt;
use crate::state::AppState;
use crate::ui::components::{Dialog, Eyebrow, Icon, Meta, Path, Prose, Spacer, TextInput};
use crate::ui::icons::{IconName, IconSize};

// ---------------------------------------------------------------------------
// Command palette
// ---------------------------------------------------------------------------

/// The static (non-catalog) commands offered by the palette. Add or remove a
/// command by editing `ALL` plus its `label`/`action` arms — no stringly-typed
/// payloads. Catalog rows (query a table/view) are generated from state.
#[derive(Clone, Copy)]
enum PaletteCommand {
    NewTable,
    Export,
    CloseProject,
}

impl PaletteCommand {
    const ALL: &'static [PaletteCommand] = &[
        PaletteCommand::NewTable,
        PaletteCommand::Export,
        PaletteCommand::CloseProject,
    ];

    fn label(self) -> &'static str {
        match self {
            PaletteCommand::NewTable => "New external table…",
            PaletteCommand::Export => "Export results…",
            PaletteCommand::CloseProject => "Close project",
        }
    }

    fn effect(self) -> Effect {
        match self {
            PaletteCommand::NewTable => Effect::Dispatch(Action::OpenConfigNew),
            PaletteCommand::Export => Effect::OpenExport,
            PaletteCommand::CloseProject => Effect::Dispatch(Action::CloseProject),
        }
    }
}

/// What selecting a palette row does: dispatch a normal `AppState` action, or open
/// an overlay-store window directly (window visibility isn't an `AppState` action).
#[derive(Clone)]
enum Effect {
    Dispatch(Action),
    OpenExport,
}

/// Always-mounted host for the command palette. Reads the overlay store reactively
/// and renders the palette only when open. Triggers (⌘K, the header search button)
/// call `overlays::toggle_cmdk`.
#[component]
pub fn CmdkHost() -> Element {
    if !crate::overlays::OVERLAYS.resolve().read().cmdk {
        return rsx! {};
    }
    rsx! {
        CommandPalette { on_close: move |_| crate::overlays::set_cmdk(false) }
    }
}

#[component]
pub fn CommandPalette(on_close: EventHandler<()>) -> Element {
    let state = use_context::<Signal<AppState>>();
    // The query is transient, component-local state: the palette is freshly
    // mounted each time it opens (the root component gates it on its local `cmdk`
    // signal), so it resets naturally.
    let mut query = use_signal(String::new);
    let cmdk_q = query();
    let q = cmdk_q.to_lowercase();

    // Each row is (label, sub, effect) — selecting it runs the effect.
    let mut items: Vec<(String, String, Effect)> = Vec::new();
    {
        let store = crate::project::store();
        let tables = store.tables();
        let views = store.views();
        for t in tables.read().iter() {
            items.push((
                format!("Query {}", t.name),
                t.meta.clone(),
                Effect::Dispatch(Action::LoadSelectStar(t.name.clone())),
            ));
        }
        for v in views.read().iter() {
            items.push((
                format!("Query {}", v.name),
                "view".into(),
                Effect::Dispatch(Action::LoadSelectStar(v.name.clone())),
            ));
        }
    }
    for cmd in PaletteCommand::ALL {
        items.push((cmd.label().to_string(), "command".into(), cmd.effect()));
    }
    let filtered: Vec<_> = items
        .into_iter()
        .filter(|(l, _, _)| q.is_empty() || l.to_lowercase().contains(&q))
        .collect();

    rsx! {
        Dialog {
            on_close: move |_| on_close.call(()),
            card_class: "cmdk".to_string(),
            z: 70,
            top: true,
            has_input: true,
            div { class: "cmdk-head",
                Icon { name: IconName::Search, size: IconSize::Md }
                TextInput {
                    bare: true,
                    grow: true,
                    autofocus: true,
                    placeholder: "Search tables, columns, views — or run a command…",
                    value: "{cmdk_q}",
                    oninput: move |v| query.set(v),
                }
                Eyebrow { class: "kbd", "ESC" }
            }
            div { class: "cmdk-list",
                if filtered.is_empty() {
                    Prose { style: "padding:var(--sp-8);text-align:center;color:var(--dim3);", "No matches" }
                }
                for (label, sub, effect) in filtered {
                    div {
                        class: "cmdk-item",
                        onclick: move |_| {
                            match effect.clone() {
                                Effect::Dispatch(a) => dispatch(state, a),
                                Effect::OpenExport => crate::overlays::open_export(),
                            }
                            on_close.call(());
                        },
                        Icon { name: IconName::Table, size: IconSize::Sm, color: "var(--dim)" }
                        Prose { class: "lbl", "{label}" }
                        Spacer {}
                        Path { class: "sub", "{sub}" }
                    }
                }
            }
            div { class: "cmdk-foot",
                Meta { "↑↓ navigate" } Meta { "↵ select" } Meta { "esc close" }
            }
        }
    }
}
