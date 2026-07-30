//! The **shortcuts grid** — Freya's built-in `Table`, dressed as the canvas's Action / Shortcut
//! editor.
//!
//! The same table the Engine pane's properties grid is, and deliberately so: it is the second
//! grid in this window, so it takes the same `TableRow` header, the same 34px rows, the same
//! column rule, the same [`RowNote`] between rows — everything P4-07 already paid for in fork
//! additions, `TableRow::on_press` included. What is different is what a row *is*. A property row
//! is selectable and editable in place; a shortcut row is neither, so it takes `Table`'s own hover
//! response, and its press starts a rebind rather than a selection.
//!
//! **A press on a row is a double-press or nothing.** The canvas rebinds on double-click (its
//! `onDoubleClick`), which is the right gesture in a table: a single click on a row means "I am
//! pointing at this", and it would be far too easy to knock a shortcut off a command by pointing
//! at it. The handler is the **row's** — one command, one chord, so the whole row is the target
//! rather than the column the chord happens to be drawn in. `EventsCombos` is how a double-press
//! is detected once the row already handles the press (AGENTS.md §3), so both live in the one
//! handler.
//!
//! The buttons *inside* the row stop their press, which is the exception to the rule the Engine
//! pane relies on: there, a press into a cell selecting the row is what you want; here, pressing
//! Reset twice quickly would reset the row and then start listening for a key.

use freya::prelude::*;
use strata_core::config::Command;
use strata_core::keymap::Rebind;

use crate::apps::settings::views::keymap::model::{Blocked, Editing, KeyRow};
use crate::apps::settings::views::RowNote;
use crate::apps::settings::{
    SettingsCtx, SettingsTheme, SettingsThemePartial, SettingsThemePreference,
};
use crate::components::badge::Badge;
use crate::components::divider::Divider;
use crate::components::icon::IconName;
use crate::components::tool_button::ToolButton;
use crate::components::typography::{Control, MonoValue, Prose};

/// The header strip and a row, matching the Engine pane's grid — one height across the window's
/// two tables. The canvas states a 30px floor for a keymap row and then fills it with 24px caps
/// inside a 4px inset, and puts a 26px reset button beside them, so its own rows land at 32-34
/// wherever they carry anything; 34 is where the two agree.
const HEAD_HEIGHT: f32 = 32.;
const ROW_HEIGHT: f32 = 34.;
/// The Shortcut column (canvas `width: 240px`), and the inset both columns share.
const SHORTCUT_WIDTH: f32 = 240.;
const CELL_INSET: f32 = 16.;
/// A key cap: its floor (a single character still reads as a key), its height and its inset.
const CAP_MIN_WIDTH: f32 = 22.;
const CAP_HEIGHT: f32 = 24.;
const CAP_INSET: f32 = 8.;
/// The heavier bottom edge that makes a cap read as a key rather than a chip.
const CAP_EDGE: f32 = 1.;
const CAP_BOTTOM_EDGE: f32 = 2.;
/// The gap between two caps of one chord, and between the chord and what sits beside it.
const CAP_GAP: f32 = 4.;
const SHORTCUT_GAP: f32 = 8.;
/// The dash pattern on an empty slot's edge (canvas `border: 1px dashed`).
const DASH: f32 = 4.;
const DASH_GAP: f32 = 3.;
/// The height of the small controls that sit in a row: the capture pill, Add shortcut, Esc.
const PILL_HEIGHT: f32 = 24.;
const PILL_RADIUS: f32 = 6.;

/// The grid: a header over one row per command, hugging its content inside the pane's scroll.
///
/// Not `Size::flex` like the Engine pane's — that one is a surface managing its own height with a
/// pinned header, and this is a list of every command, which is exactly the thing the pane's
/// scroll frame is for.
#[derive(PartialEq)]
pub struct KeyTable {
    pub rows: Vec<KeyRow>,
    pub editing: State<Editing>,
}

impl Component for KeyTable {
    fn render(&self) -> impl IntoElement {
        let editing = self.editing;
        let warning = use_theme().read().colors().warning;

        // One snapshot for the whole list: at most one row is blocked, and nothing in the loop can
        // change which.
        let editing_now = editing.read().clone();

        let mut body = TableBody::new();
        for row in &self.rows {
            let blocked = editing_now.blocked(row.command).cloned();
            body = body
                .child(
                    KeyTableRow {
                        row: row.clone(),
                        editing,
                        key: DiffKey::None,
                    }
                    // Keyed on the command, not its label: the command is the row's identity, and
                    // a label is a display string two commands could one day share.
                    .key(row.command),
                )
                .maybe_child(blocked.map(|blocked| {
                    RowNote::new(blocked.message.clone(), warning)
                        .actions(BlockedActions { blocked, editing })
                }));
        }

        Table::new()
            .column_widths(vec![Size::flex(1.), Size::px(SHORTCUT_WIDTH)])
            .child(TableHead::new().child(HeadRow))
            .child(body)
    }
}

/// The `Action` / `Shortcut` strip. A `TableRow`, so it shares the column widths and the rule
/// under it; its own theme gives it the raised fill and pins the hover fill to the same colour,
/// because a header is not a row you can act on.
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
                                    .padding(Gaps::new(0., 0., 0., CELL_INSET))
                                    .child(Control::new("Action").color(theme.item_active_color)),
                            )
                            .child(Divider::vertical().color(rule)),
                    ),
            )
            .child(
                TableCell::new()
                    .height(Size::px(HEAD_HEIGHT))
                    .padding(Gaps::new(0., CELL_INSET, 0., CELL_INSET))
                    .main_align(Alignment::Start)
                    .child(Control::new("Shortcut").color(theme.item_active_color)),
            )
    }
}

/// One command: its name on the left, whatever its shortcut column is currently showing on the
/// right.
///
/// The description is the row's **tooltip**, not a second line under the label — the canvas moved
/// it there when the pane became a table, and a nineteen-row grid where every row is two lines
/// tall is a page you scroll rather than a list you scan.
///
/// **The double-press that starts a rebind is the row's**, not the shortcut cell's: the row is the
/// unit here — one command, one chord — so pointing anywhere along it and pressing twice is the
/// gesture, which is also what the canvas's own `title` implies about the row being one target.
/// The controls inside it stop their press, so a single click on Reset or Add shortcut can't be
/// half of a rebind gesture as well.
#[derive(PartialEq)]
struct KeyTableRow {
    row: KeyRow,
    editing: State<Editing>,
    key: DiffKey,
}

impl KeyExt for KeyTableRow {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for KeyTableRow {
    fn render(&self) -> impl IntoElement {
        let rule = use_theme().read().colors().border;
        let mut editing = self.editing;
        let row = &self.row;
        let command = row.command;

        // The canvas hangs the description off the whole row's `title`; here it hangs off the
        // action's name. Not because the row is the wrong target — it is the press target — but
        // because a tooltip spanning the row would nest inside the reset button's own, and both
        // would open together over the same pointer.
        //
        // The label names no colour, deliberately: `Table` paints the ambient one from its own
        // theme's `color`, which is already the canvas's `--c-text2`. Naming the settings theme's
        // `item_color` here (a nav row at rest, a step dimmer) is how the label ends up quieter
        // than the design, and it would be a second source for one surface's text tone.
        let label = TooltipContainer::new(Tooltip::new(row.desc))
            .position(AttachedPosition::Bottom)
            .child(Prose::new(row.label));

        TableRow::new()
            // A fixed command shows its chord and offers nothing, so it takes no handler at all
            // rather than one that declines.
            .maybe(!row.fixed, |table_row| {
                table_row.on_press(move |e: Event<PressEventData>| {
                    let double = match e.data() {
                        PressEventData::Mouse(m) => {
                            EventsCombos::pressed(m.global_location).is_double()
                        }
                        // A keyboard activation carries no location and so no combo. Declining is
                        // the honest answer: "press twice" has no keyboard equivalent.
                        _ => false,
                    };
                    if double {
                        editing.set(Editing::Capturing(command));
                    }
                })
            })
            .child(
                TableCell::new()
                    .height(Size::px(ROW_HEIGHT))
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
                                    .horizontal()
                                    .cross_align(Alignment::Center)
                                    .spacing(SHORTCUT_GAP)
                                    .padding(Gaps::new(0., 0., 0., CELL_INSET))
                                    .child(label)
                                    .maybe_child(row.custom.then_some(CustomBadge)),
                            )
                            .child(Divider::vertical().color(rule)),
                    ),
            )
            .child(ShortcutCell {
                row: row.clone(),
                editing: self.editing,
            })
    }
}

/// The **Custom** marker: this command's chord is the user's, not the built-in one.
///
/// The app's [`Badge::tag`] — the same all-caps marker the catalog's `PART` and the plan view's
/// `HOTSPOT` are, with its tint derived from the foreground rather than authored. The canvas sets
/// this one two points smaller than the others and in the UI face rather than mono; taking
/// `Badge` down to it would restyle every marker in the app to suit one row, and a marker that
/// matches the app's other markers is the better answer.
#[derive(PartialEq)]
struct CustomBadge;

impl Component for CustomBadge {
    fn render(&self) -> impl IntoElement {
        let accent = use_theme().read().colors().primary;

        Badge::tag("CUSTOM", accent).padding((0., 4.))
    }
}

/// The Shortcut column: the one part of a row that changes.
///
/// Four states, exactly as the canvas has them, and they are exclusive by construction rather
/// than by four independent `showX` flags: listening for a key, a fixed command's caps, an
/// unbound command's invitation, or the chord with its reset beside it. The gesture that *starts*
/// a rebind belongs to the row, not here — see [`KeyTableRow`].
#[derive(PartialEq)]
struct ShortcutCell {
    row: KeyRow,
    editing: State<Editing>,
}

impl Component for ShortcutCell {
    fn render(&self) -> impl IntoElement {
        let mut editing = self.editing;
        let row = &self.row;
        let command = row.command;
        let capturing = editing.read().capturing(command);

        let content: Element = match (capturing, row.fixed, row.unbound()) {
            (true, _, _) => rect()
                .horizontal()
                .cross_align(Alignment::Center)
                .spacing(SHORTCUT_GAP)
                .child(CapturePill)
                .child(
                    Button::new()
                        .height(Size::px(PILL_HEIGHT))
                        .on_press(move |e: Event<PressEventData>| {
                            e.stop_propagation();
                            editing.set(Editing::Idle);
                        })
                        .child(Control::new("Esc")),
                )
                .into_element(),
            (false, false, true) => AddShortcut { editing, command }.into_element(),
            (false, _, _) => rect()
                .horizontal()
                .cross_align(Alignment::Center)
                .spacing(SHORTCUT_GAP)
                .child(Caps {
                    caps: row.caps.clone(),
                })
                // No `&& !row.fixed`: a fixed command is never custom, because the override a
                // hand-edited config gives it is ignored (`keymap::is_custom`). One predicate, so
                // the badge and this control can never disagree about the same row.
                .maybe_child(row.custom.then(|| ResetRow { editing, command }))
                .into_element(),
        };

        TableCell::new()
            .height(Size::px(ROW_HEIGHT))
            .padding(Gaps::new(0., CELL_INSET, 0., CELL_INSET))
            .main_align(Alignment::End)
            .child(content)
    }
}

/// One chord, cap by cap.
#[derive(PartialEq)]
struct Caps {
    caps: Vec<String>,
}

impl Component for Caps {
    fn render(&self) -> impl IntoElement {
        let theme = settings_theme();
        let mut row = rect().horizontal().spacing(CAP_GAP);
        for cap in &self.caps {
            row = row.child(
                rect()
                    .min_width(Size::px(CAP_MIN_WIDTH))
                    .height(Size::px(CAP_HEIGHT))
                    .padding(Gaps::new(0., CAP_INSET, 0., CAP_INSET))
                    .center()
                    .corner_radius(PILL_RADIUS)
                    .background(theme.keycap_background)
                    .border(
                        Border::new()
                            .width(BorderWidth {
                                top: CAP_EDGE,
                                right: CAP_EDGE,
                                bottom: CAP_BOTTOM_EDGE,
                                left: CAP_EDGE,
                            })
                            .fill(theme.keycap_border_fill),
                    )
                    .child(MonoValue::new(cap.clone()).color(theme.keycap_color)),
            );
        }
        row
    }
}

/// The row is listening. A dashed accent outline, because the slot is open rather than filled —
/// the same thing [`AddShortcut`]'s edge says, in the accent because it is happening now.
#[derive(PartialEq)]
struct CapturePill;

impl Component for CapturePill {
    fn render(&self) -> impl IntoElement {
        let accent = use_theme().read().colors().primary;

        rect()
            .height(Size::px(PILL_HEIGHT))
            .padding(Gaps::new(0., CELL_INSET, 0., CELL_INSET))
            .center()
            .corner_radius(PILL_RADIUS)
            .border(Border::new().width(1.).dashed(DASH, DASH_GAP).fill(accent))
            .child(Control::new("Press shortcut").color(accent))
    }
}

/// An unbound command's invitation. A single press, not a double one: there is no chord here to
/// knock off, and the button is the affordance the canvas gives instead of the caps.
#[derive(PartialEq)]
struct AddShortcut {
    editing: State<Editing>,
    command: Command,
}

impl Component for AddShortcut {
    fn render(&self) -> impl IntoElement {
        let theme = settings_theme();
        let accent = use_theme().read().colors().primary;
        let mut editing = self.editing;
        let command = self.command;

        // A flat button already resolves the muted label that lifts to the accent on hover; what
        // it cannot resolve is an *empty slot*, which is this control's whole point — so the three
        // overrides are the edge it grows, the edge that edge becomes on hover, and declining the
        // fill a flat button would take, because the canvas moves only the outline and the text.
        Button::new()
            .flat()
            .height(Size::px(PILL_HEIGHT))
            .border_style(BorderStyle::Dashed {
                dash: DASH,
                gap: DASH_GAP,
            })
            .theme_colors(ButtonColorsThemePartial {
                border_fill: Some(theme.slot_border_fill.into()),
                hover_border_fill: Some(accent.into()),
                hover_background: Some(Color::TRANSPARENT.into()),
                ..Default::default()
            })
            .on_press(move |e: Event<PressEventData>| {
                e.stop_propagation();
                editing.set(Editing::Capturing(command));
            })
            .child(Control::new("Add shortcut"))
    }
}

/// Put this command back on its built-in chord. Conflict-checked like a capture, because the
/// default it wants back can have been taken while it was away — so it goes through the pane's
/// one [`ask`](super::ask) rather than writing the draft itself.
#[derive(PartialEq)]
struct ResetRow {
    editing: State<Editing>,
    command: Command,
}

impl Component for ResetRow {
    fn render(&self) -> impl IntoElement {
        let ctx = use_consume::<SettingsCtx>();
        let editing = self.editing;
        let command = self.command;

        ToolButton::new(IconName::Reload, "Reset to default")
            .outlined()
            .on_press(EventHandler::new(move |e: Event<PressEventData>| {
                e.stop_propagation();
                super::ask(ctx, editing, command, Rebind::Default);
            }))
    }
}

/// What a blocked rebind offers: take the chord anyway, or leave it alone. The first is only
/// there when there is a command to take it *from* — a chord the policy refused outright has
/// nothing to offer but the message.
#[derive(PartialEq)]
struct BlockedActions {
    blocked: Blocked,
    editing: State<Editing>,
}

impl Component for BlockedActions {
    fn render(&self) -> impl IntoElement {
        let ctx = use_consume::<SettingsCtx>();
        let mut editing = self.editing;
        let blocked = self.blocked.clone();

        rect()
            .horizontal()
            .cross_align(Alignment::Center)
            .spacing(SHORTCUT_GAP)
            .maybe_child((!blocked.holders.is_empty()).then(|| {
                let blocked = blocked.clone();
                Button::new()
                    .filled()
                    .height(Size::px(PILL_HEIGHT))
                    .on_press(move |_: Event<PressEventData>| {
                        super::reassign(ctx, editing, &blocked);
                    })
                    .child(Control::new("Reassign"))
            }))
            .child(
                Button::new()
                    .flat()
                    .height(Size::px(PILL_HEIGHT))
                    .on_press(move |_: Event<PressEventData>| editing.set(Editing::Idle))
                    .child(Control::new("Cancel")),
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
