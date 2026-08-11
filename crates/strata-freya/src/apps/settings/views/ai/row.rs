//! **One provider row** — one line, whatever the kind takes.
//!
//! ```text
//! ┌──────────────────────────────────────────────────────────┐
//! │ (◆)  Anthropic  API KEY                     [⚙]   ( o )  │
//! │      A key is stored                                     │
//! └──────────────────────────────────────────────────────────┘
//! ```
//!
//! **The credential is not on the row.** It was, as a disclosure that opened when the provider
//! was switched on — and the input in it could never show the stored value, because a form does
//! not redisplay a secret. So a configured provider opened with an empty box beside an eye that
//! revealed nothing, which reads as a key that failed to save. An always-present input promises
//! it shows the current value; a write-only credential cannot keep that promise, so the input
//! moved to a dialog that makes no such claim (`configure`), and the row kept its one line.
//!
//! What is left is what a row can honestly hold: what the provider is, whether it is on, and one
//! line of what is known about it.
//!
//! **The switch is the only thing that toggles.** The header was a press target first, which put
//! the `Switch` inside it — and a built-in control's press reaches its ancestors (AGENTS.md §3),
//! so pressing the switch fired both and the row looked inert while pressing anywhere else
//! worked. The preferences form's answer is a label block *beside* the switch rather than around
//! it (`Row::trailing`), and that shape would work — but a row carrying a second button reads
//! more honestly with neither of them swallowing a press meant for the other.
//!
//! **The subline says what is known without asking.** The canvas puts "N models · M reasoning"
//! there, which is knowledge a request produces — so before a Test it states a fact the row
//! actually has, and the count replaces it once a probe comes back. Only real facts; see
//! `providers::subline`.

use freya::prelude::*;
use strata_core::ai::ProviderKind;

use crate::apps::settings::{settings_theme, SettingsTheme};
use crate::components::icon::{Icon, IconName};
use crate::components::tool_button::ToolButton;
use crate::components::typography::{Control, Eyebrow, Prose};

/// The canvas's mark tile: a 34px square.
const MARK: f32 = 34.;
/// Gap between the tile, the name block and the controls.
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

/// One row of the providers list.
///
/// Every field is a value or a handler — the row owns no state at all now that editing happens in
/// the dialog. What it shows lives in the settings draft and the window's probe map, and a row
/// that kept its own copy of either would be a second answer to a question the pane already has
/// one for.
#[derive(PartialEq)]
pub struct ProviderRow {
    /// The provider's brand mark ([`mark`]).
    pub mark: IconName,
    /// What the provider is called — the kind's label, from the table.
    pub name: String,
    /// The uppercase badge beside the name — `API KEY`, `LOCAL` or `CUSTOM`, naming what
    /// credential the row takes. Read off the kind's key policy, so it cannot disagree with what
    /// the dialog then asks for.
    pub badge: &'static str,
    /// What this row knows about itself without having asked anything.
    pub subline: Option<String>,
    pub enabled: bool,
    /// Whether this is the first row, which draws no top rule.
    pub first: bool,
    pub on_toggle: EventHandler<()>,
    /// Open the configure dialog. Also reached by switching a provider **on** that has nothing
    /// configured yet — turning something on that cannot answer is a dead end, so the question
    /// comes to the user rather than waiting to be found.
    pub on_configure: EventHandler<()>,
}

impl Component for ProviderRow {
    fn render(&self) -> impl IntoElement {
        let theme = settings_theme();

        let mark_color = match self.enabled {
            true => theme.selected_color,
            false => theme.mark_color,
        };

        rect()
            .width(Size::fill())
            .maybe(!self.first, |el| {
                el.border(
                    Border::new()
                        .width(row_rule())
                        .fill(theme.card_divider_fill),
                )
            })
            .child(
                rect()
                    .width(Size::fill())
                    .horizontal()
                    .cross_align(Alignment::Center)
                    .spacing(GAP)
                    .padding(PAD)
                    .content(Content::Flex)
                    .child(tile(self.mark, mark_color, &theme))
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
                                    .child(badge(self.badge, &theme)),
                            )
                            .maybe_child(
                                self.subline
                                    .clone()
                                    .map(|said| Prose::new(said).color(theme.hint_color)),
                            ),
                    )
                    .child(
                        ToolButton::new(IconName::Gear, "Configure this provider")
                            .outlined()
                            .on_press({
                                let configure = self.on_configure.clone();
                                EventHandler::new(move |_: Event<PressEventData>| {
                                    configure.call(());
                                })
                            }),
                    )
                    .child(Switch::new().toggled(self.enabled).on_toggle({
                        let toggle = self.on_toggle.clone();
                        move |()| toggle.call(())
                    })),
            )
    }
}

/// The mark tile — a raised square carrying the provider's glyph, accented while it is on.
fn tile(glyph: IconName, color: Color, theme: &SettingsTheme) -> Element {
    rect()
        .width(Size::px(MARK))
        .height(Size::px(MARK))
        .corner_radius(6.)
        .background(theme.mark_background)
        .border(Border::new().width(1.).fill(theme.card_border_fill))
        .main_align(Alignment::Center)
        .cross_align(Alignment::Center)
        .child(Icon::new(glyph).size(MARK_GLYPH).color(color))
        .into()
}

fn badge(text: &'static str, theme: &SettingsTheme) -> Element {
    rect()
        .padding(Gaps::new(1., 5., 1., 5.))
        .corner_radius(3.)
        .background(theme.mark_background)
        .border(Border::new().width(1.).fill(theme.card_border_fill))
        .child(Eyebrow::new(text).color(theme.badge_builtin_color))
        .into()
}
