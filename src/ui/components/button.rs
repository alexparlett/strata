//! `Button` + `IconButton` — the text and icon buttons of the design system
//! (`docs/DESIGN_SYSTEM.md` §03). Part of the **S28** control library.
//!
//! Additive, namespaced (`.ds-btn` / `.ds-icon-btn`) so they sit alongside the
//! app's existing ad-hoc `.btn` / `.icon-btn` classes without disturbing them —
//! call sites migrate onto these in a later pass, then the old classes retire.
//!
//! Both are **controlled** and stateless: the caller owns the `onclick` and (for
//! the stateful icon toggle) the `on` flag. Disabled buttons render the HTML
//! `disabled` attribute *and* short-circuit the handler.

use dioxus::prelude::*;

use crate::ui::icons::{IconName, IconSize};

/// Text-button variants (§03). See the design-system table for the exact palette.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum ButtonVariant {
    /// Accent fill, ink text — the primary call to action.
    #[default]
    Primary,
    /// Elevated surface + border — the neutral default.
    Secondary,
    /// Transparent until hover — low-emphasis.
    Ghost,
    /// Transparent with accent text — a "stateful"/link-like action.
    Accent,
    /// Red-tinted destructive action.
    Danger,
    /// Menu-row style (transparent, inset hover overlay).
    Soft,
    /// Small inline text action (11px, tight padding) — e.g. a drawer "Clear".
    Compact,
}

impl ButtonVariant {
    fn class(self) -> &'static str {
        match self {
            ButtonVariant::Primary => "primary",
            ButtonVariant::Secondary => "secondary",
            ButtonVariant::Ghost => "ghost",
            ButtonVariant::Accent => "accent",
            ButtonVariant::Danger => "danger",
            ButtonVariant::Soft => "soft",
            ButtonVariant::Compact => "compact",
        }
    }
}

/// A text button, optionally with a leading `icon` and a trailing `kbd` hint chip
/// (e.g. `⌘↵`). `children` is the label.
#[component]
pub fn Button(
    #[props(default)] variant: ButtonVariant,
    #[props(default)] disabled: bool,
    /// Compact 28px height (tight in-context toolbars — editor Run/Cancel, results
    /// bar). The default is the standard 34px §03 button.
    #[props(default)]
    small: bool,
    /// Optional leading icon (a typed [`IconName`]).
    icon: Option<IconName>,
    /// Leading-icon size. The §03 default is `IconSize::Sm` (14px).
    #[props(default = IconSize::Sm)]
    icon_size: IconSize,
    /// Optional trailing keyboard-hint chip text (e.g. `"⌘↵"`).
    #[props(into, default)]
    kbd: String,
    #[props(into, default)] title: String,
    onclick: EventHandler<MouseEvent>,
    children: Element,
) -> Element {
    let cls = format!(
        "ds-btn {}{}",
        variant.class(),
        if small { " sm" } else { "" }
    );
    rsx! {
        button {
            r#type: "button",
            class: "{cls}",
            disabled: disabled,
            title: "{title}",
            onclick: move |e| { if !disabled { onclick.call(e); } },
            if let Some(ic) = icon {
                span { class: "ds-btn-ico", {ic.el(icon_size)} }
            }
            span { class: "ds-btn-label", {children} }
            if !kbd.is_empty() {
                span { class: "ds-btn-kbd", "{kbd}" }
            }
        }
    }
}

/// Square icon-button variants (§03 "Icon buttons").
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum IconButtonVariant {
    /// 32px neutral, bordered — the standard toolbar button.
    #[default]
    Toolbar,
    /// 28px **accent-filled** — the editor Run action (add `.stop` via `class` for the
    /// running red Cancel state).
    Primary,
    /// 28px borderless — dismiss / close / inline.
    Ghost,
    /// 28×26 borderless — pager nav arrows.
    Pager,
    /// 28px borderless **stateful** — pairs with a segmented control; pass `on`
    /// for the pressed (accent-tinted) state (e.g. plan Raw/Tree).
    Toggle,
    /// 28px borderless **destructive** — reddens on hover (remove / delete row).
    Danger,
}

impl IconButtonVariant {
    fn class(self) -> &'static str {
        match self {
            IconButtonVariant::Toolbar => "toolbar",
            IconButtonVariant::Primary => "primary",
            IconButtonVariant::Ghost => "ghost",
            IconButtonVariant::Pager => "pager",
            IconButtonVariant::Toggle => "toggle",
            IconButtonVariant::Danger => "danger",
        }
    }
}

/// A square icon button. `icon` is the glyph (a typed [`IconName`], sized on the §07
/// scale). For the `Toggle` variant, `on` drives the pressed/accent state.
#[component]
pub fn IconButton(
    #[props(default)] variant: IconButtonVariant,
    /// The glyph — a typed [`IconName`].
    icon: IconName,
    /// Icon size (default `IconSize::Sm`, 14px; toolbar glyphs often want `Md`).
    #[props(default = IconSize::Sm)]
    icon_size: IconSize,
    /// Pressed state — only meaningful for `IconButtonVariant::Toggle`.
    #[props(default)]
    on: bool,
    /// Compact (28px) toolbar density — for tight in-context toolbars where a 32px
    /// `Toolbar` would tower over 28px neighbours (editor / results toolbars).
    #[props(default)]
    compact: bool,
    /// Attention tint (e.g. an unsaved / dirty Save button) — colours the glyph.
    #[props(default)]
    dirty: bool,
    #[props(default)] disabled: bool,
    #[props(into, default)] title: String,
    /// Extra class(es) appended to the button — a per-instance styling escape hatch
    /// (e.g. the activity rail's larger size). Prefer a variant where one fits.
    #[props(into, default)]
    class: String,
    onclick: EventHandler<MouseEvent>,
) -> Element {
    let cls = format!(
        "ds-icon-btn {}{}{}{} {}",
        variant.class(),
        if on { " on" } else { "" },
        if compact { " compact" } else { "" },
        if dirty { " dirty" } else { "" },
        class,
    );
    // `aria-pressed` only carries meaning on the stateful Toggle variant.
    let pressed = if variant == IconButtonVariant::Toggle {
        if on {
            "true"
        } else {
            "false"
        }
    } else {
        ""
    };
    rsx! {
        button {
            r#type: "button",
            class: "{cls}",
            disabled: disabled,
            title: "{title}",
            "aria-pressed": "{pressed}",
            onclick: move |e| { if !disabled { onclick.call(e); } },
            {icon.el(icon_size)}
        }
    }
}
