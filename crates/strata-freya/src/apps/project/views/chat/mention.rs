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
//! ## The token is the tail of what has been typed
//!
//! [`token`] reads the run from the last `@` to the end of the buffer, and only when that `@`
//! starts a word and nothing after it is whitespace. That is the whole rule — no caret position,
//! no parser. `Input` does not publish a caret offset, and pinning the completion to the *end* of
//! the buffer is also the honest reading of what someone typing a mention is doing: an `@`
//! abandoned mid-sentence stops matching the moment they type a space, which is exactly when they
//! meant it as prose.
//!
//! ## What can be mentioned
//!
//! The catalog **from the store** (AGENTS.md §2 — the catalog *is* `ProjectState`, never a
//! query), plus the query tab the user is looking at. Not a settled result: its rows live in that
//! run's own query entry, which no store here can read — the results toolbar pins that one,
//! because it is the surface that has it.

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

/// **The mention being typed**, or `None` when there is not one.
///
/// The run from a word-starting `@` to the end of the buffer, whitespace-free — see the module
/// note for why the tail rather than the caret. The `@` itself is not included, so an empty
/// answer means "just typed `@`", which offers everything.
pub fn token(text: &str) -> Option<&str> {
    let at = text.rfind('@')?;
    let starts_word = at == 0
        || text[..at]
            .chars()
            .next_back()
            .is_some_and(char::is_whitespace);
    let rest = &text[at + 1..];
    (starts_word && !rest.chars().any(char::is_whitespace)).then_some(rest)
}

/// Replace the mention being typed with `name`, and leave a trailing space so the next word is
/// prose rather than more of the mention.
pub fn complete(text: &str, name: &str) -> String {
    match text.rfind('@') {
        Some(at) => format!("{}@{name} ", &text[..at]),
        None => text.to_string(),
    }
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

/// One offer's row, shared by both surfaces so they read identically.
fn row(offer: &Offer, theme: &ChatTheme) -> Element {
    rect()
        .width(Size::px(MENU_W - MENU_ROW_CHROME))
        .horizontal()
        .cross_align(Alignment::Center)
        .spacing(6.)
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
#[derive(PartialEq)]
pub struct MentionPicker {
    pub id: ChatId,
    /// The composer's buffer — read to find the token, rewritten by a pick.
    pub text: State<String>,
    pub theme: ChatTheme,
}

impl Component for MentionPicker {
    fn render(&self) -> impl IntoElement {
        let mut chats = use_consume::<ChatsCtx>();
        let tables = use_radio::<ProjectState, ProjChan>(ProjChan::Tables);
        let views = use_radio::<ProjectState, ProjChan>(ProjChan::Views);
        let queries = use_radio::<ProjectState, ProjChan>(ProjChan::Queries);
        let session = use_radio::<SessionState, Chan>(Chan::Tabs);
        let mut text = self.text;
        let theme = self.theme.clone();
        let id = self.id;

        // Lowercased once here, because `contains_lowercased` takes an already-folded needle.
        let Some(typed) = token(&text.read()).map(str::to_lowercase) else {
            return rect();
        };

        let matched: Vec<Offer> = {
            let _ = views.read();
            let _ = queries.read();
            offers(&tables.read(), &session.read(), &typed)
                .into_iter()
                .take(INLINE_ROWS)
                .collect()
        };

        // Nothing matches: no popup. An empty card hovering over the transcript would be a
        // control that says nothing and covers something.
        if matched.is_empty() {
            return rect();
        }

        rect().child(
            matched
                .iter()
                .fold(Menu::new().min_width(Size::px(MENU_W)), |menu, offer| {
                    let (name, anchor) = (offer.name.clone(), offer.anchor.clone());
                    menu.child(
                        MenuButton::new()
                            .on_press(move |_| {
                                let completed = complete(&text.peek(), &name);
                                text.set(completed);
                                chats.write().pin(id, anchor.clone());
                            })
                            .child(row(offer, &theme)),
                    )
                }),
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
            rect().width(Size::fill()).vertical().spacing(1.),
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
            .corner_radius(8.)
            // A painted border is not laid out, so the inset carries it (AGENTS.md §3).
            .padding(Gaps::new_all(6.))
            .spacing(4.)
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

    /// An `@` that starts a word is a mention, and everything typed after it narrows the list.
    #[test]
    fn a_word_starting_at_is_the_token() {
        assert_eq!(token("@"), Some(""));
        assert_eq!(token("what is in @ev"), Some("ev"));
        assert_eq!(token("@events"), Some("events"));
    }

    /// **A space ends it.** An `@` the user has typed past is prose — an address, a handle, a
    /// mention they abandoned — and a list still filtering on it would sit over the transcript
    /// for the rest of the sentence.
    #[test]
    fn whitespace_after_the_at_ends_the_mention() {
        assert_eq!(token("@events and"), None);
        assert_eq!(token("@ "), None);
    }

    /// An `@` inside a word is not a mention: that is an email, not a table.
    #[test]
    fn an_at_mid_word_is_not_a_mention() {
        assert_eq!(token("alex@proton.me"), None);
        assert_eq!(token("no mentions here"), None);
    }

    /// Completing replaces what was typed and leaves a space, so the next word is prose.
    #[test]
    fn completing_rewrites_the_token_and_leaves_a_space() {
        assert_eq!(complete("what is in @ev", "events"), "what is in @events ");
        assert_eq!(complete("@", "orders"), "@orders ");
    }
}
