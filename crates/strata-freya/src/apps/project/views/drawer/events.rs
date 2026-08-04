//! The drawer's **Events** tab: the window's event log, newest first (P3-13).
//!
//! ## A log, not a live view
//!
//! The opposite of Problems in every way that matters. A problem is a live fact — derived from the
//! buffer and the catalog, replaced wholesale on every pass, and it retracts itself when the SQL is
//! fixed. An event is a *record*: it describes something that already finished, nothing can
//! re-derive it, and nothing can retract it. That is why this tab is the one with a **Clear** (the
//! shell's rule — see [`super`]), and why its rows are flat: there is no owning tab to group by and
//! nothing to jump to. A run's error appears here as one line of history; the run's own results
//! pane is where it is rendered in full.
//!
//! ## The severity ramp is the shared `tones()`
//!
//! A [`LogLevel`]'s dot is `success` / `info` / `warning` / `error` through the shared `tones()`
//! hook, like Problems' glyphs and the status bar's state dot: those four are semantic, and a
//! semantic colour follows the app-wide ramp wherever it appears (AGENTS.md §3). Everything else
//! the tab paints — the message, the timestamp, the row rule, the empty state — is the `drawer`
//! theme's.

use freya::prelude::*;

use super::{DrawerBody, DrawerCount, DrawerEmpty, DrawerTheme};
use crate::apps::project::state::{LogCtx, LogLevel};
use crate::components::divider::Divider;
use crate::components::dot::Dot;
use crate::components::icon::IconName;
use crate::components::tones::{tones, Tones};
use crate::components::typography::{Body, Meta};

/// A row's vertical padding (canvas `--sp-3`) and the panel's horizontal one (`--sp-4`).
const ROW_PAD_Y: f32 = 8.;
const PAD: f32 = 12.;
/// The severity dot's diameter, and the nudge that sits it on the message's **first** line rather
/// than centred against a message that may have wrapped to three.
const DOT: f32 = 6.;
const DOT_OFFSET: f32 = 4.;
/// The same idea for the timestamp, a smaller face that sits a little higher.
const TS_OFFSET: f32 = 2.;

/// The dot's tone for a [`LogLevel`] — the shared semantic ramp keyed by the entry's level.
fn tone_of(tones: Tones, level: LogLevel) -> Color {
    match level {
        LogLevel::Ok => tones.ok,
        LogLevel::Info => tones.info,
        LogLevel::Warning => tones.warning,
        LogLevel::Error => tones.error,
    }
}

#[derive(PartialEq)]
pub struct Events {
    pub theme: DrawerTheme,
    pub count: DrawerCount,
}

impl Component for Events {
    fn render(&self) -> impl IntoElement {
        let log = use_consume::<LogCtx>();
        let tones = tones();
        // The header's tally, resolved by the mounted body (see `DrawerCount`) — which is also
        // what enables **Clear**, so the button and the number can't disagree about whether
        // there is anything to clear.
        let count = self.count;
        let shown = log.read().len();
        use_side_effect_with_deps(&shown, move |shown| {
            let mut count = count;
            if *count.peek() != *shown {
                count.set(*shown);
            }
        });
        use_drop(move || {
            let mut count = count;
            count.set(0);
        });

        if shown == 0 {
            // The rail's own Events glyph, in the default empty tone: "no events" is not a
            // severity, so there is no semantic colour to reach for here.
            return DrawerEmpty::new(IconName::Lines, "No events yet").into_element();
        }

        // Each row keyed by its append sequence: an event arriving at the top must not hand the
        // row below it a different scope.
        let rows: Vec<Element> = log
            .read()
            .events()
            .map(|event| {
                EventRow {
                    tone: tone_of(tones, event.level),
                    // An error's message wears the error ramp too (the canvas tints the whole
                    // row): the dot alone is 6px, and a failure is the one thing worth finding
                    // by eye in a scrollback.
                    error: event.level == LogLevel::Error,
                    error_color: tones.error,
                    message: event.message.clone(),
                    at: event.at.clone(),
                    theme: self.theme.clone(),
                    key: DiffKey::None,
                }
                .key(event.seq)
                .into_element()
            })
            .collect();

        DrawerBody::new().children(rows).into_element()
    }
}

/// One event: severity dot · message · the local time it was recorded, over a bottom rule.
///
/// The message **wraps** (the canvas's `word-break: break-word`) rather than truncating: an engine
/// error is a sentence, and this is the surface that keeps it after the run it belonged to is gone.
/// That is also why the dot and the timestamp are top-aligned with a nudge rather than centred.
#[derive(PartialEq)]
struct EventRow {
    tone: Color,
    error: bool,
    error_color: Color,
    message: String,
    at: String,
    theme: DrawerTheme,
    key: DiffKey,
}

impl KeyExt for EventRow {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for EventRow {
    fn render(&self) -> impl IntoElement {
        let message = match self.error {
            true => self.error_color,
            false => self.theme.message_color,
        };

        rect()
            .width(Size::fill())
            .vertical()
            .child(
                rect()
                    .width(Size::fill())
                    .horizontal()
                    .content(Content::Flex)
                    .spacing(8.)
                    .padding((ROW_PAD_Y, PAD))
                    .child(
                        rect()
                            .margin(Gaps::new(DOT_OFFSET, 0., 0., 0.))
                            .child(Dot::new(self.tone).size(DOT)),
                    )
                    .child(
                        Body::new(self.message.clone())
                            .color(message)
                            .width(Size::flex(1.))
                            .wrap(),
                    )
                    .child(
                        rect()
                            .margin(Gaps::new(TS_OFFSET, 0., 0., 0.))
                            .child(Meta::new(self.at.clone()).color(self.theme.meta_color)),
                    ),
            )
            .child(Divider::horizontal().color(self.theme.divider_fill))
    }

    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }
}

/// The Events body driven the way the drawer drives it — over a real `Log`, under the real theme.
///
/// Layout is the point of most of these. The rows are the app's only **wrapping** list rows, in a
/// `Content::Flex` row alongside two hugging children, and an engine error is exactly the long
/// message that has to survive it; a regression there clips the sentence the log exists to keep.
#[cfg(test)]
mod tests {
    use freya::components::get_theme;
    use freya_testing::TestingRunner;
    use strata_core::theme::load;

    use super::super::{DrawerThemePartial, DrawerThemePreference};
    use super::*;
    use crate::apps::project::state::Log;
    use crate::theme::strata_theme;

    /// A short message, and one long enough to need two lines in a 420pt drawer.
    const SHORT: &str = "Opened project 'sales'";
    const LONG: &str = "Table 'events' failed to register: No files found at \
                        '/data/warehouse/events/year=2024', and the parent directory is not \
                        readable either";

    fn app() -> impl IntoElement {
        use_init_theme(|| strata_theme(&load("midnight")));
        let theme = get_theme!(&None::<DrawerThemePartial>, DrawerThemePreference, "drawer");
        let count = use_consume::<DrawerCount>();
        rect().expanded().child(Events { theme, count })
    }

    /// The log and the header's count slot — what the shell owns and hands the body.
    fn runner() -> (TestingRunner, (LogCtx, DrawerCount)) {
        TestingRunner::new(
            app,
            (420., 200.).into(),
            |r| {
                let log = r.provide_root_context(|| State::create(Log::default()));
                let count = r.provide_root_context(|| State::create(0usize));
                (log, count)
            },
            1.,
        )
    }

    /// Settle the tree *and its effects*. The count is written by a `use_side_effect_with_deps`,
    /// which runs in a spawned task woken through a `ReactiveContext` — so a render and the effect
    /// it dirties are several polls apart. The app polls continuously; a test has to say so.
    fn settle(runner: &mut TestingRunner) {
        for _ in 0..4 {
            runner.sync_and_update();
        }
    }

    /// Every text run in the tree, top to bottom.
    fn texts(runner: &TestingRunner) -> Vec<String> {
        runner.find_many(|_, element| Label::try_downcast(element).map(|l| l.text.to_string()))
    }

    /// The laid-out area of the run whose text is `text`.
    fn area_of(runner: &TestingRunner, text: &str) -> freya::prelude::Area {
        runner
            .find_many(|node, element| {
                Label::try_downcast(element)
                    .filter(|l| l.text == text)
                    .map(|_| node.layout().area)
            })
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("no text run for {text:?}"))
    }

    /// Nothing recorded yet: the centred empty state, and the header's count left at zero rather
    /// than printing one.
    #[test]
    fn an_empty_log_shows_the_empty_state() {
        let (mut runner, (_, count)) = runner();
        settle(&mut runner);

        assert!(texts(&runner).iter().any(|t| t == "No events yet"));
        assert_eq!(*count.peek(), 0);
    }

    /// Rows are newest-first, and the body resolves the header's tally.
    #[test]
    fn events_render_newest_first_and_resolve_the_count() {
        let (mut runner, (mut log, count)) = runner();
        settle(&mut runner);

        log.write().push(LogLevel::Info, "first");
        log.write().push(LogLevel::Ok, "second");
        settle(&mut runner);

        let shown: Vec<String> = texts(&runner)
            .into_iter()
            .filter(|t| t == "first" || t == "second")
            .collect();
        assert_eq!(shown, ["second", "first"], "newest at the top");
        assert_eq!(*count.peek(), 2);
    }

    /// **The wrapping row.** A long message must lay out over more than one line — and its row
    /// must grow with it, rather than the sentence being clipped to the height of a short one.
    #[test]
    fn a_long_message_wraps_instead_of_being_clipped() {
        let (mut runner, (mut log, _)) = runner();
        settle(&mut runner);

        log.write().push(LogLevel::Info, SHORT);
        log.write().push(LogLevel::Error, LONG);
        settle(&mut runner);

        let short = area_of(&runner, SHORT);
        let long = area_of(&runner, LONG);
        assert!(
            long.height() > short.height() * 1.5,
            "the long message should occupy at least two lines: {} vs {}",
            long.height(),
            short.height()
        );
        // And it stays inside the drawer rather than running off the side, which is what a
        // non-wrapping flex child would do.
        assert!(
            long.max_x() <= 420.,
            "the message overflowed the panel width: {}",
            long.max_x()
        );
    }

    /// Clearing the log (the drawer header's Clear) empties the list and resets the tally.
    #[test]
    fn clearing_the_log_empties_the_tab() {
        let (mut runner, (mut log, count)) = runner();
        settle(&mut runner);
        log.write().push(LogLevel::Error, "boom");
        settle(&mut runner);
        assert_eq!(*count.peek(), 1);

        log.write().clear();
        settle(&mut runner);

        assert!(texts(&runner).iter().any(|t| t == "No events yet"));
        assert_eq!(*count.peek(), 0);
    }
}
