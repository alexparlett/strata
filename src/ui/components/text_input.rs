//! `TextInput` + `NumberStepper` — the text-entry controls of the design system
//! (`docs/DESIGN_SYSTEM.md` §04). Part of the **S28** control library.
//!
//! `TextInput` is a bordered **field wrapper** (`.ds-field`) around a borderless
//! `<input>` (`.ds-input`) so the wrapper can carry the shared focus ring and an
//! optional leading icon slot (the search variant). `NumberStepper` adds the
//! up/down chevron column.
//!
//! Both are **controlled**: the caller owns `value` and reacts to the change
//! callback. [`SearchBar`] is a `TextInput` preset (baked search icon + clear).
//! Additive `.ds-*` classes — see `button.rs` for the migration note.

use dioxus::prelude::*;

use super::Icon;
use crate::ui::icons::{IconName, IconSize};

/// A single-line text field. Pass `icon` for the search-style leading slot; pass
/// `mono` to render the value in the monospace family. `onkeydown` is an optional
/// passthrough for escape-aware inputs (e.g. cancel-on-Esc).
#[component]
pub fn TextInput(
    #[props(into)] value: String,
    oninput: EventHandler<String>,
    /// Optional commit-on-blur / change handler (fires when the input loses focus or
    /// the value is committed) — for fields that do expensive work (e.g. a filesystem
    /// rescan) only once editing settles, not on every keystroke.
    onchange: Option<EventHandler<String>>,
    /// Optional key passthrough (Enter to commit, Escape to cancel) — the caller
    /// decides what each key does.
    onkeydown: Option<EventHandler<KeyboardEvent>>,
    #[props(default)] disabled: bool,
    /// Render the value in the monospace family (paths, numbers, keys).
    #[props(default)]
    mono: bool,
    #[props(into, default)] placeholder: String,
    /// Optional leading (start) icon — a typed [`IconName`] (search-style slot).
    icon: Option<IconName>,
    /// Leading-icon size (default `IconSize::Sm`, 14px).
    #[props(default = IconSize::Sm)]
    icon_size: IconSize,
    /// Optional trailing (end) content inside the field — a clear button, a match
    /// count, a unit, etc. Sits after the input, right-aligned (the input flexes).
    trailing: Option<Element>,
    /// Fixed width in px (0 = auto / flex to container).
    #[props(default)]
    width: u32,
    /// Grow to fill the flex parent — for a sidebar/toolbar filter that should take
    /// the remaining width.
    #[props(default)]
    grow: bool,
    /// Borderless / unstyled field — drops the border, background, padding and focus
    /// ring so the input drops into a container the caller owns (⌘K palette head, the
    /// tab-overflow dropdown). Keeps the icon slot + input behaviour. (Mirrors Ant's
    /// `variant="borderless"` / Chakra's `variant="unstyled"`.)
    #[props(default)]
    bare: bool,
    #[props(default)] autofocus: bool,
    // NOTE: Enter/Escape passthrough (`onkeydown`) is deliberately deferred to the
    // phase-2 migration, where the specific inputs that need it (tab rename, tablist
    // search, cmdk) are adopted and can be compile-tested with the exact handler shape.
) -> Element {
    let has_icon = icon.is_some();
    let wrap_cls = {
        let mut c = String::from("ds-field");
        if has_icon {
            c.push_str(" ds-field-search");
        }
        if bare {
            c.push_str(" bare");
        }
        if disabled {
            c.push_str(" disabled");
        }
        c
    };
    let input_cls = if mono {
        "ds-input ds-input-mono"
    } else {
        "ds-input"
    };
    let wstyle = {
        let mut s = String::new();
        if grow {
            s.push_str("flex:1;min-width:0;");
        }
        if width > 0 {
            s.push_str(&format!("width:{width}px;"));
        }
        s
    };
    rsx! {
        div { class: "{wrap_cls}", style: "{wstyle}",
            if let Some(ic) = icon {
                span { class: "ds-field-ico", {ic.el(icon_size)} }
            }
            input {
                class: "{input_cls}",
                r#type: "text",
                value: "{value}",
                disabled: disabled,
                placeholder: "{placeholder}",
                autofocus: autofocus,
                spellcheck: false,
                onmounted: move |e| {
                    if autofocus {
                        spawn(async move { let _ = e.set_focus(true).await; });
                    }
                },
                oninput: move |e| oninput.call(e.value()),
                onchange: move |e| { if let Some(h) = onchange { h.call(e.value()); } },
                onkeydown: move |e| { if let Some(h) = onkeydown { h.call(e); } },
            }
            if let Some(tr) = trailing {
                {tr}
            }
        }
    }
}

/// `SearchBar` — the filter/search preset over [`TextInput`]: a consistent leading
/// **search** icon + a built-in trailing **clear** button (shown when non-empty;
/// clears via `oninput("")`). `trailing` adds extra end content (e.g. a match count)
/// before the clear. Same field chrome / hover / focus ring as `TextInput` — every
/// search field uses this so the icons stay consistent app-wide.
#[component]
pub fn SearchBar(
    #[props(into)] value: String,
    oninput: EventHandler<String>,
    #[props(into, default)] placeholder: String,
    /// Fixed width in px (0 = auto).
    #[props(default)]
    width: u32,
    /// Grow to fill the flex parent (e.g. a sidebar filter).
    #[props(default)]
    grow: bool,
    #[props(default)] mono: bool,
    #[props(default)] autofocus: bool,
    /// Extra trailing content shown before the clear button (e.g. a match count).
    trailing: Option<Element>,
) -> Element {
    let has_value = !value.is_empty();
    rsx! {
        TextInput {
            value: value,
            oninput: oninput,
            icon: IconName::Search,
            placeholder: placeholder,
            mono: mono,
            width: width,
            grow: grow,
            autofocus: autofocus,
            trailing: rsx! {
                if let Some(tr) = trailing {
                    {tr}
                }
                if has_value {
                    button {
                        r#type: "button",
                        class: "ds-field-clear",
                        title: "Clear",
                        onclick: move |_| oninput.call(String::new()),
                        {IconName::Close.el(IconSize::Xs)}
                    }
                }
            },
        }
    }
}

/// A compact integer stepper: a monospace value field flanked by an up/down
/// chevron column (§04). Clamped to `[min, max]`; typing parses live. Controlled
/// via `value` + `on_change`.
#[component]
pub fn NumberStepper(
    value: i64,
    on_change: EventHandler<i64>,
    #[props(default)] disabled: bool,
    min: Option<i64>,
    max: Option<i64>,
    #[props(default = 1)] step: i64,
    /// Optional unit suffix shown after the value (e.g. `"MB"`, `"rows"`).
    #[props(into, default)]
    suffix: String,
    #[props(default = 120)] width: u32,
) -> Element {
    let lo = min.unwrap_or(i64::MIN);
    let hi = max.unwrap_or(i64::MAX);
    let clamp = move |v: i64| v.clamp(lo, hi);
    let at_min = value <= lo;
    let at_max = value >= hi;
    rsx! {
        div {
            class: if disabled { "ds-stepper disabled" } else { "ds-stepper" },
            style: "width:{width}px;",
            input {
                class: "ds-stepper-input",
                r#type: "text",
                "inputmode": "numeric",
                value: "{value}",
                disabled: disabled,
                spellcheck: false,
                oninput: move |e| {
                    if let Ok(v) = e.value().trim().parse::<i64>() {
                        on_change.call(clamp(v));
                    }
                },
            }
            if !suffix.is_empty() {
                span { class: "ds-stepper-suffix", "{suffix}" }
            }
            div { class: "ds-stepper-btns",
                button {
                    r#type: "button",
                    class: "ds-stepper-up",
                    disabled: disabled || at_max,
                    onclick: move |_| { if !disabled && !at_max { on_change.call(clamp(value + step)); } },
                    Icon { name: IconName::ChevronUp, size: IconSize::Xs }
                }
                button {
                    r#type: "button",
                    class: "ds-stepper-down",
                    disabled: disabled || at_min,
                    onclick: move |_| { if !disabled && !at_min { on_change.call(clamp(value - step)); } },
                    Icon { name: IconName::ChevronDown, size: IconSize::Xs }
                }
            }
        }
    }
}
