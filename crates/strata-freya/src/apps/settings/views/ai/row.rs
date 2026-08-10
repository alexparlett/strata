//! **One provider row** — the anatomy every kind in the table shares.
//!
//! ```text
//! ┌──────────────────────────────────────────────────────────┐
//! │ (◆)  Anthropic  API KEY                           ( o )  │  the switch, and only it
//! │      Falls back to ANTHROPIC_API_KEY                     │
//! ├ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ┤
//! │      KEY  ••••••••••••••••••           [eye]    [Test]   │  only while enabled
//! │      • connection verified, 12 models                    │
//! └──────────────────────────────────────────────────────────┘
//! ```
//!
//! **The switch is the only thing that toggles.** The header was a press target first, which put
//! the `Switch` inside it — and a built-in control's press reaches its ancestors (AGENTS.md §3),
//! so pressing the switch fired both and the row looked inert while pressing anywhere else
//! worked. The preferences form's answer is a label block *beside* the switch rather than around
//! it (`Row::trailing`), and that shape would work — but a row with nothing but a switch to press
//! reads more honestly than one where the whole strip is a target for it.
//!
//! **The disclosure carries information rather than chrome** (the canvas's own words): the
//! credential appears *because* the provider is on, so there is no accordion and no second
//! affordance to explain. A row that is off is a row with nothing to fill in.
//!
//! **The subline says what is known without asking, and never what the row is already showing.**
//! The canvas puts "N models · M reasoning" there, which is knowledge a request produces — so
//! before a Test it states a fact the row actually has, and the count replaces it once a probe
//! comes back. Only real facts.
//!
//! It also summarises a **closed** row rather than annotating an open one: with the credential
//! area drawn, an address in the subline sat directly above a box containing the same string
//! (`Runs locally at http://localhost:11434/` over `URL http://localhost:11434/`). Open, it says
//! only what the boxes cannot — see `providers::subline`.
//!
//! **A field the kind does not use is absent, not disabled.** What boxes a row draws is read
//! straight off its `PROVIDERS` entry — one key box, one URL box, or both for the
//! OpenAI-compatible kind — so a form that offered a field the table does not declare would not
//! compile.

use freya::prelude::*;
use strata_agent::assistant::{BaseUrl, KeyUse};
use strata_core::ai::ProviderKind;

use crate::apps::settings::views::ai::probe::{Probe, Tone};
use crate::apps::settings::{settings_theme, SettingsCtx, SettingsTheme};
use crate::components::form::{form_theme, ValueField, FIELD_HEIGHT};
use crate::components::icon::{Icon, IconName};
use crate::components::tones::{tones, Tones};
use crate::components::tool_button::ToolButton;
use crate::components::typography::{Body, Control, Eyebrow, Meta, Prose};

/// The canvas's mark tile: 34px square, and the same 34 the expanded area indents by so the
/// credential lines up under the name rather than under the tile.
const MARK: f32 = 34.;
/// Gap between the tile, the name block and the switch — and between the boxes below.
const GAP: f32 = 12.;
/// The canvas's row padding.
const PAD: f32 = 12.;
/// The brand mark inside the tile, at the canvas's optical size for a 34px square.
const MARK_GLYPH: f32 = 17.;

/// A 1px top-edge-only rule — the hairline *between* two rows of the list. Painted rather than
/// laid out, so the row's own padding is what keeps a child's background off it.
fn row_rule() -> BorderWidth {
    BorderWidth {
        top: 1.,
        right: 0.,
        bottom: 0.,
        left: 0.,
    }
}

/// **The brand mark for a kind** — the app's half of the provider table.
///
/// A match here rather than a column in `PROVIDERS`, because [`IconName`] is this crate's and the
/// table is `strata-agent`'s, which is Freya-free on purpose. That is the same seam the whole
/// design keeps: the *token* is shared, the *artwork* belongs to whoever draws. Exhaustive, so a
/// kind added to the table without a mark is a build error rather than a blank tile.
///
/// The OpenAI-compatible kind has no brand to carry, so it takes the app's own connection glyph —
/// which is what it is: a remote endpoint the user pointed at.
pub fn mark(kind: ProviderKind) -> IconName {
    match kind {
        ProviderKind::Anthropic => IconName::ProviderAnthropic,
        ProviderKind::OpenAi => IconName::ProviderOpenAi,
        ProviderKind::Gemini => IconName::ProviderGemini,
        ProviderKind::DeepSeek => IconName::ProviderDeepSeek,
        ProviderKind::Groq => IconName::ProviderGroq,
        ProviderKind::Xai => IconName::ProviderXai,
        ProviderKind::Ollama => IconName::ProviderOllama,
        ProviderKind::OpenAiCompatible => IconName::Connections,
    }
}

/// Which credential boxes a row draws, resolved from the kind's table entry.
///
/// A type rather than two booleans at the call site: "a key box" and "a URL box" are not
/// independent — the three shapes this can take are the three the table declares, and a fourth
/// (neither) is a row with nothing to configure, which no kind we offer is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Boxes {
    pub key: bool,
    pub url: bool,
}

impl Boxes {
    /// What the kind's `PROVIDERS` row admits. **Read, never restated**: a kind whose key policy
    /// changes changes what this pane draws, in one place.
    pub fn of(key: KeyUse, url: BaseUrl) -> Boxes {
        Boxes {
            key: !matches!(key, KeyUse::Unused),
            url: !matches!(url, BaseUrl::Provider),
        }
    }
}

/// **`OPTIONAL` beside the key box** — the form's own marker, reused.
///
/// `Row::required` already renders exactly this note for a settings row, in the form theme's
/// own colour, and inventing a second way to say it here would be a label line that drifts from
/// every other one in the app. This row is not a `form::Row` (its boxes carry inline `KEY`/`URL`
/// eyebrows rather than titled label lines), so it borrows the *marker* rather than the row.
///
/// **Outside the box, never in its leading run.** Put beside the inline eyebrow it read as part
/// of the label — `KEY OPTIONAL paste API key` — which is a compound field name rather than a
/// note about the field. The form puts the marker next to the control, and this is that: a
/// sibling on the box's own line.
///
/// **Only the key carries one, and only on the anonymous kind.** The three other answers are all
/// "the marker would be noise":
///
/// - a `REQUIRED` on the OpenAI-compatible URL was built and cut. It is the only control on its
///   line, its row cannot work without it, and the subline already says `No address set` — three
///   ways of telling the user the same thing, two of which they did not ask for;
/// - an `Editable` URL has the kind's default behind it, so required is simply false;
/// - and an `Env` key falls back to the variable the subline names.
///
/// What is left is the one real ambiguity, and the one this started from: the
/// OpenAI-compatible key box, where an empty value is not an oversight — an empty bearer is what a local runtime
/// expects and what a real one answers 401 to.
///
/// The colour is passed in rather than read here: `form_theme()` is a **hook**, and this is
/// called from inside a `then(…)` closure — a theme read that happens only when the marker is
/// drawn is a variable number of hooks per render, which corrupts hook order (AGENTS.md §3). The
/// render body reads it once, unconditionally, and hands it down.
fn key_marker(key: KeyUse, color: Color) -> Option<Element> {
    matches!(key, KeyUse::Anonymous).then(|| Element::from(Meta::new("OPTIONAL").color(color)))
}

/// One row of the providers list.
///
/// Every field is a value or a handler — the row owns no state of its own, because what it shows
/// lives in the settings draft and what it has proved lives in the window's probe map. A row
/// that kept its own copy of either would be a second answer to a question the pane already has
/// one for.
#[derive(PartialEq)]
pub struct ProviderRow {
    /// The provider's brand mark ([`mark`]), or the connection glyph for the compatible kind.
    pub mark: IconName,
    /// What the provider is called — the kind's label, from the table.
    pub name: String,
    /// The uppercase badge beside the name — `API KEY`, `LOCAL` or `CUSTOM`, naming what
    /// credential the row takes. Read off the kind's key policy, so it cannot disagree with the
    /// boxes the row then draws.
    pub badge: &'static str,
    /// What this row knows about itself without having asked anything. `None` where an open row
    /// already shows it — the subline summarises a collapsed row, and repeating the address it
    /// draws in a box directly below is the same sentence twice.
    pub subline: Option<String>,
    pub enabled: bool,
    /// Whether this is the first row, which draws no top rule.
    pub first: bool,
    pub boxes: Boxes,
    /// Which provider this row edits — what its writes are keyed by, and what a Test probes.
    pub kind: ProviderKind,
    /// The window's editing state. Held rather than handed a pile of callbacks, on the engine
    /// row's precedent (`PropRow` takes `rows` + `id`): a text box's write has to be **guarded**
    /// against the value already there, and the guard needs to read what is there *now* — which
    /// a captured `EventHandler` cannot, since its closure is built once.
    pub ctx: SettingsCtx,
    /// What the key box shows — the typed-but-uncommitted key. **Never a stored one**: a secret
    /// in the keystore is not something a form reads back to redisplay.
    pub key_text: String,
    /// The kind's key policy — what decides whether a key box is drawn at all, and whether it is
    /// marked `OPTIONAL`. Handed over as the **policy** rather than as the presentation it
    /// implies, so the two answers cannot drift apart at the call site.
    pub key_use: KeyUse,
    /// **A key is stored and the user has not asked to change it** — so the row states that and
    /// offers Replace rather than drawing a box it can never fill.
    ///
    /// False the moment Replace is pressed, and false when nothing is stored: both mean "there is
    /// a key to *set* here", which is the only state an input belongs in.
    pub key_settled: bool,
    /// What the empty box invites, once there is a box. Two answers, because setting a first key
    /// and replacing one are different acts — and only the second can also delete.
    pub key_placeholder: &'static str,
    pub key_shown: bool,
    /// Press Replace: stop treating the stored key as settled and open an empty box for a new
    /// one. Leaving that box empty is how the stored key is removed.
    pub on_replace: EventHandler<()>,
    pub url_text: String,
    pub url_placeholder: &'static str,
    pub probe: Probe,
    pub on_toggle: EventHandler<()>,
    pub on_reveal: EventHandler<()>,
    pub on_test: EventHandler<()>,
}

impl Component for ProviderRow {
    fn render(&self) -> impl IntoElement {
        let theme = settings_theme();
        let tones = tones();
        // Read once and unconditionally — see `marker`.
        let required_color = form_theme().required_color;

        // **Two buffers, always two.** `ValueField` binds a `State<String>`, so a text box
        // needs one — and a *variable* number of them per render would corrupt hook order, which
        // is why each row is its own `Component` rather than a helper the pane calls in a loop
        // (`components::form::options` settled this). A row draws at most two boxes and always
        // takes two buffers, whether or not it draws them.
        let mut key_buf = use_state({
            let seed = self.key_text.clone();
            move || seed
        });
        // **The key box follows its prop, where the other two only seed from theirs.**
        //
        // A successful Apply clears the typed keys — they are in the keystore now — and a box
        // still holding the pasted text would be written straight back by the effect below,
        // undoing the clear and re-`put`ting the key on the next Apply. `set_if_modified` is what
        // keeps this from fighting the user: while typing, the prop is derived from the buffer
        // and already equal, so this is a no-op until something *else* changes the value.
        let committed = use_reactive(&self.key_text);
        use_side_effect(move || key_buf.set_if_modified(committed.read().clone()));
        let url_buf = use_state({
            let seed = self.url_text.clone();
            move || seed
        });
        let ctx = self.ctx;
        let kind = self.kind;
        // Push each box into the window's state on every keystroke, so what Apply commits is
        // what is on screen. **Guarded** with `peek`, or the write wakes this row, whose effect
        // runs again and costs a second pass per keystroke — the engine grid's own lesson.
        //
        // Both writes retract the probe in the same breath: a "verified" beside a credential
        // that has since been retyped describes a request nobody would make now.
        use_side_effect(move || {
            let typed = key_buf.read().clone();
            let mut keys = ctx.ai_keys;
            let mut probes = ctx.probes;
            if keys.peek().get(kind) != typed.as_str() {
                keys.write().set(kind, typed);
                probes.write().forget(kind);
            }
        });
        use_side_effect(move || {
            let typed = url_buf.read().clone();
            let mut probes = ctx.probes;
            if ctx.base_url_of(kind) != typed {
                ctx.set_base_url(kind, typed);
                probes.write().forget(kind);
            }
        });

        let mark_color = match self.enabled {
            true => theme.selected_color,
            false => theme.mark_color,
        };

        let header = rect()
            .width(Size::fill())
            .horizontal()
            .cross_align(Alignment::Center)
            .spacing(GAP)
            .padding(PAD)
            .content(Content::Flex)
            // **No press on the header, and the switch is the only thing that toggles.**
            //
            // A press here reached the `Switch` too — a built-in control's press reaches its
            // ancestors (AGENTS.md §3), so pressing the switch fired both and the row appeared
            // not to respond, while pressing anywhere else fired one and it did. Two behaviours
            // for one gesture.
            //
            // The preferences form solves this by making the label block a **sibling** of the
            // switch rather than its parent (`Row::trailing`), and that shape is available here.
            // It is not taken: this header carries a mark, a name, a badge and a subline, and a
            // strip that size reading as one big press target invites the press it then answers
            // ambiguously. The switch is small, obvious, and the only thing that acts.
            .child(
                rect()
                    .width(Size::px(MARK))
                    .height(Size::px(MARK))
                    .corner_radius(6.)
                    .background(theme.mark_background)
                    .border(Border::new().width(1.).fill(theme.card_border_fill))
                    .main_align(Alignment::Center)
                    .cross_align(Alignment::Center)
                    .child(Icon::new(self.mark).size(MARK_GLYPH).color(mark_color)),
            )
            .child(
                rect()
                    .width(Size::flex(1.))
                    .spacing(2.)
                    .child(
                        rect()
                            .horizontal()
                            .cross_align(Alignment::Center)
                            .spacing(8.)
                            .child(Control::new(self.name.clone()))
                            .child(
                                rect()
                                    .padding(Gaps::new(1., 5., 1., 5.))
                                    .corner_radius(3.)
                                    .background(theme.mark_background)
                                    .border(Border::new().width(1.).fill(theme.card_border_fill))
                                    .child(
                                        Eyebrow::new(self.badge).color(theme.badge_builtin_color),
                                    ),
                            ),
                    )
                    .maybe_child(
                        self.subline
                            .clone()
                            .map(|said| Prose::new(said).color(theme.hint_color)),
                    ),
            )
            .child(Switch::new().toggled(self.enabled).on_toggle({
                let toggle = self.on_toggle.clone();
                move |()| toggle.call(())
            }));

        let credentials = self
            .enabled
            .then(|| self.credentials(key_buf, url_buf, &theme, tones, required_color));

        rect()
            .width(Size::fill())
            .maybe(!self.first, |el| {
                el.border(
                    Border::new()
                        .width(row_rule())
                        .fill(theme.card_divider_fill),
                )
            })
            .child(header)
            .maybe_child(credentials)
    }
}

impl ProviderRow {
    /// **The credential strip** — the boxes, the actions on them, and what the last Test said.
    ///
    /// Its own body rather than more of `render`, which was over the line budget and reads as two
    /// things anyway: the header is what the row *is*, and this is what it takes. No hooks live
    /// here — the two buffers are made in `render` and handed down, so this stays a plain
    /// function of its inputs and the hook count per render is fixed whether or not a row is
    /// enabled.
    ///
    /// Indented past the mark so the boxes sit under the name — the canvas's `sp-4 + 34 + sp-4`.
    fn credentials(
        &self,
        key_buf: State<String>,
        url_buf: State<String>,
        theme: &SettingsTheme,
        tones: Tones,
        required_color: Color,
    ) -> Element {
        let reveal_label = match self.key_shown {
            true => "Hide the key",
            false => "Show the key",
        };
        let status = self.probe.status().map(|(tone, said)| {
            let color = match tone {
                Tone::Working => theme.hint_color,
                Tone::Good => tones.ok,
                Tone::Bad => tones.error,
            };
            rect()
                .horizontal()
                .cross_align(Alignment::Center)
                .spacing(6.)
                .child(
                    rect()
                        .width(Size::px(6.))
                        .height(Size::px(6.))
                        .corner_radius(3.)
                        .background(color),
                )
                .child(Body::new(said).color(color))
        });

        rect()
            .width(Size::fill())
            .spacing(8.)
            .padding(Gaps::new(0., PAD, PAD, PAD + MARK + GAP))
            .maybe_child(self.boxes.url.then(|| {
                ValueField::new(url_buf)
                    .width(Size::fill())
                    .placeholder(self.url_placeholder)
                    .leading(Eyebrow::new("URL").color(theme.hint_color))
            }))
            .child(
                rect()
                    .width(Size::fill())
                    .horizontal()
                    .cross_align(Alignment::Center)
                    .spacing(8.)
                    .content(Content::Flex)
                    // **A stored key is stated, not offered as an empty box.**
                    //
                    // The value is never read back (`strata_core::secret`: a form does not
                    // redisplay a secret, and reading one per row on open would be a blocking
                    // Keychain call — an authorisation prompt just to *look* at Settings). An
                    // always-present input promises the opposite: that it shows the current value
                    // and you may edit it. It cannot, so it read as a key that failed to save,
                    // beside an eye that revealed nothing.
                    //
                    // So the input appears only when there is a key to *set*. With one stored the
                    // row says so and offers Replace, which is also how a key is removed — the
                    // box it opens is empty, and an empty box committed is a delete
                    // (`Secret::new` answers blank with `None`). One gesture, and the placeholder
                    // says both halves out loud.
                    .maybe_child((self.boxes.key && self.key_settled).then(|| {
                        rect()
                            .width(Size::flex(1.))
                            .horizontal()
                            .cross_align(Alignment::Center)
                            .spacing(8.)
                            .child(Eyebrow::new("KEY").color(theme.hint_color))
                            .child(Body::new("Stored").color(theme.hint_color))
                    }))
                    .maybe_child((self.boxes.key && self.key_settled).then(|| {
                        Button::new()
                            .outline()
                            .height(Size::px(FIELD_HEIGHT))
                            .on_press({
                                let replace = self.on_replace.clone();
                                move |_: Event<PressEventData>| replace.call(())
                            })
                            .child(Control::new("Replace"))
                    }))
                    .maybe_child((self.boxes.key && !self.key_settled).then(|| {
                        ValueField::new(key_buf)
                            .width(Size::flex(1.))
                            .masked(!self.key_shown)
                            .placeholder(self.key_placeholder)
                            .leading(Eyebrow::new("KEY").color(theme.hint_color))
                    }))
                    .maybe_child(
                        (!self.key_settled)
                            .then(|| key_marker(self.key_use, required_color))
                            .flatten(),
                    )
                    // A row with no key box still needs its Test to sit at the trailing
                    // edge, so the spacer is the flexing child instead.
                    .maybe_child((!self.boxes.key).then(|| rect().width(Size::flex(1.))))
                    // The eye only exists where there is something to reveal — a stored key is
                    // not shown at all, so a button offering to unmask it would do nothing.
                    .maybe_child((self.boxes.key && !self.key_settled).then(|| {
                        ToolButton::new(IconName::Eye, reveal_label)
                            .outlined()
                            .on_press({
                                let reveal = self.on_reveal.clone();
                                EventHandler::new(move |_: Event<PressEventData>| {
                                    reveal.call(());
                                })
                            })
                    }))
                    .child(
                        Button::new()
                            .outline()
                            .height(Size::px(FIELD_HEIGHT))
                            // A test in flight is not a second thing to start. Gated
                            // rather than `interactive(false)`, which strands the hover.
                            .on_press({
                                let test = self.on_test.clone();
                                let busy = matches!(self.probe, Probe::Testing);
                                move |_: Event<PressEventData>| {
                                    if !busy {
                                        test.call(());
                                    }
                                }
                            })
                            .child(Control::new("Test")),
                    ),
            )
            .maybe_child(status)
            .into()
    }
}
