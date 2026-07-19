//! The Settings **window** — a standalone OS window (its own `VirtualDom`) spawned
//! single-instance via `crate::window::spawn_settings_window` (header / launcher gear,
//! ⌘,, File menu).
//!
//! **Navigation uses the dioxus router** (desktop → in-memory history, no URL bar):
//! [`SettingsRoute`] under the [`SettingsChrome`] layout, one page per submodule. `/`
//! opens on Appearance.
//!
//! **Draft / save model.** [`SettingsRoot`] owns a *local* `draft` copy of the settings
//! + the Engine Properties editor's row state, provided to the pages via [`SettingsCtx`].
//! Controls edit the draft; **Apply** validates the Properties editor, merges its rows in,
//! and persists the whole draft (or jumps to the Engine page on error) then closes;
//! **Cancel** / close discards. **Theme previews live** (see [`crate::settings`]).

mod appearance;
mod data_display;
mod engine;
mod keymap;
mod system;

use std::rc::Rc;

use dioxus::desktop::{use_muda_event_handler, use_window, use_wry_event_handler};
use dioxus::html::ScrollBehavior;
use dioxus::prelude::*;

use crate::config::Settings;
use crate::ui::components::{Body, Button, ButtonVariant, Caption, Icon, Prose, SearchBar, Spacer};
use crate::ui::icons::{IconName, IconSize};

use appearance::Appearance;
use data_display::DataDisplay;
use engine::{use_engine_state, Engine, EngineState};
use keymap::Keymap;
use system::System;

/// Root class: on macOS the transparent titlebar puts the native traffic lights
/// top-left, so the titlebar row gets extra left padding there.
#[cfg(target_os = "macos")]
const SETTINGS_CLASS: &str = "settings-root mac";
#[cfg(not(target_os = "macos"))]
const SETTINGS_CLASS: &str = "settings-root";

/// Shared state for the settings pages — provided by [`SettingsRoot`], read by each page
/// + the chrome via `use_context`. `draft` is the local editable copy of the settings;
/// `engine` is the Properties editor's row/selection state, merged into `draft` on Apply.
#[derive(Clone, Copy)]
struct SettingsCtx {
    draft: Signal<Settings>,
    engine: EngineState,
    /// The settings-search jump target: the [`SETTINGS_INDEX`] id to scroll to + flash.
    /// Set when a search result is picked, cleared by the matching [`Anchor`] once it fires.
    flash: Signal<Option<&'static str>>,
}

/// A settings category (nav leaf) — maps to a route + the label shown in search results.
#[derive(Clone, Copy, PartialEq)]
enum SettingsCat {
    Appearance,
    System,
    DataDisplay,
    Keymap,
    Engine,
}

impl SettingsCat {
    fn route(self) -> SettingsRoute {
        match self {
            SettingsCat::Appearance => SettingsRoute::Appearance {},
            SettingsCat::System => SettingsRoute::System {},
            SettingsCat::DataDisplay => SettingsRoute::DataDisplay {},
            SettingsCat::Keymap => SettingsRoute::Keymap {},
            SettingsCat::Engine => SettingsRoute::Engine {},
        }
    }
    fn label(self) -> &'static str {
        match self {
            SettingsCat::Appearance => "Theme",
            SettingsCat::System => "System",
            SettingsCat::DataDisplay => "Data display",
            SettingsCat::Keymap => "Keymap",
            SettingsCat::Engine => "Properties",
        }
    }
}

/// One searchable setting: a stable `id` (matched by its [`Anchor`] on the page), the label
/// the search matches + shows, and the category it lives under.
struct SettingEntry {
    id: &'static str,
    label: &'static str,
    cat: SettingsCat,
}

/// The flat search index over every settings field. `id`s pair with an [`Anchor`] on the page.
const SETTINGS_INDEX: &[SettingEntry] = &[
    SettingEntry { id: "theme", label: "Theme", cat: SettingsCat::Appearance },
    SettingEntry { id: "sync-os", label: "Sync with OS", cat: SettingsCat::Appearance },
    SettingEntry { id: "reopen", label: "Reopen projects on startup", cat: SettingsCat::System },
    SettingEntry { id: "default-dir", label: "Default project directory", cat: SettingsCat::System },
    SettingEntry { id: "open-pref", label: "Opening a project", cat: SettingsCat::System },
    SettingEntry { id: "confirm-close", label: "Confirm before closing a running query", cat: SettingsCat::System },
    SettingEntry { id: "max-history", label: "Query history limit", cat: SettingsCat::System },
    SettingEntry { id: "density", label: "Row density", cat: SettingsCat::DataDisplay },
    SettingEntry { id: "zebra", label: "Alternating row colors", cat: SettingsCat::DataDisplay },
    SettingEntry { id: "col-width", label: "Default column width", cat: SettingsCat::DataDisplay },
    SettingEntry { id: "row-limit", label: "Default row limit", cat: SettingsCat::DataDisplay },
    SettingEntry { id: "keymap", label: "Keyboard shortcuts", cat: SettingsCat::Keymap },
    SettingEntry { id: "engine", label: "Engine properties", cat: SettingsCat::Engine },
];

/// Wraps a settings field so the search can scroll it into view + flash it when picked.
/// Self-contained: owns its element ref, watches the shared [`SettingsCtx::flash`] target,
/// and lights up (a one-shot CSS animation, cleared on `animationend`) when it's the target.
#[component]
pub fn Anchor(id: &'static str, children: Element) -> Element {
    let mut flash = use_context::<SettingsCtx>().flash;
    let mut me = use_signal(|| None::<Rc<MountedData>>);
    let mut lit = use_signal(|| false);

    let mut fire = move || {
        if *flash.peek() != Some(id) {
            return;
        }
        // Wait until the element is mounted — on a fresh navigation the effect can run
        // before `onmounted`, so leave `flash` set for `onmounted` to retry.
        let Some(m) = me.peek().clone() else {
            return;
        };
        flash.set(None);
        lit.set(true);
        spawn(async move {
            let _ = m.scroll_to(ScrollBehavior::Smooth).await;
        });
    };
    // Same-page pick (element already mounted) reacts here; a fresh navigation fires from
    // `onmounted` below once the target page's field mounts.
    use_effect(move || {
        let _ = flash.read();
        fire();
    });

    rsx! {
        div {
            class: if lit() { "settings-anchor lit" } else { "settings-anchor" },
            onmounted: move |e| {
                me.set(Some(e.data()));
                fire();
            },
            onanimationend: move |_| lit.set(false),
            {children}
        }
    }
}

/// The settings pages, routed. Each variant maps to the page component of the same name,
/// wrapped by [`SettingsChrome`].
#[derive(Routable, Clone, PartialEq)]
#[rustfmt::skip]
enum SettingsRoute {
    #[layout(SettingsChrome)]
        #[route("/")]
        Appearance {},
        #[route("/data")]
        DataDisplay {},
        #[route("/system")]
        System {},
        #[route("/engine")]
        Engine {},
        #[route("/keymap")]
        Keymap {},
}

/// The Settings window root: window-level wiring (focus / OS-theme / Edit-menu / drop),
/// the shared draft + engine form, and the router.
#[component]
pub fn SettingsRoot() -> Element {
    let win = use_window();
    let win_id = win.id();
    // Wire this window into the shared settings context (seed once + reactive theme
    // css), and register it so a repeat open focuses this window.
    let theme_css = crate::settings::use_settings();
    use_hook(crate::window::register_settings_window);
    #[cfg(target_os = "macos")]
    use_hook(|| crate::window::paint_ns_background(0.043, 0.055, 0.075));
    #[cfg(target_os = "macos")]
    use_hook(crate::window::attach_settings_to_owner);

    use_wry_event_handler(move |event, _| {
        use dioxus::desktop::tao::event::{Event as TaoEvent, WindowEvent};
        if let TaoEvent::WindowEvent {
            window_id, event, ..
        } = event
        {
            if *window_id != win_id {
                return;
            }
            match event {
                // Follow the OS light/dark switch live (Sync-with-OS, no restart).
                WindowEvent::ThemeChanged(theme) => {
                    use dioxus::desktop::tao::window::Theme;
                    crate::settings::set_os_dark(*theme == Theme::Dark);
                }
                _ => {}
            }
        }
    });

    // ⌘A / ⌘C are app-global menu commands; route them to the focused text field when
    // this window is focused (the Settings window has no grid).
    use_muda_event_handler(move |ev| {
        if !crate::window::is_focused_window(win_id) {
            return;
        }
        match crate::menu::MenuCmd::parse(&ev.id().0) {
            Some(crate::menu::MenuCmd::SelectAll) => {
                if matches!(
                    crate::keymap::focus_responder(),
                    Some(crate::keymap::Responder::TextInput)
                ) {
                    crate::window::send_select_all();
                }
            }
            Some(crate::menu::MenuCmd::Copy) => crate::window::send_copy(),
            _ => {}
        }
    });

    // On close discard any live theme preview + release the single-window slot.
    use_drop(|| {
        crate::settings::revert_theme_preview();
        crate::window::unregister_settings_window();
    });

    // The local editable draft, seeded from the committed settings.
    let draft = use_signal(crate::settings::snapshot);

    // The Properties editor's row state, seeded from the committed overrides. On Apply
    // (in the footer) it validates + normalizes into the draft; nothing is persisted
    // until then.
    let engine = use_engine_state(draft.peek().engine.clone());

    // Settings-search jump target (id to scroll to + flash); set on result pick.
    let flash = use_signal(|| None::<&'static str>);

    use_context_provider(|| SettingsCtx { draft, engine, flash });

    // The window chrome follows the live theme; density drives the row-height token.
    let density = if draft.read().density_compact {
        "compact"
    } else {
        "comfortable"
    };

    rsx! {
        style { dangerous_inner_html: crate::CSS }
        div {
            class: "{SETTINGS_CLASS}",
            // Focusable + focused on mount so Esc is caught immediately (webview keydowns
            // land on / bubble to this root). Esc closes the window — same as Cancel; the
            // drop handler reverts any live theme preview. A child that owns Esc (the keymap
            // capture) stops propagation, so it won't reach here.
            tabindex: "0",
            onmounted: move |e| { spawn(async move { let _ = e.set_focus(true).await; }); },
            onkeydown: move |e| {
                if e.key() == Key::Escape {
                    e.prevent_default();
                    dioxus::desktop::window().close();
                }
            },
            style: "{theme_css}",
            "data-density": "{density}",
            Router::<SettingsRoute> {}
        }
    }
}

/// The window chrome (layout) wrapping every page: titlebar + left nav + the routed
/// `Outlet` + the Cancel / Apply footer.
#[component]
fn SettingsChrome() -> Element {
    let route = use_route::<SettingsRoute>();
    let ctx = use_context::<SettingsCtx>();
    // Disclosure-group open state — local to the layout, so it survives page navigation
    // (the layout stays mounted). Both groups start open.
    let ap_open = use_signal(|| true);
    let eng_open = use_signal(|| true);
    // Settings search: while the query is non-empty the nav tree is replaced by a flat
    // results list (index-matched), each jumping to + flashing its field on the page.
    let mut query = use_signal(String::new);
    let (group, leaf) = crumb_of(&route);
    rsx! {
        // Titlebar (native traffic lights sit to the left of this on macOS). The webview
        // covers the native title bar, so drag from the titlebar background.
        div { class: "settings-titlebar",
            onmousedown: move |e| { e.prevent_default(); dioxus::desktop::window().drag(); },
            div { class: "settings-tb-badge", Icon { name: IconName::Gear, size: IconSize::Sm } }
            span { class: "settings-tb-title", "Settings" }
        }
        div { class: "settings-body",
            div { class: "settings-nav",
                SearchBar {
                    value: "{query}",
                    placeholder: "Search settings",
                    oninput: move |v| query.set(v),
                }
                {
                    let q = query.read().trim().to_lowercase();
                    if q.is_empty() {
                        rsx! {
                            {nav_group(ap_open, "Appearance & behaviour")}
                            if ap_open() {
                                {nav_leaf(&route, SettingsRoute::Appearance {}, "Theme")}
                                {nav_leaf(&route, SettingsRoute::System {}, "System")}
                                {nav_leaf(&route, SettingsRoute::DataDisplay {}, "Data display")}
                            }
                            {nav_top(&route, SettingsRoute::Keymap {}, "Keymap")}
                            {nav_group(eng_open, "Engine")}
                            if eng_open() {
                                {nav_leaf(&route, SettingsRoute::Engine {}, "Properties")}
                            }
                        }
                    } else {
                        let hits: Vec<&SettingEntry> = SETTINGS_INDEX
                            .iter()
                            .filter(|e| e.label.to_lowercase().contains(&q))
                            .collect();
                        if hits.is_empty() {
                            rsx! {
                                div { class: "settings-search-empty",
                                    Caption { "No settings match your search." }
                                }
                            }
                        } else {
                            rsx! {
                                div { class: "settings-search-results",
                                    for e in hits {
                                        {search_result(e, query, ctx.flash)}
                                    }
                                }
                            }
                        }
                    }
                }
            }
            div { class: "settings-pane ps-scroll",
                Prose { class: "settings-crumb",
                    if let Some(g) = group {
                        "{g} " span { style: "color:var(--faint2);", "›" } " "
                    }
                    span { style: "color:var(--text3);", "{leaf}" }
                }
                Outlet::<SettingsRoute> {}
            }
        }
        // Footer — Cancel discards (the drop handler reverts the live theme preview).
        // Apply validates the Properties editor: on error it reveals the messages and
        // jumps to the Engine page; otherwise it merges the rows into the draft, persists
        // the whole `Settings`, and closes.
        div { class: "settings-foot",
            Spacer {}
            Button { variant: ButtonVariant::Ghost, onclick: move |_| dioxus::desktop::window().close(), "Cancel" }
            Button {
                variant: ButtonVariant::Primary,
                onclick: move |_| {
                    if ctx.engine.validate_and_show() {
                        let mut s = ctx.draft.peek().clone();
                        s.engine = ctx.engine.to_map();
                        crate::settings::save_draft(s);
                        dioxus::desktop::window().close();
                    } else {
                        navigator().push(SettingsRoute::Engine {});
                    }
                },
                "Apply"
            }
        }
    }
}

/// The (group, leaf) breadcrumb labels for a route — Keymap has no parent group.
fn crumb_of(route: &SettingsRoute) -> (Option<&'static str>, &'static str) {
    match route {
        SettingsRoute::Appearance {} => (Some("Appearance & behaviour"), "Theme"),
        SettingsRoute::System {} => (Some("Appearance & behaviour"), "System"),
        SettingsRoute::DataDisplay {} => (Some("Appearance & behaviour"), "Data display"),
        SettingsRoute::Keymap {} => (None, "Keymap"),
        SettingsRoute::Engine {} => (Some("Engine"), "Properties"),
    }
}

/// A collapsible nav group header — rotating chevron + label; clicking toggles `open`.
fn nav_group(mut open: Signal<bool>, label: &str) -> Element {
    let is_open = open();
    rsx! {
        button {
            class: "settings-nav-group",
            onclick: move |_| open.set(!open()),
            span {
                class: if is_open { "settings-nav-chev open" } else { "settings-nav-chev" },
                Icon { name: IconName::ChevronRight, size: IconSize::Xs }
            }
            span { class: "settings-nav-grouplabel", "{label}" }
        }
    }
}

/// A nav leaf indented under a group — a router `Link`, active on the current route.
fn nav_leaf(current: &SettingsRoute, to: SettingsRoute, label: &str) -> Element {
    let cls = if *current == to {
        "settings-nav-item leaf on"
    } else {
        "settings-nav-item leaf"
    };
    rsx! { Link { to, class: cls, Body { "{label}" } } }
}

/// A standalone top-level nav item (aligned with the group headers), e.g. Keymap.
fn nav_top(current: &SettingsRoute, to: SettingsRoute, label: &str) -> Element {
    let cls = if *current == to {
        "settings-nav-item top on"
    } else {
        "settings-nav-item top"
    };
    rsx! { Link { to, class: cls, Body { "{label}" } } }
}

/// One search-result row (label + category). Clicking navigates to the setting's page,
/// clears the query (restoring the tree), and sets the flash target so its [`Anchor`] fires.
fn search_result(
    e: &SettingEntry,
    mut query: Signal<String>,
    mut flash: Signal<Option<&'static str>>,
) -> Element {
    let id = e.id;
    let route = e.cat.route();
    let label = e.label;
    let cat = e.cat.label();
    rsx! {
        button {
            class: "settings-search-result",
            onclick: move |_| {
                navigator().push(route.clone());
                query.set(String::new());
                flash.set(Some(id));
            },
            Body { class: "settings-search-label", "{label}" }
            Caption { class: "settings-search-cat", "{cat}" }
        }
    }
}
