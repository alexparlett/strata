//! The **properties grid** — Freya's built-in `Table`, dressed for a JetBrains-style Name/Value
//! editor.
//!
//! It is the built-in and not a hand-rolled lookalike (AGENTS.md §3), which cost four additions
//! in the fork rather than four workarounds here: `TableRow::theme` (a `pub` field that had no
//! builder, so a row could not carry its own selection or zebra fill, nor opt out of the hover
//! response a selectable table does not want), `TableRow::on_press` (the canvas selects a row by
//! pressing anywhere in it, not per cell), `TableCell::main_align` (the default is
//! `Alignment::End`, right for the numeric columns a table usually holds and wrong for two text
//! ones) and flex content on `Table` itself (it accepts a `Size` for its height and could not
//! then hand any of it to a scrolling body).
//!
//! What is composed *inside* the parts stays here: the column rule, the invalid-row stripe, and
//! the error message — a full-width sibling between rows rather than a cell, because the fault
//! belongs to the property rather than to its name or its value. The header is a `TableRow` too,
//! which is what gives it the strip's fill, the rule beneath it and the shared column widths for
//! nothing.

use freya::prelude::*;
use strata_core::engine::config::is_restart_key;

use crate::apps::settings::views::engine::model::{KeyStatus, PropRows};
use crate::apps::settings::{SettingsTheme, SettingsThemePartial, SettingsThemePreference};
use crate::components::divider::Divider;
use crate::components::form::ValueField;
use crate::components::icon::{Icon, IconName};
use crate::components::typography::{Caption, Control, MonoValue};

/// The header strip (canvas `height: 32px`) and a property row (`height: 34px`).
const HEAD_HEIGHT: f32 = 32.;
const ROW_HEIGHT: f32 = 34.;
/// The stripe down an invalid row's leading edge (canvas `border-left: 2px`).
const ERROR_STRIPE: f32 = 2.;
/// A cell's inset (canvas `padding: 0 var(--sp-3)`), and the header's (`0 var(--sp-4)`).
const CELL_INSET: f32 = 12.;
const HEAD_INSET: f32 = 16.;
/// The round marker on a row carrying a runtime property.
const MARKER_SIZE: f32 = 18.;
/// The empty grid's own floor, so it still reads as a table (canvas `min-height: 132px`).
const EMPTY_HEIGHT: f32 = 130.;
/// Alpha of the wash behind an error message, and of the tint behind the restart marker.
const ERROR_WASH_ALPHA: u8 = 20;
const MARKER_TINT_ALPHA: u8 = 38;
/// How wide the autocomplete panel stands. A property name is long and the box it hangs off is
/// half a pane wide, so without a floor every suggestion would truncate to its namespace.
const SUGGEST_WIDTH: f32 = 380.;

/// The grid: a pinned header over the rows, scrolling, filling whatever height the pane gives it.
#[derive(PartialEq)]
pub struct PropTable {
    pub rows: State<PropRows>,
}

impl Component for PropTable {
    fn render(&self) -> impl IntoElement {
        let rows = self.rows;
        let list = rows.read();
        let errors = list.errors();

        let mut body = TableBody::new();
        for (index, row) in list.rows().iter().enumerate() {
            let error = errors.get(&row.id).cloned();
            body = body
                .child(
                    PropTableRow {
                        rows,
                        id: row.id,
                        name: row.name.clone(),
                        value: row.value.clone(),
                        index,
                        invalid: error.is_some(),
                        key: DiffKey::None,
                    }
                    .key(row.id),
                )
                .maybe_child(error.map(|message| ErrorStrip { message }));
        }

        Table::new()
            .height(Size::flex(1.))
            .column_widths(vec![Size::flex(1.), Size::flex(1.)])
            .child(TableHead::new().child(HeadRow))
            .child(
                ScrollView::new().height(Size::flex(1.)).child(
                    rect()
                        .width(Size::fill())
                        .vertical()
                        .maybe_child(list.is_empty().then_some(EmptyGrid))
                        .child(body),
                ),
            )
    }
}

/// The `Name` / `Value` strip. A `TableRow` so it shares the column widths and the rule under it;
/// its own theme gives it the raised fill and pins the hover fill to the same colour, because a
/// header is not a row you can pick.
#[derive(PartialEq)]
struct HeadRow;

impl Component for HeadRow {
    fn render(&self) -> impl IntoElement {
        let theme = settings_theme();
        let rule = use_theme().read().colors().border;
        let head = theme.table_head_background;

        TableRow::new()
            .theme(TableThemePartial {
                row_background: Some(head.into()),
                hover_row_background: Some(head.into()),
                ..Default::default()
            })
            .child(
                TableCell::new()
                    .height(Size::px(HEAD_HEIGHT))
                    .padding(Gaps::new_all(0.))
                    .main_align(Alignment::Start)
                    .child(
                        rect()
                            .expanded()
                            .horizontal()
                            .content(Content::Flex)
                            .cross_align(Alignment::Center)
                            .child(
                                rect()
                                    .width(Size::flex(1.))
                                    .padding(Gaps::new(0., 0., 0., HEAD_INSET))
                                    .child(
                                        Control::new("Name").color(theme.item_active_color),
                                    ),
                            )
                            .child(Divider::vertical().color(rule)),
                    ),
            )
            .child(
                TableCell::new()
                    .height(Size::px(HEAD_HEIGHT))
                    .padding(Gaps::new(0., HEAD_INSET, 0., HEAD_INSET))
                    .main_align(Alignment::Start)
                    .child(Control::new("Value").color(theme.item_active_color)),
            )
    }
}

/// One property: its two boxes, and the two things a row says about itself — that it is selected,
/// and that it is invalid.
///
/// **The boxes are the source; the list is downstream.** Each holds its own `State<String>`,
/// seeded when the row mounts, and an effect pushes every change into the list. Nothing writes
/// the list back into a box: a two-way binding would fight the user, since each keystroke wakes
/// the whole grid and a re-seed on that wake would drag the cursor back to where the list thinks
/// it should be. The autocomplete therefore fills the *box*, not the list — the same one
/// direction of travel `NumberField` holds, and for the same reason.
///
/// The row is keyed on its id, so the paths that replace rows wholesale (paste, revert, remove)
/// remount the boxes on their new values instead of leaving them showing the old ones.
///
/// The row's press selects it; pressing *into* a box selects it too, because a built-in control's
/// press reaches its ancestors (AGENTS.md §3) — the one time that propagation is what you want
/// rather than the bug it usually is.
#[derive(PartialEq)]
struct PropTableRow {
    rows: State<PropRows>,
    id: u64,
    name: String,
    value: String,
    index: usize,
    invalid: bool,
    key: DiffKey,
}

impl KeyExt for PropTableRow {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for PropTableRow {
    fn render(&self) -> impl IntoElement {
        let theme = settings_theme();
        let theme_colors = use_theme();
        let colors = theme_colors.read().colors().clone();
        let mut rows = self.rows;
        let id = self.id;

        let mut name = use_state({
            let seed = self.name.clone();
            move || seed
        });
        let value = use_state({
            let seed = self.value.clone();
            move || seed
        });
        // The name box's id is ours, so the suggestions below can watch it take and lose focus.
        let a11y_id = use_a11y();
        let focus = use_focus(a11y_id);

        // Push each box into the list. `peek`, so the effect depends on its box alone — and
        // guarded, or the write would wake this row, whose effect would then run again and cost
        // a second pass over the grid per keystroke.
        use_side_effect(move || {
            let typed = name.read().clone();
            if rows.peek().name_of(id).as_deref() != Some(typed.as_str()) {
                rows.write().set_name(id, typed);
            }
        });
        use_side_effect(move || {
            let typed = value.read().clone();
            if rows.peek().value_of(id).as_deref() != Some(typed.as_str()) {
                rows.write().set_value(id, typed);
            }
        });

        let key = name.read().trim().to_string();
        let restart = is_restart_key(&key);
        // One lookup for the whole row, matching what the inspector says about the same name.
        // Three tones, because there are three answers: a key the catalogue doesn't know may
        // simply be newer than this build (warning), while a reserved one is refused outright and
        // the row is already carrying an error for it.
        let name_color = match KeyStatus::of(&key) {
            KeyStatus::Blank | KeyStatus::Known(_) => colors.text_primary,
            KeyStatus::Custom => colors.warning,
            KeyStatus::Reserved => colors.error,
        };

        // Suggestions are open exactly while the box has focus and the catalogue has something
        // left to offer. Picking a name fills the box, which empties the list, which closes the
        // panel — one condition, rather than an open flag to keep in step with it.
        let suggestions: Vec<(&'static str, &'static str)> = match focus() {
            Focus::Not => Vec::new(),
            _ => rows
                .read()
                .suggestions(id)
                .into_iter()
                .map(|entry| (entry.key, entry.default))
                .collect(),
        };
        let mut menu = Menu::new().min_width(Size::px(SUGGEST_WIDTH));
        for (key, default) in suggestions.iter().copied() {
            menu = menu.child(
                MenuButton::new()
                    .on_press(move |_: Event<PressEventData>| name.set(key.to_string()))
                    .child(SuggestionRow { key, default }),
            );
        }

        // Selection outranks the zebra stripe, and both pin the hover fill to themselves: a row
        // that answers a press with a selection must not also light up as the pointer crosses it.
        let selected = rows.read().selected == Some(id);
        let fill = match (selected, self.index % 2 == 1) {
            (true, _) => theme.table_selection_background,
            (false, true) => theme.table_zebra_background,
            (false, false) => Color::TRANSPARENT,
        };

        TableRow::new()
            .theme(TableThemePartial {
                row_background: Some(fill.into()),
                hover_row_background: Some(fill.into()),
                ..Default::default()
            })
            .on_press(move |_: Event<PressEventData>| rows.write().selected = Some(id))
            .child(
                TableCell::new()
                    .height(Size::px(ROW_HEIGHT))
                    .padding(Gaps::new(0., CELL_INSET, 0., 0.))
                    .main_align(Alignment::Start)
                    .child(
                        rect()
                            .expanded()
                            .horizontal()
                            .content(Content::Flex)
                            .cross_align(Alignment::Center)
                            .spacing(CELL_INSET - ERROR_STRIPE)
                            // The invalid marker. A painted rect and not a border: torin draws a
                            // border inside bounds the box already fills (AGENTS.md §3), so it
                            // would be the one edge you could not see.
                            .child(
                                rect()
                                    .width(Size::px(ERROR_STRIPE))
                                    .height(Size::fill())
                                    .background(match self.invalid {
                                        true => colors.error,
                                        false => Color::TRANSPARENT,
                                    }),
                            )
                            .child(
                                Attached::new(
                                    // The tone is set on the wrapper: `Input` paints no colour of
                                    // its own, so its text takes the ambient one.
                                    rect()
                                        .width(Size::flex(1.))
                                        .color(name_color)
                                        .child(
                                            ValueField::new(name)
                                                .bare()
                                                .placeholder("datafusion.")
                                                .height(Size::px(ROW_HEIGHT))
                                                .width(Size::fill())
                                                .a11y_id(a11y_id),
                                        ),
                                )
                                .bottom()
                                .align_start()
                                .maybe_child((!suggestions.is_empty()).then_some(menu)),
                            )
                            .maybe_child(restart.then(|| RestartMarker {
                                color: colors.warning,
                            }))
                            .child(Divider::vertical().color(colors.border)),
                    ),
            )
            .child(
                TableCell::new()
                    .height(Size::px(ROW_HEIGHT))
                    .padding(Gaps::new(0., CELL_INSET, 0., CELL_INSET))
                    .main_align(Alignment::Start)
                    .child(
                        ValueField::new(value)
                            .bare()
                            .placeholder("value")
                            .height(Size::px(ROW_HEIGHT))
                            .width(Size::fill()),
                    ),
            )
    }
}

/// One catalogue offer: the key, and the default it falls back to.
#[derive(PartialEq)]
struct SuggestionRow {
    key: &'static str,
    default: &'static str,
}

impl Component for SuggestionRow {
    fn render(&self) -> impl IntoElement {
        let theme = settings_theme();

        rect()
            .width(Size::fill())
            .horizontal()
            .content(Content::Flex)
            .cross_align(Alignment::Center)
            .spacing(16.)
            .child(
                MonoValue::new(self.key)
                    .color(theme.item_active_color)
                    .width(Size::flex(1.))
                    .text_overflow(TextOverflow::Ellipsis),
            )
            .child(
                Caption::new(match self.default {
                    "" => "(empty)",
                    default => default,
                })
                .color(theme.hint_color),
            )
    }
}

/// The round warm marker on a runtime property — it is recorded now and takes effect when the
/// engine restarts, which is a thing worth saying on the row rather than only in the inspector.
#[derive(PartialEq)]
struct RestartMarker {
    color: Color,
}

impl Component for RestartMarker {
    fn render(&self) -> impl IntoElement {
        rect()
            .width(Size::px(MARKER_SIZE))
            .height(Size::px(MARKER_SIZE))
            .corner_radius(CornerRadius::new_all(MARKER_SIZE / 2.))
            .center()
            .background(self.color.with_a(MARKER_TINT_ALPHA))
            .child(Icon::new(IconName::Reload).size(11.).color(self.color))
    }
}

/// Why the row above cannot be applied.
#[derive(PartialEq)]
struct ErrorStrip {
    message: String,
}

impl Component for ErrorStrip {
    fn render(&self) -> impl IntoElement {
        let error = use_theme().read().colors().error;

        rect()
            .width(Size::fill())
            .horizontal()
            .spacing(6.)
            .padding(Gaps::new(6., HEAD_INSET, 6., HEAD_INSET))
            .background(error.with_a(ERROR_WASH_ALPHA))
            .child(Icon::new(IconName::Alert).size(12.).color(error))
            .child(Caption::new(self.message.clone()).color(error).wrap())
    }
}

/// No overrides at all. A statement rather than an absence, so the grid says it rather than
/// standing there as two empty columns.
#[derive(PartialEq)]
struct EmptyGrid;

impl Component for EmptyGrid {
    fn render(&self) -> impl IntoElement {
        let theme = settings_theme();

        rect()
            .width(Size::fill())
            .height(Size::px(EMPTY_HEIGHT))
            .center()
            .vertical()
            .spacing(6.)
            .child(
                Icon::new(IconName::Lines)
                    .size(22.)
                    .color(theme.chevron_color),
            )
            .child(
                Caption::new("No properties. The engine uses its defaults.")
                    .color(theme.hint_color),
            )
    }
}

fn settings_theme() -> SettingsTheme {
    get_theme!(
        &None::<SettingsThemePartial>,
        SettingsThemePreference,
        "settings"
    )
}
