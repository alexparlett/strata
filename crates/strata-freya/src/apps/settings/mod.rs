//! The **Settings** window (P4-03 / design `Settings.dc.html`) — one instance app-wide,
//! pinned above whichever window asked for it (see [`crate::platform::settings`]).
//!
//! **Navigation is `freya-router`** over an in-memory history: one [`Route`] per category
//! under the [`SettingsChrome`] layout, so the window frame (title bar · nav rail · footer)
//! is mounted once and only the pane swaps. The nav tree itself is data — see [`model`].
//!
//! **Draft / save.** Every control edits [`SettingsCtx::draft`], a working copy of the
//! settings seeded from the committed ones on mount. **Apply** commits it into the app-global
//! config store (the one thing here that touches disk) and closes; **Cancel**, Esc and the red
//! button close without writing, which *is* the discard — nothing was committed. The commit is
//! a per-field diff against the seed, so a setting another window wrote while this one was
//! open survives it ([`SettingsCtx::apply`]).
//!
//! **The theme previews live.** Its half of the draft is mirrored into the app-global
//! [`ThemePreview`] on every edit, so every open window re-themes the instant a theme is
//! picked while the choice is still uncommitted (`crate::state::theme_preview` has the why).
//! Dropping that slot on the way out is what makes Cancel a revert.
//!
//! P4-03 built the **shell**: the window, the nav, the draft/save/preview machinery and the
//! entry points. P4-04 added the first pane ([`views::ThemePane`], the theme picker) and P4-05
//! the second ([`views::DataDisplayPane`]), and moved the row vocabulary every pane is built
//! from into [`crate::components::form`] — a pane is a `Form::preferences` of `Row`s, and
//! nothing about the rhythm between them lives here. P4-06 added the third
//! ([`views::SystemPane`]), P4-07 the fourth ([`views::EnginePane`]) — which is the one category
//! to take both of [`views::Pane`]'s opt-outs, being a surface that manages its own height — and
//! P4-08 the last ([`views::KeymapPane`]).
//!
//! AA-04 added a sixth, [`views::AgentAccessPane`] — the control for the MCP server AA-03
//! ships dark, and an ordinary preferences list again: the switch, the port and the token,
//! committed by the same Apply as everything else.

mod model;
mod search;
mod views;

use std::collections::BTreeMap;

use freya::prelude::*;
use freya::router::*;
use freya::winit::platform::macos::WindowAttributesExtMacOS;
use strata_core::config::{Command, Settings};

use crate::apps::settings::views::{
    AgentAccessPane, DataDisplayPane, EnginePane, KeymapPane, PropRows, SettingsChrome, SystemPane,
    ThemePane,
};
use crate::components::form::Reveal;
use crate::keymap::on_commands;
use crate::menu::MenuScope;
use crate::platform::{self, WindowKind};
use crate::state::{
    use_share_config, write_config, AppCtx, ConfigChan, ConfigStation, ThemePreview, ThemeSel,
};
use crate::theme::{peek_selection, use_roles, use_strata_theme, window_background, Role};

pub use model::{category, Category, NavGroup, CATEGORIES};
pub use search::{search, Anchor, Hit};

// `%[no_ext]`: the window's dress is read by its sibling views (chrome · nav · footer · pane)
// rather than by one `Settings` component, so there is no type for the generated
// `…ThemePartialExt` builder to hang off.
define_theme!(
    %[no_ext]
    %[component]
    pub Settings {
        %[fields]
        /// The window body — the pane behind the categories, and the footer strip.
        background: Color,
        /// The category rail's raised surface.
        nav_background: Color,
        /// The title-bar rule, the rail's right edge, and the footer's top rule.
        border_fill: Color,
        /// The window's gear glyph, and the tile behind it.
        icon_color: Color,
        icon_background: Color,
        /// A nav group heading ("Appearance & behaviour").
        group_color: Color,
        /// A group heading's disclosure chevron.
        chevron_color: Color,
        /// A category row at rest.
        item_color: Color,
        /// The current category's pill, and its label.
        item_active_background: Color,
        item_active_color: Color,
        /// The breadcrumb's leading group ("Appearance & behaviour"). **Not** a setting's
        /// subtext — that is the shared form's `hint_color`, since the row belongs to
        /// [`crate::components::form`] and a component's dress is its own theme's.
        hint_color: Color,
        /// A theme card (P4-04): its surface, its resting / hover ring, and the rule between
        /// the preview and the card's name row.
        card_background: Color,
        card_border_fill: Color,
        card_hover_border_fill: Color,
        card_divider_fill: Color,
        /// What "this one is picked" is painted in — the selected card's ring *and* its tick.
        /// One field, because they are one meaning; two would be the same colour named twice.
        selected_color: Color,
        /// A theme card's source badge, per [`Source`](strata_core::theme::Source). The fill is
        /// derived from these at `Badge`'s tint alpha, so a source is one token, not two.
        badge_builtin_color: Color,
        badge_user_color: Color,
        /// The Engine pane's properties grid (P4-07), whose surrounding dress — surface, box
        /// border, row rule, radius — is Freya's own `table` component theme. These two are what
        /// a table cannot have an opinion about, because *which* row is selected is the caller's
        /// answer: the header strip's raised fill, and the selected row's accent tint. There is
        /// no zebra token; the grid is a settings list, not a results grid.
        ///
        /// The head sits **one** step over the grid's own surface (the canvas's `--c-surface`
        /// over `--c-panel`), which is not the same step the results grid's header takes — that
        /// one is reading dense data and stands further out. Borrowing its slot was how this
        /// landed too light, and daylight hid it because both resolve to white there.
        table_head_background: Color,
        table_selection_background: Color,
        // A key cap's own three colours used to sit here, when the Keymap pane was the only
        // surface drawing one. The command palette draws them too, so they moved to the shared
        // `keycap` token group — a component's dress is the component's
        // (`components::keycap`).
        /// The **dashed** edge of a slot with nothing in it yet — the Add-shortcut button on an
        /// unbound row. Its own field rather than the table's `border_fill`, because it has to
        /// stand a step out from the grid's own hairlines to read as an invitation at all; a
        /// dashed line pitched for a box outline mostly disappears.
        slot_border_fill: Color,
    }
);

/// This window's resolved `settings` dress — the shared accessor for panes that read the window
/// theme with no prop override. Call from a component's `render` (it is a theme lookup). Three
/// panes had grown byte-identical private copies of this.
pub fn settings_theme() -> SettingsTheme {
    get_theme!(
        &None::<SettingsThemePartial>,
        SettingsThemePreference,
        "settings"
    )
}

/// The window's category pages. Theme is `/` because it is where the window opens.
///
/// Each variant names its pane component explicitly (the derive would otherwise look for one
/// called after the variant), so the pages read as `…Pane` and `Theme` stays Freya's.
#[derive(Routable, Clone, PartialEq, Eq, Debug)]
#[rustfmt::skip]
pub enum Route {
    #[layout(SettingsChrome)]
        #[route("/", ThemePane)]
        Theme,
        #[route("/system", SystemPane)]
        System,
        #[route("/data-display", DataDisplayPane)]
        DataDisplay,
        #[route("/keymap", KeymapPane)]
        Keymap,
        #[route("/agent-access", AgentAccessPane)]
        AgentAccess,
        #[route("/engine", EnginePane)]
        Engine,
}

/// The window's shared editing state, provided at the root and consumed by the chrome and
/// every category pane.
///
/// `Copy`, because all three fields are handles: the draft is this window's own `State`, and
/// the other two are app-globals it commits into.
#[derive(Clone, Copy)]
pub struct SettingsCtx {
    /// The working copy every control edits. Seeded from the committed settings on mount and
    /// thrown away with the window unless Apply commits it.
    pub draft: State<Settings>,
    /// The settings the draft was seeded from — written once, at mount, and never again.
    /// The baseline for both questions this window asks: what the user has changed
    /// ([`dirty`](Self::dirty)) and what to commit ([`apply`](Self::apply)).
    seed: State<Settings>,
    /// The Engine pane's rows (P4-07). The one piece of editing state that is **not** a field of
    /// the draft, because `Settings::engine` is a map and a map cannot hold the row you have not
    /// named yet or the duplicate you are halfway through fixing. It lives on the window rather
    /// than in the pane so that navigating to another category and back does not throw away a
    /// half-finished edit — and so the footer can ask what is blocking Apply without the pane
    /// being mounted to answer.
    pub engine: State<PropRows>,
    /// The live theme preview the draft's theme half is mirrored into.
    preview: ThemePreview,
    /// The app-global config: Apply's target.
    config: ConfigStation,
    /// Why the last Apply didn't stick (P4-15) — `None` until one fails.
    ///
    /// The app config is written by nine call sites and this is the only one that is a
    /// **deliberate commit**: the user changed a setting and pressed a button that says Apply.
    /// The rest are bookkeeping (a recent pushed, the open-set updated) that nobody asked for
    /// and nothing can be done about, so they stay `tracing`-only rather than each announcing
    /// the same failure of the same file — see `state::write_config`.
    ///
    /// Held on the window rather than the footer because it is what stops the window closing;
    /// a message owned by a component that is about to be unmounted has nowhere to be read.
    failed: State<Option<String>>,
}

impl SettingsCtx {
    fn new(config: ConfigStation, preview: ThemePreview) -> Self {
        let settings = config.peek().settings.clone();
        Self {
            engine: State::create(PropRows::from_map(&settings.engine)),
            draft: State::create(settings.clone()),
            seed: State::create(settings),
            preview,
            config,
            failed: State::create(None),
        }
    }

    /// The engine overrides the window opened on — what Revert restores the grid to.
    pub fn seed_engine(&self) -> BTreeMap<String, String> {
        self.seed.peek().engine.clone()
    }

    /// What is stopping Apply, if anything — a sentence for the footer, and the reason the button
    /// is disabled while the draft *is* dirty.
    ///
    /// Asked of the context rather than of a pane, because the footer is mounted for every
    /// category and the pane that can answer is mounted for one. Today only the Engine pane can
    /// block; a second surface that can would add a branch here rather than a second gate.
    pub fn blocker(&self) -> Option<String> {
        let faults = self.engine.read().errors().len();
        match faults {
            0 => None,
            1 => Some("1 engine property is invalid".to_string()),
            n => Some(format!("{n} engine properties are invalid")),
        }
    }

    /// Edit one field of the draft — the write path every control on every pane goes through.
    ///
    /// Takes `self` by value, like `ExportCtx::edit`: the caller consumed the context during
    /// its own render, so this is safe to call from an event handler, where there is no scope
    /// left to read one from. (`State` is `Copy`, which is what makes the local `mut` binding
    /// the way a handler reaches the draft at all.)
    pub fn edit(self, edit: impl FnOnce(&mut Settings)) {
        let mut draft = self.draft;
        edit(&mut draft.write());
    }

    /// Whether the draft has anything to commit — i.e. whether **the user** has changed
    /// something. Reactive on the draft, so a control's edit repaints the footer's Apply.
    ///
    /// Measured against the seed, not against what is committed now: another window can
    /// commit a setting while this one is open, and comparing to the live config would then
    /// enable Apply for a change the user never made — an Apply that, since it is a per-field
    /// merge, would commit nothing at all. The seed never changes, so this reads no config
    /// state and the footer isn't woken by config writes it has no interest in.
    pub fn dirty(&self) -> bool {
        *self.draft.read() != *self.seed.peek()
    }

    /// Publish the draft's theme selection as the live preview. Driven by a side effect at
    /// the root, so *any* control that touches `theme` or `sync_os` previews without knowing
    /// this exists.
    ///
    /// `set_if_modified`: the draft also carries the row limit, the default directory and
    /// every other field, and waking every window's theme derivation on a keystroke in one of
    /// those is exactly what the preview being narrow is for.
    fn sync_preview(&self) {
        let sel = ThemeSel::from(&*self.draft.read());
        let mut preview = self.preview;
        preview.set_if_modified(Some(sel));
    }

    /// Commit the draft: publish it to every window and persist it. The preview is dropped in
    /// the same breath — the committed settings now resolve to the identical theme, so the
    /// derivation's id guard means nothing repaints twice.
    ///
    /// A **per-field merge** against the seed, not a whole-struct write: this window's draft
    /// is a snapshot of the settings as they were when it opened, and another window can
    /// commit one of its own in the meantime (the close confirm's "Don't ask again" writes
    /// `confirm_close_running` from a window that never shows it). Writing the draft wholesale
    /// would carry its stale copy of that field back over the top. `Settings::merge_onto`
    /// commits only the fields this draft actually changed.
    /// On a commit that lands, the seed advances to what was just committed, so the diff is
    /// always measured against the last commit rather than against mount. Today the footer
    /// closes the window straight after, but an Apply that *didn't* close would otherwise
    /// re-commit this same diff on the next press — over whatever another window had written
    /// in between.
    ///
    /// Returns whether it reached disk. `false` leaves the window **open** with
    /// [`failed`](Self::failed) set, which is the whole reason this reports at all: the commit
    /// still applied to every live window, so closing would look exactly like success and the
    /// setting would be gone at the next launch with nothing having said so.
    ///
    /// The in-memory half is kept either way — the draft is published to every window — because
    /// the settings *are* now what the user asked for everywhere except on disk. Rolling the
    /// live windows back would be answering a durability failure by undoing a change that
    /// worked.
    ///
    /// **The seed does not advance on a failure, and that is what makes the retry possible.**
    /// `dirty()` is `draft != seed`, and the footer gates Apply on it — so advancing the seed
    /// here would disable the button the moment the failure it reports appeared, leaving the
    /// user looking at "could not be saved" with no way to try again once they had fixed the
    /// disk. It would not even recover by reopening the window: `new` seeds both draft and seed
    /// from the config store, which `write_config` has already merged into, so a fresh Settings
    /// window would come up equally undirty and the setting could never reach disk again this
    /// session. Holding the seed keeps the same diff pending, and re-applying it is harmless —
    /// `merge_onto` writes the identical fields over values that already hold them.
    pub fn apply(&self) -> bool {
        let draft = self.draft.peek().clone();
        let seed = self.seed.peek().clone();
        let landed = write_config(self.config, &[ConfigChan::Settings], {
            let draft = draft.clone();
            move |cfg| draft.merge_onto(&seed, &mut cfg.settings)
        });
        let mut failed = self.failed;
        failed.set((!landed).then(|| {
            "The settings are in use, but could not be saved and will be lost when Strata \
             restarts."
                .to_string()
        }));
        if !landed {
            return false;
        }
        let mut reseed = self.seed;
        reseed.set(draft);
        self.discard();
        true
    }

    /// Why the last Apply didn't stick, for the footer to state.
    pub fn failure(&self) -> Option<String> {
        self.failed.read().clone()
    }

    /// Drop the live preview, reverting every window to the committed theme. Called from the
    /// root's `use_drop`, so it runs however the window goes — Cancel, Esc, the red button,
    /// the owner closing, or a quit.
    fn discard(&self) {
        let mut preview = self.preview;
        preview.set_if_modified(None);
    }
}

/// The Settings window: the canvas's 940×660 frame, with the title bar drawn by
/// [`SettingsChrome`] rather than AppKit (the same transparent-titlebar treatment as the
/// project and launcher windows, so the traffic lights float in our own 50px strip).
pub struct SettingsApp {
    pub app: AppCtx,
}

impl SettingsApp {
    pub fn window(app: AppCtx) -> WindowConfig {
        // Match the theme's window body so a resize doesn't flash the default white. Through
        // `peek_selection`, not the raw settings: this window is opened *while* another one
        // may already be previewing a theme.
        let background = {
            let sel = peek_selection(app.config, app.preview);
            let id = sel.effective(strata_core::theme::os_is_dark());
            window_background(app.themes.get_or_default(&id))
        };
        WindowConfig::new_app(SettingsApp { app })
            .with_title("Settings")
            .with_size(940., 660.)
            // Below this the nav rail crowds the pane and the footer's two buttons collide
            // with the engine table's toolbar.
            .with_min_size(740., 480.)
            .with_background(background)
            // The 50px strip centres macOS's 16px buttons at y = 17; AppKit's default origin
            // is (7, 6), so the inset is the difference (the canvas's x = 16).
            .with_traffic_light_inset(9., 11.)
            .with_window_attributes(move |attrs, _| {
                attrs
                    .with_titlebar_transparent(true)
                    .with_fullsize_content_view(true)
                    .with_title_hidden(true)
            })
    }
}

impl App for SettingsApp {
    fn render(&self) -> impl IntoElement {
        // The same window-root steps every app takes. The shared theme **registry** into
        // context first — the Appearance pane's theme list reads it, and a route component
        // takes no props, so context is the only way in — then this window's theme derived
        // from the shared settings and its own preview.
        let themes = use_provide_context({
            let themes = self.app.themes.clone();
            move || themes
        });
        use_strata_theme(themes.clone(), self.app.config, self.app.preview);
        use_share_config(self.app.config);
        use_provide_context({
            let app = self.app.clone();
            move || app
        });
        // Join the live window registry, so a second ⌘, focuses this window rather than
        // opening another — and keep the registry's Settings pin true for this window's life,
        // handing focus back to its owner on the way out.
        //
        // The same call points the menubar here as a **panel**: none of the File or Window
        // commands has a listener in this window, so every item that would reach it through
        // the keyboard pipeline greys, Settings… included.
        //
        // Greying Settings… does not change what ⌘, does here, whichever way AppKit resolves a
        // disabled item's key equivalent — **unverified, and deliberately not relied on**. If
        // it skips the item the press falls through to this window's consuming listener below;
        // if it claims it, the press stops at the menubar. Both end in "nothing happens", which
        // is the right answer for a window that is already open, so the listener stays as the
        // one that does *not* depend on the question.
        platform::use_register_window(&self.app, || WindowKind::Settings, MenuScope::Panel);
        platform::use_settings_pin(self.app.clone());

        let config = self.app.config;
        let ctx = use_provide_context({
            let preview = self.app.preview;
            move || SettingsCtx::new(config, preview)
        });
        // The search's pointer at one row of one pane (P4-09). Provided **above** the router,
        // because the nav writes it before the page holding the row has mounted — see
        // `components::form::reveal`.
        use_provide_context(Reveal::empty);
        // Every edit to the draft's theme half previews across all windows…
        use_side_effect(move || ctx.sync_preview());
        // …and the preview never outlives this window, whichever way it goes.
        use_drop(move || ctx.discard());

        // Taken in the render scope so the key handler below can close this window from an
        // event handler, where there is no scope left to read it from.
        let platform = use_hook(Platform::get);

        rect()
            .expanded()
            .vertical()
            // The window's ambient text colour, like the launcher's: runs that don't name one
            // inherit it rather than Freya's base-theme default.
            .color(use_roles().get(Role::Text))
            .child(Router::<Route>::new(|| {
                RouterConfig::default().with_initial_path(Route::Theme)
            }))
            // Esc and ⌘Q. Deliberately the LAST child — same-name global listeners fire in
            // document order, so anything a pane later mounts outranks this.
            .child(rect().on_global_key_down(on_commands(config, {
                move |cmd| match cmd {
                    // Esc is Cancel: close without committing. The draft goes with the
                    // window and the preview is dropped by the `use_drop` above.
                    Command::Cancel => {
                        platform.close_current_window();
                        true
                    }
                    Command::Quit => {
                        platform::quit();
                        true
                    }
                    // Already here. Consumed so the press can't fall through to the window
                    // underneath and re-pin this one above itself.
                    Command::OpenSettings => true,
                    _ => false,
                }
            })))
    }
}

// Every category now has its page, so the `panes!` macro that generated a `Pane::not_built`
// placeholder per unbuilt category is gone, and so is that constructor (see `views::pane`). A
// sixth route — Connections (W7) is the candidate — brings its own page rather than inheriting a
// placeholder nobody is using.
