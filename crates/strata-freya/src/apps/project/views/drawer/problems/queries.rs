//! The Problems drawer's **Queries** scope: every open tab's live SQL diagnostics, grouped by
//! the tab they belong to.
//!
//! ## Live, not a log
//!
//! Diagnostics self-clear by construction, which is why there is no Clear button here and no
//! dismissal state to build (`DEV_TASKS` U10). Each validation pass replaces a tab's slice
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
//! A failed **run**. This scope is the SQL-validation surface; a query failure belongs to a run,
//! not to the text, and the results pane renders it in full — banner, code frame, caret, hint —
//! from `QueryError`. Folding it in would mean either a copy of the error on the store that
//! outlives the run it describes, or one freya-query subscription per tab in the drawer *and*
//! in the rail badge.
//!
//! A failed **write**, or a def the engine refused: those are conditions about the *project*
//! rather than about a query's text, and they are the [`Project`](super::project::Project)
//! scope's (P4-15).

use freya::prelude::*;
use freya::radio::use_radio;
use strata_core::util::plural;
use strata_model::{Diagnostic, Severity, TabId};

use super::super::{DrawerBody, DrawerEmpty, DrawerTheme};
use super::{GROUP_HEIGHT, PAD, ROW_HEIGHT, ROW_INDENT};
use crate::apps::project::state::{Chan, ProblemGroup, SessionState};
use crate::components::icon::{Icon, IconName};
use crate::components::tones::{tones, Tones};
use crate::components::typography::{Body, Caption, Control, Path};

#[derive(PartialEq)]
pub struct Queries {
    pub theme: DrawerTheme,
}

impl Component for Queries {
    fn render(&self) -> impl IntoElement {
        // The rows themselves…
        let session = use_radio::<SessionState, Chan>(Chan::Diagnostics);
        // …and the group labels: a tab's name is written on `Chan::Tabs`, so a rename relabels
        // its group without anything re-validating.
        let strip = use_radio::<SessionState, Chan>(Chan::Tabs);
        let tones = tones();

        let _ = strip.read();
        let groups = session.read().problem_groups();

        // No `DrawerCount` write here: the tally belongs to the scope **strip** above this body
        // (`super::Problems`), which has to know both scopes' counts to label its tabs and so is
        // the only place that can total them without a second walk of either store.
        let el: Element = match groups.is_empty() {
            true => DrawerEmpty::new(IconName::Check, "No problems found")
                .icon_color(tones.ok)
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
        let tally = plural(self.group.rows.len(), "problem");

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
/// for one line (`DEV_TASKS` U10). The owning tab comes from the **group**, not from the
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
