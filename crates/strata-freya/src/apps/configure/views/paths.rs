//! **SOURCE PATHS** — the label with its `REQUIRED` marker and resolution tooltip, the
//! three-button toolbar, and the list of path rows.
//!
//! Browse is in the **toolbar**, acting on the selected row, which is the canvas's arrangement
//! and not a per-row button. It opens a menu rather than a dialog, because one native dialog
//! cannot offer files *and* folders: `NSOpenPanel` is configured for one or the other, so the
//! canvas's single "Browse… (file or folder)" button becomes one button with two ways to answer
//! it. Picking files is multi-select — a table *is* many paths, and five files one dialog at a
//! time is the same five rows with four more dialogs.
//!
//! **An object-store table is one path, in the same list.** On a connection the section is singular
//! throughout — `SOURCE PATH`, no toolbar, one row wearing the bucket as a non-editable prefix —
//! because an object store has no file dialog and its paths are text. Still this list one row long
//! rather than a second control: the row is where the two-way sync with the draft lives, and what
//! actually goes is the toolbar and the empty state.
//!
//! That row is the draft's `remote_source` and the local list is its `local_sources` — two fields
//! projected by the LOCATION in play (`ConfigureDraft::path_at`), so the toggle swaps what these
//! rows show without moving a path between two roots it means different things under.

use freya::prelude::*;

use crate::apps::configure::ConfigureCtx;
use crate::components::form::{form_theme, Row, ValueField, FIELD_HEIGHT};
use crate::components::icon::{Icon, IconName};
use crate::components::metrics::{EMPTY_TABLE_HEIGHT, SP_3, SP_4};
use crate::components::tones::tones;
use crate::components::tool_button::ToolButton;
use crate::components::typography::{MonoValue, Prose};
use crate::components::window::window_theme;

/// The gap between the toolbar's buttons (their size is the shared control's).
const TOOL_GAP: f32 = SP_3;
/// The list's empty state (canvas `min-height: 88px`), and the gap between two path fields. A
/// row's own height is the form's `FIELD_HEIGHT` — this list holds fields, so it does not get to
/// invent a height for them.
/// A cell's inset — the properties grid's own (`padding: 0 var(--sp-3)`).
const CELL_INSET: f32 = SP_4;
/// The gap between the label row, the toolbar and the list.
const STACK_GAP: f32 = SP_3;
/// The browse dropdown's width — enough for its two labels without the card hugging them.
const MENU_WIDTH: f32 = 180.;
/// The most of a source row the **bucket prefix** may take before it ellipsizes. Half, so the
/// path always keeps at least half the box it is typed into however long the connection's URL is.
const PREFIX_MAX_PERCENT: f32 = 50.;

/// What each shape of path resolves to — the canvas's ⓘ, verbatim in substance.
const RESOLUTION_HINT: &str = "Each path resolves to one or more data files in the format chosen \
                               above, combined into this table. A file is one file; a folder is \
                               every file in it; a glob such as **/*.csv matches recursively. \
                               Paths are absolute.";

/// The same sentence for a path written against a bucket — the canvas's other half of the ⓘ.
///
/// It says the trailing slash out loud, which the local hint does not have to: Browse writes one
/// for a folder it picked, and nothing browses a bucket. Without it `events/2024` is a request
/// for one object of that exact name, and the table registers empty.
const STORE_HINT: &str = "The path resolves to one or more data files in the format chosen \
                          above, relative to the connection's bucket. A folder ends with / and \
                          is every file in it; a glob such as **/*.csv matches recursively.";

#[derive(PartialEq)]
pub struct SourcePaths;

impl Component for SourcePaths {
    fn render(&self) -> impl IntoElement {
        let ctx = use_consume::<ConfigureCtx>();
        let (remote, internal) = {
            let draft = ctx.draft.read();
            (draft.remote(), draft.internal())
        };
        if internal {
            return rect().into_element();
        }

        Row::new(match remote {
            true => "SOURCE PATH",
            false => "SOURCE PATHS",
        })
        .required()
        .hint(match remote {
            true => STORE_HINT,
            false => RESOLUTION_HINT,
        })
        .child(
            rect()
                .width(Size::fill())
                .vertical()
                .spacing(STACK_GAP)
                .maybe_child((!remote).then_some(Toolbar))
                .child(PathList),
        )
        .into_element()
    }
}

/// Add · remove · browse. The canvas's three, in its order.
#[derive(PartialEq)]
struct Toolbar;

impl Component for Toolbar {
    fn render(&self) -> impl IntoElement {
        let win = window_theme();
        let error = tones().error;
        let ctx = use_consume::<ConfigureCtx>();
        let has_rows = ctx.draft.read().path_count() > 0;

        rect()
            .horizontal()
            .cross_align(Alignment::Center)
            .spacing(TOOL_GAP)
            .child(
                ToolButton::new(IconName::Plus, "Add path")
                    .outlined()
                    .color(win.icon_color)
                    .on_press(move |_| {
                        let mut selected = ctx.selected_path;
                        let mut at = *selected.peek();
                        ctx.edit(|draft| at = draft.add_path());
                        selected.set(at);
                    }),
            )
            .child(
                ToolButton::new(IconName::Minus, "Remove path")
                    .outlined()
                    .color(error)
                    .enabled(has_rows)
                    .on_press(move |_| {
                        let mut selected = ctx.selected_path;
                        let at = *selected.peek();
                        let mut next = at;
                        ctx.edit(|draft| next = draft.remove_path(at));
                        selected.set(next);
                    }),
            )
            .child(BrowseButton)
    }
}

/// Browse — one button, two answers, because one native dialog cannot offer both.
#[derive(PartialEq)]
struct BrowseButton;

impl Component for BrowseButton {
    fn render(&self) -> impl IntoElement {
        let form = form_theme();
        let ctx = use_consume::<ConfigureCtx>();
        let mut open = use_state(|| false);
        let mut request = use_state(|| None::<Pick>);
        use_side_effect(move || {
            let Some(kind) = *request.read() else {
                return;
            };
            request.set(None);
            pick(ctx, kind);
        });

        let menu = Menu::new()
            .min_width(Size::px(MENU_WIDTH))
            .on_close(move |()| open.set(false))
            .child(
                MenuButton::new()
                    .on_press(move |_| {
                        open.set(false);
                        request.set(Some(Pick::Files));
                    })
                    .child(menu_row(IconName::File, "Choose files…")),
            )
            .child(
                MenuButton::new()
                    .on_press(move |_| {
                        open.set(false);
                        request.set(Some(Pick::Folder));
                    })
                    .child(menu_row(IconName::Folder, "Choose a folder…")),
            );

        Attached::new(
            ToolButton::new(IconName::Folder, "Browse for a source")
                .outlined()
                .color(form.label_color)
                .on_press(move |_| open.toggle()),
        )
        .bottom()
        .align_start()
        .offset(4.)
        .maybe_child(open().then_some(menu))
    }
}

/// One row of the browse menu: a glyph and its label.
fn menu_row(icon: IconName, label: &str) -> impl IntoElement {
    rect()
        .horizontal()
        .cross_align(Alignment::Center)
        .spacing(SP_3)
        .child(Icon::new(icon).size(14.))
        .child(Prose::new(label.to_string()))
}

/// Which dialog the browse menu asked for. `Copy`, because it rides a `State` across the scope
/// boundary described in [`BrowseButton`].
#[derive(Clone, Copy, PartialEq)]
enum Pick {
    Files,
    Folder,
}

/// Ask for paths and put them in the list at the selection. Spawned — the dialog waits on the
/// user. Dismissing it is a decision, not a failure, so the list keeps what it had.
fn pick(ctx: ConfigureCtx, kind: Pick) {
    let start = {
        let draft = ctx.draft.peek();
        draft.path_at(draft.clamp_selection(*ctx.selected_path.peek()))
    };
    let start = (!start.trim().is_empty()).then_some(start);

    spawn(async move {
        let mut dialog = rfd::AsyncFileDialog::new().set_title("Choose a source");
        if let Some(start) = &start {
            dialog = dialog.set_directory(start);
        }
        let picked: Vec<String> = match kind {
            Pick::Files => dialog
                .pick_files()
                .await
                .unwrap_or_default()
                .iter()
                .map(|h| h.path().to_string_lossy().into_owned())
                .collect(),
            Pick::Folder => dialog
                .pick_folder()
                .await
                .map(|h| {
                    let path = h.path().to_string_lossy().into_owned();
                    match path.ends_with('/') {
                        true => path,
                        false => format!("{path}/"),
                    }
                })
                .into_iter()
                .collect(),
        };
        let mut selected = ctx.selected_path;
        let at = *selected.peek();
        let mut next = at;
        ctx.edit(|draft| next = draft.set_paths(at, picked));
        selected.set(next);
    });
}

/// The list of path rows, or its empty state.
///
/// **Freya's built-in `Table`**, the same one Settings ▸ Engine's properties grid is built from
/// (`apps/settings/views/engine/table.rs`) — not a hand-rolled box-with-rules lookalike
/// (AGENTS.md §3). The row rule, the shared column width and the hover response come with it,
/// and the four fork additions that pane paid for (`TableRow::theme`, `TableRow::on_press`,
/// `TableCell::main_align`, flex content) are exactly what a selectable single-column list of
/// text fields needs. One column, so no header: the section's own label already names it.
#[derive(PartialEq)]
struct PathList;

impl Component for PathList {
    fn render(&self) -> impl IntoElement {
        let form = form_theme();
        let ctx = use_consume::<ConfigureCtx>();
        let (count, selected, prefix, remote) = {
            let draft = ctx.draft.read();
            (
                draft.path_count(),
                draft.clamp_selection(*ctx.selected_path.read()),
                draft.bucket_prefix(),
                draft.remote(),
            )
        };

        if count == 0 {
            return Table::new().child(
                rect()
                    .width(Size::fill())
                    .height(Size::px(EMPTY_TABLE_HEIGHT))
                    .center()
                    .child(
                        Prose::new("No paths yet. Add one to point at your data.")
                            .color(form.hint_color),
                    ),
            );
        }

        let mut body = TableBody::new();
        for index in 0..count {
            body = body.child(
                PathRow {
                    index,
                    selected: index == selected,
                    prefix: prefix.clone(),
                    remote,
                    key: DiffKey::None,
                }
                .key(index),
            );
        }

        Table::new().column_widths(vec![Size::flex(1.)]).child(body)
    }
}

/// One row of the table: a bare field in its only cell, filled when it is the selected row.
///
/// **The row owns its buffer and the traffic runs both ways**, which a one-way field cannot do
/// here, because this row's value moves for two reasons that are not typing: a row above it is
/// removed (the list is keyed by position, so index 0's scope is *kept* when the list shrinks, and
/// a buffer seeded once would write the deleted path back over the survivor), and the browse picker
/// sets it (that writes the draft, not the box). `reported` is what keeps the two directions from
/// fighting — in state rather than captured, since `use_side_effect` builds its closure once.
#[derive(PartialEq)]
struct PathRow {
    index: usize,
    selected: bool,
    /// The bucket this row's path is written against, or `None` on the local disk *and* while no
    /// connection is chosen — the list's answer, carried as a prop rather than read here (see
    /// [`PathList`]).
    prefix: Option<String>,
    /// Whether LOCATION is on Remote. Not `prefix.is_some()`: a remote row with no connection
    /// picked yet has no prefix to wear and is still the row that has no Browse button behind it.
    remote: bool,
    key: DiffKey,
}

impl KeyExt for PathRow {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for PathRow {
    fn render(&self) -> impl IntoElement {
        let win = window_theme();
        let ctx = use_consume::<ConfigureCtx>();
        let index = self.index;

        let initial = ctx.draft.peek().path_at(index);
        let text = use_state({
            let initial = initial.clone();
            move || initial
        });
        let mut reported = use_state(move || initial);

        let field = use_a11y();
        let focus = use_focus(field);
        let mut selected = ctx.selected_path;
        use_side_effect(move || {
            if focus() != Focus::Not && *selected.peek() != index {
                selected.set(index);
            }
        });

        use_side_effect(move || {
            let path = text.read().clone();
            if path == *reported.peek() {
                return;
            }
            reported.set(path.clone());
            ctx.edit(move |draft| draft.set_path(index, path));
        });
        use_side_effect(move || {
            let outer = ctx.draft.read().path_at(index);
            if outer == *reported.peek() {
                return;
            }
            reported.set(outer.clone());
            let mut text = text;
            text.set(outer);
        });

        let fill = match self.selected {
            true => win.row_selected_background,
            false => Color::TRANSPARENT,
        };

        let form = form_theme();
        let prefix = self.prefix.clone();
        let remote = self.remote;

        TableRow::new()
            .theme(TableThemePartial {
                row_background: Some(fill.into()),
                hover_row_background: Some(fill.into()),
                ..Default::default()
            })
            .on_press(move |_: Event<PressEventData>| selected.set(index))
            .child(
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
                            .maybe_child(prefix.map(|prefix| {
                                MonoValue::new(prefix)
                                    .color(form.hint_color)
                                    .max_width(Size::percent(PREFIX_MAX_PERCENT))
                                    .max_lines(1)
                                    .text_overflow(TextOverflow::Ellipsis)
                            }))
                            .child(
                                ValueField::new(text)
                                    .bare()
                                    .width(Size::flex(1.))
                                    .height(Size::px(FIELD_HEIGHT))
                                    .maybe(remote, |field| {
                                        field.placeholder("events/2024/**/*.parquet")
                                    })
                                    .a11y_id(field),
                            ),
                    ),
            )
    }
}
