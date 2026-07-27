//! HIVE PARTITIONING — the enable toggle, and behind it the two-pane column transfer.
//!
//! Named for the thing it does: Hive-style `key=value` folders, which is a different concern
//! from the writer's own file-part splitting (the canvas renamed it for exactly that reason).
//!
//! **The toggle gates the selection, it doesn't clear it.** A selection with the toggle off is
//! no partitioning at all — [`PartitionDraft::effective`] is the single answer every consumer
//! (preview, suggested name, spec) reads, because they once disagreed.
//!
//! The AVAILABLE pane offers **numeric and string columns only**: a directory name has to be a
//! short stable scalar, and a timestamp or a struct has no sensible one.

use freya::components::use_theme;
use freya::prelude::*;

use crate::apps::export::{ExportCtx, ExportThemePartial, ExportThemePreference};
use crate::components::divider::Divider;
use crate::components::icon::{Icon, IconName};
use crate::components::typography::{Caption, Meta, MonoValue, Prose};

/// Above this many unselected columns the AVAILABLE pane grows a filter (the canvas's rule).
const FILTER_THRESHOLD: usize = 8;
/// The panes' fixed box, so a wide schema scrolls rather than growing the window.
const PANE_MIN_HEIGHT: f32 = 128.;
const PANE_MAX_HEIGHT: f32 = 176.;
const PANE_HEADER_HEIGHT: f32 = 30.;
const ROW_HEIGHT: f32 = 32.;

#[derive(PartialEq)]
pub struct Partition;

impl Component for Partition {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(&None::<ExportThemePartial>, ExportThemePreference, "export");
        let ctx = use_consume::<ExportCtx>();
        let draft = ctx.draft.read();
        let enabled = draft.partition.enabled;
        let has_selection = !draft.partition.columns.is_empty();
        drop(draft);

        // The toggle's own row, with the state named beside it — the canvas's copy, worded so
        // on and off don't contradict each other.
        let state_label = if enabled {
            "Splitting output into a partitioned directory tree"
        } else {
            "Writing a flat output directory (no partition folders)"
        };
        let toggle = rect()
            .horizontal()
            .cross_align(Alignment::Center)
            .spacing(12.)
            .child(
                Switch::new()
                    .toggled(enabled)
                    .on_toggle(move |_| ctx.edit(|d| d.partition.enabled = !enabled)),
            )
            .child(Prose::new(state_label));

        rect()
            .width(Size::fill())
            .vertical()
            .spacing(12.)
            .child(
                crate::components::typography::Eyebrow::new("HIVE PARTITIONING")
                    .color(theme.label_color),
            )
            .child(toggle)
            .maybe_child(enabled.then_some(Panes))
            .maybe_child((enabled && has_selection).then_some(KeepColumns))
            .maybe_child((enabled && has_selection).then_some(NullWarning))
    }
}

/// The transfer: AVAILABLE on the left, SELECTED (ordered) on the right.
#[derive(PartialEq)]
struct Panes;

impl Component for Panes {
    fn render(&self) -> impl IntoElement {
        rect()
            .width(Size::fill())
            .vertical()
            .spacing(12.)
            .child(Prose::new(
                "Writes a Hive-style directory tree — one folder level per column, in the \
                 order shown.",
            ))
            .child(
                rect()
                    .width(Size::fill())
                    .horizontal()
                    .content(Content::Flex)
                    .spacing(12.)
                    .child(rect().width(Size::flex(1.)).child(Available))
                    .child(rect().width(Size::flex(1.)).child(Selected)),
            )
    }
}

/// The pane frame both halves share — a bordered box with a header strip over a scroll body.
fn pane(header: impl IntoElement, body: impl IntoElement) -> impl IntoElement {
    let theme = get_theme!(&None::<ExportThemePartial>, ExportThemePreference, "export");
    rect()
        .width(Size::fill())
        .vertical()
        .corner_radius(6.)
        .overflow(Overflow::Clip)
        .background(theme.panel_background)
        .border(Border::new().width(1.).fill(theme.control_border_fill))
        .child(
            rect()
                .width(Size::fill())
                .height(Size::px(PANE_HEADER_HEIGHT))
                .horizontal()
                .cross_align(Alignment::Center)
                .main_align(Alignment::SpaceBetween)
                .spacing(8.)
                .padding((0., 12.))
                .background(theme.background)
                .child(header),
        )
        .child(Divider::horizontal().color(theme.border_fill))
        // The pane grows with its rows between a floor and a ceiling, so a short list doesn't
        // leave a hole and a wide schema scrolls instead of growing the window. `ScrollView`
        // carries only the ceiling, so the floor is the wrapper's.
        .child(
            rect()
                .width(Size::fill())
                .min_height(Size::px(PANE_MIN_HEIGHT))
                .max_height(Size::px(PANE_MAX_HEIGHT))
                .child(
                    ScrollView::new()
                        .max_height(Size::px(PANE_MAX_HEIGHT))
                        // This list sits inside the window's own scrolling body, so a wheel
                        // gesture that starts over it (and can move it) stays latched to it —
                        // no mid-gesture spill into the body scrolling underneath — while a
                        // gesture starting at its end, or over a list too short to scroll,
                        // passes through, so the pane is never a hover trap. The record view's
                        // nested block does exactly this, for exactly this reason.
                        .latch_wheel()
                        .child(body),
                ),
        )
}

/// The columns not yet chosen. Pressing one appends it as the next directory level.
#[derive(PartialEq)]
struct Available;

impl Component for Available {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(&None::<ExportThemePartial>, ExportThemePreference, "export");
        let ctx = use_consume::<ExportCtx>();
        // Hooks first and unconditionally: the filter box comes and goes with the column
        // count, and the row list is a loop — calling either of these inside one would make
        // the hook count vary per render.
        let accent = use_theme().read().colors().primary;
        let filter_text = use_state(String::new);
        let target = ctx.target.read();
        let draft = ctx.draft.read();
        let filter = draft.partition.filter.to_lowercase();

        let unchosen: Vec<String> = target
            .partitionable()
            .into_iter()
            .filter(|c| !draft.partition.columns.contains(&c.name))
            .map(|c| c.name.clone())
            .collect();
        let matching: Vec<String> = unchosen
            .iter()
            .filter(|name| name.to_lowercase().contains(&filter))
            .cloned()
            .collect();
        let show_filter = unchosen.len() > FILTER_THRESHOLD;
        let query = draft.partition.filter.clone();
        drop(draft);
        drop(target);

        // Past a handful of columns the eyebrow gives way to a filter — the canvas's rule, so
        // the pane scales to a wide schema.
        // `Input` writes its bound state directly, so the box is the buffer and this carries
        // it into the draft (where the filtering above reads it).
        use_side_effect(move || {
            let typed = filter_text.read().clone();
            if typed != ctx.draft.peek().partition.filter {
                ctx.edit(|d| d.partition.filter = typed);
            }
        });

        let header: Element = if show_filter {
            let text = filter_text;
            rect()
                .width(Size::fill())
                .horizontal()
                .cross_align(Alignment::Center)
                .content(Content::Flex)
                .spacing(8.)
                .child(
                    Icon::new(IconName::Search)
                        .size(12.)
                        .color(theme.label_color),
                )
                .child(
                    crate::components::typography::InputTypography::body(
                        Input::new(text)
                            .placeholder("Filter…")
                            .width(Size::fill())
                            .compact()
                            // Bare: the pane's header strip wears the chrome, so the input's
                            // own dress goes fully transparent (the find popover's recipe).
                            .background(Color::TRANSPARENT)
                            .focus_background(Color::TRANSPARENT)
                            .border_fill(Color::TRANSPARENT)
                            .focus_border_fill(Color::TRANSPARENT),
                    )
                    .width(Size::flex(1.)),
                )
                .into_element()
        } else {
            Caption::new("AVAILABLE")
                .color(theme.label_color)
                .into_element()
        };

        let body: Element = if matching.is_empty() {
            let message = if unchosen.is_empty() {
                "All columns added".to_string()
            } else {
                format!("No match for \"{query}\"")
            };
            empty(&message).into_element()
        } else {
            let mut list = rect().width(Size::fill()).vertical();
            for name in matching {
                let pressed = name.clone();
                list = list.child(
                    rect()
                        .width(Size::fill())
                        .height(Size::px(ROW_HEIGHT))
                        .horizontal()
                        .cross_align(Alignment::Center)
                        .spacing(8.)
                        .padding((0., 12.))
                        .on_press(move |_| {
                            // Adding clears the filter, so the next pick starts from the whole
                            // list rather than a query matching nothing. The **box** is cleared
                            // too, not just the draft: the box is the buffer the `Input`
                            // renders, so clearing only the draft left a query on screen that
                            // was no longer being applied — and put it back on the next
                            // keystroke.
                            let mut filter_text = filter_text;
                            filter_text.set(String::new());
                            ctx.edit(|d| {
                                if !d.partition.columns.contains(&pressed) {
                                    d.partition.columns.push(pressed.clone());
                                    d.partition.filter.clear();
                                }
                            })
                        })
                        .child(Icon::new(IconName::Plus).size(12.).color(accent))
                        .child(MonoValue::new(name.clone()).width(Size::flex(1.)))
                        .key(name),
                );
            }
            list.into_element()
        };

        pane(header, body)
    }
}

/// The chosen levels, outermost first, each removable.
///
/// Reordering is by the ▲▼ buttons rather than drag-and-drop: the canvas uses HTML5 drag
/// events, which have no Freya equivalent here, and order is the whole meaning of this list —
/// it needs to be adjustable with one unambiguous press, not a gesture that can be dropped.
#[derive(PartialEq)]
struct Selected;

impl Component for Selected {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(&None::<ExportThemePartial>, ExportThemePreference, "export");
        let ctx = use_consume::<ExportCtx>();
        let chosen = ctx.draft.read().partition.columns.clone();

        let header = rect()
            .width(Size::fill())
            .horizontal()
            .content(Content::Flex)
            .cross_align(Alignment::Center)
            .main_align(Alignment::SpaceBetween)
            .child(Caption::new("SELECTED").color(theme.label_color))
            .maybe_child(
                (chosen.len() > 1).then(|| Meta::new("outermost first").color(theme.label_color)),
            );

        let body: Element = if chosen.is_empty() {
            empty("Press a column on the left to add it here").into_element()
        } else {
            let mut list = rect().width(Size::fill()).vertical();
            for (index, name) in chosen.iter().enumerate() {
                list = list.child(
                    SelectedRow {
                        name: name.clone(),
                        index,
                        last: index + 1 == chosen.len(),
                        key: DiffKey::None,
                    }
                    .key(name.clone()),
                );
            }
            list.into_element()
        };

        pane(header, body)
    }
}

/// One chosen level: its 1-based order badge, its name, the move controls and remove.
#[derive(PartialEq)]
struct SelectedRow {
    name: String,
    index: usize,
    last: bool,
    key: DiffKey,
}

impl KeyExt for SelectedRow {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for SelectedRow {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(&None::<ExportThemePartial>, ExportThemePreference, "export");
        let ctx = use_consume::<ExportCtx>();
        let index = self.index;

        let badge = rect()
            .width(Size::px(16.))
            .height(Size::px(16.))
            .corner_radius(8.)
            .center()
            .background(theme.badge_background)
            .child(Meta::new((index + 1).to_string()).color(theme.badge_color));

        let up = Button::new()
            .flat()
            .width(Size::px(22.))
            .height(Size::px(22.))
            .enabled(index > 0)
            .on_press(move |_| swap_level(ctx, index, -1))
            .child(Icon::new(IconName::ChevronUp).size(12.));
        let down = Button::new()
            .flat()
            .width(Size::px(22.))
            .height(Size::px(22.))
            .enabled(!self.last)
            .on_press(move |_| swap_level(ctx, index, 1))
            .child(Icon::new(IconName::ChevronDown).size(12.));
        let remove = Button::new()
            .flat()
            .width(Size::px(22.))
            .height(Size::px(22.))
            .on_press(move |_| {
                ctx.edit(|d| {
                    d.partition.columns.remove(index);
                })
            })
            .child(Icon::new(IconName::Close).size(12.));

        rect()
            .width(Size::fill())
            .height(Size::px(ROW_HEIGHT + 2.))
            .horizontal()
            .content(Content::Flex)
            .cross_align(Alignment::Center)
            .spacing(8.)
            .padding((0., 8.))
            .child(badge)
            .child(MonoValue::new(self.name.clone()).width(Size::flex(1.)))
            .child(up)
            .child(down)
            .child(remove)
    }

    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }
}

/// Move the level at `index` by `delta`, if there is somewhere to move it to.
///
/// A free fn rather than a closure shared by the two buttons: writing the draft needs `&mut`,
/// so a shared closure would be `FnMut` and can't be handed to two handlers.
fn swap_level(ctx: ExportCtx, index: usize, delta: isize) {
    ctx.edit(|draft| {
        let target = index as isize + delta;
        if target >= 0 && (target as usize) < draft.partition.columns.len() {
            draft.partition.columns.swap(index, target as usize);
        }
    });
}

/// The keep-columns toggle and the consequence of leaving it off.
#[derive(PartialEq)]
struct KeepColumns;

impl Component for KeepColumns {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(&None::<ExportThemePartial>, ExportThemePreference, "export");
        let ctx = use_consume::<ExportCtx>();
        // Hoisted: the banner below is conditional, and `use_theme` inside it would make the
        // hook count vary with the toggle.
        let warning = use_theme().read().colors().warning;
        let keep = ctx.draft.read().partition.keep_columns;

        let row = rect()
            .horizontal()
            .cross_align(Alignment::Center)
            .spacing(12.)
            .child(
                Switch::new()
                    .toggled(keep)
                    .on_toggle(move |_| ctx.edit(|d| d.partition.keep_columns = !keep)),
            )
            .child(Prose::new("Keep partition columns inside files"));

        rect()
            .width(Size::fill())
            .vertical()
            .spacing(12.)
            .child(row)
            // A statement of what the export will do, shown exactly when it applies. (The
            // canvas also warned about high-cardinality columns off a distinct count taken
            // from an 80-row sample; that number is derived from what happens to be on screen,
            // which is the kind of figure the inspector rejected, so it isn't here.)
            .maybe_child((!keep).then(|| {
                rect()
                    .width(Size::fill())
                    .horizontal()
                    .spacing(8.)
                    .padding((8., 12.))
                    .corner_radius(6.)
                    .background(theme.warning_background)
                    .border(Border::new().width(1.).fill(theme.warning_border_fill))
                    .child(Icon::new(IconName::Warning).size(14.).color(warning))
                    .child(
                        Prose::new(
                            "Partition columns are written as directory names and removed \
                             from file contents.",
                        )
                        .color(warning)
                        .wrap(),
                    )
            }))
    }
}

/// The NULL caveat — the one thing about a partitioned export the user genuinely cannot see
/// coming, and cannot recover from afterwards.
///
/// A directory name can't hold a NULL, and DataFusion 54 does **not** use the Hive convention
/// (`__HIVE_DEFAULT_PARTITION__`) for it: it files a NULL row under a neighbouring value's
/// directory instead, so the row comes back out labelled with a value it never had. Nothing on
/// our side can prevent that — the writer chooses the directory — so the honest thing is to say
/// it before the user commits, while they can still pick a different column.
///
/// The wording is deliberately about the **outcome** rather than the cause: "wrong value" is
/// what matters, not whose bug it is. Pinned by core's
/// `a_null_partition_value_is_misfiled_under_another_value`, which fails if a DataFusion
/// upgrade ever fixes it — at which point this component goes.
#[derive(PartialEq)]
struct NullWarning;

impl Component for NullWarning {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(&None::<ExportThemePartial>, ExportThemePreference, "export");
        let warning = use_theme().read().colors().warning;

        rect()
            .width(Size::fill())
            .horizontal()
            .spacing(8.)
            .padding((8., 12.))
            .corner_radius(6.)
            .background(theme.warning_background)
            .border(Border::new().width(1.).fill(theme.warning_border_fill))
            .child(Icon::new(IconName::Warning).size(14.).color(warning))
            .child(
                Prose::new(
                    "Rows with a NULL in a partition column are written into another value's \
                     folder, so they read back with the wrong value. Partition on a column \
                     with no NULLs.",
                )
                .color(warning)
                .wrap(),
            )
    }
}

/// A pane's empty state — a centred line, at the pane's own resting height.
fn empty(message: &str) -> impl IntoElement {
    let theme = get_theme!(&None::<ExportThemePartial>, ExportThemePreference, "export");
    rect()
        .width(Size::fill())
        .height(Size::px(PANE_MIN_HEIGHT))
        .center()
        .padding((12., 16.))
        .child(
            Prose::new(message)
                .color(theme.label_color)
                .align(TextAlign::Center)
                .wrap(),
        )
}
