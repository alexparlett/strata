//! **Settings ▸ AI ▸ Providers** — which brains exist, and what addresses each one.
//!
//! One row per kind, drawn from `PROVIDERS` in the table's own order — never from a list of
//! kinds spelled out here, which would silently omit a provider the day one is added. A kind
//! absent from the draft has simply never been enabled, which is what its own toggle says.
//!
//! Nothing here names a model. A row carries what *addresses* a provider — on, endpoint, key —
//! and what a provider is *asked* is a conversation's, seeded from AI ▸ Chat. That is the
//! def/runtime split applied to the assistant, and it is why this pane needs no model field even
//! though `Selection` has one.
//!
//! **The OpenAI-compatible kind is a row like the others.** It was briefly a second, user-managed
//! list of named endpoints so several could exist at once; that is withdrawn. Gateways exist to
//! multiplex — `LiteLLM` and its kind put many backends behind one OpenAI-compatible address — so
//! a list here would be a second multiplexer in front of a solved problem, and it cost a
//! sum-typed identity that the composer's picker, a chat's selection and the transcript would all
//! have had to carry. One row, addressed by its base URL, and the model list the gateway reports
//! is what distinguishes what is behind it.

use freya::prelude::*;
use strata_agent::assistant::{all, info, BaseUrl, KeyUse};
use strata_core::ai::{Ai, ProviderKind, ProviderSetup};
use strata_core::secret::Secret;

use crate::apps::settings::views::ai::probe::{self, Ask, Probe};
use crate::apps::settings::views::ai::row::{mark, Boxes, ProviderRow};
use crate::apps::settings::views::Pane;
use crate::apps::settings::{settings_theme, SettingsCtx, SettingsTheme};
use crate::components::icon::{Icon, IconName};
use crate::components::typography::Prose;

/// The gap between the explainer and the list.
const SECTION_GAP: f32 = 24.;

/// The placeholder the OpenAI-compatible URL box carries. A shape rather than a host: any real
/// example would read as a default nobody set.
const CUSTOM_URL: &str = "https://host/v1/";

#[derive(PartialEq)]
pub struct ProvidersPane;

impl Component for ProvidersPane {
    fn render(&self) -> impl IntoElement {
        let ctx = use_consume::<SettingsCtx>();
        let theme = settings_theme();
        // Which key boxes are unmasked. Pane-local: a reveal is a glance, not a setting — the
        // same call `McpPane` makes about its token.
        let mut revealed = use_state(Vec::<ProviderKind>::new);

        // **Read guards, not clones.** These are held across the `map` below and dropped before
        // anything takes a `write` — deep-copying instead meant every keystroke (each of which
        // writes the draft or `ai_keys`, waking this pane) duplicated the roster, every typed
        // key, and every model name a Test had returned. An OpenAI list alone is ~80 strings.
        //
        // Subscribing is the point: the pane *should* rebuild when any of the three changes.
        let draft = ctx.draft.read();
        let ai = &draft.ai;
        let keys = ctx.ai_keys.read();
        let probes = ctx.probes.read();

        let rows = all()
            .enumerate()
            .map(|(index, kind)| {
                let provider = info(kind);
                let setup = ai.setup(kind);
                let enabled = setup.is_some_and(|s| s.enabled);
                let stored = setup.is_some_and(|s| s.key.is_some());
                Element::from(ProviderRow {
                    mark: mark(kind),
                    name: provider.label.to_string(),
                    badge: badge(provider.key),
                    subline: subline(kind, setup, stored, probes.get(kind), enabled),
                    enabled,
                    first: index == 0,
                    boxes: Boxes::of(provider.key, provider.base_url),
                    kind,
                    ctx,
                    key_text: keys.get(kind).to_string(),
                    key_use: provider.key,
                    key_shown: revealed.read().contains(&kind),
                    url_text: setup.map(|s| s.base_url.clone()).unwrap_or_default(),
                    url_placeholder: default_url(kind),
                    probe: probes.get(kind).clone(),
                    on_toggle: EventHandler::new(move |()| toggle(ctx, kind, revealed)),
                    on_reveal: EventHandler::new(move |()| {
                        let mut shown = revealed.write();
                        match shown.iter().position(|k| *k == kind) {
                            Some(at) => drop(shown.remove(at)),
                            None => shown.push(kind),
                        }
                    }),
                    on_test: EventHandler::new(move |()| test(ctx, kind)),
                })
            })
            .collect::<Vec<Element>>();

        Pane::new(
            rect()
                .width(Size::fill())
                .spacing(SECTION_GAP)
                .child(note(&theme))
                .child(list_box(&theme, rows)),
        )
    }
}

/// The bordered list every provider row sits in — one box, hairlines between rows.
fn list_box(theme: &SettingsTheme, rows: Vec<Element>) -> Element {
    rect()
        .width(Size::fill())
        .corner_radius(8.)
        .background(theme.card_background)
        .border(Border::new().width(1.).fill(theme.card_border_fill))
        .children(rows)
        .into()
}

/// The pane's one standing statement: what a provider is for, and where the keys go. The
/// canvas's info box, and the only place the trust story is told — a per-row repeat would be
/// the same sentence eight times.
fn note(theme: &SettingsTheme) -> Element {
    rect()
        .width(Size::fill())
        .horizontal()
        .spacing(10.)
        .padding(12.)
        .corner_radius(8.)
        .background(theme.item_active_background)
        .border(Border::new().width(1.).fill(theme.card_border_fill))
        // **Both halves are needed for the sentence to wrap**, and each fails silently on its
        // own: `Size::flex` is only divided by a parent whose `content` is `Flex` (AGENTS.md §3),
        // so without this the run keeps hugging its text and rides off the edge — and the
        // typography roles cap at one line by default, so even a correctly-sized box would
        // truncate rather than wrap without `.wrap()` below.
        .content(Content::Flex)
        .child(
            Icon::new(IconName::Info)
                .size(14.)
                .color(theme.selected_color),
        )
        .child(
            Prose::new(
                "Providers power the chat pane. Enable one and give it a credential. Keys are \
                 kept in the OS keychain, never in the project or the config file.",
            )
            .width(Size::flex(1.))
            .wrap()
            .color(theme.hint_color),
        )
        .into()
}

/// The uppercase badge beside a name — read off the kind's key policy, so it cannot disagree
/// with the boxes the row then draws.
fn badge(key: KeyUse) -> &'static str {
    match key {
        KeyUse::Env(_) => "API KEY",
        KeyUse::Unused => "LOCAL",
        KeyUse::Anonymous => "CUSTOM",
    }
}

/// The kind's default endpoint, for the URL box's placeholder — the table's own value, so a box
/// left blank and the request that follows agree about where it went.
fn default_url(kind: ProviderKind) -> &'static str {
    match info(kind).base_url {
        BaseUrl::Editable(default) => default,
        _ => CUSTOM_URL,
    }
}

/// **What a row knows without having asked anything — and never what it is already showing.**
///
/// The canvas puts "N models · M reasoning" here, which is knowledge a request produces, so
/// before a Test the row states a fact it actually has and the count replaces it once a probe
/// comes back. Only real facts.
///
/// The second half of the rule arrived from the screen: an **open** row draws its address in a
/// box directly underneath, so a subline naming the address too printed
/// `Runs locally at http://localhost:11434/` immediately above a box reading
/// `URL http://localhost:11434/`. The subline summarises a **closed** row — where no box is
/// drawn and this is the only place the address can appear — so when the row is open it says
/// only what the boxes cannot: the variable an empty key box falls back to, or what a probe came
/// back with.
fn subline(
    kind: ProviderKind,
    setup: Option<&ProviderSetup>,
    stored: bool,
    probe: &Probe,
    open: bool,
) -> Option<String> {
    if let Probe::Verified { models } = probe {
        return Some(models_line(models.len()));
    }
    // **What is missing outranks what is present.** A kind whose address has no default cannot
    // work without one, so a closed row says that before it says anything about a key it does
    // have — reporting "A key is stored" while `blocker` refuses Apply over the empty URL names
    // the one fact that is fine and hides the one that is not.
    let unaddressed = matches!(info(kind).base_url, BaseUrl::Required)
        && setup
            .map(|s| s.base_url.trim())
            .unwrap_or_default()
            .is_empty();
    if unaddressed {
        return (!open).then(|| "No address set".to_string());
    }
    if stored {
        return Some("A key is stored".to_string());
    }
    match info(kind).key {
        // The box below is empty and stays empty — the fallback is the whole answer to "so what
        // will it use?", open or not.
        KeyUse::Env(var) => Some(format!("Falls back to {var}")),
        // Open, the URL box says where it runs and the `LOCAL` badge says what it is; there is
        // nothing left for this line to add.
        KeyUse::Unused => (!open).then(|| format!("Runs locally at {}", default_url(kind))),
        // The compatible kind has no default address, so a closed row is the only place the one
        // it was given can be read — and its absence is the thing that stops it working.
        KeyUse::Anonymous => {
            (!open).then(
                || match setup.map(|s| s.base_url.trim()).unwrap_or_default() {
                    "" => "No address set".to_string(),
                    url => url.to_string(),
                },
            )
        }
    }
}

/// One place the count is worded, so nothing pluralizes it differently.
fn models_line(count: usize) -> String {
    match count {
        1 => "1 model".to_string(),
        n => format!("{n} models"),
    }
}

/// Turn a provider on or off. Creating the entry on first touch is what makes "absent from the
/// map" mean "never enabled" rather than a state the pane has to pre-seed.
///
/// **A revealed key re-masks on the way out**, which is the rule the canvas already states for
/// the MCP token. Reveal is a glance at a value you are checking *now*, and disabling a row takes
/// the box off screen — so a `revealed` set that survived would bring the key back in plaintext
/// the next time the row opened, long after the glance it was granted for. Masking is the resting
/// state, and anything that closes the box returns to it.
fn toggle(ctx: SettingsCtx, kind: ProviderKind, mut revealed: State<Vec<ProviderKind>>) {
    // One edit, not two: the flip and the re-point are adjacent writes to the same `settings.ai`,
    // and two would wake the pane twice — which is two full rebuilds of the row list per press.
    ctx.edit(|settings| {
        let setup = settings.ai.providers.entry(kind).or_default();
        setup.enabled = !setup.enabled;
        repoint(&mut settings.ai);
    });
    if !ctx.draft.peek().ai.is_enabled(kind) {
        revealed.write().retain(|shown| *shown != kind);
    }
}

/// **Keep the chat default pointing at something enabled.**
///
/// The canvas re-points rather than dangling, and this follows it — but visibly: it happens in
/// the draft, on the pane the user is looking at, before Apply. A default silently repaired at
/// read time would be a setting that changed with nobody told.
fn repoint(ai: &mut Ai) {
    let still_good = ai.default_provider.is_some_and(|kind| ai.is_enabled(kind));
    if !still_good {
        let next = ai.enabled().next();
        ai.default_provider = next;
    }
}

/// Ask the provider what it serves. The keystore read and the request both happen on the probe's
/// own thread — this only marks the row busy and stores whatever comes back.
fn test(ctx: SettingsCtx, kind: ProviderKind) {
    let ask = build_ask(ctx, kind);
    let mut probes = ctx.probes;
    probes.write().set(kind, Probe::Testing);
    spawn(async move {
        let settled = probe::run(ask).await;
        let mut probes = ctx.probes;
        probes.write().set(kind, settled);
    });
}

/// Everything the probe needs, copied out of the draft before the thread starts.
///
/// **A box the user has touched is the whole answer, including when they emptied it.** The typed
/// value wins because it is what Apply would write — and that has to hold for a *cleared* box
/// too, or clearing a key and pressing Test reports "verified" using the stored credential Apply
/// is about to delete. So `stored` is offered only when the box was never touched; once it has
/// been, an empty box means "no key", and the kind's own environment fallback (or an empty
/// bearer) is what answers, which `list_models` decides rather than this.
fn build_ask(ctx: SettingsCtx, kind: ProviderKind) -> Ask {
    let draft = ctx.draft.peek();
    let setup = draft.ai.setup(kind);
    let keys = ctx.ai_keys.peek();
    let touched = keys.touched(kind);
    Ask {
        kind,
        base_url: setup.map(|s| s.base_url.clone()).unwrap_or_default(),
        typed: touched.then(|| Secret::new(keys.get(kind))).flatten(),
        stored: (!touched)
            .then(|| setup.and_then(|s| s.key.clone()))
            .flatten(),
    }
}
