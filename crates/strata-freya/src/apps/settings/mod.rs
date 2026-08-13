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
//! **A pane is a `Form::preferences` of `Row`s** ([`crate::components::form`]), so nothing about
//! the rhythm between rows lives here. [`views::EnginePane`] is the one category to take both of
//! [`views::Pane`]'s opt-outs, being a surface that manages its own height.
//!
//! [`views::McpPane`] sits under an AI heading beside [`views::ProvidersPane`] and
//! [`views::ChatPane`] rather than alone as "Agent access": outbound model credentials and inbound
//! MCP hosting are different capabilities, and a page called "Agent access" beside a Providers page
//! that also serves agents named the wrong axis.

mod model;
mod search;
mod views;

use std::collections::BTreeMap;

use freya::prelude::*;
use freya::router::*;
use freya::winit::platform::macos::WindowAttributesExtMacOS;
use strata_agent::assistant::label;
use strata_core::ai::ProviderKind;
use strata_core::config::{Command, Settings};

use crate::apps::settings::views::{
    ChatPane, DataDisplayPane, EnginePane, KeymapPane, McpPane, PropRows, ProvidersPane,
    SettingsChrome, SystemPane, ThemePane, TypedKeys,
};
use crate::components::form::Reveal;
use crate::keymap::on_commands;
use crate::menu::MenuScope;
use crate::platform::{self, WindowKind};
use crate::state::{
    use_share_config, write_config, write_listings, AppCtx, ConfigChan, ConfigStation,
    ModelListings, ProviderProbes, ThemePreview, ThemeSel,
};
use crate::task::offload;
use crate::theme::{peek_selection, use_roles, use_strata_theme, window_background, Role};

pub use model::{category, Category, NavGroup, CATEGORIES};
pub use search::{search, Anchor, Hit};

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
        /// The **dashed** edge of a slot with nothing in it yet — the Add-shortcut button on an
        /// unbound row. Its own field rather than the table's `border_fill`, because it has to
        /// stand a step out from the grid's own hairlines to read as an invitation at all; a
        /// dashed line pitched for a box outline mostly disappears.
        slot_border_fill: Color,
        /// The AI ▸ Providers row's **mark tile** — the 34px square carrying a provider's brand
        /// logo (`IconName::Provider…`).
        ///
        /// Its own pair rather than the theme card's, though both resolve to a raised box with a
        /// hairline today: a card is a *pressable preview* of a whole theme and this is an
        /// identifying glyph beside a name, and the two have already been pulled apart once
        /// before in this window (`table_head_background` borrowed the results grid's slot and
        /// landed too light). Sharing the slot would mean any future tuning of one silently
        /// retunes the other.
        mark_background: Color,
        /// The mark's glyph while the provider is **off** — the tile is present but inert, so it
        /// sits at the dim end of the text ramp. An enabled row paints the mark in
        /// [`selected_color`](SettingsTheme::selected_color), which is the same accent the picked
        /// theme card's tick uses and means the same thing: this one is in play.
        mark_color: Color,
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
        #[route("/ai/providers", ProvidersPane)]
        Providers,
        #[route("/ai/chat", ChatPane)]
        Chat,
        #[route("/ai/mcp", McpPane)]
        Mcp,
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
    /// **Keys typed into AI ▸ Providers and not yet applied.**
    ///
    /// Deliberately *not* a field of the draft, and that is the whole design: [`Settings`] holds
    /// a [`SecretRef`](strata_core::secret::SecretRef) and no secret, which is a property of the
    /// types rather than a rule to remember — so a pasted key has nowhere in the draft to live.
    /// It sits here for the window's lifetime, goes to the keystore at Apply, and only the
    /// marker merges. Emptying an entry is how "clear this key" is spelled, because
    /// `Secret::new` answers a blank string with `None` and no secret *is* a delete.
    pub ai_keys: State<TypedKeys>,
    /// **Whether an Apply is in flight** — the arm in front of the keystore's blocking half.
    ///
    /// [`apply`](Self::apply) runs `commit` on a worker, so the window stays live while the OS
    /// is being asked (and, on a freshly signed bundle, while it is prompting). Live means
    /// pressable: without this the user could start a second Apply over the same typed keys, and
    /// two concurrent `commit`s would both see no marker and each mint one for the same secret,
    /// stranding whichever lost.
    ///
    /// On the window rather than the footer for [`failed`](Self::failed)'s reason — the footer
    /// reads it, but it is a fact about the window's state, not about the strip that draws it.
    applying: State<bool>,
    /// What AI ▸ Providers has actually asked each provider ([`Probe`]).
    ///
    /// On the window rather than the pane for `engine`'s reason: Providers runs the test and
    /// Chat reports what it said, and a result thrown away by navigating between the two would
    /// leave the model picker unable to say why it has nothing to offer.
    pub probes: ProviderProbes,
    /// **What each provider last reported serving** (AS-06) — the app-global satellite, not
    /// this window's.
    ///
    /// A fetched list outlives the window that fetched it and the run of the app that fetched
    /// it, which is the whole point: a `Select` fed only by a live call is empty at every
    /// launch. So the window holds a *handle*, like `config` and `preview`, and a refresh writes
    /// through [`write_listings`](crate::state::write_listings).
    pub listings: ModelListings,
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

/// Hand-written for `AppCtx`'s reason: `RadioStation` has no `PartialEq`, and the two station
/// handles here are process-wide singletons — two `SettingsCtx` values are always the same
/// config store and the same preview slot, so they contribute nothing to the comparison. What
/// does is the per-window editing state, which is what a component holding one as a prop is
/// actually asking about.
impl PartialEq for SettingsCtx {
    fn eq(&self, other: &Self) -> bool {
        self.draft == other.draft
            && self.seed == other.seed
            && self.engine == other.engine
            && self.ai_keys == other.ai_keys
            && self.applying == other.applying
            && self.probes == other.probes
            && self.failed == other.failed
    }
}

impl SettingsCtx {
    fn new(
        config: ConfigStation,
        preview: ThemePreview,
        listings: ModelListings,
        probes: ProviderProbes,
    ) -> Self {
        let settings = config.peek().settings.clone();
        Self {
            engine: State::create(PropRows::from_map(&settings.engine)),
            ai_keys: State::create(TypedKeys::default()),
            applying: State::create(false),
            probes,
            listings,
            draft: State::create(settings.clone()),
            seed: State::create(settings),
            preview,
            config,
            failed: State::create(None),
        }
    }

    /// **Retract everything this window knows about a provider's endpoint** — its base URL or
    /// its key just moved, so the last answer describes a request nobody would make now.
    ///
    /// One call rather than two, because the two halves of that answer are kept in two places
    /// (the probe here, the names in the satellite) and dropping either alone leaves the other
    /// making the claim: a picker still offering the old endpoint's models, or a row still
    /// saying "12 models" for an address that has never been asked.
    pub fn forget_provider(self, kind: ProviderKind) {
        let mut probes = self.probes;
        probes.write().forget(kind);
        write_listings(self.listings, |listings| listings.forget(kind));
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
        let blocked = match faults {
            0 => None,
            1 => Some("1 engine property is invalid".to_string()),
            n => Some(format!("{n} engine properties are invalid")),
        };
        blocked.or_else(|| {
            let draft = self.draft.read();
            let keys = self.ai_keys.read();
            let on: Vec<ProviderKind> = draft.ai.enabled().collect();
            on.into_iter().find_map(|kind| {
                views::missing(&draft.ai, &keys, kind)
                    .map(|why| format!("{} has {why}", label(kind)))
            })
        })
    }

    /// The base URL configured for `kind`.
    ///
    /// A pair with [`set_base_url`](Self::set_base_url) rather than the panes reaching into
    /// `draft.ai` themselves, so "absent" and "empty" are decided once — see below.
    /// **`peek`, not `read`.** These answer a *guard* — "is what the box holds already what the
    /// draft holds?" — run from a row's `use_side_effect`, and a `read` there subscribes the
    /// effect to the whole draft: every keystroke in any box on any row would re-run the URL and
    /// name effects of every mounted row. The engine grid's `PropRow` peeks in its guard for
    /// exactly this reason.
    ///
    /// **Absent reads as empty**, which is the other half of the guard being right. A built-in
    /// with no entry yet has no base URL *and* a box holding `""`, and those are the same state;
    /// returning `None` made the guard fire on mount and write an entry for every provider the
    /// user had never touched — which dirtied the draft with no edit and persisted seven empty
    /// rows.
    pub fn base_url_of(&self, kind: ProviderKind) -> String {
        self.draft
            .peek()
            .ai
            .setup(kind)
            .map(|setup| setup.base_url.clone())
            .unwrap_or_default()
    }

    /// Write `kind`'s base URL into the draft, creating its entry if this is the first thing
    /// ever set on it.
    ///
    /// Only ever reached with a value that differs from [`base_url_of`](Self::base_url_of), so
    /// the `or_default()` here creates an entry for a provider the user has actually typed into.
    pub fn set_base_url(self, kind: ProviderKind, url: String) {
        let mut draft = self.draft;
        draft.write().ai.providers.entry(kind).or_default().base_url = url;
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
    /// **A typed key counts, though it is not in the draft.** `Settings` holds a `SecretRef` and
    /// no secret, so a pasted key lives beside the draft rather than in it — which meant a window
    /// whose only edit was a credential compared equal to its seed, left Apply disabled, and made
    /// the key unsaveable. It saved at all only when some *other* setting had been changed in the
    /// same sitting, which is a coincidence rather than a design.
    ///
    /// An empty entry counts too: that is a pending *removal*, which is every bit as much an edit
    /// as a pending key.
    pub fn dirty(&self) -> bool {
        *self.draft.read() != *self.seed.peek() || !self.ai_keys.read().is_empty()
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
    /// A **per-field merge** against the seed, not a whole-struct write: this draft is a snapshot
    /// of the settings as they were when the window opened, and another window can commit one of
    /// its own in the meantime, so writing the draft wholesale would carry a stale field back over
    /// the top. On a commit that lands the seed advances, so the diff is measured against the last
    /// commit rather than against mount.
    ///
    /// Returns whether it reached disk. `false` leaves the window **open** with
    /// [`failed`](Self::failed) set: the commit still applied to every live window, so closing
    /// would look exactly like success and the setting would be gone at the next launch. The
    /// in-memory half is kept either way, because rolling live windows back would answer a
    /// durability failure by undoing a change that worked.
    ///
    /// **The seed does not advance on a failure, and that is what makes the retry possible.**
    /// `dirty()` is `draft != seed` and the footer gates Apply on it, so advancing here would
    /// disable the button the moment the failure appeared — and reopening would not recover it
    /// either, since `new` seeds from the config store that `write_config` has already merged into.
    ///
    /// **Async because the keystore blocks**, and not hypothetically: Keychain access is per code
    /// signature, so the first Apply from a newly signed bundle is when macOS raises an
    /// authorisation prompt, which on the render thread appears over a frozen window.
    /// [`applying`](Self::applying) is the arm that keeps the window live meanwhile, and the footer
    /// gates Apply on it — without it a second press runs a concurrent `commit` over the same typed
    /// keys, racing to mint a marker for one secret.
    pub async fn apply(&self) -> bool {
        let mut draft = self.draft.peek().clone();
        let seed = self.seed.peek().clone();

        let mut applying = self.applying;
        applying.set(true);
        let keys = self.ai_keys.peek().clone();
        let ai = std::mem::take(&mut draft.ai);
        let answer = offload(move || {
            let mut ai = ai;
            let outcome = views::commit(&keys, &mut ai);
            (outcome, ai, keys)
        })
        .await;
        applying.set(false);

        let Some((landed_keys, ai, committed)) = answer else {
            let mut failed = self.failed;
            failed.set(Some(
                "The settings could not be saved: a worker did not answer.".into(),
            ));
            return false;
        };
        draft.ai = ai;

        let mut live = self.draft;
        live.set(draft.clone());
        if let Err(e) = landed_keys {
            let mut failed = self.failed;
            failed.set(Some(e.to_string()));
            return false;
        }
        let mut typed = self.ai_keys;
        typed.write().forget_committed(&committed);
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

    /// Whether an Apply is in flight — what the footer disables its button on. See
    /// [`applying`](Self::applying).
    pub fn applying(&self) -> bool {
        *self.applying.read()
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
        let background = {
            let sel = peek_selection(app.config, app.preview);
            let id = sel.effective(strata_core::theme::os_is_dark());
            window_background(app.themes.get_or_default(&id))
        };
        WindowConfig::new_app(SettingsApp { app })
            .with_title("Settings")
            .with_size(940., 660.)
            .with_min_size(740., 480.)
            .with_background(background)
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
        let themes = use_provide_context({
            let themes = self.app.themes.clone();
            move || themes
        });
        use_strata_theme(themes, self.app.config, self.app.preview);
        use_share_config(self.app.config);
        use_provide_context({
            let app = self.app.clone();
            move || app
        });
        platform::use_register_window(&self.app, || WindowKind::Settings, MenuScope::Panel);
        platform::use_settings_pin(self.app.clone());

        let config = self.app.config;
        let ctx = use_provide_context({
            let preview = self.app.preview;
            let listings = self.app.listings;
            let probes = self.app.probes;
            move || SettingsCtx::new(config, preview, listings, probes)
        });
        use_provide_context(Reveal::empty);
        use_side_effect(move || ctx.sync_preview());
        use_drop(move || ctx.discard());

        let platform = use_hook(Platform::get);

        rect()
            .expanded()
            .vertical()
            .color(use_roles().get(Role::Text))
            .child(Router::<Route>::new(|| {
                RouterConfig::default().with_initial_path(Route::Theme)
            }))
            .child(rect().on_global_key_down(on_commands(config, {
                move |cmd| match cmd {
                    Command::Cancel => {
                        platform.close_current_window();
                        true
                    }
                    Command::Quit => {
                        platform::quit();
                        true
                    }
                    Command::OpenSettings => true,
                    _ => false,
                }
            })))
    }
}
