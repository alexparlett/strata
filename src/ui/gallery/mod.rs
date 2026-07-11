//! **Dev-only** component gallery (a Storybook-style preview), opened from the
//! Help menu in debug builds and gated out of release via `#[cfg(debug_assertions)]`
//! on the module (see `ui/mod.rs`) + its window spawn (`window.rs`) + menu item
//! (`menu.rs`). A standalone window (its own `VirtualDom`, like the launcher) that
//! injects the real stylesheet + theme and renders every S28/S29 control **live**
//! (so hover/press is real — no static "hover" column) alongside its **disabled**
//! state. Not a product surface — delete/adjust freely. Skips the activity rail +
//! data grid (aligned during their own tasks).

use dioxus::prelude::*;

use crate::ui::components::{
    Badge, BadgeVariant, Body, Button, ButtonVariant, Callout, CalloutVariant, Caption, Checkbox,
    Code, Control, Dot, DotStatus, Eyebrow, Hero, Icon, IconButton, IconButtonVariant, Meta,
    Metric, Micro, MonoValue, NumberStepper, Pager, Path, Prose, RadioGroup, RadioOption, Readout,
    SearchBar, Segment, SegmentOption, Select, SelectOption, SplitButton, StatusDot, Strong,
    TextInput, Title, Toggle,
};
use crate::ui::icons::{self, IconName, IconSize};

const SECTION: &str = "margin-bottom:var(--sp-7);";
const H2: &str =
    "font:var(--t-eyebrow);letter-spacing:.8px;color:var(--dim3);text-transform:uppercase;margin:0 0 var(--sp-2);";
const SUB: &str = "font:var(--t-prose);color:var(--dim);margin:0 0 var(--sp-4);";
const ROW: &str =
    "display:flex;flex-wrap:wrap;gap:var(--sp-5);align-items:center;margin-bottom:var(--sp-4);";
const DIS: &str =
    "font:var(--t-micro);letter-spacing:.8px;color:var(--faint);text-transform:uppercase;margin:var(--sp-1) 0 var(--sp-3);";

#[component]
pub fn GalleryRoot() -> Element {
    // Single-instance registration: a repeat "open" focuses this window instead of
    // spawning a second; the slot clears on close. It's a plain standalone window
    // (see `window::spawn_gallery_window`), so no child-window / AppKit poking.
    use_hook(crate::window::register_gallery_window);
    use_drop(|| crate::window::unregister_gallery_window());

    let theme_css = use_hook(|| {
        let cfg = crate::config::load();
        let id = crate::theme::effective_id(
            &cfg.settings.theme,
            cfg.settings.sync_os,
            crate::theme::os_is_dark(),
        );
        crate::theme::css_for(&id)
    });

    // Live state.
    let mut toggle_a = use_signal(|| true);
    let mut toggle_b = use_signal(|| false);
    let mut checked = use_signal(|| true);
    let mut checked2 = use_signal(|| false);
    let mut icon_on = use_signal(|| true);
    let mut seg_view = use_signal(|| "grid".to_string());
    let mut seg_scope = use_signal(|| "all".to_string());
    let mut radio_auth = use_signal(|| "ambient".to_string());
    let mut select_fmt = use_signal(|| "parquet".to_string());
    let mut text_name = use_signal(String::new);
    let mut text_search = use_signal(String::new);
    let mut row_limit = use_signal(|| 1000i64);
    let mut pager_page = use_signal(|| 1u32);

    rsx! {
        style { dangerous_inner_html: crate::CSS }
        div {
            style: "{theme_css}width:100vw;height:100vh;box-sizing:border-box;background:var(--panel);display:flex;flex-direction:column;font-family:var(--ui);color:var(--text);",

            div { style: "flex:1;overflow:auto;padding:var(--sp-6) var(--sp-7);",

                // ---- Buttons ----
                div { style: "{SECTION}",
                    div { style: "{H2}", "Button" }
                    div { style: "{SUB}", "Live — hover / press to confirm states." }
                    div { style: "{ROW}",
                        Button { variant: ButtonVariant::Primary, onclick: move |_| {}, "Primary" }
                        Button { variant: ButtonVariant::Secondary, onclick: move |_| {}, "Secondary" }
                        Button { variant: ButtonVariant::Ghost, onclick: move |_| {}, "Ghost" }
                        Button { variant: ButtonVariant::Accent, onclick: move |_| {}, "Accent" }
                        Button { variant: ButtonVariant::Danger, onclick: move |_| {}, "Danger" }
                        Button { variant: ButtonVariant::Soft, onclick: move |_| {}, "Soft" }
                        Button { variant: ButtonVariant::Compact, onclick: move |_| {}, "Compact" }
                    }
                    div { style: "{ROW}",
                        Button { variant: ButtonVariant::Primary, icon: IconName::Play, icon_size: IconSize::Sm, kbd: "⌘↵", onclick: move |_| {}, "Run" }
                        Button { variant: ButtonVariant::Secondary, icon: IconName::Download, icon_size: IconSize::Sm, onclick: move |_| {}, "Export" }
                    }
                    div { style: "{DIS}", "Disabled" }
                    div { style: "{ROW}",
                        Button { variant: ButtonVariant::Primary, disabled: true, onclick: move |_| {}, "Primary" }
                        Button { variant: ButtonVariant::Secondary, disabled: true, onclick: move |_| {}, "Secondary" }
                        Button { variant: ButtonVariant::Ghost, disabled: true, onclick: move |_| {}, "Ghost" }
                        Button { variant: ButtonVariant::Danger, disabled: true, onclick: move |_| {}, "Danger" }
                        Button { variant: ButtonVariant::Compact, disabled: true, onclick: move |_| {}, "Compact" }
                    }
                }

                // ---- Icon buttons ----
                div { style: "{SECTION}",
                    div { style: "{H2}", "IconButton" }
                    div { style: "{SUB}", "Toolbar · ghost · pager · stateful toggle · with count badge." }
                    div { style: "{ROW}",
                        IconButton { icon: IconName::Gear, icon_size: IconSize::Md, variant: IconButtonVariant::Toolbar, title: "Toolbar", onclick: move |_| {}, }
                        IconButton { icon: IconName::Close, variant: IconButtonVariant::Ghost, title: "Ghost / close", onclick: move |_| {}, }
                        IconButton { icon: IconName::ChevronRight, variant: IconButtonVariant::Pager, title: "Pager", onclick: move |_| {}, }
                        IconButton { icon: IconName::CubeLines,
                            variant: IconButtonVariant::Toggle,
                            title: "Stateful toggle",
                            on: icon_on(),
                            onclick: move |_| icon_on.set(!icon_on()),
                        }
                        div { class: "ds-badge-anchor",
                            IconButton { icon: IconName::Problems, icon_size: IconSize::Md, variant: IconButtonVariant::Toolbar, title: "Problems", onclick: move |_| {}, }
                            span { class: "ds-count-badge err", "3" }
                        }
                    }
                    div { style: "{DIS}", "Disabled" }
                    div { style: "{ROW}",
                        IconButton { icon: IconName::Gear, icon_size: IconSize::Md, variant: IconButtonVariant::Toolbar, disabled: true, title: "Disabled", onclick: move |_| {}, }
                        IconButton { icon: IconName::Close, variant: IconButtonVariant::Ghost, disabled: true, title: "Disabled", onclick: move |_| {}, }
                    }
                }

                // ---- Split button ----
                div { style: "{SECTION}",
                    div { style: "{H2}", "SplitButton" }
                    div { style: "{SUB}", "Accent face + caret → solid-accent menu (the one coloured menu)." }
                    div { style: "display:flex;gap:var(--sp-6);align-items:flex-start;flex-wrap:wrap;",
                        SplitButton {
                            label: "Run", kbd: "⌘↵", icon: IconName::Play, icon_size: IconSize::Sm,
                            on_main: move |_| {},
                            button { class: "ds-accent-item", onclick: move |_| {}, Icon { name: IconName::Table, size: IconSize::Sm } span { "Explain plan" } }
                            button { class: "ds-accent-item", onclick: move |_| {}, Icon { name: IconName::Clock, size: IconSize::Sm } span { "Explain analyze" } }
                        }
                        SplitButton { label: "Run", show_caret: false, icon: IconName::Play, icon_size: IconSize::Sm, on_main: move |_| {}, }
                    }
                }

                // ---- Inputs ----
                div { style: "{SECTION}",
                    div { style: "{H2}", "TextInput / NumberStepper" }
                    div { style: "{SUB}", "Text · search (icon slot) · mono · number stepper. Focus for the accent ring." }
                    div { style: "{ROW}",
                        TextInput { value: text_name(), oninput: move |v| text_name.set(v), placeholder: "Table name…", width: 220 }
                        SearchBar { value: text_search(), oninput: move |v| text_search.set(v), placeholder: "Search…", width: 220 }
                        NumberStepper { value: row_limit(), on_change: move |v| row_limit.set(v), min: 0, max: 100000, step: 100, suffix: "rows" }
                    }
                    div { style: "{ROW}",
                        TextInput { value: text_name(), oninput: move |v| text_name.set(v), mono: true, placeholder: "s3://bucket/path", width: 260 }
                    }
                    div { style: "{DIS}", "Disabled" }
                    div { style: "{ROW}",
                        TextInput { value: "read only".to_string(), oninput: move |_| {}, disabled: true, width: 200 }
                        NumberStepper { value: 100, on_change: move |_| {}, disabled: true, suffix: "rows" }
                    }
                }

                // ---- Select ----
                div { style: "{SECTION}",
                    div { style: "{H2}", "Select (S29)" }
                    div { style: "{SUB}", "App-themed dropdown — click to open." }
                    div { style: "{ROW}",
                        Select {
                            value: select_fmt(),
                            on_select: move |v| select_fmt.set(v),
                            width: 180,
                            options: vec![
                                SelectOption::new("parquet", "Parquet"),
                                SelectOption::new("csv", "CSV"),
                                SelectOption::new("json", "JSON (NDJSON)"),
                                SelectOption::new("arrow", "Arrow IPC"),
                            ],
                        }
                    }
                }

                // ---- Segmented ----
                div { style: "{SECTION}",
                    div { style: "{H2}", "Segment" }
                    div { style: "{SUB}", "Single-select multi-button — soft-tint selected." }
                    div { style: "{ROW}",
                        Segment {
                            value: seg_view(),
                            on_select: move |v| seg_view.set(v),
                            options: vec![
                                SegmentOption::with_icon("grid", "Table", IconName::Table),
                                SegmentOption::with_icon("chart", "Chart", IconName::Chart),
                            ],
                        }
                        Segment {
                            value: seg_scope(),
                            on_select: move |v| seg_scope.set(v),
                            options: vec![
                                SegmentOption::new("all", "All rows"),
                                SegmentOption::new("page", "This page"),
                                SegmentOption::new("sel", "Selection"),
                            ],
                        }
                    }
                    div { style: "{DIS}", "Disabled" }
                    div { style: "{ROW}",
                        Segment {
                            value: "a".to_string(),
                            on_select: move |_| {},
                            disabled: true,
                            options: vec![SegmentOption::new("a", "Comfortable"), SegmentOption::new("b", "Compact")],
                        }
                    }
                }

                // ---- Switch · Checkbox · Radio ----
                div { style: "{SECTION}",
                    div { style: "{H2}", "Toggle · Checkbox · Radio" }
                    div { style: "{SUB}", "Switches glow the track on hover; checkboxes/radios glow their border." }
                    div { style: "display:flex;gap:var(--sp-9);align-items:flex-start;flex-wrap:wrap;",
                        div { style: "display:flex;flex-direction:column;gap:var(--sp-4);",
                            Toggle { on: toggle_a(), on_toggle: move |v| toggle_a.set(v), "Reopen on startup" }
                            Toggle { on: toggle_b(), on_toggle: move |v| toggle_b.set(v), "Sync with OS" }
                            Toggle { on: true, on_toggle: move |_| {}, disabled: true, "Disabled" }
                        }
                        div { style: "display:flex;flex-direction:column;gap:var(--sp-4);",
                            Checkbox { checked: checked(), on_toggle: move |v| checked.set(v), "Remember choice" }
                            Checkbox { checked: checked2(), on_toggle: move |v| checked2.set(v), "Verbose logging" }
                            Checkbox { checked: true, on_toggle: move |_| {}, disabled: true, "Disabled" }
                        }
                        div { style: "display:flex;flex-direction:column;gap:var(--sp-4);",
                            RadioGroup {
                                value: radio_auth(),
                                on_select: move |v| radio_auth.set(v),
                                options: vec![
                                    RadioOption::new("ambient", "Ambient credentials"),
                                    RadioOption::new("profile", "Named profile"),
                                    RadioOption::new("anon", "Anonymous"),
                                ],
                            }
                            RadioGroup {
                                value: "x".to_string(),
                                on_select: move |_| {},
                                disabled: true,
                                options: vec![RadioOption::new("x", "Disabled group")],
                            }
                        }
                    }
                }

                // ---- Pagination ----
                div { style: "{SECTION}",
                    div { style: "{H2}", "Pager" }
                    div { style: "{SUB}", "First / prev / jump / next / last — bounds auto-disable." }
                    Pager { page: pager_page(), page_count: 6, on_jump: move |n| pager_page.set(n) }
                }

                // ---- Typography ----
                div { style: "{SECTION}",
                    div { style: "{H2}", "Typography" }
                    div { style: "{SUB}", "One component per role (v18 ramp; mono is three-weight)." }
                    div { style: "display:grid;grid-template-columns:1fr 1fr;gap:var(--sp-6);max-width:760px;",
                        div { style: "display:flex;flex-direction:column;gap:var(--sp-4);align-items:flex-start;",
                            Eyebrow { "IBM Plex Sans" }
                            Hero { "Query your data" }
                            Title { "Title / window · 600 · 14.5" }
                            Strong { "Strong body · 600 · 13" }
                            Body { "Body medium · 500 · 13" }
                            Control { "Control · button / control label · 600 · 12.5" }
                            Prose { "Body regular · 400 · 12.5 — descriptions and secondary prose." }
                            Caption { "Caption · 400 · 11" }
                        }
                        div { style: "display:flex;flex-direction:column;gap:var(--sp-4);align-items:flex-start;",
                            Eyebrow { "JetBrains Mono" }
                            Code { "SELECT * FROM users" }
                            Metric { "1,284,097" }
                            MonoValue { "Data value · 500 · 12.5" }
                            Readout { "Code & data block · 400 · 12" }
                            Eyebrow { "Field label · 600 · 10" }
                            Meta { "2026-07-09 14:32 · 4.2 ms · zstd — meta · 500 · 10" }
                            Path { "s3://warehouse/events/2026-07-08.parquet — path · 400 · 11" }
                            Micro { "Micro · 600 · 9" }
                        }
                    }
                }

                // ---- Badges + status ----
                div { style: "{SECTION}",
                    div { style: "{H2}", "Badge · Dot · StatusDot" }
                    div { style: "{ROW}",
                        Badge { variant: BadgeVariant::Accent, "Connected" }
                        Badge { variant: BadgeVariant::Ready, "Ready" }
                        Badge { variant: BadgeVariant::Cached, "Cached" }
                        Badge { variant: BadgeVariant::Error, "Error" }
                        Badge { variant: BadgeVariant::Draft, "Draft" }
                    }
                    div { style: "display:flex;flex-wrap:wrap;gap:var(--sp-6);align-items:center;",
                        span { style: "display:flex;align-items:center;gap:var(--sp-3);font:var(--t-body);color:var(--text3);", StatusDot { status: DotStatus::Idle } "Idle" }
                        span { style: "display:flex;align-items:center;gap:var(--sp-3);font:var(--t-body);color:var(--text3);", StatusDot { status: DotStatus::Run } "Running" }
                        span { style: "display:flex;align-items:center;gap:var(--sp-3);font:var(--t-body);color:var(--text3);", StatusDot { status: DotStatus::Ok } "Ok" }
                        span { style: "display:flex;align-items:center;gap:var(--sp-3);font:var(--t-body);color:var(--text3);", StatusDot { status: DotStatus::Err } "Error" }
                        span { style: "display:flex;align-items:center;gap:var(--sp-3);font:var(--t-body);color:var(--text3);", StatusDot { status: DotStatus::Plan } "Plan" }
                    }
                    div { style: "{DIS}", "Dot — colour · square swatch · pulse · size" }
                    div { style: "display:flex;flex-wrap:wrap;gap:var(--sp-5);align-items:center;",
                        Dot {}
                        Dot { color: "var(--accent)" }
                        Dot { color: "var(--green)" }
                        Dot { color: "var(--red2)" }
                        Dot { color: "var(--accent)", pulse: true }
                        Dot { color: "var(--t-num)", square: true }
                        Dot { color: "var(--t-str)", square: true }
                        Dot { color: "var(--t-bool)", square: true }
                        Dot { color: "var(--accent)", size: 12 }
                    }
                }

                // ---- Callouts ----
                div { style: "{SECTION}",
                    div { style: "{H2}", "Callout" }
                    div { style: "display:flex;flex-direction:column;gap:var(--sp-4);max-width:420px;margin-top:var(--sp-3);",
                        Callout { variant: CalloutVariant::Info, "Snapshot is 3 minutes old — refresh to re-run." }
                        Callout { variant: CalloutVariant::Warn, "This query scans 4.2 GB across 180 files." }
                        Callout { variant: CalloutVariant::Error, "Unknown table `bak` — did you register it?" }
                    }
                }

                // ---- Icon library ----
                div { style: "{SECTION}",
                    div { style: "{H2}", "Icon library" }
                    div { style: "display:grid;grid-template-columns:repeat(auto-fill,minmax(94px,1fr));gap:var(--sp-4);margin-top:var(--sp-3);",
                        for (name, ic) in icons::catalog().iter().copied() {
                            div {
                                key: "{name}",
                                style: "display:flex;flex-direction:column;align-items:center;gap:var(--sp-3);padding:var(--sp-4) var(--sp-3);border:1px solid var(--line);border-radius:var(--r-2);background:var(--bg);color:var(--text2);",
                                {ic(20)}
                                span { style: "font:var(--t-meta);color:var(--dim2);text-align:center;word-break:break-all;", "{name}" }
                            }
                        }
                    }
                }
            }
        }
    }
}
