//! The drawer's **Problems** tab: every open tab's live SQL diagnostics, grouped by the tab
//! they belong to.
//!
//! ## Live, not a log
//!
//! Diagnostics self-clear by construction, which is why there is no Clear button here and no
//! dismissal state to build (DEV_TASKS U10). Each validation pass replaces a tab's slice
//! **wholesale**, so fixing the SQL — or fixing the catalog the SQL reads — retracts the rows on
//! the next pass, without the user opening the tab.
//!
//! ## A pure view
//!
//! Everything on screen comes from [`SessionState::problem_groups`]: the window's one driver
//! (`state::diagnostics`) is the only thing that writes diagnostics, and this reads them. Two
//! subscriptions, both cross-tab, which is exactly why `Chan::Diagnostics` is not per tab.
//!
//! ## What is deliberately not here
//!
//! A failed **run**. Problems is the SQL-validation surface; a query failure belongs to a run,
//! not to the text, and the results pane renders it in full — banner, code frame, caret, hint —
//! from `QueryError`. Folding it in would mean either a copy of the error on the store that
//! outlives the run it describes, or one freya-query subscription per tab in the drawer *and*
//! in the rail badge.

use freya::prelude::*;
use freya::radio::use_radio;
use strata_model::{Diagnostic, Severity, TabId};

use super::{DrawerBody, DrawerCount, DrawerEmpty, DrawerTheme};
use crate::apps::project::state::{Chan, ProblemGroup, SessionState};
use crate::components::icon::{Icon, IconName};
use crate::components::typography::{Body, Caption, Control, Path};

/// One problem row, and the group header above it (canvas `--sp-2` / `--sp-3` verticals).
const ROW_HEIGHT: f32 = 26.;
const GROUP_HEIGHT: f32 = 32.;
/// A row's left indent — the canvas's `--sp-7`, so rows sit under their group's name.
const ROW_INDENT: f32 = 32.;
/// The panel's horizontal padding (canvas `--sp-4`).
const PAD: f32 = 12.;

/// The semantic tones a diagnostic wears, plus the clean state's tick. Read straight off the
/// **sheet** rather than restated on the drawer's own theme: `error` / `warning` / `info` /
/// `success` must follow the app-wide ramp wherever they appear (AGENTS.md §3).
#[derive(Clone, Copy, PartialEq)]
struct Tones {
    error: Color,
    warning: Color,
    info: Color,
    ok: Color,
}

/// Resolve [`Tones`] from the active theme. A **hook** (one theme read), so call it exactly once
/// per render — like `type_palette`.
fn tones() -> Tones {
    let theme = use_theme();
    let t = theme.read();
        let c = t.colors();
    Tones {
        error: c.error,
        warning: c.warning,
        info: c.info,
        ok: c.success,
    }
}

#[derive(PartialEq)]
pub struct Problems {
    pub theme: DrawerTheme,
    pub count: DrawerCount,
}

impl Component for Problems {
    fn render(&self) -> impl IntoElement {
        // The rows themselves…
        let session = use_radio::<SessionState, Chan>(Chan::Diagnostics);
        // …and the group labels: a tab's name is written on `Chan::Tabs`, so a rename relabels
        // its group without anything re-validating.
        let strip = use_radio::<SessionState, Chan>(Chan::Tabs);
        let tones = tones();

        let _ = strip.read();
        let groups = session.read().problem_groups();
        // Counted off the groups in hand rather than walking the store a second time. The two
        // agree by construction: `error_count` spans validated tabs, and a validated tab with
        // no rows contributes none — so it can only ever be the errors among these rows.
        let errors = groups
            .iter()
            .flat_map(|g| g.rows.iter())
            .filter(|d| d.is_error())
            .count();

        // The header's tally, resolved by the mounted body (see `DrawerCount`).
        let count = self.count;
        use_side_effect_with_deps(&errors, move |errors| {
            let mut count = count;
            if *count.peek() != *errors {
                count.set(*errors);
            }
        });
        use_drop(move || {
            let mut count = count;
            count.set(0);
        });

        let el: Element = match groups.is_empty() {
            true => DrawerEmpty::new(IconName::Check, "No problems found")
                .icon_color(tones.ok)
                .color(self.theme.empty_color)
                .into_element(),
            false => DrawerBody::new()
                .children(groups.into_iter().map(|group| {
                    // Keyed by the tab, so a group appearing or clearing above another doesn't
                    // shuffle the rest through each other's scopes.
                    let tab = group.tab;
                    Group {
                        group,
                        theme: self.theme.clone(),
                        tones,
                        key: DiffKey::None,
                    }
                    .key(tab)
                    .into_element()
                }))
                .into_element(),
        };
        el
    }
}

/// One tab's block: a header naming it and tallying its rows, then the rows.
#[derive(PartialEq)]
struct Group {
    group: ProblemGroup,
    theme: DrawerTheme,
    tones: Tones,
    key: DiffKey,
}

impl KeyExt for Group {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for Group {
    fn render(&self) -> impl IntoElement {
        let tally = match self.group.rows.len() {
            1 => "1 problem".to_string(),
            n => format!("{n} problems"),
        };

        rect()
            .width(Size::fill())
            .vertical()
            .child(
                rect()
                    .width(Size::fill())
                    .height(Size::px(GROUP_HEIGHT))
                    .horizontal()
                    .cross_align(Alignment::Center)
                    .spacing(8.)
                    .padding((0., PAD))
                    .child(
                        Icon::new(IconName::File)
                            .color(self.theme.group_icon_color)
                            .size(14.),
                    )
                    .child(Control::new(self.group.name.clone()).color(self.theme.group_color))
                    .child(Caption::new(tally).color(self.theme.meta_color)),
            )
            .children(self.group.rows.iter().map(|d| {
                ProblemRow {
                    tab: self.group.tab,
                    diagnostic: d.clone(),
                    theme: self.theme.clone(),
                    tones: self.tones,
                }
                .into_element()
            }))
    }

    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }
}

/// One problem: severity glyph · message · the `line L:C` it was reported at, and a press that
/// takes you to the tab it belongs to (the canvas's `onProblemJump`).
///
/// No code chip — the canvas's row is those three, and a rule code was a fourth thing competing
/// for one line (DEV_TASKS U10). The owning tab comes from the **group**, not from the
/// diagnostic: a `Diagnostic` deliberately carries no `TabId`, so there is no second copy to
/// disagree with the tab it is stored on.
#[derive(PartialEq)]
struct ProblemRow {
    tab: TabId,
    diagnostic: Diagnostic,
    theme: DrawerTheme,
    tones: Tones,
}

impl Component for ProblemRow {
    fn render(&self) -> impl IntoElement {
        let mut session = use_radio::<SessionState, Chan>(Chan::Tabs);
        let (glyph, tone) = match self.diagnostic.severity {
            Severity::Error => (IconName::Alert, self.tones.error),
            Severity::Warning => (IconName::Warning, self.tones.warning),
            Severity::Info => (IconName::Info, self.tones.info),
        };
        let tab = self.tab;

        rect()
            .width(Size::fill())
            .height(Size::px(ROW_HEIGHT))
            .horizontal()
            .content(Content::Flex)
            .cross_align(Alignment::Center)
            .spacing(8.)
            .padding(Gaps::new(0., PAD, 0., ROW_INDENT))
            // Switching to the tab, not jumping to the span: the span is a byte range into the
            // text the pass validated, and moving the caret is the editor's delicate half
            // (AGENTS.md §8) — its own change.
            .on_press(move |_| {
                session.write_channel(Chan::Tabs).switch(tab);
            })
            .child(Icon::new(glyph).color(tone).size(15.))
            .child(
                Body::new(self.diagnostic.message.clone())
                    .color(self.theme.message_color)
                    .width(Size::flex(1.))
                    .text_overflow(TextOverflow::Ellipsis),
            )
            .maybe_child(
                self.diagnostic
                    .loc
                    .clone()
                    .map(|loc| Path::new(loc).color(self.theme.meta_color)),
            )
    }
}
