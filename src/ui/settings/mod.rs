//! The Settings **window** — a standalone OS window (its own `VirtualDom`) spawned
//! single-instance via `crate::window::spawn_settings_window` (header / launcher gear,
//! ⌘,, File menu).
//!
//! **Navigation uses the dioxus router** (desktop → in-memory history, no URL bar):
//! [`SettingsRoute`] under the [`SettingsChrome`] layout, one page per submodule. `/`
//! opens on Appearance.
//!
//! **Draft / save model.** [`SettingsRoot`] owns a *local* `draft` copy of the settings
//! + the engine sub-form, provided to the pages via [`SettingsCtx`]. Controls edit the
//! draft; **Apply** (`engine.submit()`) merges + persists the whole draft and closes;
//! **Cancel** / close discards. **Theme previews live** (see [`crate::settings`]).

mod appearance;
mod data_display;
mod engine;
mod keymap;
mod system;

use dioxus::desktop::{use_muda_event_handler, use_window, use_wry_event_handler};
use dioxus::prelude::*;

use strata_forms::{use_form, FormState};

use crate::config::Settings;
use crate::ui::components::{Body, Button, ButtonVariant, Eyebrow, Icon, Prose, Spacer};
use crate::ui::icons::{IconName, IconSize};

use appearance::Appearance;
use data_display::DataDisplay;
use engine::{engine_form_from, engine_form_to, Engine, EngineForm};
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
/// `engine` is the engine sub-form whose `on_submit` persists the whole draft.
#[derive(Clone, Copy)]
struct SettingsCtx {
    draft: Signal<Settings>,
    engine: FormState<EngineForm>,
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
                // Track focus so app-menu commands don't misroute to a background window.
                WindowEvent::Focused(f) => crate::window::note_focused(win_id, *f),
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
                if crate::menu::select_all_scope() == crate::menu::SelectAllScope::Input {
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

    // The engine sub-form owns its own working copy (seeded from the committed
    // overrides). Apply is a single `engine.submit()`: on a valid form its `on_submit`
    // merges the overrides back into the draft, persists the whole `Settings`, and
    // closes the window.
    let engine = use_form(
        move || engine_form_from(&draft.peek().engine),
        {
            let win_close = win.clone();
            move |form: EngineForm| {
                let mut s = draft.peek().clone();
                s.engine = engine_form_to(&form);
                crate::settings::save_draft(s);
                win_close.close();
            }
        },
    );

    use_context_provider(|| SettingsCtx { draft, engine });

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
    let crumb = match &route {
        SettingsRoute::Appearance {} => "Appearance",
        SettingsRoute::DataDisplay {} => "Data display",
        SettingsRoute::System {} => "System",
        SettingsRoute::Engine {} => "Engine",
        SettingsRoute::Keymap {} => "Keymap",
    };
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
                Eyebrow { class: "settings-navlabel", "SETTINGS" }
                {nav_link(&route, SettingsRoute::Appearance {}, IconName::Palette, "Appearance")}
                {nav_link(&route, SettingsRoute::DataDisplay {}, IconName::Grid, "Data display")}
                {nav_link(&route, SettingsRoute::System {}, IconName::Sliders, "System")}
                {nav_link(&route, SettingsRoute::Engine {}, IconName::Engine, "Engine")}
                {nav_link(&route, SettingsRoute::Keymap {}, IconName::Keyboard, "Keymap")}
            }
            div { class: "settings-pane ps-scroll",
                Prose { class: "settings-crumb",
                    "Settings " span { style: "color:var(--faint2);", "›" } " "
                    span { style: "color:var(--text3);", "{crumb}" }
                }
                Outlet::<SettingsRoute> {}
            }
        }
        // Footer — Cancel discards (the drop handler reverts the live theme preview);
        // Apply runs `engine.submit()`, which persists the whole draft + closes.
        div { class: "settings-foot",
            Spacer {}
            Button { variant: ButtonVariant::Ghost, onclick: move |_| dioxus::desktop::window().close(), "Cancel" }
            Button { variant: ButtonVariant::Primary, onclick: move |_| ctx.engine.submit(), "Apply" }
        }
    }
}

/// One left-nav entry — a router `Link` styled as a nav item, marked active when it
/// targets the current route.
fn nav_link(current: &SettingsRoute, to: SettingsRoute, icon: IconName, label: &str) -> Element {
    let cls = if *current == to {
        "settings-nav-item on"
    } else {
        "settings-nav-item"
    };
    rsx! {
        Link { to, class: cls,
            span { class: "sn-ic", {icon.el(IconSize::Sm)} }
            Body { "{label}" }
        }
    }
}
