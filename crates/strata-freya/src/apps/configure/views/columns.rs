//! **COLUMNS** — what a internal table is made of (IT-01): the label with its `REQUIRED` marker,
//! the add/remove toolbar, and one row per declared column.
//!
//! Drawn only on [`Where::Internal`], and otherwise an empty box rather than an unmounted child —
//! `views::hive`'s rule, for the differ.
//!
//! ## It is SOURCE PATHS' list, with two columns instead of one
//!
//! Deliberately the same control: Freya's built-in `Table`, a selected-row fill, a `+`/`−`
//! toolbar of `ToolButton`s, and two-way-synced bare fields in the cells. The two sections answer
//! the same shape of question — a list of text the user types, one row at a time — and this
//! window already has an answer for it. A second dress here would be a second dress to keep in
//! step, and the reason the standalone panel this replaced looked wrong.
//!
//! ## The type box is free text, and the row says what the planner made of it
//!
//! There is no Arrow → SQL inverse to author a picker from: `convert_simple_data_type` is
//! many-to-one, and the same spelling reaches *different* Arrow types depending on session config
//! (`map_string_types_to_utf8view`, `execution.time_zone`). So nothing is declared — the planner
//! is asked, per row, debounced, and the third column shows the Arrow type this table will
//! actually carry or the planner's refusal in its own words. Deferring that to Save would mean
//! filling eight rows and then hunting for the one that was wrong.

use freya::prelude::*;

use crate::apps::configure::ConfigureCtx;
use crate::components::form::{form_theme, Row, ValueField, FIELD_HEIGHT};
use crate::components::icon::IconName;
use crate::components::metrics::{EMPTY_TABLE_HEIGHT, SP_3, SP_4};
use crate::components::tones::tones;
use crate::components::tool_button::ToolButton;
use crate::components::typography::{Meta, Prose};
use crate::components::window::window_theme;

/// The gap between the toolbar's buttons, and between the label row, the toolbar and the list —
/// SOURCE PATHS' own, because this is that section's control.
const TOOL_GAP: f32 = SP_3;
const STACK_GAP: f32 = SP_3;
/// A cell's inset — the properties grid's own (`padding: 0 var(--sp-3)`).
const CELL_INSET: f32 = SP_4;
/// The three columns' share of the row: the name and the type get equal room to type in, and the
/// verdict beside them a little more, because a planner refusal is a sentence.
const NAME_SHARE: f32 = 1.;
const TYPE_SHARE: f32 = 1.;
const VERDICT_SHARE: f32 = 1.3;

/// What the ⓘ says a column list is for.
const COLUMNS_HINT: &str = "Each row declares one column of a table Strata stores in this \
                            project. Columns are nullable: constraints and defaults are not \
                            supported. Add rows with INSERT once the table exists.";

#[derive(PartialEq)]
pub struct Columns;

impl Component for Columns {
    fn render(&self) -> impl IntoElement {
        let ctx = use_consume::<ConfigureCtx>();
        if !ctx.draft.read().internal() {
            return rect().into_element();
        }

        Row::new("COLUMNS")
            .required()
            .hint(COLUMNS_HINT)
            .child(
                rect()
                    .width(Size::fill())
                    .vertical()
                    .spacing(STACK_GAP)
                    .child(Toolbar)
                    .child(ColumnList),
            )
            .into_element()
    }
}

/// Add · remove — SOURCE PATHS' toolbar minus the Browse it has no use for.
#[derive(PartialEq)]
struct Toolbar;

impl Component for Toolbar {
    fn render(&self) -> impl IntoElement {
        let win = window_theme();
        let error = tones().error;
        let ctx = use_consume::<ConfigureCtx>();
        let removable = ctx.draft.read().columns.len() > 1;

        rect()
            .horizontal()
            .cross_align(Alignment::Center)
            .spacing(TOOL_GAP)
            .child(
                ToolButton::new(IconName::Plus, "Add column")
                    .outlined()
                    .color(win.icon_color)
                    .on_press(move |_| {
                        let mut selected = ctx.selected_column;
                        let mut at = *selected.peek();
                        ctx.edit(|draft| at = draft.add_column());
                        selected.set(at);
                    }),
            )
            .child(
                ToolButton::new(IconName::Minus, "Remove column")
                    .outlined()
                    .color(error)
                    .enabled(removable)
                    .on_press(move |_| {
                        let mut selected = ctx.selected_column;
                        let at = *selected.peek();
                        let mut next = at;
                        ctx.edit(|draft| next = draft.remove_column(at));
                        selected.set(next);
                    }),
            )
    }
}

/// The list of column rows, or its empty state.
#[derive(PartialEq)]
struct ColumnList;

impl Component for ColumnList {
    fn render(&self) -> impl IntoElement {
        let form = form_theme();
        let ctx = use_consume::<ConfigureCtx>();
        let (count, selected, faults, verdicts) = {
            let draft = ctx.draft.read();
            let probes = ctx.probes.read();
            let verdicts: Vec<Option<String>> = draft
                .columns
                .iter()
                .map(|column| match probes.get(column.sql_type()) {
                    Some(Ok(dtype)) => Some(dtype.clone()),
                    _ => None,
                })
                .collect();
            (
                draft.columns.len(),
                (*ctx.selected_column.read()).min(draft.columns.len().saturating_sub(1)),
                draft.column_faults(&probes),
                verdicts,
            )
        };

        if count == 0 {
            return Table::new().child(
                rect()
                    .width(Size::fill())
                    .height(Size::px(EMPTY_TABLE_HEIGHT))
                    .center()
                    .child(
                        Prose::new("No columns yet. Add one to declare this table.")
                            .color(form.hint_color),
                    ),
            );
        }

        let mut body = TableBody::new();
        for index in 0..count {
            body = body.child(
                ColumnRow {
                    index,
                    selected: index == selected,
                    fault: faults.get(&index).cloned(),
                    verdict: verdicts.get(index).cloned().flatten(),
                    key: DiffKey::None,
                }
                .key(index),
            );
        }

        Table::new()
            .column_widths(vec![
                Size::flex(NAME_SHARE),
                Size::flex(TYPE_SHARE),
                Size::flex(VERDICT_SHARE),
            ])
            .child(body)
    }
}

/// One row: the column's name, its SQL type, and what the planner made of that type.
#[derive(PartialEq)]
struct ColumnRow {
    index: usize,
    selected: bool,
    /// What is wrong with this row, if anything — resolved by the list (see [`ColumnList`]).
    fault: Option<String>,
    /// The Arrow type this row's box resolves to, once the planner has answered.
    verdict: Option<String>,
    key: DiffKey,
}

impl KeyExt for ColumnRow {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for ColumnRow {
    fn render(&self) -> impl IntoElement {
        let win = window_theme();
        let form = form_theme();
        let error = tones().error;
        let ctx = use_consume::<ConfigureCtx>();
        let index = self.index;

        let (initial_name, initial_type) = {
            let draft = ctx.draft.peek();
            let column = draft.columns.get(index).cloned().unwrap_or_default();
            (column.name, column.sql_type)
        };
        let name = use_state({
            let seed = initial_name.clone();
            move || seed
        });
        let mut name_reported = use_state(move || initial_name);
        let sql_type = use_state({
            let seed = initial_type.clone();
            move || seed
        });
        let mut type_reported = use_state(move || initial_type);

        let name_field = use_a11y();
        let type_field = use_a11y();
        let name_focus = use_focus(name_field);
        let type_focus = use_focus(type_field);
        let mut selected = ctx.selected_column;
        use_side_effect(move || {
            let focused = name_focus() != Focus::Not || type_focus() != Focus::Not;
            if focused && *selected.peek() != index {
                selected.set(index);
            }
        });

        use_side_effect(move || {
            let typed = name.read().clone();
            if typed == *name_reported.peek() {
                return;
            }
            name_reported.set(typed.clone());
            ctx.edit(move |draft| draft.set_column_name(index, typed));
        });
        use_side_effect(move || {
            let typed = sql_type.read().clone();
            if typed == *type_reported.peek() {
                return;
            }
            type_reported.set(typed.clone());
            ctx.edit(move |draft| draft.set_column_type(index, typed));
        });
        use_side_effect(move || {
            let draft = ctx.draft.read();
            let column = draft.columns.get(index).cloned().unwrap_or_default();
            drop(draft);
            if column.name != *name_reported.peek() {
                name_reported.set(column.name.clone());
                let mut name = name;
                name.set(column.name);
            }
            if column.sql_type != *type_reported.peek() {
                type_reported.set(column.sql_type.clone());
                let mut sql_type = sql_type;
                sql_type.set(column.sql_type);
            }
        });

        let fill = match self.selected {
            true => win.row_selected_background,
            false => Color::TRANSPARENT,
        };
        let (detail, detail_color) = match (&self.fault, &self.verdict) {
            (Some(why), _) => (why.clone(), error),
            (None, Some(dtype)) => (dtype.clone(), form.hint_color),
            (None, None) => (String::new(), form.hint_color),
        };

        let cell = |child: Element| {
            TableCell::new()
                .height(Size::px(FIELD_HEIGHT))
                .padding(Gaps::new(0., CELL_INSET, 0., CELL_INSET))
                .main_align(Alignment::Start)
                .child(
                    rect()
                        .expanded()
                        .horizontal()
                        .content(Content::Flex)
                        .cross_align(Alignment::Center)
                        .child(child),
                )
        };

        TableRow::new()
            .theme(TableThemePartial {
                row_background: Some(fill.into()),
                hover_row_background: Some(fill.into()),
                ..Default::default()
            })
            .on_press(move |_: Event<PressEventData>| selected.set(index))
            .child(cell(
                ValueField::new(name)
                    .bare()
                    .width(Size::flex(1.))
                    .height(Size::px(FIELD_HEIGHT))
                    .placeholder("name")
                    .a11y_id(name_field)
                    .into_element(),
            ))
            .child(cell(
                ValueField::new(sql_type)
                    .bare()
                    .width(Size::flex(1.))
                    .height(Size::px(FIELD_HEIGHT))
                    .placeholder("VARCHAR")
                    .a11y_id(type_field)
                    .into_element(),
            ))
            .child(cell(
                Meta::new(detail)
                    .color(detail_color)
                    .width(Size::flex(1.))
                    .text_overflow(TextOverflow::Ellipsis)
                    .into_element(),
            ))
    }
}
