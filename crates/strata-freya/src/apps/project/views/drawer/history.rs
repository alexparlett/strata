//! The drawer's **History** tab: the queries this project has run, newest first (P3-14).
//!
//! ## A record you can act on
//!
//! Events' sibling — both are logs of finished things, neither can be re-derived, and both own a
//! **Clear** — with one difference that shapes the whole surface: a history entry is *still
//! runnable*. So a row is a pressable card rather than a line of text: a press loads its SQL into
//! the active tab, a double-press loads it and runs it. Both go through the editor's own actions
//! ([`actions::load_sql`] / [`actions::press_query`]), so a query re-run from here is the same
//! press as one from the toolbar — same nonce, same cache entry, same keeper.
//!
//! ## Every row is a run that returned data
//!
//! The satellite records **only successful data runs** (`state::history`), which is why a row has
//! no status mark: the canvas's leading dot encodes ok / cancelled / failed, and a dot with one
//! possible value is decoration implying a distinction the data does not carry. What is left is
//! what was really measured — the run's elapsed time and row count, its SQL, and when it
//! finished. A failed run is not silently missing from a list that claims to be complete; it was
//! never history, and the Events tab beside this one is where it is recorded.
//!
//! ## The timestamp is coarse, and does not tick
//!
//! Resolved at render through [`ago`], the same helper the inspector's scan age uses. Nothing
//! re-renders this list on a clock: an age this coarse is still true minutes later, and a surface
//! that repaints itself once a second to keep a "2 min ago" honest costs more than the honesty is
//! worth.

use freya::prelude::*;
use freya::radio::{use_radio, Radio};
use strata_core::util::{ago, collapse_sql, fmt_int, now_secs, plural};

use super::super::workbench::editor::actions;
use super::{DrawerBody, DrawerCount, DrawerEmpty, DrawerTheme};
use crate::apps::project::query::QueryMode;
use crate::apps::project::state::{Chan, HistoryCtx, SessionState};
use crate::components::badge::Badge;
use crate::components::icon::IconName;
use crate::components::metrics::{R_2, SP_1, SP_2, SP_3, SP_4};
use crate::components::typography::{Meta, Path};

/// A card's inner padding (canvas `--sp-3` / `--sp-4`) and its radius (`--r-2`).
const CARD_PAD_Y: f32 = SP_3;
const CARD_PAD_X: f32 = SP_4;
const CARD_RADIUS: f32 = R_2;
/// The inset that keeps a card's hover surface off the panel edges, and the gap between cards
/// (canvas `--sp-1`).
const LIST_PAD_X: f32 = SP_3;
const CARD_GAP: f32 = SP_1;
/// The gap between the meta line and the SQL under it (`--sp-2`), and within the meta line
/// itself (`--sp-3`).
const META_GAP: f32 = SP_2;
const META_SPACING: f32 = SP_3;
/// How many lines of the SQL preview are shown before it truncates — the canvas's
/// `-webkit-line-clamp: 2`.
const PREVIEW_LINES: usize = 2;

#[derive(PartialEq)]
pub struct History {
    pub theme: DrawerTheme,
    pub count: DrawerCount,
}

impl Component for History {
    fn render(&self) -> impl IntoElement {
        let history = use_consume::<HistoryCtx>();
        let session = use_radio::<SessionState, Chan>(Chan::Tabs);

        let count = self.count;
        let shown = history.read().entries.len();
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
            return DrawerEmpty::new(IconName::Clock, "No queries run yet").into_element();
        }

        let now = now_secs();
        let rows: Vec<Element> = history
            .read()
            .entries
            .iter()
            .map(|entry| {
                let preview = collapse_sql(&entry.sql);
                Row {
                    session,
                    sql: entry.sql.clone(),
                    preview: preview.clone(),
                    stats: format!(
                        "{} ms · {}",
                        fmt_int(entry.elapsed_ms),
                        plural(entry.rows as usize, "row")
                    ),
                    lines: line_count(&entry.sql),
                    at: ago(now.saturating_sub(entry.ts_ms / 1000)),
                    theme: self.theme.clone(),
                    key: DiffKey::None,
                }
                .key(&preview)
                .into_element()
            })
            .collect();

        DrawerBody::new().children(rows).into_element()
    }
}

/// How many lines the stored SQL has — the `3 lines` pill. `None` for a one-liner, which is the
/// canvas's rule and the honest one: "1 line" beside a preview that is visibly one line is a
/// label with nothing to say.
fn line_count(sql: &str) -> Option<usize> {
    match sql.trim().lines().count() {
        n if n > 1 => Some(n),
        _ => None,
    }
}

/// One past run: its figures, its line-count pill and its age over a two-line SQL preview.
///
/// Press loads, double-press loads and runs — both in the **one** `on_press` handler, because
/// `EventsCombos` is how a double-press is detected once the node already handles the press
/// (AGENTS.md §3: a second registration under the same event name replaces the first). The
/// double-press deliberately loads again before running: the first press of the pair already
/// loaded it, so this is a no-op write that keeps the two paths one sentence rather than two.
#[derive(PartialEq)]
struct Row {
    session: Radio<SessionState, Chan>,
    /// The stored text — what a press loads, formatting and all.
    sql: String,
    /// That text as one line — what the row shows, and its identity (see the list above).
    preview: String,
    stats: String,
    lines: Option<usize>,
    at: String,
    theme: DrawerTheme,
    key: DiffKey,
}

impl KeyExt for Row {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for Row {
    fn render(&self) -> impl IntoElement {
        let mut hovered = use_state(|| false);
        let session = self.session;
        let sql = self.sql.clone();

        rect()
            .width(Size::fill())
            .vertical()
            .margin(Gaps::new(0., LIST_PAD_X, CARD_GAP, LIST_PAD_X))
            .padding((CARD_PAD_Y, CARD_PAD_X))
            .corner_radius(CARD_RADIUS)
            .background(match hovered() {
                true => self.theme.row_hover_fill,
                false => Color::TRANSPARENT,
            })
            .on_pointer_enter(move |_| hovered.set(true))
            .on_pointer_leave(move |_| hovered.set(false))
            .on_press(move |e: Event<PressEventData>| {
                let double = match e.data() {
                    PressEventData::Mouse(m) => {
                        EventsCombos::pressed(m.global_location).is_double()
                    }
                    _ => false,
                };
                let Some(id) = session.read().active else {
                    return;
                };
                actions::load_sql(session, id, &sql);
                if double {
                    actions::press_query(session, id, QueryMode::Run);
                }
            })
            .child(
                rect()
                    .width(Size::fill())
                    .horizontal()
                    .cross_align(Alignment::Center)
                    .main_align(Alignment::SpaceBetween)
                    .margin(Gaps::new(0., 0., META_GAP, 0.))
                    .child(
                        rect()
                            .horizontal()
                            .cross_align(Alignment::Center)
                            .spacing(META_SPACING)
                            .child(Meta::new(self.stats.clone()).color(self.theme.value_color))
                            .maybe_child(self.lines.map(|n| {
                                Badge::value(plural(n, "line"), self.theme.meta_color)
                                    .outlined()
                                    .padding(Gaps::new(0., SP_2, 0., SP_2))
                            })),
                    )
                    .child(Meta::new(self.at.clone()).color(self.theme.meta_color)),
            )
            .child(
                Path::new(self.preview.clone())
                    .color(self.theme.message_color)
                    .width(Size::fill())
                    .max_lines(PREVIEW_LINES)
                    .text_overflow(TextOverflow::Ellipsis),
            )
    }

    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }
}

/// The History body driven the way the drawer drives it — over a real satellite and the real
/// theme, with a session store behind it so the two presses are testable end to end.
#[cfg(test)]
mod tests {
    use freya::components::get_theme;
    use freya::radio::RadioStation;
    use freya_testing::prelude::{MouseEventName, PlatformEvent};
    use freya_testing::TestingRunner;
    use std::path::Path as FsPath;
    use strata_core::theme::load;
    use strata_model::HistoryEntry;

    use super::super::{DrawerThemePartial, DrawerThemePreference};
    use super::*;
    use crate::apps::project::state::History as HistoryStore;
    use crate::theme::strata_theme;

    const ONE_LINER: &str = "SELECT * FROM events LIMIT 50";
    const MULTI: &str = "SELECT country,\n       sum(amount)\nFROM events\nGROUP BY 1";
    /// Long enough to need more than two lines in a 520pt drawer once collapsed.
    const LONG: &str = "WITH monthly AS (\n  SELECT date_trunc('month', e.ts) AS month, \
                        u.country, e.action, count(*) AS events, sum(e.amount) AS revenue\n  \
                        FROM events AS e JOIN users AS u ON u.user_id = e.user_id\n  \
                        WHERE e.ts >= '2024-01-01' GROUP BY 1, 2, 3\n)\nSELECT * FROM monthly \
                        ORDER BY month DESC, revenue DESC";

    fn app() -> impl IntoElement {
        use_init_theme(|| strata_theme(&load("midnight")));
        let theme = get_theme!(&None::<DrawerThemePartial>, DrawerThemePreference, "drawer");
        let count = use_consume::<DrawerCount>();
        rect().expanded().child(History { theme, count })
    }

    fn entry(sql: &str, elapsed_ms: u64, rows: u64) -> HistoryEntry {
        HistoryEntry {
            sql: sql.into(),
            ts_ms: 0,
            elapsed_ms,
            rows,
        }
    }

    type Handles = (HistoryCtx, DrawerCount, RadioStation<SessionState, Chan>);

    fn runner() -> (TestingRunner, Handles) {
        TestingRunner::new(
            app,
            (520., 400.).into(),
            |r| {
                let history = r.provide_root_context(|| {
                    State::create(HistoryStore::load(FsPath::new("/nonexistent"), 100))
                });
                let count = r.provide_root_context(|| State::create(0usize));
                let session = r.provide_root_context(|| {
                    RadioStation::<SessionState, Chan>::create(SessionState::default())
                });
                (history, count, session)
            },
            1.,
        )
    }

    fn settle(runner: &mut TestingRunner) {
        for _ in 0..4 {
            runner.sync_and_update();
        }
    }

    fn texts(runner: &TestingRunner) -> Vec<String> {
        runner.find_many(|_, element| Label::try_downcast(element).map(|l| l.text.to_string()))
    }

    /// The laid-out box of the first text run equal to `text`.
    fn text_area(runner: &TestingRunner, text: &str) -> Area {
        runner
            .find(|node, element| {
                Label::try_downcast(element)
                    .filter(|l| l.text == text)
                    .map(|_| node.layout().area)
            })
            .unwrap_or_else(|| panic!("no text run {text:?} in the tree"))
    }

    /// Click the centre of the first text run equal to `text`.
    fn click_text(runner: &mut TestingRunner, text: &str) {
        let area = text_area(runner, text);
        let point = (
            (area.min_x() + area.width() / 2.) as f64,
            (area.min_y() + area.height() / 2.) as f64,
        );
        runner.move_cursor(point);
        runner.click_cursor(point);
        settle(runner);
    }

    /// Right-click the centre of the first text run equal to `text` — a full press, both
    /// halves, because it is the *up* that Freya turns into a `PointerPress`.
    fn right_click_text(runner: &mut TestingRunner, text: &str) {
        let area = text_area(runner, text);
        let point = (
            (area.min_x() + area.width() / 2.) as f64,
            (area.min_y() + area.height() / 2.) as f64,
        );
        runner.move_cursor(point);
        for name in [MouseEventName::MouseDown, MouseEventName::MouseUp] {
            runner.send_event(PlatformEvent::Mouse {
                name,
                cursor: point.into(),
                button: Some(MouseButton::Right),
            });
            runner.sync_and_update();
        }
        settle(runner);
    }

    /// Nothing run yet: the centred empty state, and no count under the header.
    #[test]
    fn an_empty_history_shows_the_empty_state() {
        let (mut runner, (_, count, _)) = runner();
        settle(&mut runner);

        assert!(texts(&runner).iter().any(|t| t == "No queries run yet"));
        assert_eq!(*count.peek(), 0);
    }

    /// Rows are newest-first (the satellite's order), and the body resolves the header's tally.
    #[test]
    fn runs_render_newest_first_and_resolve_the_count() {
        let (mut runner, (mut history, count, _)) = runner();
        settle(&mut runner);

        history.write().entries.push_front(entry(ONE_LINER, 41, 3));
        history.write().entries.push_front(entry(MULTI, 88, 8));
        settle(&mut runner);

        let shown: Vec<String> = texts(&runner)
            .into_iter()
            .filter(|t| t.starts_with("SELECT"))
            .collect();
        assert_eq!(
            shown,
            [collapse_sql(MULTI), collapse_sql(ONE_LINER)],
            "newest at the top"
        );
        assert_eq!(*count.peek(), 2);
    }

    /// A row states what was measured: elapsed and rows, a line-count pill for a multi-line
    /// query and none for a one-liner, and the age of the run.
    #[test]
    fn a_row_states_its_figures_and_pills_only_multi_line_sql() {
        let (mut runner, (mut history, _, _)) = runner();
        settle(&mut runner);
        history.write().entries.push_front(entry(ONE_LINER, 41, 1));
        history
            .write()
            .entries
            .push_front(entry(MULTI, 1234, 8_000));
        settle(&mut runner);

        let texts = texts(&runner);
        assert!(texts.iter().any(|t| t == "1,234 ms · 8,000 rows"));
        assert!(texts.iter().any(|t| t == "41 ms · 1 row"));
        assert!(texts.iter().any(|t| t == "4 lines"), "{texts:?}");
        assert!(
            !texts.iter().any(|t| t == "1 line"),
            "a one-liner gets no pill"
        );
    }

    /// **The clamped preview.** A long query wraps to a second line and stops there, so one
    /// entry can't take the whole panel — and, collapsed first, those two lines are two lines of
    /// query rather than the first two lines of a formatted statement.
    #[test]
    fn a_long_query_previews_over_two_lines_at_most() {
        let (mut runner, (mut history, _, _)) = runner();
        settle(&mut runner);
        history.write().entries.push_front(entry(ONE_LINER, 41, 3));
        history.write().entries.push_front(entry(LONG, 214, 240));
        settle(&mut runner);

        let short = text_area(&runner, &collapse_sql(ONE_LINER));
        let long = text_area(&runner, &collapse_sql(LONG));
        assert!(
            long.height() > short.height() * 1.5,
            "the preview should use its second line: {} vs {}",
            long.height(),
            short.height()
        );
        assert!(
            long.height() < short.height() * 2.5,
            "and stop at two: {}",
            long.height()
        );
        assert!(
            long.max_x() <= 520.,
            "the preview overflowed the panel width: {}",
            long.max_x()
        );
    }

    /// A press loads the row's SQL into the **active** tab, replacing what it held.
    #[test]
    fn pressing_a_row_loads_it_into_the_active_tab() {
        let (mut runner, (mut history, _, session)) = runner();
        let mut store = session;
        let id = store.write_channel(Chan::Tabs).open_blank();
        history.write().entries.push_front(entry(MULTI, 88, 8));
        settle(&mut runner);

        click_text(&mut runner, &collapse_sql(MULTI));

        assert_eq!(session.peek().tabs.get(&id).unwrap().text(), MULTI);
        assert!(session.peek().request(id).is_none());
    }

    /// **A right-click does nothing at all** — the row loads on a *left* press only.
    ///
    /// Pinning a guarantee we rely on but do not implement: `on_press` filters to the left
    /// button inside Freya itself, so this row needs no check of its own. That is worth a test
    /// precisely because it is invisible here — we vendor the fork and do change it, and a
    /// widening of `on_press` would otherwise turn a stray right-click into a silent overwrite
    /// of the active tab's buffer (and, twice, a run, since `EventsCombos` keys on location and
    /// timing rather than on the button).
    #[test]
    fn right_clicking_a_row_neither_loads_nor_runs() {
        let (mut runner, (mut history, _, session)) = runner();
        let mut store = session;
        let id = store.write_channel(Chan::Tabs).open_blank();
        if let Some(t) = store.write_channel(Chan::Tab(id)).tabs.get_mut(&id) {
            t.editor.set_text("SELECT keep_me");
        }
        history.write().entries.push_front(entry(ONE_LINER, 41, 3));
        settle(&mut runner);

        right_click_text(&mut runner, &collapse_sql(ONE_LINER));
        right_click_text(&mut runner, &collapse_sql(ONE_LINER));

        assert_eq!(
            session.peek().tabs.get(&id).unwrap().text(),
            "SELECT keep_me",
            "a right-click must not touch the buffer"
        );
        assert!(
            session.peek().request(id).is_none(),
            "and a right double-click must not run anything"
        );
    }

    /// A double-press loads it **and** runs it, through the same trigger the toolbar sets.
    #[test]
    fn double_pressing_a_row_loads_and_runs_it() {
        let (mut runner, (mut history, _, session)) = runner();
        let mut store = session;
        let id = store.write_channel(Chan::Tabs).open_blank();
        history.write().entries.push_front(entry(ONE_LINER, 41, 3));
        settle(&mut runner);

        click_text(&mut runner, &collapse_sql(ONE_LINER));
        click_text(&mut runner, &collapse_sql(ONE_LINER));

        assert_eq!(session.peek().tabs.get(&id).unwrap().text(), ONE_LINER);
        let request = session.peek().request(id).cloned();
        assert_eq!(
            request.map(|r| r.sql),
            Some(ONE_LINER.to_string()),
            "the double-press should have pressed Run on the loaded SQL"
        );
    }

    /// Clearing the satellite (the drawer header's Clear) empties the list and resets the tally.
    #[test]
    fn clearing_the_history_empties_the_tab() {
        let (mut runner, (mut history, count, _)) = runner();
        settle(&mut runner);
        history.write().entries.push_front(entry(ONE_LINER, 41, 3));
        settle(&mut runner);
        assert_eq!(*count.peek(), 1);

        history.write().clear();
        settle(&mut runner);

        assert!(texts(&runner).iter().any(|t| t == "No queries run yet"));
        assert_eq!(*count.peek(), 0);
    }
}
