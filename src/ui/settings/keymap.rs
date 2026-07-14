//! Settings ▸ Keymap page — rebind command shortcuts (W4). Each row shows a command, its
//! current chord (override-aware, from the draft), and controls to **click-to-capture** a
//! new chord, **reset** to default, or (via the conflict flow) end up unbound. Edits the
//! shared draft's [`crate::config::Settings::keybinds`]; nothing persists until Apply, at
//! which point `crate::hotkeys` re-registers the OS global hotkeys from the new bindings.

use dioxus::prelude::*;

use crate::config::{Command, KeyBind, KeyChord, Settings};
use crate::ui::components::{
    Badge, BadgeVariant, Body, Button, ButtonVariant, Caption, Eyebrow, Icon, IconButton,
    IconButtonVariant, Spacer,
};
use crate::ui::icons::{IconName, IconSize};

#[component]
pub(super) fn Keymap() -> Element {
    let draft = use_context::<super::SettingsCtx>().draft;
    // Which command is capturing a chord right now, and any pending conflict (the command
    // being rebound, the captured chord, and the command that already holds it).
    let capturing = use_signal(|| None::<Command>);
    let conflict = use_signal(|| None::<(Command, KeyChord, Command)>);

    let has_overrides = !draft.read().keybinds.is_empty();

    rsx! {
        super::Anchor { id: "keymap",
            div { class: "keymap-head",
                Caption { style: "color:var(--dim2);", "Click a shortcut to rebind it. ⌘ shortcuts also respond to Ctrl." }
                Spacer {}
                if has_overrides {
                    Button {
                        variant: ButtonVariant::Secondary,
                        small: true,
                        onclick: move |_| reset_all(draft),
                        "Reset all"
                    }
                }
            }
            div { class: "keymap-box",
                for cmd in crate::keymap::all_commands() {
                    {keymap_row(cmd, draft, capturing, conflict)}
                }
            }
        }
    }
}

/// One command row: label + desc + Custom badge, an inline conflict prompt when a captured
/// chord collides, and the chord controls (caps / capture / add / reset).
fn keymap_row(
    cmd: Command,
    draft: Signal<Settings>,
    mut capturing: Signal<Option<Command>>,
    mut conflict: Signal<Option<(Command, KeyChord, Command)>>,
) -> Element {
    let binds = draft.read().keybinds.clone();
    let (label, desc) = crate::keymap::describe(cmd);
    let chord = draft_chord(&binds, cmd);
    let overridden = binds.iter().any(|b| b.command == cmd);
    let capturing_here = *capturing.read() == Some(cmd);
    let conflict_here = conflict.read().clone().filter(|(c, _, _)| *c == cmd);

    rsx! {
        div { class: "keymap-row",
            div { class: "keymap-rowmain",
                div { class: "row", style: "gap:var(--sp-3);align-items:center;",
                    Body { "{label}" }
                    if overridden {
                        Badge { variant: BadgeVariant::Accent, "Custom" }
                    }
                }
                Caption { style: "display:block;color:var(--dim2);margin-top:var(--sp-1);", "{desc}" }
                if let Some((_, ch, other)) = conflict_here {
                    {
                        let ch_disp = crate::keymap::chord_caps(&ch).join("");
                        let other_label = crate::keymap::describe(other).0;
                        let ch2 = ch.clone();
                        rsx! {
                            div { class: "keymap-conflict",
                                Caption { style: "color:var(--red);", "{ch_disp} is already bound to {other_label}." }
                                Button {
                                    variant: ButtonVariant::Primary,
                                    small: true,
                                    onclick: move |_| {
                                        set_bind(draft, cmd, Some(ch2.clone()));
                                        set_bind(draft, other, None);
                                        conflict.set(None);
                                    },
                                    "Reassign"
                                }
                                Button {
                                    variant: ButtonVariant::Ghost,
                                    small: true,
                                    onclick: move |_| conflict.set(None),
                                    "Cancel"
                                }
                            }
                        }
                    }
                }
            }
            div { class: "keymap-controls",
                if capturing_here {
                    span {
                        class: "keymap-capture",
                        tabindex: "0",
                        onmounted: move |e| { spawn(async move { let _ = e.set_focus(true).await; }); },
                        onfocusout: move |_| { if *capturing.peek() == Some(cmd) { capturing.set(None); } },
                        onkeydown: move |e| {
                            e.prevent_default();
                            e.stop_propagation();
                            if crate::keymap::is_modifier_key(&e) {
                                return;
                            }
                            if e.key() == Key::Escape {
                                capturing.set(None);
                                return;
                            }
                            let ch = crate::keymap::chord_from_event(&e);
                            let binds = draft.read().keybinds.clone();
                            match conflicting_cmd(&binds, &ch, cmd) {
                                Some(other) => {
                                    conflict.set(Some((cmd, ch, other)));
                                    capturing.set(None);
                                }
                                None => {
                                    set_bind(draft, cmd, Some(ch));
                                    capturing.set(None);
                                }
                            }
                        },
                        "Press shortcut…"
                    }
                    Button {
                        variant: ButtonVariant::Secondary,
                        small: true,
                        onclick: move |_| capturing.set(None),
                        "Esc"
                    }
                } else if let Some(ch) = chord {
                    button {
                        class: "keymap-caps",
                        r#type: "button",
                        title: "Click to rebind",
                        onclick: move |_| { conflict.set(None); capturing.set(Some(cmd)); },
                        for cap in crate::keymap::chord_caps(&ch) {
                            Eyebrow { class: "keycap", "{cap}" }
                        }
                        Icon { name: IconName::Pencil, size: IconSize::Xs }
                    }
                } else {
                    button {
                        class: "keymap-add",
                        r#type: "button",
                        onclick: move |_| { conflict.set(None); capturing.set(Some(cmd)); },
                        "Add shortcut"
                    }
                }
                if overridden {
                    IconButton {
                        variant: IconButtonVariant::Ghost,
                        icon: IconName::Refresh,
                        icon_size: IconSize::Xs,
                        title: "Reset to default",
                        // Reset routes through the same conflict flow: if the default chord is
                        // now held by another command, prompt (Reassign unbinds it) rather than
                        // silently creating a duplicate binding.
                        onclick: move |_| {
                            let binds = draft.read().keybinds.clone();
                            let default = crate::keymap::default_chord(cmd);
                            match conflicting_cmd(&binds, &default, cmd) {
                                Some(other) => conflict.set(Some((cmd, default, other))),
                                None => reset(draft, cmd),
                            }
                        },
                    }
                }
            }
        }
    }
}

/// The draft's effective chord for `cmd` — its override (possibly an explicit unbind →
/// `None`), else the built-in default.
fn draft_chord(binds: &[KeyBind], cmd: Command) -> Option<KeyChord> {
    match binds.iter().find(|b| b.command == cmd) {
        Some(b) => b.chord.clone(),
        None => Some(crate::keymap::default_chord(cmd)),
    }
}

/// The other command whose draft chord already equals `chord` (a conflict), if any.
fn conflicting_cmd(binds: &[KeyBind], chord: &KeyChord, exclude: Command) -> Option<Command> {
    crate::keymap::all_commands()
        .find(|&c| c != exclude && draft_chord(binds, c).as_ref() == Some(chord))
}

/// Write `cmd`'s override (`Some` = bind, `None` = explicit unbind). Binding back to the
/// default drops the override entirely, so the row is no longer "Custom".
fn set_bind(mut draft: Signal<Settings>, cmd: Command, chord: Option<KeyChord>) {
    let is_default = chord.as_ref() == Some(&crate::keymap::default_chord(cmd));
    let mut binds = draft.read().keybinds.clone();
    binds.retain(|b| b.command != cmd);
    if !is_default {
        binds.push(KeyBind { command: cmd, chord });
    }
    draft.write().keybinds = binds;
}

/// Drop `cmd`'s override (back to its default chord).
fn reset(mut draft: Signal<Settings>, cmd: Command) {
    draft.write().keybinds.retain(|b| b.command != cmd);
}

/// Drop every override.
fn reset_all(mut draft: Signal<Settings>) {
    draft.write().keybinds.clear();
}
