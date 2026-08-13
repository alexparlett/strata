//! **Settings ▸ Keymap** (P4-08, `DEV_TASKS` W4, design `Settings.dc.html`) — every command the
//! app dispatches, and the chord it answers to.
//!
//! **The mechanism was already here; this is the control.** [`strata_core::keymap`]'s table,
//! `effective_chord`, the conflict policy and the unbind all shipped with P2-20, so every hint,
//! dispatcher and editor binding already follows them. This adds the page.
//!
//! **Every change is one funnel.** Capture, the per-row reset and Reassign all go through [`ask`],
//! which asks [`keymap::propose`] what the change would cost. So no path writes a binding without
//! the conflict check, and the check lives in core beside `validate_bind` — the same rules a
//! hand-edited config meets. **A reset is conflict-checked too**, which is easy to miss: a
//! command's default chord can have been taken in the meantime.
//!
//! **The menubar is disarmed while a row is listening and this window has the keys.** The OS
//! resolves an accelerator before the window sees the key, so pressing ⌘C to bind it would copy
//! while the row went on waiting. [`MenuHandles::suspend_accelerators`] holds them off — but only
//! while this window is focused, because Settings is not modal and an abandoned capture would
//! otherwise leave the whole app's menubar disarmed.
//!
//! **Divergences from the canvas:**
//!
//! - Its intro still reads "Click a shortcut to rebind it" from before the pane became a table.
//!   The gesture is the deliberate half of that pair — a single click in a table row means "I am
//!   pointing at this" — and it is on the **row**, which is one command with one chord.
//! - **No zebra**, the answer P4-07 settled for both of this window's tables: a settings list is
//!   not a results grid.
//! - **No unbind control**, because the canvas has none. A chord becomes free by being taken. The
//!   state is fully supported; there is simply no button for it, which is worth revisiting with
//!   the designer rather than inventing.

mod model;
mod table;

use freya::prelude::*;
use strata_core::config::Command;
use strata_core::keymap::{self, Bind, Rebind};

use crate::apps::settings::views::keymap::model::{has_overrides, rows, Blocked, Editing};
use crate::apps::settings::views::keymap::table::KeyTable;
use crate::apps::settings::views::Pane;
use crate::apps::settings::{settings_theme, SettingsCtx};
use crate::components::metrics::{COMPACT_BUTTON, SP_5};
use crate::components::typography::{Control, Prose};
use crate::keymap::chord_from_event;
use crate::menu::menu_chords;
use crate::state::{use_config, AppCtx, ConfigChan};

/// The gap under the intro line, before the grid (canvas `margin-bottom: var(--sp-5)`).
const INTRO_GAP: f32 = SP_5;
/// What the pane says about itself, once, above the grid. A double press on the row, not a single
/// click on the chord — see the module doc.
const INTRO: &str =
    "Double-click a row to rebind its shortcut. \u{2318} shortcuts also respond to Ctrl.";

#[derive(PartialEq)]
pub struct KeymapPane;

impl Component for KeymapPane {
    fn render(&self) -> impl IntoElement {
        let theme = settings_theme();
        let ctx = use_consume::<SettingsCtx>();
        let mut editing = use_state(Editing::default);

        let (rows, overridden) = {
            let draft = ctx.draft.read();
            (rows(&draft), has_overrides(&draft))
        };

        let committed = use_config(ConfigChan::Settings);
        let focused = use_hook(Platform::get).is_app_focused;
        let mut menu = use_consume::<AppCtx>().menu;
        use_side_effect(move || {
            let listening = *focused.read() && editing.read().capturing_command().is_some();
            let chords = menu_chords(&committed.read().settings);
            if let Some(handles) = menu.write().as_mut() {
                handles.sync_chords(&chords);
                handles.suspend_accelerators(listening);
            }
        });
        use_drop(move || {
            if let Some(handles) = menu.write().as_mut() {
                handles.suspend_accelerators(false);
            }
        });

        let body = rect()
            .width(Size::fill())
            .vertical()
            .child(
                rect()
                    .width(Size::fill())
                    .horizontal()
                    .content(Content::Flex)
                    .cross_align(Alignment::Center)
                    .child(
                        rect().width(Size::flex(1.)).child(
                            Prose::new(INTRO)
                                .color(theme.hint_color)
                                .max_width(Size::px(620.))
                                .wrap(),
                        ),
                    )
                    .maybe_child(overridden.then(|| {
                        Button::new()
                            .height(Size::px(COMPACT_BUTTON))
                            .on_press(move |_: Event<PressEventData>| {
                                ctx.edit(keymap::reset_all);
                                editing.set(Editing::Idle);
                            })
                            .child(Control::new("Reset all"))
                    })),
            )
            .child(rect().height(Size::px(INTRO_GAP)))
            .child(KeyTable { rows, editing })
            .maybe_child(
                editing
                    .read()
                    .capturing_command()
                    .map(|command| CaptureListener { command, editing }),
            );

        Pane::new(body)
    }
}

/// Listens for the chord a row is waiting for.
///
/// It consumes **every** press it can fold into a chord, which is the point: while a row is
/// listening, ⌘W must be captured rather than close a tab. A modifier on its own is not a chord,
/// so those fall through and the row goes on waiting. Escape cancels, with any modifiers — the
/// canvas's rule, and nobody wants ⌘Esc bound to anything.
#[derive(PartialEq)]
struct CaptureListener {
    command: Command,
    editing: State<Editing>,
}

impl Component for CaptureListener {
    fn render(&self) -> impl IntoElement {
        let ctx = use_consume::<SettingsCtx>();
        let mut editing = self.editing;
        let command = self.command;

        rect().on_global_key_down(move |e: Event<KeyboardEventData>| {
            let Some(chord) = chord_from_event(&e) else {
                return;
            };
            e.prevent_default();
            if chord.key == "Escape" {
                editing.set(Editing::Idle);
                return;
            }
            ask(ctx, editing, command, Rebind::To(chord));
        })
    }
}

/// Ask for a rebind: commit it when nothing is in the way, otherwise raise the note under the row
/// and wait for an answer. The **one** write path for a binding on this page.
fn ask(ctx: SettingsCtx, mut editing: State<Editing>, command: Command, rebind: Rebind) {
    let proposal = {
        let draft = ctx.draft.peek();
        keymap::propose(&draft, command, &rebind)
    };
    let blocked = |holders, message| {
        Editing::Blocked(Blocked {
            command,
            rebind: rebind.clone(),
            holders,
            message,
        })
    };
    match proposal {
        Bind::Ready => {
            ctx.edit(|settings| keymap::apply(settings, command, &rebind));
            editing.set(Editing::Idle);
        }
        Bind::Clash { holders, message } => editing.set(blocked(holders, message)),
        Bind::Refused { message } => editing.set(blocked(Vec::new(), message)),
    }
}

/// Push a clash through: take the chord off every command holding it, then give it to the one
/// that asked. Each binding that changes is written out — a "steal" that only recorded the winner
/// would leave two commands claiming one chord, which `resolve` would settle silently by table
/// order, and one that freed only the *first* holder would do the same for a chord a hand-edited
/// config had already duplicated.
fn reassign(ctx: SettingsCtx, mut editing: State<Editing>, blocked: &Blocked) {
    let command = blocked.command;
    let rebind = blocked.rebind.clone();
    let holders = blocked.holders.clone();
    ctx.edit(move |settings| {
        for holder in holders {
            keymap::apply(settings, holder, &Rebind::Off);
        }
        keymap::apply(settings, command, &rebind);
    });
    editing.set(Editing::Idle);
}
