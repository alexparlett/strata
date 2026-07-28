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
use crate::components::form::{Row, ValueField};
use crate::components::icon::{Icon, IconName};
use crate::components::typography::Prose;

/// The toolbar's square buttons (the app's icon-button size), and the gap between them.
const TOOL_SIZE: f32 = 28.;
const TOOL_ICON: f32 = 15.;
const TOOL_GAP: f32 = 6.;
/// A path row's height, and the list's empty state (canvas `min-height: 88px`).
const ROW_HEIGHT: f32 = 34.;
const EMPTY_HEIGHT: f32 = 88.;
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
        let colors = use_theme().read().colors().clone();
        let ctx = use_consume::<ConfigureCtx>();
        // Subscribes: remove is disabled on an empty list, which is the one thing the toolbar
        // has to know about the list.
        let has_rows = !ctx.draft.read().sources.is_empty();

        rect()
            .horizontal()
            .cross_align(Alignment::Center)
            .spacing(TOOL_GAP)
            .child(
                ToolButton {
                    icon: IconName::Plus,
                    color: colors.primary,
                    label: "Add path",
                    enabled: true,
                }
                .on_press(move |_| ctx.edit(|draft| draft.add_path())),
            )
            .child(
                ToolButton {
                    icon: IconName::Minus,
                    color: colors.error,
                    label: "Remove path",
                    enabled: has_rows,
                }
                .on_press(move |_| ctx.edit(|draft| draft.remove_path())),
            )
            .child(BrowseButton)
    }
}

/// One 28 × 28 toolbar button. `Button::new().outline()` at the app's icon-button size — never
/// a hand-rolled square.
#[derive(PartialEq)]
struct ToolButton {
    icon: IconName,
    color: Color,
    label: &'static str,
    enabled: bool,
}

impl ToolButton {
    fn on_press(self, on_press: impl Into<EventHandler<Event<PressEventData>>>) -> Element {
        let on_press = on_press.into();
        let enabled = self.enabled;
        let button = Button::new()
            .outline()
            .enabled(enabled)
            .theme_layout(
                ButtonLayoutThemePartial::default()
                    .width(Size::px(TOOL_SIZE))
                    .height(Size::px(TOOL_SIZE))
                    // The stated box *is* the size: the stock padding would leave the glyph
                    // ~10px to sit in, and a button clips its overflow.
                    .padding(Gaps::new_all(0.)),
            )
            .on_press(move |e: Event<PressEventData>| on_press.call(e))
            .child(Icon::new(self.icon).size(TOOL_ICON).color(self.color));
        // The tooltip is the label: an icon-only button has no text of its own, and the app's
        // other icon clusters name themselves the same way.
        TooltipContainer::new(Tooltip::new(self.label))
            .position(AttachedPosition::Top)
            .child(button)
            .into_element()
    }
}

/// Browse — one button, two answers, because one native dialog cannot offer both.
#[derive(PartialEq)]
struct BrowseButton;

impl Component for BrowseButton {
    fn render(&self) -> impl IntoElement {
        let form = crate::components::form::form_theme();
        let ctx = use_consume::<ConfigureCtx>();
        let mut open = use_state(|| false);

        let menu = Menu::new()
            .min_width(Size::px(MENU_WIDTH))
            .on_close(move |_| open.set(false))
            .child(
                MenuButton::new()
                    .on_press(move |_| {
                        open.set(false);
                        pick(ctx, Pick::Files);
                    })
                    .child(menu_row(IconName::File, "Choose files…")),
            )
            .child(
                MenuButton::new()
                    .on_press(move |_| {
                        open.set(false);
                        pick(ctx, Pick::Folder);
                    })
                    .child(menu_row(IconName::Folder, "Choose a folder…")),
            );

        Attached::new(
            TooltipContainer::new(Tooltip::new("Browse for a source"))
                .position(AttachedPosition::Top)
                .child(
                    Button::new()
                        .outline()
                        .theme_layout(
                            ButtonLayoutThemePartial::default()
                                .width(Size::px(TOOL_SIZE))
                                .height(Size::px(TOOL_SIZE))
                                .padding(Gaps::new_all(0.)),
                        )
                        .on_press(move |_| open.toggle())
                        .child(
                            Icon::new(IconName::Folder)
                                .size(TOOL_ICON)
                                .color(form.label_color),
                        ),
                ),
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

/// The bordered list of path rows, or its empty state.
#[derive(PartialEq)]
struct PathList;

impl Component for PathList {
    fn render(&self) -> impl IntoElement {
        let colors = use_theme().read().colors().clone();
        let form = crate::components::form::form_theme();
        let ctx = use_consume::<ConfigureCtx>();
        let (count, selected) = {
            let draft = ctx.draft.read();
            (draft.sources.len(), draft.selected())
        };

        // Padded by the border's own width: torin draws a border *inside* the bounds its
        // children already occupy, so a row with a background would otherwise erase it.
        let mut list = rect()
            .width(Size::fill())
            .vertical()
            .padding(Gaps::new_all(1.))
            .corner_radius(6.)
            .background(colors.surface_secondary)
            .border(Border::new().width(1.).fill(colors.border));

        if count == 0 {
            return list.child(
                rect()
                    .width(Size::fill())
                    .height(Size::px(EMPTY_HEIGHT))
                    .center()
                    .child(
                        Prose::new("No paths yet — add one to point at your data.")
                            .color(form.hint_color),
                    ),
            );
        }

        for index in 0..count {
            if index > 0 {
                list = list
                    .child(crate::components::divider::Divider::horizontal().color(colors.border));
            }
            list = list.child(
                PathRow {
                    index,
                    selected: index == selected,
                    key: DiffKey::None,
                }
                // Keyed by position: a removed row must take its buffer with it, or the row
                // below would inherit what the removed one was showing.
                .key(index),
            );
        }
        list
    }
}

/// One path row: a bare mono field on the row's own background, selected by pressing it.
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
        let colors = use_theme().read().colors().clone();
        let ctx = use_consume::<ConfigureCtx>();
        let index = self.index;

        // The row owns its buffer and reports into the draft, like every other field here. It
        // is seeded once: the draft is only ever written *from* here for this row, so there is
        // nothing to sync back — and a pick, which does write it, re-keys the list.
        let text = use_state({
            let initial = ctx
                .draft
                .peek()
                .sources
                .get(index)
                .cloned()
                .unwrap_or_default();
            move || initial
        });
        use_side_effect(move || {
            let path = text.read().clone();
            ctx.edit(move |draft| {
                if let Some(slot) = draft.sources.get_mut(index) {
                    *slot = path;
                }
            });
        });

        rect()
            .width(Size::fill())
            .height(Size::px(ROW_HEIGHT))
            .cross_align(Alignment::Center)
            .maybe(self.selected, |el| el.background(colors.active))
            .on_pointer_down(move |_| ctx.edit(move |draft| draft.selected = index))
            .child(
                ValueField::new(text)
                    .bare()
                    .width(Size::fill())
                    .height(Size::px(ROW_HEIGHT))
                    .placeholder("/data/2024/  ·  *.parquet"),
            )
    }
}
