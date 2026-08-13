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
//! ## An object-store table is **one** path, in the same list
//!
//! On a connection (W7 · 04) the section is singular throughout — `SOURCE PATH`, no toolbar, one
//! row wearing the bucket as a non-editable prefix — because a remote source is written against
//! the connection's bucket and there is nothing to browse: an object store has no file dialog,
//! and its paths are text.
//!
//! It is still this list, one row long, rather than a second control drawn beside it. The row is
//! where the two-way sync between the box and the draft lives, and a canvas that draws a framed
//! box in place of a one-row framed table is drawing the same thing: what actually goes is the
//! toolbar (a control that would add rows the def cannot carry) and the empty state (a list that
//! always holds exactly one row has none).

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
        // One path on a connection, so the label, the explanation and the toolbar are all
        // singular — see the module doc.
        let (remote, internal) = {
            let draft = ctx.draft.read();
            (draft.remote(), draft.internal())
        };
        // A internal table brings no files at all, so this section has nothing to ask. Drawn as an
        // empty box rather than unmounted, for the differ (`views::hive`'s rule).
        if internal {
            return rect().into_element();
        }

        // The label line, its REQUIRED marker and its resolution tooltip are all the shared
        // row's; this window contributes the toolbar and the list that go under them.
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
        // `error` is one of the four semantic tones, so it is read from the shared ramp wherever
        // it appears rather than restated on a component theme.
        let error = tones().error;
        let ctx = use_consume::<ConfigureCtx>();
        // Subscribes: remove is disabled on an empty list, which is the one thing the toolbar
        // has to know about the list.
        let has_rows = !ctx.draft.read().sources.is_empty();

        rect()
            .horizontal()
            .cross_align(Alignment::Center)
            .spacing(TOOL_GAP)
            .child(
                ToolButton::new(IconName::Plus, "Add path")
                    .outlined()
                    .color(win.icon_color)
                    .on_press(move |_| {
                        // Seeded from the current selection, like the two handlers below: an
                        // edit refused while a registration is in flight leaves `at` untouched,
                        // and moving the highlight to row 0 for a row that was never added
                        // would be the one piece of window state that ignores that refusal.
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
        // **The picked request, not the picking.** A `MenuButton`'s press closes the menu, and
        // `spawn` binds a task to the scope it is called from — which inside that handler is the
        // item the press just unmounted, so the dialog was dropped before it was ever polled and
        // nothing happened at all. The request crosses the scope boundary in state instead, and
        // this component (which stays mounted when the menu goes) is what acts on it. The same
        // reason the catalog's re-scan raises a counter rather than spawning its own pass.
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
    // Start where the selected row points, so browsing from a set path opens there rather than
    // wherever the OS last left the panel.
    let start = {
        let draft = ctx.draft.peek();
        draft
            .sources
            .get(draft.clamp_selection(*ctx.selected_path.peek()))
            .cloned()
    }
    .filter(|s| !s.trim().is_empty());

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
            // A folder keeps its trailing separator: `ListingTableUrl` reads a path without one
            // as a single file, and the engine's own normalization only sees what is stored.
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
        // The bucket and the mode are read **here**, once, and carried to the rows as props.
        // They are the same for every row, and a row that read them itself would subscribe to
        // the whole draft — waking all of them on every keystroke in the name box, for a value
        // none of them saw change. This component already subscribes; a row only re-renders
        // when one of its props actually differs.
        let (count, selected, prefix, remote) = {
            let draft = ctx.draft.read();
            (
                draft.sources.len(),
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
                // Keyed by position, and the row syncs both ways against the draft — see
                // `PathRow`.
                .key(index),
            );
        }

        Table::new().column_widths(vec![Size::flex(1.)]).child(body)
    }
}

/// One row of the table: a bare field in its only cell, filled when it is the selected row.
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

        // The row owns its buffer, and the traffic runs **both ways** — which a one-way field
        // cannot do here, because this row's value changes underneath it for two reasons that
        // have nothing to do with typing:
        //
        // - **a row above it is removed.** The list is keyed by position, so the scope at index
        //   0 is *kept* when the list shrinks; a buffer seeded once would then write the deleted
        //   path back over the survivor.
        // - **the browse picker sets it.** That writes the draft, not the box, so a
        //   report-only field would leave the box showing the old path while the draft held the
        //   new one.
        //
        // `reported` is what keeps the two directions from fighting: it tracks the last value
        // this row and the draft agreed on, so neither effect acts on a change the other made.
        // In state rather than captured — `use_side_effect` builds its closure once, so a
        // captured comparison value freezes at the first render.
        let initial = ctx
            .draft
            .peek()
            .sources
            .get(index)
            .cloned()
            .unwrap_or_default();
        let text = use_state({
            let initial = initial.clone();
            move || initial
        });
        let mut reported = use_state(move || initial);

        // `TableRow::on_press` selects the row, and **focus does too**: Freya's `Input`
        // registers `on_focus_press` — sugar over `on_pointer_down` — and stops propagation, so
        // the row's press never fires over the field itself. Clicking into a row to edit it
        // would otherwise leave the toolbar pointing at whichever row was selected before, and
        // Remove would delete that one.
        let field = use_a11y();
        let focus = use_focus(field);
        let mut selected = ctx.selected_path;
        use_side_effect(move || {
            if focus() != Focus::Not && *selected.peek() != index {
                selected.set(index);
            }
        });

        // Out: what was typed reaches the draft.
        use_side_effect(move || {
            let path = text.read().clone();
            if path == *reported.peek() {
                return;
            }
            reported.set(path.clone());
            ctx.edit(move |draft| draft.set_path(index, path));
        });
        // In: a value this row did not type reaches the box.
        use_side_effect(move || {
            let outer = ctx
                .draft
                .read()
                .sources
                .get(index)
                .cloned()
                .unwrap_or_default();
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

        // The chosen bucket, standing in front of the box as text rather than in it: what the
        // user writes is the part they can change, and a prefix inside the field would be a
        // prefix they could delete. Absent on the local disk, where the path is the whole
        // address — and that absence is also what says this row is local, so the two never
        // disagree.
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
                        // A flex row *inside* the cell, not the cell itself: a `TableCell` lays
                        // its children out under `Content::Normal`, where a flexing box takes a
                        // share rather than the remainder (AGENTS.md §3).
                        rect()
                            .expanded()
                            .horizontal()
                            .content(Content::Flex)
                            .cross_align(Alignment::Center)
                            .maybe_child(prefix.map(|prefix| {
                                // Capped and ellipsized, because the prefix is laid out at its
                                // natural width **before** the field divides what is left: an
                                // HTTP connection is a whole origin, and a long host would
                                // otherwise take the cell and leave nothing to type into. The
                                // whole bucket is one row up in the picker either way.
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
                                    // Only where there is no browse button to fill the box for
                                    // you: a bucket-relative path has no shape the user can
                                    // infer from the label, where a local one is a path they
                                    // already know how to write (and usually pick).
                                    .maybe(remote, |field| {
                                        field.placeholder("events/2024/**/*.parquet")
                                    })
                                    .a11y_id(field),
                            ),
                    ),
            )
    }
}
