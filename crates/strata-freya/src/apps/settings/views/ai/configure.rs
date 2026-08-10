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

use crate::apps::settings::views::ai::probe::{self, Ask, Probe, Tone};
use crate::apps::settings::views::ai::providers::will_have_key;
use crate::apps::settings::views::ai::row::mark;
use crate::apps::settings::SettingsCtx;
use crate::components::dialog::{Dialog, DialogHeader};
use crate::components::form::{form_theme, ValueField, FIELD_HEIGHT};
use crate::components::icon::IconName;
use crate::components::tones::{tones, Tones};
use crate::components::tool_button::ToolButton;
use crate::components::typography::{Body, Control, Eyebrow, Meta, Prose, Title};
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
                                let mut probes = ctx.probes;
                                if matches!(probes.peek().get(kind), Probe::Testing) {
                                    return;
                                }
                                probes.write().set(kind, Probe::Testing);
                                // The values in these boxes, not the applied ones — a test that
                                // proved something other than what is on screen would be worse
                                // than none.
                                let ask = Ask {
                                    kind,
                                    base_url: url_buf.peek().clone(),
                                    typed: Secret::new(&key_buf.peek()),
                                    stored: match key_buf.peek().trim().is_empty() {
                                        true => ctx
                                            .draft
                                            .peek()
                                            .ai
                                            .setup(kind)
                                            .and_then(|setup| setup.key.clone()),
                                        false => None,
                                    },
                                };
                                spawn(async move {
                                    let settled = probe::run(ask).await;
                                    let mut probes = ctx.probes;
                                    probes.write().set(kind, settled);
                                });
                            })
                            .child(Control::new("Test")),
                    )
                    .child(rect().width(Size::flex(1.)))
                    .child(status),
            );

        Dialog::new()
            .on_dismiss(move |()| slot.set(None))
            .header(DialogHeader::new(
                mark(kind),
                roles.get(Role::Accent),
                Title::new(format!("Configure {}", label(kind))),
            ))
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
                    .on_press(move |_| {
                        save(ctx, kind, &key_buf.peek(), &url_buf.peek());
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

/// **Write what the dialog holds into the window's editing state.**
///
/// Not the keystore and not the config file: Settings' Apply is the one commit point, and this
/// is a second gesture into the state it commits — the URL onto the draft, the key beside it.
///
/// A key box left **empty is not an edit**, which is the difference between this and the Remove
/// press: closing a dialog you only looked at must not queue a deletion of the key you came to
/// check on. Removing is explicit, and says so on its own button.
fn save(ctx: SettingsCtx, kind: ProviderKind, key: &str, url: &str) {
    if ctx.base_url_of(kind) != url {
        ctx.set_base_url(kind, url.to_string());
        let mut probes = ctx.probes;
        probes.write().forget(kind);
    }
    if !key.trim().is_empty() {
        let mut keys = ctx.ai_keys;
        keys.write().set(kind, key.to_string());
        let mut probes = ctx.probes;
        probes.write().forget(kind);
    }
}
