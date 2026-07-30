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
use crate::apps::settings::{Anchor, SettingsCtx, SettingsThemePartial, SettingsThemePreference};
use crate::components::badge::Badge;
use crate::components::divider::Divider;
use crate::components::form::Form;
use crate::components::icon::{Icon, IconName};
use crate::components::typography::Body;
use crate::state::ThemeSel;
use crate::theme::{pc, ThemesCtx};

/// Cards per row (canvas `grid-template-columns: 1fr 1fr`), and the gutter between them.
const CARDS_PER_ROW: usize = 2;
const CARD_GAP: f32 = 12.;

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

        // Which card wears the tick is the **effective** theme, not the stored id: while
        // Sync-with-OS is on, the id in the draft isn't what any window is wearing, and the
        // grid is the only thing on this pane that answers "so which theme am I using?".
        // Resolved through `ThemeSel::effective` — the same pure function `use_strata_theme`
        // derives with — and then through `get_or_default`, the same fallback it resolves the
        // id *with*. Both steps are needed for the tick to agree with what is on screen: a
        // persisted id whose theme is gone (a user file deleted since it was picked) paints
        // the default, so that is the card the tick belongs on, not no card at all.
        let sel = ThemeSel::from(&*ctx.draft.read());
        let sync_os = sel.sync_os;
        // Short-circuited, so this only subscribes to the OS appearance while syncing.
        let os_dark = sync_os && *preferred.read() == PreferredTheme::Dark;
        let selected = themes.get_or_default(&sel.effective(os_dark)).id.clone();

        // Why the grid is inert rather than absent: it is still the answer to "which theme am
        // I using?", which Sync-with-OS doesn't tell you — so while syncing it keeps its place
        // and gains the line saying whose choice it now is. That line is the one row subtext in
        // the window that is *conditional*, which is why it is set here rather than in the index.
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
                            .on_toggle(move |_| ctx.edit(|s| s.sync_os = !s.sync_os)),
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
        // The grid is the radio **group**: without it each card announces "radio button" with
        // no set to belong to, so there is no way to hear how many themes there are.
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
            // Pad a short final row so its cards keep the width they'd have in a full one.
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
            body: pc(&theme.sheet.background),
            raised: pc(&theme.sheet.surface_secondary),
            line: pc(&theme.sheet.border),
            accent: pc(&theme.sheet.primary),
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
const CARD_RADIUS: f32 = 10.;
const PREVIEW_HEIGHT: f32 = 78.;
const NAME_ROW_PADDING: Gaps = Gaps::new_symmetric(8., 12.);
const CHECK_SIZE: f32 = 16.;

impl Component for ThemeCard {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(
            &None::<SettingsThemePartial>,
            SettingsThemePreference,
            "settings"
        );
        let ctx = use_consume::<SettingsCtx>();
        let mut hovering = use_state(|| false);
        let a11y_id = use_a11y();

        // `sync_os` is left alone: while it is on this card takes no press (see below), so a
        // pick never runs from a synced state. Turning it off is the switch's job.
        let theme_id = self.theme_id.clone();
        let pick = move || {
            let mut draft = ctx.draft;
            draft.write().theme = theme_id.clone();
        };

        // Hover is *tracked* even while inert and only *dressed* when live — the shape Freya's
        // own controls use for a disabled state (`Switch` gates `on_press` on `enabled` and
        // leaves `on_pointer_enter` / `on_pointer_leave` registered unconditionally). Killing
        // the pointer events instead is what leaves the flag stuck: `pointer_leave` never
        // arrives, so the ring comes back stale the moment the cards go live again.
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
            // A radio, not a button: the cards are one choice with one answer, and the tick is
            // the selected state a reader needs announced.
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
            // The thumbnail bleeds to the card's edge, so it has to be clipped to the radius.
            .overflow(Overflow::Clip)
            // The ring is **inner**, and the content is inset by exactly its width so it can't
            // paint over it. The thumbnail bleeds edge to edge, so without the inset its own
            // background covered the ring on three sides, leaving it visible only along the
            // name row (where the card's background shows through). `Outer` is not the fix:
            // that puts the ring outside the bounds children are clipped to, where it gets
            // overpainted anyway — all that survives is the corner arcs.
            .padding(Gaps::new_all(width))
            .border(
                Border::new()
                    .fill(ring)
                    .width(width)
                    .alignment(BorderAlignment::Inner),
            )
            // Sync-with-OS owns the choice, so while it is on the card takes no press rather
            // than writing a theme the OS immediately overrides. Only the *press* is gated:
            // hover keeps tracking (see the ring above), so nothing can be left stale.
            .maybe(!self.inert, |el| {
                el.on_press(move |_: Event<PressEventData>| {
                    a11y_id.request_focus();
                    pick();
                })
            })
            .on_pointer_enter(move |_| hovering.set(true))
            .on_pointer_leave(move |_| hovering.set(false))
            // The inset means the card's own clip no longer cuts the preview's corners — they
            // sit a ring's width inside it, where the card's radius has already curved away.
            // So the preview carries the matching inner radius: the card's, less the inset.
            .child(Preview {
                swatch: self.swatch,
                radius: CARD_RADIUS - width,
            })
            .child(Divider::horizontal().color(theme.card_divider_fill))
            .child(
                rect()
                    .width(Size::fill())
                    .horizontal()
                    // For the spacer below, which pushes the tick to the trailing edge.
                    .content(Content::Flex)
                    .cross_align(Alignment::Center)
                    .spacing(8.)
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
#[derive(PartialEq)]
struct Preview {
    swatch: Swatch,
    /// The card's inner radius, applied to the **top** corners only — the bottom of the card
    /// is the name row, whose corners the card's own background rounds.
    radius: f32,
}

impl Component for Preview {
    fn render(&self) -> impl IntoElement {
        let Swatch {
            body,
            raised,
            line,
            accent,
        } = self.swatch;
        /// One dim text run in the miniature.
        fn run(width: f32, color: Color) -> impl IntoElement {
            rect()
                .width(Size::percent(width))
                .height(Size::px(5.))
                .corner_radius(2.)
                .background(color)
        }

        let mut radius = CornerRadius::default();
        radius.fill_top(self.radius);

        rect()
            .width(Size::fill())
            .height(Size::px(PREVIEW_HEIGHT))
            .vertical()
            .content(Content::Flex)
            .background(body)
            .corner_radius(radius)
            // The title strip fills the width, so without a clip here *its* square corners
            // would poke through the rounding this rect just gained.
            .overflow(Overflow::Clip)
            // The title strip: a traffic light and a window title.
            .child(
                rect()
                    .width(Size::fill())
                    .height(Size::px(16.))
                    .horizontal()
                    .cross_align(Alignment::Center)
                    .spacing(4.)
                    .padding(Gaps::new_symmetric(0., 8.))
                    .background(raised)
                    .child(
                        rect()
                            .width(Size::px(5.))
                            .height(Size::px(5.))
                            .corner_radius(50.)
                            .background(line),
                    )
                    .child(
                        rect()
                            .width(Size::px(34.))
                            .height(Size::px(4.))
                            .corner_radius(2.)
                            .background(line),
                    ),
            )
            // The body: the sidebar, then the content's text runs.
            .child(
                rect()
                    .width(Size::fill())
                    .height(Size::flex(1.))
                    .horizontal()
                    // For the text-run column's `flex(1.)`, which takes the width the rail
                    // leaves — and whose `percent` runs are measured against it.
                    .content(Content::Flex)
                    .spacing(8.)
                    .padding(Gaps::new_all(8.))
                    .child(
                        rect()
                            .width(Size::px(26.))
                            .height(Size::fill())
                            .corner_radius(2.)
                            .background(raised),
                    )
                    .child(
                        rect()
                            .width(Size::flex(1.))
                            .vertical()
                            .spacing(4.)
                            .child(run(70., accent))
                            .child(run(45., line))
                            .child(run(55., line)),
                    ),
            )
    }
}
