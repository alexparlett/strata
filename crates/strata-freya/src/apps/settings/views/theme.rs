//! **Settings ▸ Appearance & behaviour ▸ Theme** (P4-04, design `Settings.dc.html`) — the
//! Sync-with-OS switch and the theme grid.
//!
//! The controls only write [`SettingsCtx::draft`]'s `theme` / `sync_os`. Everything that makes
//! the pick *live across every window* is already built: the window root mirrors that half of
//! the draft into the app-global `ThemePreview`, which each window's `use_strata_theme` resolves
//! ahead of the committed settings, and the footer's Apply is what persists. So nothing here
//! touches the preview slot or the config store — see [`crate::state::theme_preview`].
//!
//! **The swatches are the previewed theme's own colours, not this window's.** A card's
//! thumbnail is painted from four slots of the theme it stands for — `background`,
//! `surface_secondary`, `border`, `primary` — so a user theme dropped in the themes dir gets a
//! true preview with nothing authored per theme. The card's *frame* (surface, ring, badge) is
//! the `settings` component theme, because that belongs to this window.

use freya::prelude::*;
use strata_core::theme::{Source, StrataTheme};

use crate::apps::settings::views::Pane;
use crate::apps::settings::{settings_theme, Anchor, SettingsCtx};
use crate::components::badge::Badge;
use crate::components::divider::Divider;
use crate::components::form::Form;
use crate::components::icon::{Icon, IconName};
use crate::components::metrics::{pill, R_3, SP_2, SP_3, SP_4};
use crate::components::typography::Body;
use crate::state::ThemeSel;
use crate::theme::{authored_role, Role, ThemesCtx};

/// Cards per row (canvas `grid-template-columns: 1fr 1fr`), and the gutter between them.
const CARDS_PER_ROW: usize = 2;
const CARD_GAP: f32 = SP_4;

/// How far the grid fades once Sync-with-OS owns the choice. It stays legible — it still shows
/// which theme is in force — but reads as not yours to set.
const INERT_OPACITY: f32 = 0.55;

/// The pane: the Sync-with-OS row, a rule, then the theme grid.
#[derive(PartialEq)]
pub struct ThemePane;

impl Component for ThemePane {
    fn render(&self) -> impl IntoElement {
        let themes = use_consume::<ThemesCtx>();
        let ctx = use_consume::<SettingsCtx>();
        let preferred = use_hook(Platform::get).preferred_theme;

        let sel = ThemeSel::from(&*ctx.draft.read());
        let sync_os = sel.sync_os;
        let os_dark = sync_os && *preferred.read() == PreferredTheme::Dark;
        let selected = themes.get_or_default(&sel.effective(os_dark)).id.clone();

        let mut grid = Anchor::Theme.row().child(ThemeGrid {
            themes,
            selected,
            inert: sync_os,
        });
        if sync_os {
            grid = grid.hint(
                "Following your system appearance. \
                 Turn off Sync with OS to choose a theme.",
            );
        }

        let body = Form::new()
            .preferences()
            .child(
                Anchor::SyncOs
                    .row()
                    .trailing()
                    .on_press(move |_: Event<PressEventData>| ctx.edit(|s| s.sync_os = !s.sync_os))
                    .child(
                        Switch::new()
                            .toggled(sync_os)
                            .on_toggle(move |()| ctx.edit(|s| s.sync_os = !s.sync_os)),
                    ),
            )
            .child(grid);

        Pane::new(body)
    }
}

/// Every discovered theme, two to a row.
///
/// Rows rather than a grid because Freya lays out in flex, not CSS grid: each row is a
/// horizontal strip of `flex(1.)` cards, so a short last row leaves its gap instead of
/// stretching one card across the pane.
#[derive(PartialEq)]
struct ThemeGrid {
    themes: ThemesCtx,
    selected: String,
    inert: bool,
}

impl Component for ThemeGrid {
    fn render(&self) -> impl IntoElement {
        let group = use_a11y();
        let entries = self.themes.entries();
        let rows = entries.chunks(CARDS_PER_ROW).map(|chunk| {
            let mut row = rect()
                .width(Size::fill())
                .horizontal()
                .spacing(CARD_GAP)
                .content(Content::Flex);
            for entry in chunk {
                row = row.child(
                    ThemeCard {
                        theme_id: entry.theme.id.clone(),
                        name: entry.theme.name.clone(),
                        source: entry.source,
                        swatch: Swatch::of(&entry.theme),
                        selected: entry.theme.id == self.selected,
                        inert: self.inert,
                        group,
                        key: DiffKey::None,
                    }
                    .key(entry.theme.id.clone()),
                );
            }
            for _ in chunk.len()..CARDS_PER_ROW {
                row = row.child(rect().width(Size::flex(1.)));
            }
            row.into_element()
        });

        rect()
            .width(Size::fill())
            .vertical()
            .spacing(CARD_GAP)
            .opacity(if self.inert { INERT_OPACITY } else { 1. })
            .a11y_id(group)
            .a11y_role(AccessibilityRole::RadioGroup)
            .a11y_alt("Theme")
            .children(rows.collect::<Vec<_>>())
    }
}

/// The four colours a card's thumbnail is painted from, read off the theme it previews.
///
/// Named for what they are in the miniature, not for the slot they came from: `body` is the
/// window behind everything, `raised` the title strip and side rail, `line` the hairlines and
/// dim text runs, `accent` the one coloured run.
#[derive(PartialEq, Clone, Copy)]
struct Swatch {
    body: Color,
    raised: Color,
    line: Color,
    accent: Color,
}

impl Swatch {
    fn of(theme: &StrataTheme) -> Self {
        Self {
            body: authored_role(theme, Role::Background),
            raised: authored_role(theme, Role::SurfaceRaised),
            line: authored_role(theme, Role::Border),
            accent: authored_role(theme, Role::Accent),
        }
    }
}

/// One theme: a miniature of the app in that theme, then its name, source and tick.
#[derive(PartialEq)]
struct ThemeCard {
    theme_id: String,
    name: String,
    source: Source,
    swatch: Swatch,
    selected: bool,
    inert: bool,
    /// The enclosing [`ThemeGrid`]'s radio group, so this card announces as one of a set.
    group: AccessibilityId,
    key: DiffKey,
}

/// Keyed by theme id, so the user themes dir gaining or losing a file re-associates the cards
/// with their themes rather than shifting each one's hover state along the row.
impl KeyExt for ThemeCard {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

/// Canvas card metrics.
const CARD_RADIUS: f32 = R_3;

const RUN: f32 = 5.;
const TITLE: f32 = 4.;
const TITLE_RUN: f32 = 34.;
const RAIL: f32 = 26.;
const MINI_RADIUS: f32 = 2.;
const PREVIEW_HEIGHT: f32 = 78.;
const NAME_ROW_PADDING: Gaps = Gaps::new_symmetric(SP_3, SP_4);
const CHECK_SIZE: f32 = 16.;

impl Component for ThemeCard {
    fn render(&self) -> impl IntoElement {
        let theme = settings_theme();
        let ctx = use_consume::<SettingsCtx>();
        let mut hovering = use_state(|| false);
        let a11y_id = use_a11y();

        let theme_id = self.theme_id.clone();
        let pick = move || {
            let mut draft = ctx.draft;
            draft.write().theme = theme_id.clone();
        };

        let (ring, width) = if self.selected {
            (theme.selected_color, 2.)
        } else if hovering() && !self.inert {
            (theme.card_hover_border_fill, 1.)
        } else {
            (theme.card_border_fill, 1.)
        };
        let source = match self.source {
            Source::Builtin => ("BUNDLED", theme.badge_builtin_color),
            Source::User => ("USER", theme.badge_user_color),
        };

        rect()
            .width(Size::flex(1.))
            .vertical()
            .a11y_id(a11y_id)
            .a11y_role(AccessibilityRole::RadioButton)
            .a11y_member_of(self.group)
            .a11y_focusable(!self.inert)
            .a11y_alt(format!("{} theme", self.name))
            .a11y_builder({
                let selected = self.selected;
                move |b| b.set_toggled(Toggled::from(selected))
            })
            .background(theme.card_background)
            .corner_radius(CARD_RADIUS)
            .overflow(Overflow::Clip)
            .padding(Gaps::new_all(width))
            .border(
                Border::new()
                    .fill(ring)
                    .width(width)
                    .alignment(BorderAlignment::Inner),
            )
            .maybe(!self.inert, |el| {
                el.on_press(move |_: Event<PressEventData>| {
                    a11y_id.request_focus();
                    pick();
                })
            })
            .on_pointer_enter(move |_| hovering.set(true))
            .on_pointer_leave(move |_| hovering.set(false))
            .child(Preview {
                swatch: self.swatch,
                radius: CARD_RADIUS - width,
            })
            .child(Divider::horizontal().color(theme.card_divider_fill))
            .child(
                rect()
                    .width(Size::fill())
                    .horizontal()
                    .content(Content::Flex)
                    .cross_align(Alignment::Center)
                    .spacing(SP_3)
                    .padding(NAME_ROW_PADDING)
                    .child(Body::new(self.name.clone()))
                    .child(Badge::tag(source.0, source.1))
                    .child(rect().width(Size::flex(1.)))
                    .maybe_child(self.selected.then(|| {
                        Icon::new(IconName::Check)
                            .color(theme.selected_color)
                            .size(CHECK_SIZE)
                    })),
            )
    }
}

/// The miniature app inside a card: a title strip over a rail and three text runs, each drawn
/// in the previewed theme's own colours.
///
/// The **shapes** below are deliberately off the layout scale: this is a drawing of a window at
/// roughly a tenth scale, so its 4px and 5px runs are a picture of the app's type and chrome
/// rather than any of it, and rounding them to a scale step would leave a blurred rectangle
/// rather than a consistent miniature. Its gaps and insets are on the scale like everywhere else.
#[derive(PartialEq)]
struct Preview {
    swatch: Swatch,
    /// The card's inner radius, applied to the **top** corners only — the bottom of the card
    /// is the name row, whose corners the card's own background rounds.
    radius: f32,
}

impl Component for Preview {
    fn render(&self) -> impl IntoElement {
        /// One dim text run in the miniature.
        fn run(width: f32, color: Color) -> impl IntoElement {
            rect()
                .width(Size::percent(width))
                .height(Size::px(RUN))
                .corner_radius(pill(RUN))
                .background(color)
        }

        let Swatch {
            body,
            raised,
            line,
            accent,
        } = self.swatch;
        let mut radius = CornerRadius::default();
        radius.fill_top(self.radius);

        rect()
            .width(Size::fill())
            .height(Size::px(PREVIEW_HEIGHT))
            .vertical()
            .content(Content::Flex)
            .background(body)
            .corner_radius(radius)
            .overflow(Overflow::Clip)
            .child(
                rect()
                    .width(Size::fill())
                    .height(Size::px(16.))
                    .horizontal()
                    .cross_align(Alignment::Center)
                    .spacing(SP_2)
                    .padding(Gaps::new_symmetric(0., SP_3))
                    .background(raised)
                    .child(
                        rect()
                            .width(Size::px(RUN))
                            .height(Size::px(RUN))
                            .corner_radius(pill(RUN))
                            .background(line),
                    )
                    .child(
                        rect()
                            .width(Size::px(TITLE_RUN))
                            .height(Size::px(TITLE))
                            .corner_radius(pill(TITLE))
                            .background(line),
                    ),
            )
            .child(
                rect()
                    .width(Size::fill())
                    .height(Size::flex(1.))
                    .horizontal()
                    .content(Content::Flex)
                    .spacing(SP_3)
                    .padding(Gaps::new_all(SP_3))
                    .child(
                        rect()
                            .width(Size::px(RAIL))
                            .height(Size::fill())
                            .corner_radius(MINI_RADIUS)
                            .background(raised),
                    )
                    .child(
                        rect()
                            .width(Size::flex(1.))
                            .vertical()
                            .spacing(SP_2)
                            .child(run(70., accent))
                            .child(run(45., line))
                            .child(run(55., line)),
                    ),
            )
    }
}
