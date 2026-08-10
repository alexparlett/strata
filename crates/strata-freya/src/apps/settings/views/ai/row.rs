//! **One provider row** — the anatomy the built-in kinds and the custom endpoints share.
//!
//! ```text
//! ┌──────────────────────────────────────────────────────────┐
//! │ (◆)  Anthropic  API KEY                           ( o )  │  the switch, and only it
//! │      Falls back to ANTHROPIC_API_KEY                     │
//! ├ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ┤
//! │      KEY  ••••••••••••••••••           [eye]    [Test]   │  only while enabled
//! │      • connection verified, 12 models                    │
//! └──────────────────────────────────────────────────────────┘
//!
//! ┌──────────────────────────────────────────────────────────┐
//! │ (☁)  [ Workstation llama.cpp ]              [🗑]  ( o )  │  the name is a box here
//! │      http://localhost:8080/v1/                           │  …only while it is closed
//! └──────────────────────────────────────────────────────────┘
//! ```
//!
//! **The switch is the only thing that toggles.** The header was a press target first, which put
//! the `Switch` inside it — and a built-in control's press reaches its ancestors (AGENTS.md §3),
//! so pressing the switch fired both and the row looked inert while pressing anywhere else
//! worked. The preferences form's answer is a label block *beside* the switch rather than around
//! it (`Row::trailing`), but that is not right here either: a custom endpoint's name is a box in
//! this header, and a text field inside a press target toggles the row every time you click in
//! to rename it.
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
//! straight off its `PROVIDERS` entry — one key box, one URL box, or both for a custom
//! endpoint — so a form that offered a field the table does not declare would not compile.

use freya::prelude::*;
use strata_agent::assistant::{BaseUrl, KeyUse};
use strata_core::ai::{BrainRef, ProviderKind};

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
/// A custom endpoint's name box. Wide enough for a descriptive name ("Workstation llama.cpp")
/// and bounded, so the badge beside it sits still instead of drifting with the text.
const NAME_WIDTH: f32 = 220.;

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
/// A custom endpoint has no brand to carry, so it takes the app's own connection glyph — which is
/// what it is: a remote endpoint the user pointed at.
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
/// - a `REQUIRED` on a custom endpoint's URL was built and cut. It is the only control on its
///   line, its row cannot work without it, and the subline already says `No address set` — three
///   ways of telling the user the same thing, two of which they did not ask for;
/// - an `Editable` URL has the kind's default behind it, so required is simply false;
/// - and an `Env` key falls back to the variable the subline names.
///
/// What is left is the one real ambiguity, and the one this started from: a custom endpoint's
/// key box, where an empty value is not an oversight — an empty bearer is what a local runtime
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
    /// The provider's brand mark ([`mark`]), or the connection glyph for a custom endpoint.
    pub mark: IconName,
    /// What the provider is called: the kind's label, or the user's name for an endpoint.
    pub name: String,
    /// Whether that name is the **user's** to change — true for a custom endpoint, false for a
    /// built-in, whose name is its table row's and is not a thing to rename.
    pub renameable: bool,
    /// The uppercase badge beside the name — `API KEY` or `LOCAL`, naming what credential the
    /// row takes. `None` where that adds nothing: a custom endpoint sits under a heading that
    /// already says what it is.
    pub badge: Option<&'static str>,
    /// What this row knows about itself without having asked anything. `None` where an open row
    /// already shows it — the subline summarises a collapsed row, and repeating the address it
    /// draws in a box directly below is the same sentence twice.
    pub subline: Option<String>,
    pub enabled: bool,
    /// Whether this is the first row, which draws no top rule.
    pub first: bool,
    pub boxes: Boxes,
    /// Which brain this row edits — what its writes are keyed by, and what a Test probes.
    pub brain: BrainRef,
    /// The window's editing state. Held rather than handed a pile of callbacks, on the engine
    /// row's precedent (`PropRow` takes `rows` + `id`): a text box's write has to be **guarded**
    /// against the value already there, and the guard needs to read what is there *now* — which
    /// a captured `EventHandler` cannot, since its closure is built once.
    pub ctx: SettingsCtx,
    /// What the key box shows — the typed-but-uncommitted key. **Never a stored one**: a secret
    /// in the keystore is not something a form reads back to redisplay, so an already-configured
    /// provider opens with an empty box and a subline that says a key is stored.
    pub key_text: String,
    /// The kind's key policy — what decides whether a key box is drawn at all, and whether it is
    /// marked `OPTIONAL`. Handed over as the **policy** rather than as the presentation it
    /// implies, so the two answers cannot drift apart at the call site.
    pub key_use: KeyUse,
    pub key_shown: bool,
    pub url_text: String,
    pub url_placeholder: &'static str,
    pub probe: Probe,
    pub on_toggle: EventHandler<()>,
    pub on_reveal: EventHandler<()>,
    pub on_test: EventHandler<()>,
    /// Removing a custom endpoint. `None` for a built-in, which is not the user's to delete —
    /// its absence *is* the difference between the two lists, so it is a missing handler rather
    /// than a disabled button.
    pub on_remove: Option<EventHandler<()>>,
}

impl Component for ProviderRow {
    fn render(&self) -> impl IntoElement {
        let theme = settings_theme();
        let tones = tones();
        // Read once and unconditionally — see `marker`.
        let required_color = form_theme().required_color;

        // **Three buffers, always three.** `ValueField` binds a `State<String>`, so a text box
        // needs one — and a *variable* number of them per render would corrupt hook order, which
        // is why each row is its own `Component` rather than a helper the pane calls in a loop
        // (`components::form::options` settled this). A row draws at most three boxes and always
        // takes three buffers, whether or not it draws them.
        let key_buf = use_state({
            let seed = self.key_text.clone();
            move || seed
        });
        let url_buf = use_state({
            let seed = self.url_text.clone();
            move || seed
        });
        let name_buf = use_state({
            let seed = self.name.clone();
            move || seed
        });

        let ctx = self.ctx;
        let brain = self.brain;
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
            if keys.peek().get(&brain) != typed.as_str() {
                keys.write().set(brain, typed);
                probes.write().forget(&brain);
            }
        });
        use_side_effect(move || {
            let typed = url_buf.read().clone();
            let mut probes = ctx.probes;
            if ctx.base_url_of(&brain) != typed {
                ctx.set_base_url(&brain, typed);
                probes.write().forget(&brain);
            }
        });
        // Renaming is **not** a credential change, so it retracts nothing: what the endpoint is
        // called has no bearing on whether the last request reached it.
        //
        // A built-in has no name in the draft at all (`name_of` is `None`), and no `set_name` to
        // reach — its name is its table row's. Skipping outright rather than leaning on
        // `set_name`'s own no-op keeps this guard from being permanently unsatisfied, which is
        // what it was: `None != Some("Anthropic")` on every run, forever.
        use_side_effect(move || {
            let typed = name_buf.read().clone();
            let Some(named) = ctx.name_of(&brain) else {
                return;
            };
            if named != typed {
                ctx.set_name(&brain, typed);
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
            // switch rather than its parent (`Row::trailing`), and that shape is available — but
            // it is not what this row wants either: a custom endpoint's name is an editable box
            // in this header, and a name field inside a press target is a row that toggles every
            // time you click into the box to rename it.
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
                            // **A custom endpoint is named by its user; a built-in is named by
                            // the table.** So one is a box and the other is a label — and the box
                            // is in the *header*, not in the credential area below, because a
                            // disabled endpoint has to be renameable too and that area only
                            // exists while the row is on.
                            .child(match self.renameable {
                                // **A box, not a bare run.** `bare()` is the engine grid's dress,
                                // where the *cell* is what says "you can type here"; there is no
                                // cell around this one, so bare read as a label and nobody would
                                // guess it could be renamed.
                                true => ValueField::new(name_buf)
                                    .width(Size::px(NAME_WIDTH))
                                    .height(Size::px(FIELD_HEIGHT))
                                    .placeholder("Name this endpoint")
                                    .into(),
                                false => Element::from(Control::new(self.name.clone())),
                            })
                            // The badge names what a row's credential *is* — `API KEY` or
                            // `LOCAL` — which is a real distinction between the built-ins. A
                            // custom endpoint carries none: it is under a heading that already
                            // says so, and a badge repeating its own section is furniture.
                            .maybe_child(self.badge.map(|badge| {
                                rect()
                                    .padding(Gaps::new(1., 5., 1., 5.))
                                    .corner_radius(3.)
                                    .background(theme.mark_background)
                                    .border(Border::new().width(1.).fill(theme.card_border_fill))
                                    .child(Eyebrow::new(badge).color(theme.badge_builtin_color))
                            })),
                    )
                    .maybe_child(
                        self.subline
                            .clone()
                            .map(|said| Prose::new(said).color(theme.hint_color)),
                    ),
            )
            // **Remove belongs to the row, so it sits on the row's own line.**
            //
            // It was at the end of the credential strip, after Test — where it read as an action
            // on the key box it was beside rather than on the endpoint, which is a destructive
            // gesture mislabelled by its position. Up here it is next to the switch, and what
            // both act on is unambiguous: the thing this row is.
            //
            // To the *left* of the switch, not the right: every row in both lists carries a
            // switch at the trailing edge, and the eye reads that column. Putting remove last on
            // the rows that have one would step the switches in and out.
            .maybe_child(self.on_remove.clone().map(|remove| {
                ToolButton::new(IconName::Trash, "Remove this endpoint")
                    .outlined()
                    .color(tones.error)
                    .on_press(EventHandler::new(move |_: Event<PressEventData>| {
                        remove.call(());
                    }))
            }))
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
                    .maybe_child(self.boxes.key.then(|| {
                        ValueField::new(key_buf)
                            .width(Size::flex(1.))
                            .masked(!self.key_shown)
                            .placeholder("paste API key")
                            .leading(Eyebrow::new("KEY").color(theme.hint_color))
                    }))
                    .maybe_child(key_marker(self.key_use, required_color))
                    // A row with no key box still needs its Test to sit at the trailing
                    // edge, so the spacer is the flexing child instead.
                    .maybe_child((!self.boxes.key).then(|| rect().width(Size::flex(1.))))
                    .maybe_child(self.boxes.key.then(|| {
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
