//! `@`-mentions — **two ways to attach the same things**, over one list.
//!
//! - [`MentionPicker`] is inline completion: typing `@` opens a list above the field and every
//!   further character narrows it, the way an `@` behaves everywhere else that has one.
//! - [`AttachPicker`] is the `+` in the composer bar: the same offers with a search box and a
//!   scrolling body, for when you know you want to attach something but not what it is called.
//!
//! Both read [`offers`], so the two surfaces cannot drift about what may be mentioned or how a
//! name is matched.
//!
//! ## The inline list is driven from the field, not from itself
//!
//! [`Mentions`] is that state, and the composer's `Input` hands it the arrow keys, Enter/Tab and
//! Escape. The field never gives up focus: a focused `Input` owns the keyboard (AGENTS.md §3), so
//! a list holding it would be a list nothing can narrow. A press in the list and Enter on it are
//! one method ([`Mentions::accept`]) for the same reason ⌘S and typed view DDL are one funnel.
//!
//! ## A completion is an edit at the caret, not at the end of the buffer
//!
//! [`token`] reads the `@…` run the caret sits in, and hands back its **span** — the same shape
//! as the SQL editor's `CompletionItem::replace`, and accepted the same way: replace that span,
//! put the caret after what was inserted (`CodeEditorData::replace_range`). One rule, two
//! surfaces, rather than a second answer to the same question.
//!
//! This is what `Input::caret` in the fork exists for. The first version had no caret to read, so
//! the token was pinned to the end of the buffer and a mention typed mid-sentence stopped
//! completing the moment the sentence carried on past it. An `@` the user has typed *past* still
//! stops matching — whitespace between the `@` and the caret ends it — because that is when they
//! meant it as prose.
//!
//! ## What can be mentioned
//!
//! The catalog **from the store** (AGENTS.md §2 — the catalog *is* `ProjectState`, never a
//! query), plus the query tab the user is looking at. Not a settled result: its rows live in that
//! run's own query entry, which no store here can read — the results toolbar pins that one,
//! because it is the surface that has it.

use std::ops::Range;

use freya::components::{Menu, MenuButton, ScrollView};
use freya::prelude::*;
use freya::radio::use_radio;
use strata_core::util::contains_lowercased;
use strata_model::CatalogKind;

use super::ChatTheme;
use crate::apps::project::state::{
    Anchor, Chan, ChatId, ChatsCtx, ProjChan, ProjectState, SessionState,
};
use crate::components::icon::{Icon, IconName};
use crate::components::metrics::{HAIRLINE, R_2, SP_2, SP_3};
use crate::components::tool_button::ToolButton;
use crate::components::typography::{InputTypography, Meta};

const MENU_W: f32 = 250.;
const MENU_ROW_CHROME: f32 = 44.;
/// How many completions the **inline** list offers at once. A catalog can be long and a
/// completion list is not a browser: typing one more character is the faster narrowing, and the
/// count is what keeps the popup from covering the transcript it is meant to sit over.
const INLINE_ROWS: usize = 8;
/// How tall the **attach** popup's body may grow before it scrolls. That one *is* a browser — it
/// is what you press when you do not know the name — so it is bounded by height and scrolls,
/// rather than by a count that would hide the tail with nothing to say so.
const ATTACH_BODY_H: f32 = 260.;

/// **The mention being typed at the caret**: what narrows the list, and what an accept replaces.
///
/// The same shape as the SQL editor's `CompletionItem::replace` (`strata-code-editor`), and for
/// the same reason: a completion is an edit to *a span*, not to the end of the buffer. Offsets are
/// **bytes** into the buffer.
#[derive(Debug, PartialEq)]
pub struct Token<'a> {
    /// The whole tag, `@` included: from the `@` to the end of the run it starts. This is what
    /// [`complete`] replaces, so accepting inside a half-typed name rewrites that name rather
    /// than splicing into the middle of it.
    pub span: Range<usize>,
    /// What has been typed between the `@` and the caret. Everything after the caret is part of
    /// the tag but not part of the question being asked, exactly as in an IDE.
    pub typed: &'a str,
}

/// The mention at `caret` (a byte offset), or `None` when there is not one.
///
/// An `@` qualifies when it starts a word and nothing between it and the caret is whitespace, so
/// an `@` the user typed past is prose, an address is not a mention, and an abandoned one stops
/// matching the moment they type a space.
pub fn token(text: &str, caret: usize) -> Option<Token<'_>> {
    let caret = caret.min(text.len());
    let before = &text[..caret];
    let at = before.rfind('@')?;
    let starts_word = at == 0
        || before[..at]
            .chars()
            .next_back()
            .is_some_and(char::is_whitespace);
    let typed = &before[at + 1..];
    if !starts_word || typed.chars().any(char::is_whitespace) {
        return None;
    }
    // The tail of the tag past the caret belongs to the span but not to the question: the user is
    // editing a name that is already there.
    let tail = text[caret..]
        .find(char::is_whitespace)
        .unwrap_or(text.len() - caret);
    Some(Token {
        span: at..caret + tail,
        typed,
    })
}

/// Replace `span` with `@name`, and say where the caret lands: **after what was inserted**, which
/// is the rule `CodeEditorData::replace_range` already holds the editor to.
///
/// A trailing space, so the next word is prose rather than more of the mention — unless there is
/// already one there, which is the ordinary case for a mention completed mid-sentence.
pub fn complete(text: &str, span: Range<usize>, name: &str) -> (String, usize) {
    let spaced = !text[span.end..].starts_with(|c: char| c.is_whitespace());
    let insert = match spaced {
        true => format!("@{name} "),
        false => format!("@{name}"),
    };
    let caret = span.start + insert.len();
    let mut completed = String::with_capacity(text.len() + insert.len());
    completed.push_str(&text[..span.start]);
    completed.push_str(&insert);
    completed.push_str(&text[span.end..]);
    (completed, caret)
}

/// **The mention to complete right now**: the token at the caret, unless that same token is the
/// one put away.
///
/// `dismissed` names a token by the offset of its `@`, so Escape silences the mention it was
/// pressed on and no other.
pub fn asking(text: &str, caret: usize, dismissed: Option<usize>) -> Option<Token<'_>> {
    token(text, caret).filter(|token| dismissed != Some(token.span.start))
}

/// A byte offset in `text` as a **UTF-16 code-unit** offset, and back.
///
/// The unit `Input` publishes its caret in (`freya_edit`'s offsets are UTF-16 code units,
/// like the SQL editor's), converted once at that boundary so nothing above it counts in two
/// units at the same time.
pub fn utf16_of(text: &str, byte: usize) -> usize {
    text[..byte.min(text.len())]
        .chars()
        .map(char::len_utf16)
        .sum()
}

pub fn byte_of(text: &str, utf16: usize) -> usize {
    let mut seen = 0;
    for (at, ch) in text.char_indices() {
        if seen >= utf16 {
            return at;
        }
        seen += ch.len_utf16();
    }
    text.len()
}

/// One thing that can be mentioned: how it lists, and what it pins.
#[derive(Clone, PartialEq)]
pub struct Offer {
    pub icon: IconName,
    pub name: String,
    pub anchor: Anchor,
}

/// **Everything that can be mentioned, narrowed by `matching`** — the one list both surfaces
/// read, so a name that completes inline is a name the attach popup offers and the other way
/// round.
///
/// The **tab first**: it is the one mention that is about what the user is looking at right now,
/// so it should not be buried under a long catalog. `matching` is expected already lowercased,
/// which is what [`contains_lowercased`] takes.
pub fn offers(store: &ProjectState, session: &SessionState, matching: &str) -> Vec<Offer> {
    let tab = session
        .active
        .and_then(|id| session.tabs.get(&id))
        .map(|tab| Offer {
            icon: IconName::File,
            name: tab.name.clone(),
            anchor: Anchor::Tab {
                id: tab.id,
                name: tab.name.clone(),
                error: None,
            },
        });
    let tables = store.tables.iter().map(|row| Offer {
        icon: IconName::for_catalog(CatalogKind::Table),
        name: row.def.name.clone(),
        anchor: Anchor::Entry {
            name: row.def.name.clone(),
            kind: CatalogKind::Table,
        },
    });
    let views = store.views.iter().map(|row| Offer {
        icon: IconName::for_catalog(CatalogKind::View),
        name: row.def.name.clone(),
        anchor: Anchor::Entry {
            name: row.def.name.clone(),
            kind: CatalogKind::View,
        },
    });
    let saved = store.saved_queries.iter().map(|query| Offer {
        icon: IconName::for_catalog(CatalogKind::Query),
        name: query.name.clone(),
        anchor: Anchor::SavedQuery {
            id: query.id,
            name: query.name.clone(),
        },
    });

    tab.into_iter()
        .chain(tables)
        .chain(views)
        .chain(saved)
        // The same case-insensitive substring the catalog filter uses, so a mention narrows the
        // way the sidebar does.
        .filter(|offer| contains_lowercased(&offer.name, matching))
        .collect()
}

/// **The mention being typed, and everything an accepted one needs to land.**
///
/// The completion list is driven from the composer's own field rather than from the popup: a
/// focused `Input` owns the keyboard (AGENTS.md §3), so the arrow keys reach it and nothing else,
/// and taking focus *away* to a list would stop the very typing that narrows it. The field keeps
/// focus throughout and hands this three keys; what moves is the highlight.
///
/// One value rather than five props, because the press in [`MentionPicker`] and the Enter in the
/// composer are the same gesture and must not become two implementations of it.
#[derive(Clone, Copy, PartialEq)]
pub struct Mentions {
    pub id: ChatId,
    pub chats: ChatsCtx,
    /// The composer's buffer: read to find the token, rewritten by a pick.
    pub text: State<String>,
    /// The field's caret, in UTF-16 code units, bound two-way to the `Input`. What makes the
    /// completion an edit at a **position** rather than at the end of the buffer.
    pub caret: State<usize>,
    /// The field the caret belongs in. A press in the list takes focus, and the user is still
    /// mid-sentence, so an accepted mention puts it back.
    pub field: AccessibilityId,
    /// Which row the keyboard is on. Reset whenever the buffer changes, so the first row is
    /// always what Enter takes for a token that has just narrowed.
    pub selected: State<usize>,
    /// The mention put away with Escape or a press outside, **named** by the byte offset of its
    /// `@`. A flag would put away every other mention in the message with it: moving the caret
    /// into an untouched `@` elsewhere in the same sentence is a different question, and one the
    /// user never declined. Cleared by the next keystroke too, because a mention still being
    /// typed is a question they have not finished asking.
    pub dismissed: State<Option<usize>>,
}

impl Mentions {
    /// What is on offer for the mention being typed: empty when there is not one, when nothing
    /// matches, or when the list has been dismissed.
    ///
    /// Capped at [`INLINE_ROWS`] — a completion list is not a browser, and one more character is
    /// the faster narrowing. The `+` popup is the surface for not knowing the name.
    pub fn offered(&self, store: &ProjectState, session: &SessionState) -> Vec<Offer> {
        let text = self.text.read();
        let caret = byte_of(&text, *self.caret.read());
        // Lowercased once here, because `contains_lowercased` takes an already-folded needle.
        let Some(typed) =
            asking(&text, caret, *self.dismissed.read()).map(|token| token.typed.to_lowercase())
        else {
            return Vec::new();
        };
        offers(store, session, &typed)
            .into_iter()
            .take(INLINE_ROWS)
            .collect()
    }

    /// The highlighted row, clamped to what is actually offered.
    pub fn at(&self, len: usize) -> usize {
        (*self.selected.read()).min(len.saturating_sub(1))
    }

    /// **Take one.** Replace the tag's span, put the caret after what was inserted, pin what it
    /// points at, and give the field the focus a press in the list took from it.
    ///
    /// A no-op when there is no token at the caret, which is what a press on a list that has
    /// already been overtaken by typing would be.
    pub fn accept(&mut self, offer: &Offer) {
        let (completed, caret) = {
            let text = self.text.peek();
            let caret = byte_of(&text, *self.caret.peek());
            let Some(token) = token(&text, caret) else {
                return;
            };
            let (completed, caret) = complete(&text, token.span, &offer.name);
            let caret = utf16_of(&completed, caret);
            (completed, caret)
        };
        self.text.set(completed);
        self.caret.set(caret);
        self.chats.write().pin(self.id, offer.anchor.clone());
        self.field.request_focus();
    }

    /// **Put this mention away**, and only this one: the token at the caret is recorded by name,
    /// so an untouched `@` elsewhere in the message still asks.
    pub fn dismiss(&mut self) {
        let text = self.text.peek();
        let caret = byte_of(&text, *self.caret.peek());
        self.dismissed
            .set(token(&text, caret).map(|token| token.span.start));
    }

    /// **The three keys the list claims while it is up**, and `false` for everything else so the
    /// field goes on behaving like a field.
    ///
    /// Enter and Tab take the highlighted row rather than sending the message, which is the whole
    /// reason this is here rather than on the popup: `on_submit` would otherwise fire first.
    pub fn claim(&mut self, e: &Event<KeyboardEventData>, offered: &[Offer]) -> bool {
        if offered.is_empty() {
            return false;
        }
        match &e.key {
            Key::Named(key @ (NamedKey::ArrowDown | NamedKey::ArrowUp)) => {
                let at = step(
                    self.at(offered.len()),
                    offered.len(),
                    *key == NamedKey::ArrowDown,
                );
                self.selected.set(at);
                true
            }
            Key::Named(NamedKey::Enter | NamedKey::Tab) => {
                let at = self.at(offered.len());
                self.accept(&offered[at]);
                true
            }
            Key::Named(NamedKey::Escape) => {
                self.dismiss();
                true
            }
            _ => false,
        }
    }
}

/// The next row in `down`'s direction, wrapping at both ends.
///
/// Wrapping rather than stopping, because eight rows is a list you cycle rather than a document
/// you scroll: pressing up from the first is the shortest way to the last.
fn step(at: usize, len: usize, down: bool) -> usize {
    match (down, at) {
        (true, at) => (at + 1) % len.max(1),
        (false, 0) => len.saturating_sub(1),
        (false, at) => at - 1,
    }
}

/// One offer's row, shared by both surfaces so they read identically.
fn row(offer: &Offer, theme: &ChatTheme) -> Element {
    rect()
        .width(Size::px(MENU_W - MENU_ROW_CHROME))
        .horizontal()
        .cross_align(Alignment::Center)
        .spacing(SP_3)
        .child(Icon::new(offer.icon).size(12.).color(theme.meta_color))
        .child(
            Meta::new(offer.name.clone())
                .color(theme.title_color)
                .width(Size::fill())
                .max_lines(1)
                .text_overflow(TextOverflow::Ellipsis),
        )
        .into_element()
}

/// The inline completion list, drawn only while a mention is being typed and something matches.
///
/// A renderer over [`Mentions`]: what to offer is decided where the keyboard is, so the list and
/// the Enter key cannot disagree about which row is next.
#[derive(PartialEq)]
pub struct MentionPicker {
    pub mentions: Mentions,
    /// What the composer matched this render, already capped.
    pub offered: Vec<Offer>,
    pub theme: ChatTheme,
}

impl Component for MentionPicker {
    fn render(&self) -> impl IntoElement {
        let mut mentions = self.mentions;
        let theme = self.theme.clone();

        // Nothing matches: no popup. An empty card hovering over the transcript would be a
        // control that says nothing and covers something.
        if self.offered.is_empty() {
            return rect();
        }

        let at = mentions.at(self.offered.len());
        rect().child(
            self.offered.iter().enumerate().fold(
                // **A press outside puts the list away**, which a bare floating card has no way to
                // notice: the token is still being typed, so nothing else here would close it.
                Menu::new()
                    .min_width(Size::px(MENU_W))
                    .on_close(move |()| mentions.dismiss()),
                |menu, (row_at, offer)| {
                    let picked = offer.clone();
                    menu.child(
                        MenuButton::new()
                            // The keyboard's row reads as the pressed one, so arrowing down the list
                            // and hovering it look the same.
                            .selected(row_at == at)
                            .on_press(move |_| mentions.accept(&picked))
                            .child(row(offer, &theme)),
                    )
                },
            ),
        )
    }
}

/// The composer bar's `+` — the same offers, **searched and scrolled**.
///
/// A card rather than a `Menu`, because a menu is a list of items and this is a list *plus* the
/// box that narrows it.
#[derive(PartialEq)]
pub struct AttachPicker {
    pub id: ChatId,
    pub theme: ChatTheme,
}

impl Component for AttachPicker {
    fn render(&self) -> impl IntoElement {
        let mut chats = use_consume::<ChatsCtx>();
        let tables = use_radio::<ProjectState, ProjChan>(ProjChan::Tables);
        let views = use_radio::<ProjectState, ProjChan>(ProjChan::Views);
        let queries = use_radio::<ProjectState, ProjChan>(ProjChan::Queries);
        let session = use_radio::<SessionState, Chan>(Chan::Tabs);
        let mut open = use_state(|| false);
        let mut query = use_state(String::new);
        let theme = self.theme.clone();
        let id = self.id;

        let matched: Vec<Offer> = {
            let _ = views.read();
            let _ = queries.read();
            offers(
                &tables.read(),
                &session.read(),
                &query.read().to_lowercase(),
            )
        };

        let body = matched.iter().fold(
            // A hairline between rows, not a gap — off the spacing scale on purpose, like
            // the canvas's own `gap: 1px` lists.
            rect().width(Size::fill()).vertical().spacing(HAIRLINE),
            |body, offer| {
                let anchor = offer.anchor.clone();
                body.child(
                    Button::new()
                        .flat()
                        .width(Size::fill())
                        .on_press(move |_| {
                            chats.write().pin(id, anchor.clone());
                            open.set(false);
                        })
                        .child(row(offer, &theme)),
                )
            },
        );

        let card = rect()
            .width(Size::px(MENU_W))
            .vertical()
            .background(theme.card_background)
            .border(Border::new().width(1.).fill(theme.card_border_fill))
            .corner_radius(R_2)
            // A painted border is not laid out, so the inset carries it (AGENTS.md §3).
            .padding(Gaps::new_all(SP_3))
            .spacing(SP_2)
            .child(
                InputTypography::body(
                    Input::new(query)
                        .leading(
                            Icon::new(IconName::Search)
                                .size(13.)
                                .color(theme.meta_color),
                        )
                        .compact()
                        .auto_focus(true)
                        .width(Size::fill())
                        .placeholder("Search"),
                )
                .width(Size::fill()),
            )
            // Nothing to offer says so, rather than scrolling an empty body.
            .child(match matched.is_empty() {
                true => Meta::new("Nothing matches.")
                    .color(theme.meta_color)
                    .width(Size::fill())
                    .into_element(),
                false => ScrollView::new()
                    .width(Size::fill())
                    .height(Size::px(ATTACH_BODY_H))
                    .child(body)
                    .into_element(),
            });

        Attached::new(
            ToolButton::new(IconName::Plus, "Attach a table, view or query").on_press(move |_| {
                query.set(String::new());
                open.toggle();
            }),
        )
        .top()
        .align_start()
        .offset(6.)
        // **Inside a `Menu`**, which is what dismisses it on an outside press — the tab strip's
        // overflow search is the same card in the same wrapper for the same reason. A bare
        // floating card has no backdrop, so pressing away from it (especially after focusing the
        // search box) left it up with nothing to close it.
        .maybe_child(open().then(|| Menu::new().on_close(move |()| open.set(false)).child(card)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An `@` that starts a word is a mention, and everything typed between it and the caret
    /// narrows the list.
    #[test]
    fn a_word_starting_at_is_the_token() {
        assert_eq!(typed_at("@", 1), Some(""));
        assert_eq!(typed_at("what is in @ev", 14), Some("ev"));
        assert_eq!(typed_at("@events", 7), Some("events"));
    }

    /// **The mention does not have to be the last thing in the box.** Tying it to the end of the
    /// buffer meant a mention typed mid-sentence stopped completing the moment the sentence
    /// carried on past it: `@F fake me` with the caret after the `F` offered nothing.
    #[test]
    fn a_mention_mid_sentence_completes_at_the_caret() {
        let text = "@F fake me";
        assert_eq!(typed_at(text, 2), Some("F"));
        assert_eq!(
            token(text, 2).map(|token| token.span),
            Some(0..2),
            "the span is the tag, not the rest of the sentence"
        );
    }

    /// The caret inside a name already typed edits **that name**: the span runs to the end of the
    /// tag, so accepting rewrites it rather than splicing into the middle of it.
    #[test]
    fn the_span_covers_the_whole_tag_the_caret_is_in() {
        let token = token("@region and more", 4).expect("a mention");
        assert_eq!(token.typed, "reg", "only what precedes the caret narrows");
        assert_eq!(token.span, 0..7);
    }

    /// **A space ends it.** An `@` the user has typed past is prose — an address, a handle, a
    /// mention they abandoned — and a list still filtering on it would sit over the transcript
    /// for the rest of the sentence.
    #[test]
    fn whitespace_before_the_caret_ends_the_mention() {
        assert_eq!(typed_at("@events and", 11), None);
        assert_eq!(typed_at("@ ", 2), None);
    }

    /// An `@` inside a word is not a mention: that is an email, not a table.
    #[test]
    fn an_at_mid_word_is_not_a_mention() {
        assert_eq!(typed_at("alex@proton.me", 14), None);
        assert_eq!(typed_at("no mentions here", 16), None);
    }

    /// Completing replaces the tag's span and says where the caret lands: after the name, so the
    /// user types on from there. The same rule `CodeEditorData::replace_range` holds the SQL
    /// editor to.
    #[test]
    fn completing_replaces_the_span_and_places_the_caret() {
        assert_eq!(
            complete("what is in @ev", 11..14, "events"),
            ("what is in @events ".to_string(), 19)
        );
        assert_eq!(complete("@", 0..1, "orders"), ("@orders ".to_string(), 8));
    }

    /// **No second space.** A mention completed mid-sentence already has one after it, and the
    /// caret goes before it rather than after a gap the user did not type.
    #[test]
    fn completing_mid_sentence_does_not_double_the_space() {
        assert_eq!(
            complete("@F fake me", 0..2, "facts"),
            ("@facts fake me".to_string(), 6)
        );
    }

    /// **Escape puts away the mention it was pressed on, not every mention in the message.** A
    /// flag would silence a `@` elsewhere in the same sentence that the user never declined, and
    /// only a text edit would bring it back — so moving the caret into it did nothing at all.
    #[test]
    fn dismissing_one_mention_leaves_the_others_asking() {
        let text = "@events @tabl";
        let second = text.rfind('@').expect("the second mention");

        assert!(
            asking(text, text.len(), Some(second)).is_none(),
            "the one Escape was pressed on stays away"
        );
        let first = asking(text, 7, Some(second)).expect("the first mention still asks");
        assert_eq!(first.typed, "events");
        assert!(
            asking(text, text.len(), None).is_some(),
            "nothing put away, so the caret's own token asks"
        );
    }

    /// The caret crosses one boundary, so it converts at exactly one place. An emoji is two
    /// UTF-16 code units and four bytes, which is where counting in the wrong one shows up.
    #[test]
    fn the_caret_converts_between_bytes_and_utf16() {
        let text = "hi 🎉 @ev";
        let byte = text.find("@ev").expect("the tag") + 3;
        assert_eq!(byte_of(text, utf16_of(text, byte)), byte);
        assert_eq!(typed_at(text, byte), Some("ev"));
        assert_eq!(byte_of(text, usize::MAX), text.len(), "past the end clamps");
    }

    /// What the list narrows on, for a caret given in bytes.
    fn typed_at(text: &str, caret: usize) -> Option<&str> {
        token(text, caret).map(|token| token.typed)
    }

    /// **The highlight cycles both ways.** Eight rows is a list you cycle rather than a document
    /// you scroll, so up from the first is the shortest way to the last.
    #[test]
    fn the_highlight_wraps_at_both_ends() {
        assert_eq!(step(0, 3, true), 1);
        assert_eq!(step(2, 3, true), 0, "down from the last is the first");
        assert_eq!(step(0, 3, false), 2, "up from the first is the last");
        assert_eq!(step(1, 3, false), 0);
    }

    /// A one-row list has nowhere to go, and a zero-row one is never keyed against — but neither
    /// may divide by zero on the way to saying so.
    #[test]
    fn stepping_a_short_list_stays_in_range() {
        assert_eq!(step(0, 1, true), 0);
        assert_eq!(step(0, 1, false), 0);
        assert_eq!(step(0, 0, true), 0);
        assert_eq!(step(0, 0, false), 0);
    }
}
