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
//! entry points. P4-04 added the first pane ([`views::ThemePane`], the theme picker); the
//! remaining categories belong to P4-05…P4-08 and render a placeholder until those land.

mod model;
mod views;

use freya::prelude::*;
use freya::router::*;
use freya::winit::platform::macos::WindowAttributesExtMacOS;
use strata_core::config::{Command, Settings};

use crate::apps::settings::views::{Pane, SettingsChrome, ThemePane};
use crate::keymap::on_commands;
use crate::menu::use_file_menu;
use crate::platform::{self, WindowKind};
use crate::state::{
    use_share_config, write_config, AppCtx, ConfigChan, ConfigStation, ThemePreview, ThemeSel,
};
use crate::theme::{peek_selection, use_strata_theme, window_background};

pub use model::{category, Category, NavGroup, CATEGORIES};

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
        /// Explanatory subtext in a pane — every setting's one-line description, and the
        /// breadcrumb's leading group.
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
    }
);

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
    /// The live theme preview the draft's theme half is mirrored into.
    preview: ThemePreview,
    /// The app-global config: Apply's target.
    config: ConfigStation,
}

impl SettingsCtx {
    fn new(config: ConfigStation, preview: ThemePreview) -> Self {
        let settings = config.peek().settings.clone();
        Self {
            draft: State::create(settings.clone()),
            seed: State::create(settings),
            preview,
            config,
        }
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
    /// The seed advances to what was just committed, so the diff is always measured against
    /// the last commit rather than against mount. Today the footer closes the window straight
    /// after, but an Apply that *didn't* close would otherwise re-commit this same diff on the
    /// next press — over whatever another window had written in between.
    pub fn apply(&self) {
        let draft = self.draft.peek().clone();
        let seed = self.seed.peek().clone();
        write_config(self.config, &[ConfigChan::Settings], {
            let draft = draft.clone();
            move |cfg| draft.merge_onto(&seed, &mut cfg.settings)
        });
        let mut reseed = self.seed;
        reseed.set(draft);
        self.discard();
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
        platform::use_register_window(self.app.windows, || WindowKind::Settings);
        platform::use_settings_pin(self.app.clone());
        // While this window is focused the File menu is *its* File menu: the recents, and no
        // Close Project — there is no project here to close, exactly as on the launcher.
        use_file_menu(&self.app, None);

        let config = self.app.config;
        let ctx = use_provide_context({
            let preview = self.app.preview;
            move || SettingsCtx::new(config, preview)
        });
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
            .color(use_theme().read().colors().text_primary)
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

/// The category pane's content — what [`SettingsChrome`] wraps in the scroll frame and the
/// breadcrumb. One per [`Route`]; each is a placeholder until its own task lands. `ThemePane`
/// has landed (P4-04) and lives in [`views`], so it isn't in this list.
macro_rules! panes {
    ($( $Comp:ident => $owner:literal, $what:literal ),* $(,)?) => {
        $(
            #[derive(PartialEq)]
            pub struct $Comp;

            impl Component for $Comp {
                fn render(&self) -> impl IntoElement {
                    Pane::not_built($what, $owner)
                }
            }
        )*
    };
}

panes! {
    SystemPane => "P4-06", "System preferences",
    DataDisplayPane => "P4-05", "Data-display preferences",
    KeymapPane => "P4-08", "Keyboard shortcuts",
    EnginePane => "P4-07", "Engine properties",
}
