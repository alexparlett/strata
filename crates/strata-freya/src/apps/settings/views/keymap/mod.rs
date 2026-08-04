//! **Settings ▸ Keymap** (P4-08, DEV_TASKS W4, design `Settings.dc.html`) — every command the
//! app dispatches, and the chord it answers to.
//!
//! **The mechanism was already here; this is the control.** P2-20 shipped the whole
//! settings-driven resolution — [`strata_core::keymap`]'s table, `effective_chord`, the
//! conflict policy, the unbind — so rebinding by hand-editing `config.json` has worked all
//! along and every hint, dispatcher and editor binding already follows it. What P4-08 adds is
//! the page: the grid, capture, the conflict answer, reset.
//!
//! **Every change is one funnel.** Capture, the per-row reset and Reassign all go through
//! [`ask`], which asks [`keymap::propose`] what the change would cost and either commits it or
//! raises the note. So there is no path that writes a binding without the conflict check, and
//! the check itself is in core beside `validate_bind` rather than here — the same rules a
//! hand-edited config meets.
//!
//! **A reset is conflict-checked too**, which is easy to miss: a command's default chord can
//! have been taken by another command in the meantime (bind Save query to ⌘G, bind Find to the
//! ⌘S that freed up, then reset Save query), so resetting is a proposal like any other rather
//! than a `retain` on the draft.
//!
//! **The menubar is disarmed while a row is listening and this window has the keys.** The OS
//! resolves a menu accelerator before the window sees the key, so with the menubar armed, pressing
//! ⌘C to bind it would copy and the row would go on waiting. Half of what a user reaches for here
//! is a menu accelerator (⌘Z ⌘X ⌘C ⌘V ⌘A ⌘O ⌘Q ⌘,), so
//! [`MenuHandles::suspend_accelerators`] holds them off — but only while this window is focused,
//! because the listener it is protecting is this window's and cannot fire otherwise. Settings is
//! not modal, so an abandoned capture (click the project window, never press a key) would
//! otherwise leave the whole app's menubar disarmed until the window closed.
//!
//! ## Divergences from the canvas
//!
//! - Its intro still reads "Click a shortcut to rebind it" from before the pane became a table,
//!   while the cell it describes now carries `onDoubleClick` and the title "Double-click to
//!   rebind". The gesture is the deliberate half of that pair — a single click in a table row
//!   means "I am pointing at this" — so the sentence follows the gesture rather than the other
//!   way round. It also names the **row**, which is where the handler sits: the canvas hangs
//!   `onDoubleClick` off the shortcut cell alone, and a row is one command with one chord, so
//!   there is no part of it that means something else to press.
//! - **No zebra.** The canvas bands these rows; the Engine pane's grid was banded in the canvas
//!   too and P4-07 settled that a settings list is not a results grid (AGENTS.md §3). One answer
//!   for both of this window's tables.
//! - **There is no unbind control**, because the canvas has none: a chord becomes free by being
//!   taken (Reassign), which is also the only thing that produces the unbound row the canvas
//!   *does* draw, with its Add shortcut. The state is fully supported — `effective_chord` returns
//!   `None`, hints vanish, menu items ship disabled — there is simply no button that says "leave
//!   this command with no shortcut". Worth revisiting with the designer rather than inventing.

mod model;
mod table;

use freya::prelude::*;
use strata_core::config::Command;
use strata_core::keymap::{self, Bind, Rebind};

use crate::apps::settings::views::keymap::model::{has_overrides, rows, Blocked, Editing};
use crate::apps::settings::views::keymap::table::KeyTable;
use crate::apps::settings::views::Pane;
use crate::apps::settings::{settings_theme, SettingsCtx};
use crate::components::typography::{Control, Prose};
use crate::keymap::chord_from_event;
use crate::menu::menu_chords;
use crate::state::{use_config, AppCtx, ConfigChan};

/// The gap under the intro line, before the grid (canvas `margin-bottom: var(--sp-5)`).
const INTRO_GAP: f32 = 16.;
/// Reset all's height, matching the Engine pane's Revert (canvas `height: 26px`).
const BUTTON_HEIGHT: f32 = 26.;

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

        // The rows are a pure projection of the draft — a chord is already the shape the setting
        // stores, so unlike the Engine pane there is no editing model between the two.
        let (rows, overridden) = {
            let draft = ctx.draft.read();
            (rows(&draft), has_overrides(&draft))
        };

        // Hold the menubar's accelerators off for as long as a row is listening **in this
        // window while it has the keys** (module doc), and put back exactly what the *committed*
        // settings say the moment either stops being true — the same set the focused window's
        // routine sync applies, so the two can't disagree.
        //
        // Focus is half the condition, not a detail. The suspension exists to stop the OS
        // resolving an accelerator before the capture listener sees the key, and that listener is
        // this window's: it cannot receive one while another window has focus, so there is nothing
        // to protect and no reason to hold the menubar off. Settings is deliberately **not** modal
        // (`platform::settings`), so clicking the project window behind it mid-capture is an
        // ordinary thing to do — and without focus in the condition the accelerators would stay
        // off app-wide until the capture was finished or the window closed, taking every Edit-menu
        // item's chord *and* its enabled state with them.
        let committed = use_config(ConfigChan::Settings);
        let focused = use_hook(Platform::get).is_app_focused;
        let mut menu = use_consume::<AppCtx>().menu;
        // Every input is read **inside** the effect. `use_side_effect` builds its closure once, so
        // a value computed in the render above would freeze at its first reading and neither a
        // capture nor a focus change would ever move it (AGENTS.md §3).
        use_side_effect(move || {
            let listening = *focused.read() && editing.read().capturing_command().is_some();
            let chords = menu_chords(&committed.read().settings);
            if let Some(handles) = menu.write().as_mut() {
                handles.sync_chords(&chords);
                handles.suspend_accelerators(listening);
            }
        });
        // A capture must not outlive the page it started on: leaving the category unmounts the
        // listener below, so the menubar has to be re-armed on the way out too. The effect above
        // covers a window that merely loses focus; this covers one that goes.
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
                            .height(Size::px(BUTTON_HEIGHT))
                            .on_press(move |_: Event<PressEventData>| {
                                ctx.edit(keymap::reset_all);
                                editing.set(Editing::Idle);
                            })
                            .child(Control::new("Reset all"))
                    })),
            )
            .child(rect().height(Size::px(INTRO_GAP)))
            .child(KeyTable { rows, editing })
            // The capture listener, mounted only while a row is listening. Deliberately the LAST
            // child: same-name global listeners fire in document order, and the window root's own
            // Esc/⌘Q handler sits after the router — so this outranks it and Esc cancels the
            // capture instead of closing the window.
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
    // Read in a block: `ctx.edit` takes a write guard on the same draft.
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
