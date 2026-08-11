//! **Configure a provider** — the dialog where a credential is actually set.
//!
//! ## Why a dialog and not a field on the row
//!
//! Setting a key is a *task*, and the value it produces can never be shown back: a form does not
//! redisplay a secret, and reading one per row on open would be a blocking Keychain call — an
//! authorisation prompt just to look at Settings. An always-present input promises the opposite,
//! that it shows the current value and you may edit it, so an already-configured provider opened
//! with an empty box beside an eye that revealed nothing. It read as a key that had failed to
//! save, and was reported as exactly that.
//!
//! A dialog makes no such promise. It is opened *to do something*, it says what it will do, and
//! it closes. The row goes back to one line and the pane fits eight providers without scrolling.
//!
//! ## It has its own buffers, and Cancel means it
//!
//! Every other control in this window writes the draft on every keystroke, because every other
//! control shows a value it can also read. This one holds its edits locally and only writes them
//! on **Save** — which is what makes Cancel a real revert rather than a word on a button that
//! discards nothing. That is the same bargain the window's own draft makes with the config
//! store, one level down.
//!
//! It is still not a *commit*: Save writes the draft and `ai_keys`, and Settings' Apply is what
//! reaches the keystore and the config file. There is one commit point in this window and this
//! is not a second one.
//!
//! ## Test lives here
//!
//! Because it tests what is in these boxes, not what was last applied — and because the row it
//! came from has no room for a button whose answer is a sentence. The row shows the settled
//! result in its subline; this shows the live one.

use freya::prelude::*;
use strata_agent::assistant::{info, label, BaseUrl, KeyUse};
use strata_core::ai::ProviderKind;
use strata_core::secret::Secret;

use crate::apps::settings::views::ai::probe;
use crate::apps::settings::views::ai::providers::will_have_key;
use crate::apps::settings::views::ai::row::mark;
use crate::apps::settings::SettingsCtx;
use crate::components::dialog::{Dialog, DialogHeader};
use crate::components::form::{form_theme, ValueField, FIELD_HEIGHT};
use crate::components::icon::IconName;
use crate::components::tones::{tones, Tones};
use crate::components::tool_button::ToolButton;
use crate::components::typography::{Body, Control, Eyebrow, Meta, Prose, Title};
use crate::state::{Ask, Probe, Tone};
use crate::theme::{use_roles, Role, RoleColors};

/// The dialog's field column.
const FIELD_WIDTH: f32 = 380.;

/// Which provider is being configured, or none. The pane holds one of these; a dialog is a
/// question about a single row.
pub type Configuring = Option<ProviderKind>;

#[derive(PartialEq)]
pub struct ConfigureDialog {
    pub kind: ProviderKind,
    /// The pane's slot — closing is setting it to `None`, which is also what dismissal does.
    pub slot: State<Configuring>,
    pub ctx: SettingsCtx,
}

impl Component for ConfigureDialog {
    fn render(&self) -> impl IntoElement {
        let kind = self.kind;
        let ctx = self.ctx;
        let mut slot = self.slot;
        let provider = info(kind);
        let roles = use_roles();
        let tones = tones();
        let required_color = form_theme().required_color;

        // **Local, and seeded once.** The draft is not written until Save, so these are the only
        // copy of what the user is typing — which is what makes Cancel a revert.
        //
        // The key box seeds from what is *pending*, and never from what is stored: a key typed
        // and not yet applied is the user's own input sitting in memory, and hiding it would make
        // reopening the dialog look like the paste had been lost. A stored key has nothing to
        // seed from — that is the point of the dialog rather than a defect in it, and what it
        // gets instead is a sentence saying so, below.
        let key_buf = use_state({
            let seed = ctx.ai_keys.peek().get(kind).to_string();
            move || seed
        });
        let url_buf = use_state({
            let seed = ctx.base_url_of(kind);
            move || seed
        });
        let mut revealed = use_state(|| false);
        // **What a Test here was run against** — the boxes as they stood at the press, which is
        // what decides whether the listing it fetched still describes the provider once this
        // dialog closes ([`retract_if_stale`]).
        //
        // The values and not a flag, because the boxes keep moving after a Test: a flag can only
        // ask "do the boxes differ *now*", which throws away a good listing when a test against
        // the draft's own values is followed by an idle edit, and keeps a bad one when an edited
        // box is tested and then typed back. Local, and exactly as long-lived as the key box it
        // copies from, so it is no more exposure than the box itself.
        let tested = use_state(|| None::<Tested>);
        // **The window's probe, not a local one.** A test here is the only place one is taken,
        // and its answer is read by two surfaces that are not this dialog: the row's subline and
        // AI ▸ Chat's model list. A local copy would leave both of them permanently untested.
        let probe = ctx.probes.read().get(kind).clone();

        // "Will there be a key after Apply", not "is one filed now" — the marker only moves when
        // Apply reaches the keystore, so reading it alone leaves this note describing the state
        // before an edit the user has already made.
        let stored = will_have_key(&ctx.draft.read().ai, &ctx.ai_keys.read(), kind);

        let boxes_url = !matches!(provider.base_url, BaseUrl::Provider);
        let boxes_key = !matches!(provider.key, KeyUse::Unused);

        let key_note = self.key_note(stored, &roles);

        let status = status_line(&probe, &roles, tones);

        let body = rect()
            .width(Size::px(FIELD_WIDTH))
            .spacing(12.)
            .maybe_child(boxes_url.then(|| {
                rect()
                    .width(Size::fill())
                    .spacing(6.)
                    .child(
                        rect()
                            .horizontal()
                            .cross_align(Alignment::Center)
                            .spacing(6.)
                            .child(Eyebrow::new("BASE URL").color(roles.get(Role::TextMuted)))
                            .maybe_child(
                                matches!(provider.base_url, BaseUrl::Required)
                                    .then(|| Meta::new("REQUIRED").color(required_color)),
                            ),
                    )
                    .child(
                        ValueField::new(url_buf)
                            .width(Size::fill())
                            .placeholder(default_url(kind)),
                    )
            }))
            .maybe_child(boxes_key.then(|| {
                rect()
                    .width(Size::fill())
                    .spacing(6.)
                    .child(Eyebrow::new("API KEY").color(roles.get(Role::TextMuted)))
                    .child(
                        rect()
                            .width(Size::fill())
                            .horizontal()
                            .cross_align(Alignment::Center)
                            .spacing(8.)
                            .content(Content::Flex)
                            .child(
                                ValueField::new(key_buf)
                                    .width(Size::flex(1.))
                                    .masked(!*revealed.read())
                                    .placeholder("paste API key"),
                            )
                            .child(
                                ToolButton::new(
                                    IconName::Eye,
                                    match *revealed.read() {
                                        true => "Hide the key",
                                        false => "Show the key",
                                    },
                                )
                                .outlined()
                                .on_press(EventHandler::new(move |_: Event<PressEventData>| {
                                    let shown = *revealed.peek();
                                    revealed.set(!shown);
                                })),
                            ),
                    )
                    .child(key_note)
            }))
            .child(
                rect()
                    .width(Size::fill())
                    .horizontal()
                    .cross_align(Alignment::Center)
                    .spacing(8.)
                    .content(Content::Flex)
                    .child(
                        Button::new()
                            .outline()
                            .height(Size::px(FIELD_HEIGHT))
                            .on_press(move |_: Event<PressEventData>| {
                                // **What Apply would send, which is not always what is filed.**
                                // A test that proved something other than what is on screen is
                                // worse than none — so the boxes win, and the stored key is
                                // only the answer when nothing is pending against it.
                                //
                                // An empty box is not "nothing pending": `ai_keys` can hold a
                                // queued *removal* this dialog did not make (switching the
                                // provider off records one, and the gear still opens). Falling
                                // back to the marker there authenticated with a key on its way
                                // out and reported "verified" directly beneath a note saying
                                // there was none.
                                let typed_now = key_buf.peek().clone();
                                let pending = ctx.ai_keys.peek().touched(kind);
                                let ask = Ask {
                                    kind,
                                    base_url: url_buf.peek().clone(),
                                    typed: Secret::new(&typed_now),
                                    stored: (!pending && typed_now.trim().is_empty())
                                        .then(|| {
                                            ctx.draft
                                                .peek()
                                                .ai
                                                .setup(kind)
                                                .and_then(|setup| setup.key.clone())
                                        })
                                        .flatten(),
                                };
                                // The one funnel — which also holds the in-flight guard, so a
                                // second press during a request is left alone rather than
                                // racing it. The names it returns reach the satellite, so a
                                // Test is what fills the model picker for good rather than
                                // only for this window — which is exactly why closing has to
                                // know what this test was run against.
                                let asked = (url_buf.peek().clone(), typed_now);
                                // Only when a request actually started: the guard swallows a
                                // press made during one, and the handle for *that* request is
                                // the one already held here.
                                if let Some(task) = probe::refresh(ctx, ask) {
                                    let mut tested = tested;
                                    tested.set(Some(Tested {
                                        asked,
                                        task: Some(task),
                                    }));
                                }
                            })
                            .child(Control::new("Test")),
                    )
                    .child(rect().width(Size::flex(1.)))
                    .child(status),
            );

        Dialog::new()
            .on_dismiss(move |()| discard(ctx, kind, tested, slot))
            .header(DialogHeader::new(
                mark(kind),
                roles.get(Role::Accent),
                Title::new(format!("Configure {}", label(kind))),
            ))
            .body(body)
            .action(
                Button::new()
                    .flat()
                    .on_press(move |_| discard(ctx, kind, tested, slot))
                    .child(Control::new("Cancel")),
            )
            .action(
                Button::new()
                    .filled()
                    .on_press(move |_| {
                        save(
                            ctx,
                            kind,
                            &key_buf.peek(),
                            &url_buf.peek(),
                            tested.peek().clone(),
                        );
                        slot.set(None);
                    })
                    .child(Control::new("Save")),
            )
    }
}

impl ConfigureDialog {
    /// What an empty key box means, which is not the same in both directions: with nothing
    /// stored it is "no key, use the fallback"; with one stored it is "leave it alone".
    ///
    /// **There is no Remove here.** Removing a key is switching the provider off — enabled and
    /// configured are one state, so a second gesture for "keep it on but forget the key" would be
    /// a way to reach the one arrangement Apply refuses.
    fn key_note(&self, stored: bool, roles: &RoleColors) -> Element {
        match (!matches!(info(self.kind).key, KeyUse::Unused), stored) {
            (false, _) => rect().into(),
            (true, true) => Prose::new("A key is stored. Type a new one to replace it.")
                .width(Size::fill())
                .wrap()
                .color(roles.get(Role::TextPlaceholder))
                .into(),
            (true, false) => match info(self.kind).key {
                KeyUse::Env(var) => Prose::new(format!(
                    "Leave empty to use the '{var}' environment variable."
                ))
                .width(Size::fill())
                .wrap()
                .color(roles.get(Role::TextPlaceholder))
                .into(),
                _ => Prose::new("Leave empty to connect without a key.")
                    .width(Size::fill())
                    .wrap()
                    .color(roles.get(Role::TextPlaceholder))
                    .into(),
            },
        }
    }
}

/// The last thing a Test said, in the tone it said it.
///
/// **Wrapped**, because a provider's own error is a sentence — and genai's carry a second line
/// naming the cause, which is precisely the half a single-line run drops on the floor.
fn status_line(probe: &Probe, roles: &RoleColors, tones: Tones) -> Element {
    match probe.status() {
        None => rect().into(),
        Some((tone, said)) => {
            let color = match tone {
                Tone::Working => roles.get(Role::TextPlaceholder),
                Tone::Good => tones.ok,
                Tone::Bad => tones.error,
            };
            rect()
                .width(Size::fill())
                .horizontal()
                .spacing(6.)
                .content(Content::Flex)
                .child(
                    rect()
                        .width(Size::px(6.))
                        .height(Size::px(6.))
                        .corner_radius(3.)
                        .background(color),
                )
                // Wrapped, because a provider's own error is a sentence and genai's carry a
                // second line naming the cause — the half a single-line run silently drops.
                .child(Body::new(said).width(Size::flex(1.)).wrap().color(color))
                .into()
        }
    }
}

/// The kind's default endpoint, for the URL box's placeholder.
fn default_url(kind: ProviderKind) -> &'static str {
    match info(kind).base_url {
        BaseUrl::Editable(default) => default,
        _ => "https://host/v1/",
    }
}

/// **The endpoint and credential a model listing describes** — the pair a retraction compares.
///
/// The key travels as the box's own text rather than a [`Secret`], because this is only ever
/// asked "is it still the same one": it is never sent anywhere, and it lives exactly as long as
/// the box it was copied from.
type Asked = (String, String);

/// **What a listing was fetched against, and the request still proving it.**
///
/// The pair travels together because a retraction has to act on both: dropping an answer that
/// has *not landed yet* accomplishes nothing on its own — the settle would write it straight
/// back over the retraction — so whoever decides an answer is unwanted also stops it arriving.
///
/// `task` is `None` for the answer nobody here fetched: a listing from an earlier sitting was
/// fetched against what this dialog opened on, and there is no request of ours to stop.
#[derive(Clone, PartialEq)]
struct Tested {
    asked: Asked,
    task: Option<TaskHandle>,
}

/// What is in effect for `kind` right now: the draft's endpoint, and the key pending against it.
///
/// The state any listing this dialog did **not** fetch was fetched against — nothing here writes
/// the draft until Save, so "what the dialog opened on" and "what is in effect" are the same
/// value right up to that write.
fn in_effect(ctx: SettingsCtx, kind: ProviderKind) -> Asked {
    (
        ctx.base_url_of(kind),
        ctx.ai_keys.peek().get(kind).to_string(),
    )
}

/// **Drop the provider's listing when it no longer describes the provider.**
///
/// The one rule both closing gestures apply, because both leave *something* in effect and the
/// only question worth asking is whether the last answer was fetched against it. A listing now
/// outlives the dialog in a store the draft does not reach — the app-global satellite, which
/// survives a restart and is what every model picker offers from — so a stale one is not a lost
/// cache but a picker offering a staging gateway's models for a production address.
///
/// `fetched_with` is `None` when nothing is known to have been fetched, which is not the same as
/// "unchanged": there is simply nothing to retract, so nothing is.
///
/// **A request still in flight is cancelled, not just out-voted.** `refresh` runs on the window
/// so it survives this dialog — which is what stops a probe stranding at `Testing` — and that is
/// exactly why an answer for a state nobody kept would otherwise land *after* the retraction and
/// undo it. Cancelling strands nothing either, because [`SettingsCtx::forget_provider`] puts the
/// probe back to `Untested` in the same breath, which is also what lets the next Test start.
fn retract_if_stale(
    ctx: SettingsCtx,
    kind: ProviderKind,
    fetched_with: Option<Tested>,
    after: &Asked,
) {
    let Some(tested) = fetched_with else {
        return;
    };
    if tested.asked == *after {
        return;
    }
    if let Some(task) = tested.task {
        task.cancel();
    }
    ctx.forget_provider(kind);
}

/// **Write what the dialog holds into the window's editing state.**
///
/// Not the keystore and not the config file: Settings' Apply is the one commit point, and this
/// is a second gesture into the state it commits — the URL onto the draft, the key beside it.
///
/// A key box left **empty is not an edit**, which is the difference between this and the Remove
/// press: closing a dialog you only looked at must not queue a deletion of the key you came to
/// check on. Removing is explicit, and says so on its own button.
///
/// **Saving a key that was just tested keeps the listing that test fetched.** This forgot the
/// provider whenever the key box was merely non-empty, which is the *ordinary* setup flow —
/// type a key, Test it, Save — so the common path threw away the list it had just filled and
/// made the picker blank and re-fetch. What decides it is not "did the box change" either, but
/// whether the answer in hand was fetched against what is about to be in effect: testing a
/// changed key and then saving that key is the case the box comparison also gets wrong.
fn save(ctx: SettingsCtx, kind: ProviderKind, key: &str, url: &str, tested: Option<Tested>) {
    let before = in_effect(ctx, kind);
    // What the dialog leaves behind: the URL box, and the key box unless it is empty — an empty
    // box is not an edit, so what stays in effect is whatever was already pending.
    let after = (
        url.to_string(),
        match key.trim().is_empty() {
            true => before.1.clone(),
            false => key.to_string(),
        },
    );
    // A listing this dialog did not fetch was fetched against what it opened on, and there is no
    // request of ours behind it to stop.
    let fetched_with = tested.or(Some(Tested {
        asked: before.clone(),
        task: None,
    }));

    if before.0 != *url {
        ctx.set_base_url(kind, url.to_string());
    }
    if !key.trim().is_empty() {
        let mut keys = ctx.ai_keys;
        keys.write().set(kind, key.to_string());
    }
    retract_if_stale(ctx, kind, fetched_with, &after);
}

/// **Close without saving — and take back what a Test proved about boxes nobody kept.**
///
/// Cancel is a revert, and until AS-06 that cost nothing: the dialog's edits were local, so
/// throwing them away threw away everything they had produced. A Test is the exception, because
/// its answer outlives the dialog. So a test run against an endpoint or credential the user then
/// discarded has to be retracted, or the picker offers a staging gateway's models for a
/// production address — fresh for a day, and across relaunches.
///
/// Nothing is written here, so what is in effect afterwards is what was in effect before; only a
/// test **this dialog** ran can be stale against it. A listing from anywhere else was fetched
/// against that same unchanged state and is left alone, which is what stops a glance at the
/// dialog costing a round trip.
fn discard(
    ctx: SettingsCtx,
    kind: ProviderKind,
    tested: State<Option<Tested>>,
    mut slot: State<Configuring>,
) {
    retract_if_stale(ctx, kind, tested.peek().clone(), &in_effect(ctx, kind));
    slot.set(None);
}
