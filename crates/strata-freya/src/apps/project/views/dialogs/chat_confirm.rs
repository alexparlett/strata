//! The chat pane's **destructive confirms** (AS-07) — deleting one stored conversation, and
//! clearing them all.
//!
//! Its own dialog rather than a fifth [`DropTarget`](super::DropTarget) variant, because that
//! vocabulary is a *named catalog row*: it renders the name as a `MonoValue` under the verb and
//! counts the views a drop leaves invalid. A clear-all has no name at all, and a conversation has
//! no dependents — both fields would be empty or invented. What the two shapes do share is the
//! part that matters, and it is shared for real: the card, the action strip and the Esc/Enter
//! barrier are the same [`Dialog`].
//!
//! **Mounted at the window root, with the other confirms.** Global key listeners fire in document
//! order and a handled command consumes the press, so a dialog mounted inside the pane it belongs
//! to is a barrier over nothing — with a query running, Esc would reach the results pane's cancel
//! before the dialog that is on screen. This was built inside the chat header first and moved
//! here for exactly that reason.
//!
//! ## Why a per-row delete asks at all
//!
//! Before AS-07 the switcher's delete discarded a transcript this window happened to be holding.
//! It now removes a file, which is the user's record of an investigation — so it asks, on the
//! same terms as every other path that destroys a project's work.

use freya::components::{get_theme, Button, ButtonColorsThemePartial};
use freya::prelude::*;
use freya::radio::use_radio_station;

use crate::apps::project::state::{
    clear_all, discard, seed_pick, use_report, ChatsCtx, ProjChan, ProjectState, RowKey,
};
use crate::apps::project::views::{CancelButtonThemePartial, CancelButtonThemePreference};
use crate::components::dialog::{Dialog, DialogHeader};
use crate::components::icon::{Icon, IconName};
use crate::components::tones::tones;
use crate::components::typography::{Control, Prose, Title};
use crate::state::{use_config, ConfigChan};
use crate::theme::{use_roles, Role};

/// What the chat pane is about to destroy.
#[derive(Clone, PartialEq, Debug)]
pub enum ChatDrop {
    /// One conversation, named — a switcher row's delete, whichever kind of row it was.
    One { key: RowKey, title: String },
    /// Every conversation this project has stored.
    All,
}

impl ChatDrop {
    /// The action, used for the title and the button — the drop confirm's own rule, where the two
    /// are one string.
    fn verb(&self) -> &'static str {
        match self {
            ChatDrop::One { .. } => "Delete conversation",
            ChatDrop::All => "Clear conversations",
        }
    }

    /// What it does, and what it leaves alone. Both lines answer the question the user is really
    /// asking: is this only the chat, or the work it was about?
    fn body(&self) -> &'static str {
        match self {
            ChatDrop::One { .. } => {
                "Removes this conversation from the project. The tables, queries and views it \
                 talked about are not affected."
            }
            ChatDrop::All => {
                "Removes every saved conversation in this project. The tables, queries and views \
                 they talked about are not affected."
            }
        }
    }
}

/// The dialog. Mount once at the window root, after the close confirm: a question about a running
/// query outranks one about a transcript.
#[derive(PartialEq)]
pub struct ChatConfirm {
    pub target: State<Option<ChatDrop>>,
}

impl Component for ChatConfirm {
    fn render(&self) -> impl IntoElement {
        let mut slot = self.target;
        let target = slot.read().clone();

        let chats = use_consume::<ChatsCtx>();
        let mut pending = use_state(|| false);
        let project = use_radio_station::<ProjectState, ProjChan>();
        let root = project.peek().root.clone();
        let report = use_report();
        let config = use_config(ConfigChan::Settings);
        let roles = use_roles();
        let error_tone = tones().error;
        // The destructive action wears the shared `cancel_button` dress — the themes' authored
        // destructive tone, never a hardcoded red.
        let danger = get_theme!(
            &None::<CancelButtonThemePartial>,
            CancelButtonThemePreference,
            "cancel_button"
        );

        // The work itself, off the pressed element's scope. It reads the target the press left
        // rather than taking it as an argument, because by the time this runs the slot is clear.
        let armed = target.clone();
        use_side_effect(move || {
            if !*pending.read() {
                return;
            }
            pending.set(false);
            let Some(target) = armed.clone() else {
                return;
            };
            let fresh = seed_pick(&config.read().settings.ai);
            match &target {
                ChatDrop::One { key, .. } => discard(chats, root.clone(), report, *key, fresh),
                ChatDrop::All => clear_all(chats, root.clone(), report, fresh),
            }
        });

        // Read after the hooks, never before: a component's hooks run a fixed number of times
        // per render, so the early return has to come after all of them.
        let Some(target) = target else {
            return rect().into_element();
        };
        let verb = target.verb();
        // **The action over its subject, in the header** — the drop confirm's shape exactly. The
        // heading belongs to the header run and the body carries only the prose; saying it in
        // both is the card stating its own name twice.
        let title = rect()
            .width(Size::fill())
            .vertical()
            .child(Title::new(verb).color(roles.get(Role::Text)))
            .maybe_child(match &target {
                ChatDrop::One { title, .. } => Some(
                    Prose::new(title.clone())
                        .color(roles.get(Role::TextMuted))
                        .max_lines(1)
                        .text_overflow(TextOverflow::Ellipsis),
                ),
                ChatDrop::All => None,
            });

        // **The press records the intent; this component's own scope performs it.** `discard` and
        // `clear_all` spawn the file work, and `spawn` binds to whichever scope is current during
        // dispatch — the pressed Button's, which `slot.set(None)` unmounts in the same handler.
        // Spawned from there the delete was dropped before its first poll, so the row vanished
        // and the file stayed. The effect below runs in this component's scope, which outlives
        // the dialog's contents.
        let mut confirm = move || {
            pending.set(true);
            slot.set(None);
        };
        let mut key_confirm = confirm;

        let body = Prose::new(target.body())
            .color(roles.get(Role::TextMuted))
            .width(Size::fill())
            .wrap();

        Dialog::new()
            .on_dismiss(move |()| slot.set(None))
            .on_confirm(move |()| key_confirm())
            .header(DialogHeader::new(IconName::Trash, error_tone, title))
            .body(body)
            .action(
                Button::new()
                    .flat()
                    .on_press(move |_| slot.set(None))
                    .child(Control::new("Cancel")),
            )
            .action(
                Button::new()
                    .filled()
                    .theme_colors(
                        ButtonColorsThemePartial::default()
                            .background(danger.background)
                            .hover_background(danger.hover_background)
                            .border_fill(danger.border_fill)
                            .hover_border_fill(danger.border_fill)
                            .color(danger.color)
                            .hover_color(danger.color),
                    )
                    .on_press(move |_| confirm())
                    .child(
                        rect()
                            .horizontal()
                            .cross_align(Alignment::Center)
                            .spacing(8.)
                            .child(Icon::new(IconName::Trash).size(13.))
                            .child(Control::new(verb)),
                    ),
            )
            .into_element()
    }
}
