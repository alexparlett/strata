//! Command palette (⌘K) — search tables/columns + run commands.
use dioxus::prelude::*;

use crate::action::{dispatch, Action};
use crate::state::AppState;
use crate::ui::components::Dialog;
use crate::ui::icons;

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

    fn action(self) -> Action {
        match self {
            PaletteCommand::NewTable => Action::OpenConfigNew,
            PaletteCommand::Export => Action::OpenExport,
            PaletteCommand::CloseProject => Action::CloseProject,
        }
    }
}

/// Always-mounted host for the command palette. Reads the overlay store reactively
/// and renders the palette only when open. Triggers (⌘K, the header search button)
/// call `overlays::toggle_cmdk`.
#[component]
pub fn CmdkHost() -> Element {
    if !crate::overlays::OVERLAYS.read().cmdk {
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

    // Each row is (label, sub, action) — selecting it just dispatches `action`.
    let mut items: Vec<(String, String, Action)> = Vec::new();
    {
        let s = state.read();
        for t in &s.project.tables {
            items.push((
                format!("Query {}", t.name),
                t.meta.clone(),
                Action::LoadSelectStar(t.name.clone()),
            ));
        }
        for v in &s.project.views {
            items.push((
                format!("Query {}", v.name),
                "view".into(),
                Action::LoadSelectStar(v.name.clone()),
            ));
        }
    }
    for cmd in PaletteCommand::ALL {
        items.push((cmd.label().to_string(), "command".into(), cmd.action()));
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
                {icons::search(17)}
                input {
                    class: "cmdk-input",
                    autofocus: true,
                    placeholder: "Search tables, columns, views — or run a command…",
                    value: "{cmdk_q}",
                    oninput: move |e| query.set(e.value()),
                }
                span { class: "kbd", "ESC" }
            }
            div { class: "cmdk-list",
                if filtered.is_empty() {
                    div { style: "padding:40px;text-align:center;color:var(--dim3);", "No matches" }
                }
                for (label, sub, action) in filtered {
                    div {
                        class: "cmdk-item",
                        onclick: move |_| {
                            dispatch(state, action.clone());
                            on_close.call(());
                        },
                        span { style: "display:flex;color:var(--dim);", {icons::table(15)} }
                        span { class: "lbl", "{label}" }
                        div { class: "spacer" }
                        span { class: "sub", "{sub}" }
                    }
                }
            }
            div { class: "cmdk-foot",
                span { "↑↓ navigate" } span { "↵ select" } span { "esc close" }
            }
        }
    }
}

