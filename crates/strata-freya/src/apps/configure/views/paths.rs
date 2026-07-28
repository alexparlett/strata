//! **SOURCE PATHS** — the label with its `REQUIRED` marker and resolution tooltip, the
//! three-button toolbar, and the list of path rows.
//!
//! Browse is in the **toolbar**, acting on the selected row, which is the canvas's arrangement
//! and not a per-row button. It opens a menu rather than a dialog, because one native dialog
//! cannot offer files *and* folders: `NSOpenPanel` is configured for one or the other, so the
//! canvas's single "Browse… (file or folder)" button becomes one button with two ways to answer
//! it. Picking files is multi-select — a table *is* many paths, and five files one dialog at a
//! time is the same five rows with four more dialogs.

use freya::prelude::*;

use crate::apps::configure::ConfigureCtx;
use crate::components::form::{form_theme, Row, ValueField};
use crate::components::icon::{Icon, IconName};
use crate::components::tool_button::ToolButton;
use crate::components::typography::Prose;
use crate::components::window::window_theme;

/// The gap between the toolbar's buttons (their size is the shared control's).
const TOOL_GAP: f32 = 6.;
/// The list's empty state (canvas `min-height: 88px`), and the gap between two path fields. A
/// row's own height is the form's `FIELD_HEIGHT` — this list holds fields, so it does not get to
/// invent a height for them.
const EMPTY_HEIGHT: f32 = 88.;
const ROW_GAP: f32 = 6.;
/// The band around the selected row's field — enough to read as a band rather than a hairline.
const SELECTED_INSET: f32 = 4.;
/// The gap between the label row, the toolbar and the list.
const STACK_GAP: f32 = 8.;
/// The browse dropdown's width — enough for its two labels without the card hugging them.
const MENU_WIDTH: f32 = 180.;

/// What each shape of path resolves to — the canvas's ⓘ, verbatim in substance.
const RESOLUTION_HINT: &str = "Each path resolves to one or more data files in the format chosen \
                               above, combined into this table. A file is one file; a folder is \
                               every file in it; a glob such as **/*.csv matches recursively. \
                               Paths are absolute.";

#[derive(PartialEq)]
pub struct SourcePaths;

impl Component for SourcePaths {
    fn render(&self) -> impl IntoElement {
        // The label line, its REQUIRED marker and its resolution tooltip are all the shared
        // row's; this window contributes the toolbar and the list that go under them.
        Row::new("SOURCE PATHS")
            .required()
            .hint(RESOLUTION_HINT)
            .child(
                rect()
                    .width(Size::fill())
                    .vertical()
                    .spacing(STACK_GAP)
                    .child(Toolbar)
                    .child(PathList),
            )
    }
}

/// Add · remove · browse. The canvas's three, in its order.
#[derive(PartialEq)]
struct Toolbar;

impl Component for Toolbar {
    fn render(&self) -> impl IntoElement {
        let win = window_theme();
        // `error` is one of the sheet's four semantic slots, so it is read from there wherever
        // it appears rather than restated on a component theme.
        let error = use_theme().read().colors().error;
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
                    .on_press(move |_| ctx.edit(|draft| draft.add_path())),
            )
            .child(
                ToolButton::new(IconName::Minus, "Remove path")
                    .outlined()
                    .color(error)
                    .enabled(has_rows)
                    .on_press(move |_| ctx.edit(|draft| draft.remove_path())),
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
            .on_close(move |_| open.set(false))
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
        .maybe_child(open().then(|| menu))
    }
}

/// One row of the browse menu: a glyph and its label.
fn menu_row(icon: IconName, label: &str) -> impl IntoElement {
    rect()
        .horizontal()
        .cross_align(Alignment::Center)
        .spacing(8.)
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
        draft.sources.get(draft.selected()).cloned()
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
        ctx.edit(move |draft| draft.set_paths(picked));
    });
}

/// The list of path rows, or its empty state.
///
/// Each row is an ordinary [`ValueField`] — its own box, its own focus ring, like every other
/// input in the app. So the list itself draws no chrome: a bordered container around boxed
/// fields is the second box the canvas's own bare rows were avoiding, and the rows are what the
/// user is actually pointing at.
#[derive(PartialEq)]
struct PathList;

impl Component for PathList {
    fn render(&self) -> impl IntoElement {
        let form = form_theme();
        let ctx = use_consume::<ConfigureCtx>();
        let (count, selected) = {
            let draft = ctx.draft.read();
            (draft.sources.len(), draft.selected())
        };

        if count == 0 {
            return rect()
                .width(Size::fill())
                .height(Size::px(EMPTY_HEIGHT))
                .center()
                .child(
                    Prose::new("No paths yet. Add one to point at your data.")
                        .color(form.hint_color),
                );
        }

        rect()
            .width(Size::fill())
            .vertical()
            .spacing(ROW_GAP)
            .children((0..count).map(|index| {
                PathRow {
                    index,
                    selected: index == selected,
                    key: DiffKey::None,
                }
                // Keyed by position, and the row syncs both ways against the draft — see
                // `PathRow`.
                .key(index)
                .into_element()
            }))
    }
}

/// One path row: the field, marked when it is the row the toolbar acts on.
///
/// The mark earns its place — it is not decoration. **Remove path** deletes this row and
/// **Browse** opens in this row's directory, so with three paths in the list there is otherwise
/// no way to see what either button is about to do before pressing it.
#[derive(PartialEq)]
struct PathRow {
    index: usize,
    selected: bool,
    key: DiffKey,
}

impl KeyExt for PathRow {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for PathRow {
    fn render(&self) -> impl IntoElement {
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

        // Out: what was typed reaches the draft.
        use_side_effect(move || {
            let path = text.read().clone();
            if path == *reported.peek() {
                return;
            }
            reported.set(path.clone());
            ctx.edit(move |draft| {
                if let Some(slot) = draft.sources.get_mut(index) {
                    *slot = path;
                }
            });
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

        // The band sits *around* the field rather than behind it: the field now draws its own
        // box, so a background directly under it would show as a hairline and read as nothing.
        rect()
            .width(Size::fill())
            .padding(Gaps::new_all(SELECTED_INSET))
            .corner_radius(8.)
            .maybe(self.selected, |el| {
                el.background(window_theme().row_selected_background)
            })
            .on_pointer_down(move |_| ctx.edit(move |draft| draft.selected = index))
            .child(ValueField::new(text).width(Size::fill()))
    }
}
