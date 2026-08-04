//! The **Export** window (P4-10 / design `Export.dc.html` for the layout,
//! `Strata.exportGroups()` for the options) — opened from the results toolbar, pinned above
//! the project window that asked.
//!
//! **It exports a result, not a tab.** The window is opened on the run that is on screen and
//! carries that run's facts ([`ExportTarget`]) as launch values: the snapshot handle, its
//! schema, its row count, the grid's active sort, and the page in hand for the preview. That is
//! sound precisely because a snapshot is immutable — and the window **pins** it
//! ([`EngineCtx::pin_snapshot`]) for its whole life, so a re-run in the tab behind can't retire
//! the table out from under it. Without that pin a re-run either truncates a running `COPY` or
//! makes a later Export report no results when there are plainly some on screen
//! (`docs/SNAPSHOT_SPEC.md` §4).
//!
//! **Options are data** ([`model`]): [`ExportDraft::groups`] returns the list and
//! [`views::Options`] renders whatever it is handed, so a new option is a row in a table rather
//! than a new branch in a component. There is no ADVANCED section — the canvas folded it away.
//!
//! **Nothing here is committed until Export is pressed**, and the destination is the native
//! save dialog, so there is no draft to persist and no Cancel to undo: closing the window is
//! the discard.

mod model;
mod preview;
#[cfg(test)]
mod tests;
mod views;

use std::rc::Rc;

use freya::prelude::*;
use freya::winit::platform::macos::WindowAttributesExtMacOS;
use freya::winit::window::WindowId;
use strata_core::config::Command;
use strata_core::engine::SnapshotPin;

use crate::apps::export::views::{ExportBody, Footer, TitleBar};
use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::LogCtx;
use crate::keymap::on_commands;
use crate::menu::MenuScope;
use crate::platform::{quit, use_owner_pin, use_register_window, Subtree, WindowKind};
use crate::state::{use_share_config, AppCtx};
use crate::theme::{peek_selection, use_roles, use_strata_theme, window_background, Role};

pub use model::{ExportDraft, ExportTarget, FormatId, ScopeChoice};

// `%[no_ext]`: the window's dress is read by its sibling views (title bar · formats · options ·
// partition · preview · footer) rather than by one `Export` component, so there is no type for
// the generated `…ThemePartialExt` builder to hang off.
define_theme!(
    %[no_ext]
    %[component]
    pub Export {
        %[fields]
        /// The window body (canvas `--c-pop`) — the *lightest* of the three tones here.
        background: Color,
        /// A **recessed** inset (canvas `--c-panel`) — a format card at rest, a text field,
        /// the preview block, and the two transfer panes. Below the body, not above it: the
        /// canvas sinks a form's boxes into the window rather than floating them on it.
        panel_background: Color,
        /// A transfer pane's header strip (canvas `--c-surface`), between the two — which is
        /// what makes it read as a header rather than as more of either.
        header_background: Color,
        /// The title-bar rule, the preview's separator, and a panel's edge.
        border_fill: Color,
        /// A rule *inside* a panel — a transfer pane's header strip from its list, and a row
        /// from the next. The canvas's `--c-line`, a step below `border`: a division within a
        /// box should be quieter than the box's own edge.
        divider_fill: Color,
        /// A control's edge — a text field, a card, a transfer pane's header.
        control_border_fill: Color,
        /// The window's download glyph, and the tile behind it.
        icon_color: Color,
        icon_background: Color,
        /// An option group's uppercase label, and a transfer pane's eyebrow.
        label_color: Color,
        /// The ⓘ hint glyph beside a label.
        hint_color: Color,
        /// A transfer pane's empty-state line. `text_muted`, like the drawer's — the canvas's
        /// `--c-faint` is dimmer than anything else in this window reads at, and a sentence the
        /// user is meant to act on should not be the faintest thing on screen.
        empty_color: Color,
        /// A format card's name at rest, and the selected card's dress.
        card_color: Color,
        card_active_background: Color,
        card_active_border_fill: Color,
        /// The selected-partition row's order badge, and its text.
        badge_background: Color,
        badge_color: Color,
        /// The high-cardinality warning banner (its glyph and text take the shared ramp's
        /// `warning`, which is semantic and must follow the app wherever it appears).
        warning_background: Color,
        warning_border_fill: Color,
    }
);

/// Everything a press of the results toolbar's Download needs, resolved once where the data
/// and the DI handles both live (the results pane) and carried to the button as a prop.
///
/// **Props, not context, deliberately.** The toolbar is a shallow, known consumer — the
/// state-architecture's rule for exactly this (`AGENTS.md` §4: context is for DI handles and
/// deep, open-ended trees). Consuming `EngineCtx` and `AppCtx` in the button itself would put a
/// context requirement on a leaf that only ever renders inside the pane that already has both.
#[derive(Clone)]
pub struct ExportLaunch {
    pub target: ExportTarget,
    pub engine: EngineCtx,
    pub app: AppCtx,
    /// The **project window's** event log (P3-13). An export is a write the user asked for, so
    /// both arms are recorded there — and it has to be the opener's log, because this window
    /// closes itself on success and the user is looking at that one.
    pub log: LogCtx,
    /// The **open project** that log belongs to — what ties the window's lifetime to that
    /// subtree rather than to the owner's window id, which outlives it both across a re-root and
    /// across an engine restart (see [`crate::platform::owner`]).
    pub subtree: Subtree,
}

/// Compares on the **target** alone. Everything else is fixed for the window's whole life — one
/// engine, one set of app-globals, one log, and one `Subtree` that can only change by remounting
/// the very subtree this is built in — so none of it carries information a diff could act on. The
/// same reasoning as freya-query's `Captured`, which is invisible to cache identity for exactly
/// this reason.
impl PartialEq for ExportLaunch {
    fn eq(&self, other: &Self) -> bool {
        self.target == other.target
    }
}

/// How the export is going. `Failed` keeps the message on screen instead of closing, because
/// the window is the only place that can explain what went wrong.
#[derive(Clone, PartialEq)]
pub enum Status {
    /// Waiting for the user.
    Idle,
    /// A `COPY` is running. The footer's button says so and stops taking presses.
    Writing,
    /// The last attempt failed; the footer shows this.
    Failed(String),
}

/// The window's shared state, provided at the root and consumed by every view.
///
/// `Copy`, because all three fields are handles. There is no draft/commit split like Settings'
/// — nothing here is persisted, so the window closing *is* the discard.
#[derive(Clone, Copy)]
pub struct ExportCtx {
    /// Every control's edits land here.
    pub draft: State<ExportDraft>,
    /// The run being exported. A `State` only so the context stays `Copy` — nothing writes it.
    pub target: State<ExportTarget>,
    pub status: State<Status>,
}

impl ExportCtx {
    /// Mutate the draft — **the one write path**, used by the data-driven option groups and by
    /// the partition pane's bespoke controls alike.
    ///
    /// It exists to make two rules unforgettable rather than to wrap a `write()`.
    ///
    /// A failure message describes the spec that produced it, so any change to the draft makes
    /// it a lie; clearing it here means no control has to remember to.
    ///
    /// And it is **idempotent**: an edit that changes nothing neither writes nor clears the
    /// message. That is what lets a control just apply what the user typed instead of guarding
    /// the write itself — a guard the text controls got *wrong*, because `use_side_effect`
    /// builds its closure once, so any value they captured to compare against was frozen at
    /// their first render and reverting a field to its original value silently did nothing.
    pub fn edit(mut self, f: impl FnOnce(&mut ExportDraft)) {
        {
            let mut next = self.draft.peek().clone();
            f(&mut next);
            if next == *self.draft.peek() {
                return;
            }
            self.draft.set(next);
        }
        if *self.status.peek() != Status::Idle {
            self.status.set(Status::Idle);
        }
    }
}

/// The Export window: the canvas's 780×640 frame (`_winDefault("export")`), with the title bar
/// drawn by [`TitleBar`] rather than AppKit — the same transparent-titlebar treatment as every
/// other window in the app.
pub struct ExportApp {
    pub app: AppCtx,
    /// The project window's engine. An `Arc` clone, so this window can export (and pin) without
    /// reaching back across the window boundary.
    pub engine: EngineCtx,
    /// The **open project** the log below belongs to — what bounds this window's lifetime
    /// ([`crate::platform::owner`]).
    pub subtree: Subtree,
    pub target: ExportTarget,
    /// Where this export's outcome is recorded — the opener's log, not this window's.
    pub log: LogCtx,
    /// The window this one belongs to — what it is pinned above, and what
    /// [`platform::use_owner_pin`](crate::platform::use_owner_pin) resolves the live subtree
    /// through. A launch value rather than a field on the registry entry, because the pin is its
    /// only reader and a second copy in the registry is one that can go stale.
    pub owner: WindowId,
}

impl ExportApp {
    pub fn window(
        app: AppCtx,
        engine: EngineCtx,
        subtree: Subtree,
        target: ExportTarget,
        log: LogCtx,
        owner: WindowId,
    ) -> WindowConfig {
        // Match the theme's window body so a resize doesn't flash the default white — through
        // `peek_selection`, since Settings may be previewing a theme right now.
        let background = {
            let sel = peek_selection(app.config, app.preview);
            let id = sel.effective(strata_core::theme::os_is_dark());
            window_background(app.themes.get_or_default(&id))
        };
        WindowConfig::new_app(ExportApp {
            app,
            engine,
            subtree,
            target,
            log,
            owner,
        })
        .with_title("Export results")
        .with_size(780., 640.)
        // The canvas's own floor: below this the four format cards stop fitting on one row and
        // the transfer panes lose their column names.
        .with_min_size(560., 420.)
        .with_background(background)
        // The 50px strip centres macOS's 16px buttons at y = 17; AppKit's default origin is
        // (7, 6), so the inset is the difference.
        .with_traffic_light_inset(9., 11.)
        .with_window_attributes(move |attrs, _| {
            attrs
                .with_titlebar_transparent(true)
                .with_fullsize_content_view(true)
                .with_title_hidden(true)
        })
    }
}

impl App for ExportApp {
    fn render(&self) -> impl IntoElement {
        // The window-root steps every app takes: this window's theme derived from the shared
        // settings, and the app-globals into context.
        use_strata_theme(self.app.themes.clone(), self.app.config, self.app.preview);
        use_share_config(self.app.config);
        use_provide_context({
            let app = self.app.clone();
            move || app
        });
        // The engine, so the footer's Export can call the facade directly, and the opener's
        // log, so it can record the outcome there.
        use_provide_context({
            let engine = self.engine.clone();
            move || engine
        });
        let log = self.log;
        use_provide_context(move || log);
        // Join the live window registry, so the app knows which window this panel belongs to,
        // and point the menubar here as a **panel** while this window is focused. Without that
        // the menubar kept the owner project window's File menu, and Close Project (and its
        // ⇧⌘W) closed *this* window while naming the project. Esc is how an export closes.
        let owner = self.owner;
        use_register_window(&self.app, || WindowKind::Export, MenuScope::Panel);
        // …and close with the *subtree* the log above belongs to, not merely with the window that
        // owns it: a re-root and an engine restart both drop it while leaving that window open
        // under the same id.
        use_owner_pin(self.app.clone(), owner, self.subtree.clone());

        // **Hold the snapshot open for this window's life.** This is what makes the target's
        // facts stay true: a re-run in the tab behind defers its retire until this drops
        // (SNAPSHOT_SPEC §4). `use_hook` so it is taken once at mount and dropped with the
        // scope — the RAII guard *is* the lifetime, so there is nothing to release by hand.
        let snapshot = self.target.snapshot;
        let engine = self.engine.clone();
        use_hook(move || SnapshotHold(Rc::new(engine.pin_snapshot(snapshot))));

        let ctx = use_provide_context({
            let target = self.target.clone();
            move || ExportCtx {
                draft: State::create(ExportDraft::default()),
                target: State::create(target),
                status: State::create(Status::Idle),
            }
        });

        let theme = get_theme!(&None::<ExportThemePartial>, ExportThemePreference, "export");
        let config = self.app.config;
        let platform = use_hook(Platform::get);

        rect()
            .expanded()
            .vertical()
            .content(Content::Flex)
            .background(theme.background)
            // The window's ambient text colour, like the launcher's: runs that don't name one
            // inherit it rather than Freya's base-theme default.
            .color(use_roles().get(Role::Text))
            .child(TitleBar)
            .child(ExportBody)
            .child(Footer)
            // Esc and ⌘Q. Deliberately the LAST child — same-name global listeners fire in
            // document order, so anything a view mounts outranks this.
            .child(rect().on_global_key_down(on_commands(config, {
                move |cmd| match cmd {
                    // Esc closes — nothing was committed, so there is nothing to undo. A write
                    // in flight is the exception: the file is half-written and the window is
                    // the only thing that will report how it ends.
                    Command::Cancel => {
                        if *ctx.status.peek() != Status::Writing {
                            platform.close_current_window();
                        }
                        true
                    }
                    Command::Quit => {
                        quit();
                        true
                    }
                    _ => false,
                }
            })))
    }
}

/// The snapshot hold, parked in a hook so it lives exactly as long as the window.
///
/// A newtype rather than the bare pin because `use_hook`'s value is what the scope drops, and
/// naming it is what makes the lifetime legible at the call site. `Rc` because a hook's value
/// must be `Clone` and a hold deliberately is not — the `Rc` clones, the hold does not, so the
/// snapshot is released exactly once, when the last clone goes with the scope.
#[derive(Clone)]
struct SnapshotHold(#[allow(dead_code)] Rc<SnapshotPin>);
