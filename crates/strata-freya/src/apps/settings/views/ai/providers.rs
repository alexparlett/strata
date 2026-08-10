//! **Settings ▸ AI ▸ Providers** — which brains exist, and what addresses each one.
//!
//! Two lists, because they are two different things (`strata_core::ai`'s own split): the
//! built-in kinds, whose identity *is* their kind and which are therefore always all present as
//! rows whether or not the user has touched them, and the **custom endpoints**, which the user
//! adds and names and which are identified by a minted id.
//!
//! Nothing here names a model. A row carries what *addresses* a provider — on, endpoint, key —
//! and what a provider is *asked* is a conversation's, seeded from AI ▸ Chat. That is the
//! def/runtime split applied to the assistant, and it is why this pane needs no model field even
//! though `Selection` has one.
//!
//! **The row list is drawn from `PROVIDERS`**, in the table's own order — never from a list of
//! kinds spelled out here, which would silently omit a provider the day one is added.

use freya::prelude::*;
use strata_agent::assistant::{all, info, BaseUrl, KeyUse};
use strata_core::ai::{Ai, BrainRef, CustomEndpoint, ProviderKind};
use strata_core::secret::Secret;
use uuid::Uuid;

use crate::apps::settings::views::ai::probe::{self, Ask, Probe};
use crate::apps::settings::views::ai::row::{mark, Boxes, ProviderRow};
use crate::apps::settings::views::Pane;
use crate::apps::settings::{settings_theme, SettingsCtx, SettingsTheme};
use crate::components::form::FIELD_HEIGHT;
use crate::components::icon::{Icon, IconName};
use crate::components::typography::{Control, Prose, Strong};

/// The gap between the two sections, and between a section's heading and its box.
const SECTION_GAP: f32 = 24.;
const HEADING_GAP: f32 = 8.;

/// The placeholder a custom endpoint's URL box carries. A shape rather than a host: any real
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
        let mut revealed = use_state(Vec::<BrainRef>::new);

        // **Read guards, not clones.** These are held across the two `map` closures below and
        // dropped before anything takes a `write` — deep-copying instead meant every keystroke
        // (each of which writes the draft or `ai_keys`, waking this pane) duplicated both roster
        // maps, every typed key, and every model name a Test had returned. An OpenAI list alone
        // is ~80 strings.
        //
        // Subscribing is the point: the pane *should* rebuild when any of the three changes.
        let draft = ctx.draft.read();
        let ai = &draft.ai;
        let keys = ctx.ai_keys.read();
        let probes = ctx.probes.read();

        // -- The built-ins, in the table's order. `all()` reads `PROVIDERS`, so a kind added
        //    there appears here with no edit to this pane; the compatible kind is skipped
        //    because it is what every *custom endpoint* is, not a row of its own.
        let builtin = all()
            .filter(|kind| *kind != ProviderKind::OpenAiCompatible)
            .enumerate()
            .map(|(index, kind)| {
                let brain = BrainRef::Builtin(kind);
                let provider = info(kind);
                let setup = ai.providers.get(&kind);
                let enabled = setup.is_some_and(|s| s.enabled);
                let stored = setup.is_some_and(|s| s.key.is_some());
                Element::from(ProviderRow {
                    mark: mark(kind),
                    name: provider.label.to_string(),
                    renameable: false,
                    badge: Some(badge(provider.key)),
                    subline: subline(kind, stored, probes.get(&brain), enabled),
                    enabled,
                    first: index == 0,
                    boxes: Boxes::of(provider.key, provider.base_url),
                    brain,
                    ctx,
                    key_text: keys.get(&brain).to_string(),
                    key_use: provider.key,
                    key_shown: revealed.read().contains(&brain),
                    url_text: setup.map(|s| s.base_url.clone()).unwrap_or_default(),
                    url_placeholder: default_url(kind),
                    probe: probes.get(&brain).clone(),
                    on_toggle: EventHandler::new(move |()| toggle(ctx, brain, revealed)),
                    on_reveal: EventHandler::new(move |()| {
                        let mut shown = revealed.write();
                        match shown.iter().position(|b| *b == brain) {
                            Some(at) => drop(shown.remove(at)),
                            None => shown.push(brain),
                        }
                    }),
                    on_test: EventHandler::new(move |()| test(ctx, brain)),
                    on_remove: None,
                })
            })
            .collect::<Vec<Element>>();

        // -- The custom endpoints. Ordered by their map key, which is stable and meaningless —
        //    the list is short and the name is what the eye uses.
        // Owned before the closures, because each row keeps handlers that outlive this render.
        let endpoints: Vec<(Uuid, CustomEndpoint)> = ai
            .endpoints
            .iter()
            .map(|(id, endpoint)| (*id, endpoint.clone()))
            .collect();
        let custom = endpoints
            .iter()
            .enumerate()
            .map(|(index, (id, endpoint))| {
                let brain = BrainRef::Custom(*id);
                let provider = info(ProviderKind::OpenAiCompatible);
                Element::from(ProviderRow {
                    mark: mark(ProviderKind::OpenAiCompatible),
                    name: endpoint.name.clone(),
                    renameable: true,
                    badge: None,
                    subline: custom_subline(endpoint, probes.get(&brain), endpoint.enabled),
                    enabled: endpoint.enabled,
                    first: index == 0,
                    boxes: Boxes::of(provider.key, provider.base_url),
                    brain,
                    ctx,
                    key_text: keys.get(&brain).to_string(),
                    key_use: provider.key,
                    key_shown: revealed.read().contains(&brain),
                    url_text: endpoint.base_url.clone(),
                    url_placeholder: CUSTOM_URL,
                    probe: probes.get(&brain).clone(),
                    on_toggle: EventHandler::new(move |()| toggle(ctx, brain, revealed)),
                    on_reveal: EventHandler::new(move |()| {
                        let mut shown = revealed.write();
                        match shown.iter().position(|b| *b == brain) {
                            Some(at) => drop(shown.remove(at)),
                            None => shown.push(brain),
                        }
                    }),
                    on_test: EventHandler::new(move |()| test(ctx, brain)),
                    on_remove: Some(EventHandler::new({
                        let id = *id;
                        move |()| remove(ctx, id)
                    })),
                })
            })
            .collect::<Vec<Element>>();

        let body = rect()
            .width(Size::fill())
            .spacing(SECTION_GAP)
            .child(note(&theme))
            .child(list_box(&theme, builtin))
            .child(
                rect()
                    .width(Size::fill())
                    .spacing(HEADING_GAP)
                    .child(
                        rect()
                            .width(Size::fill())
                            .horizontal()
                            .cross_align(Alignment::Center)
                            .content(Content::Flex)
                            .child(
                                rect()
                                    .width(Size::flex(1.))
                                    .spacing(2.)
                                    .child(Strong::new("Custom endpoints"))
                                    .child(
                                        Prose::new(
                                            "Any host that speaks OpenAI's chat-completions API \
                                             — llama.cpp, vLLM, LM Studio, a gateway.",
                                        )
                                        .width(Size::fill())
                                        .wrap()
                                        .color(theme.hint_color),
                                    ),
                            )
                            .child(
                                Button::new()
                                    .outline()
                                    .height(Size::px(FIELD_HEIGHT))
                                    .on_press(move |_: Event<PressEventData>| add(ctx))
                                    .child(
                                        rect()
                                            .horizontal()
                                            .cross_align(Alignment::Center)
                                            .spacing(6.)
                                            .child(Icon::new(IconName::Plus).size(13.))
                                            .child(Control::new("Add endpoint")),
                                    ),
                            ),
                    )
                    // An empty list says so rather than drawing an empty box: there is nothing
                    // to look at, and a bordered nothing reads as a load that failed.
                    .child(match ai.endpoints.is_empty() {
                        true => empty(&theme),
                        false => list_box(&theme, custom),
                    }),
            );

        Pane::new(body)
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
/// the same sentence seven times.
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

fn empty(theme: &SettingsTheme) -> Element {
    rect()
        .width(Size::fill())
        .padding(14.)
        .corner_radius(8.)
        .border(
            Border::new()
                .width(1.)
                .fill(theme.slot_border_fill)
                .dashed(4., 4.),
        )
        .child(Prose::new("No custom endpoints.").color(theme.hint_color))
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
/// `URL http://localhost:11434/`. The subline summarises a **collapsed** row — where no box is
/// drawn and this is the only place the address can appear — so when the row is open it says
/// only what the boxes cannot: the variable an empty key box falls back to, or what a probe came
/// back with.
fn subline(kind: ProviderKind, stored: bool, probe: &Probe, open: bool) -> Option<String> {
    if let Probe::Verified { models } = probe {
        return Some(models_line(models.len()));
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
        KeyUse::Anonymous => (!open).then(|| "No key required".to_string()),
    }
}

/// One place the count is worded, so the two lists cannot pluralize it differently.
fn models_line(count: usize) -> String {
    match count {
        1 => "1 model".to_string(),
        n => format!("{n} models"),
    }
}

fn custom_subline(endpoint: &CustomEndpoint, probe: &Probe, open: bool) -> Option<String> {
    if let Probe::Verified { models } = probe {
        return Some(models_line(models.len()));
    }
    // Open, the URL box carries the address and its own placeholder invites one — repeating
    // either here is the same sentence twice.
    if open {
        return None;
    }
    Some(match endpoint.base_url.trim().is_empty() {
        true => "No address set".to_string(),
        false => endpoint.base_url.clone(),
    })
}

/// Turn a brain on or off. Creating the built-in's entry on first touch is what makes "absent
/// from the map" mean "never enabled" rather than a state the pane has to pre-seed.
///
/// **A revealed key re-masks on the way out**, which is the rule the canvas already states for
/// the MCP token. Reveal is a glance at a value you are checking *now*, and disabling a row takes
/// the box off screen — so a `revealed` set that survived would bring the key back in plaintext
/// the next time the row opened, long after the glance it was granted for. Masking is the resting
/// state, and anything that closes the box returns to it.
fn toggle(ctx: SettingsCtx, brain: BrainRef, mut revealed: State<Vec<BrainRef>>) {
    // One edit, not two: the flip and the re-point are adjacent writes to the same `settings.ai`,
    // and two would wake the pane twice — which is two full rebuilds of both row lists per press.
    ctx.edit(|settings| {
        match brain {
            BrainRef::Builtin(kind) => {
                let setup = settings.ai.providers.entry(kind).or_default();
                setup.enabled = !setup.enabled;
            }
            BrainRef::Custom(id) => {
                if let Some(endpoint) = settings.ai.endpoints.get_mut(&id) {
                    endpoint.enabled = !endpoint.enabled;
                }
            }
        }
        repoint(&mut settings.ai);
    });
    if !ctx.draft.peek().ai.is_enabled(&brain) {
        revealed.write().retain(|shown| *shown != brain);
    }
}

fn add(ctx: SettingsCtx) {
    let id = Uuid::new_v4();
    ctx.edit(move |settings| {
        settings.ai.endpoints.insert(
            id,
            CustomEndpoint {
                name: String::new(),
                base_url: String::new(),
                enabled: false,
                key: None,
            },
        );
    });
}

/// Remove an endpoint, and everything keyed to it. The typed key and the probe go too: both
/// describe a thing that no longer exists, and a later Apply must not try to write a key to it.
fn remove(ctx: SettingsCtx, id: Uuid) {
    let brain = BrainRef::Custom(id);
    ctx.edit(move |settings| {
        settings.ai.endpoints.remove(&id);
        repoint(&mut settings.ai);
    });
    let mut keys = ctx.ai_keys;
    keys.write().forget(&brain);
    let mut probes = ctx.probes;
    probes.write().forget(&brain);
}

/// **Keep the chat default pointing at something enabled.**
///
/// The canvas re-points rather than dangling, and this follows it — but visibly: it happens in
/// the draft, on the pane the user is looking at, before Apply. A default silently repaired at
/// read time would be a setting that changed with nobody told.
fn repoint(ai: &mut Ai) {
    let still_good = ai.default_brain.is_some_and(|brain| ai.is_enabled(&brain));
    if !still_good {
        let next = ai.enabled().next();
        ai.default_brain = next;
    }
}

/// Ask the provider what it serves. The keystore read and the request both happen on the probe's
/// own thread — this only marks the row busy and stores whatever comes back.
fn test(ctx: SettingsCtx, brain: BrainRef) {
    let Some(ask) = build_ask(ctx, brain) else {
        return;
    };
    let mut probes = ctx.probes;
    probes.write().set(brain, Probe::Testing);
    spawn(async move {
        let settled = probe::run(ask).await;
        let mut probes = ctx.probes;
        probes.write().set(brain, settled);
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
fn build_ask(ctx: SettingsCtx, brain: BrainRef) -> Option<Ask> {
    let draft = ctx.draft.peek();
    let setup = draft.ai.setup(&brain)?;
    let keys = ctx.ai_keys.peek();
    let touched = keys.touched(&brain);
    Some(Ask {
        kind: setup.kind,
        base_url: setup.base_url.to_string(),
        typed: touched.then(|| Secret::new(keys.get(&brain))).flatten(),
        stored: (!touched).then(|| setup.key.cloned()).flatten(),
    })
}
