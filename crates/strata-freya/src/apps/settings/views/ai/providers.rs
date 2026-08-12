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
use strata_core::models::Listings;

use crate::apps::settings::views::ai::configure::{ConfigureDialog, Configuring};
use crate::apps::settings::views::ai::keys::TypedKeys;
use crate::apps::settings::views::ai::probe::{self, FromDraft};
use crate::apps::settings::views::ai::row::{mark, ProviderRow};
use crate::apps::settings::views::Pane;
use crate::apps::settings::{settings_theme, SettingsCtx, SettingsTheme};
use crate::components::icon::{Icon, IconName};
use crate::components::metrics::{R_2, SP_4, SP_6};
use crate::components::typography::Prose;
use crate::state::Ask;

/// The gap between the explainer and the list.
const SECTION_GAP: f32 = SP_6;

/// The placeholder the OpenAI-compatible URL box carries. A shape rather than a host: any real
/// example would read as a default nobody set.
const CUSTOM_URL: &str = "https://host/v1/";

#[derive(PartialEq)]
pub struct ProvidersPane;

impl Component for ProvidersPane {
    fn render(&self) -> impl IntoElement {
        let ctx = use_consume::<SettingsCtx>();
        let theme = settings_theme();
        // Which provider's dialog is open, if any. Pane-local: a dialog is a question being
        // asked right now, not a setting.
        let mut configuring = use_state(Configuring::default);

        // **Read guards, not clones** — held across the `map` below and dropped before anything
        // takes a `write`. Cloning instead duplicated the roster and every model name a Test had
        // returned (an OpenAI list alone is ~80 strings) on every render. Subscribing is the
        // point: the pane *should* rebuild when either changes.
        let draft = ctx.draft.read();
        let ai = &draft.ai;
        let keys = ctx.ai_keys.read();
        let listings = ctx.listings.read();

        let rows = all()
            .enumerate()
            .map(|(index, kind)| {
                let provider = info(kind);
                let setup = ai.setup(kind);
                let enabled = setup.is_some_and(|s| s.enabled);
                let stored = will_have_key(ai, &keys, kind);
                Element::from(ProviderRow {
                    mark: mark(kind),
                    name: provider.label.to_string(),
                    badge: badge(provider.key),
                    subline: subline(kind, setup, stored, &listings),
                    enabled,
                    first: index == 0,
                    on_toggle: EventHandler::new(move |()| toggle(ctx, kind, configuring)),
                    on_configure: EventHandler::new(move |()| configuring.set(Some(kind))),
                })
            })
            .collect::<Vec<Element>>();

        let open = *configuring.read();
        Pane::new(
            rect()
                .width(Size::fill())
                .spacing(SECTION_GAP)
                .child(note(&theme))
                .child(list_box(&theme, rows))
                .maybe_child(open.map(|kind| ConfigureDialog {
                    kind,
                    slot: configuring,
                    ctx,
                })),
        )
    }
}

/// The bordered list every provider row sits in — one box, hairlines between rows.
fn list_box(theme: &SettingsTheme, rows: Vec<Element>) -> Element {
    rect()
        .width(Size::fill())
        .corner_radius(R_2)
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
        .spacing(SP_4)
        .padding(SP_4)
        .corner_radius(R_2)
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

/// **Will this provider have a key once Apply lands?**
///
/// The marker in the draft is not the answer on its own: it only moves when Apply reaches the
/// keystore, so a pending edit is invisible to it. Reading it alone is what made Remove look
/// dead — the deletion was recorded, and every surface kept saying "A key is stored" because
/// none of them were looking at the thing that had changed.
///
/// A pending edit wins, and an empty one is a pending *removal*.
pub fn will_have_key(ai: &Ai, keys: &TypedKeys, kind: ProviderKind) -> bool {
    match keys.touched(kind) {
        true => !keys.get(kind).trim().is_empty(),
        false => ai.setup(kind).is_some_and(|setup| setup.key.is_some()),
    }
}

/// **What stops this provider working, if anything** — the reason Apply refuses while it is on.
///
/// A provider that is enabled and cannot answer is the one state this pane can reach that is
/// simply broken: the chat pane will offer it, and every send will fail. `Brain::resolve` refuses
/// exactly these before a socket opens, so this is the same judgement made early enough to act
/// on.
///
/// **Only where a credential is genuinely needed.** Ollama sends no key and a compatible endpoint
/// may legitimately send an empty bearer, so neither is ever short of one — and a keyed provider
/// with its environment variable set is not either, which is the fallback AS-02 built and this
/// must not contradict.
pub fn missing(ai: &Ai, keys: &TypedKeys, kind: ProviderKind) -> Option<&'static str> {
    if matches!(info(kind).base_url, BaseUrl::Required)
        && ai
            .setup(kind)
            .map(|s| s.base_url.trim())
            .unwrap_or_default()
            .is_empty()
    {
        return Some("no base URL");
    }
    match info(kind).key {
        KeyUse::Env(var) => {
            let from_env = std::env::var(var).is_ok_and(|value| !value.trim().is_empty());
            (!will_have_key(ai, keys, kind) && !from_env).then_some("no API key")
        }
        KeyUse::Unused | KeyUse::Anonymous => None,
    }
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

/// **What a row knows without having asked anything.**
///
/// The canvas puts "N models · M reasoning" here, which is knowledge a request produces, so
/// before anything has been asked the row states a fact it actually has and the count replaces
/// it once an answer comes back. Only real facts.
///
/// It is the row's whole second line now that the credential lives in a dialog, so it says the
/// most useful true thing rather than the one the boxes did not cover.
///
/// **The count comes from the satellite, not from this window's probe** (AS-06). What the
/// provider last reported survives a quit, so a row can say "12 models" at the next launch
/// without a request having been made in this one — which is true, and is the same list the
/// model picker is offering two pages over. Reading the probe instead would blank the count on
/// every restart and make the two pages disagree about the same provider.
fn subline(
    kind: ProviderKind,
    setup: Option<&ProviderSetup>,
    stored: bool,
    listings: &Listings,
) -> Option<String> {
    if let Some(listing) = listings.get(kind) {
        return Some(models_line(listing.models.len()));
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
        return Some("No address set".to_string());
    }
    if stored {
        return Some("A key is stored".to_string());
    }
    match info(kind).key {
        // Nothing is stored, so what it *will* use is the whole answer.
        KeyUse::Env(var) => Some(format!("Falls back to {var}")),
        KeyUse::Unused => Some(format!("Runs locally at {}", default_url(kind))),
        // The compatible kind has no default address, so the one it was given is the only thing
        // worth saying about it — and the empty case was answered above, since it is what stops
        // the row working at all.
        KeyUse::Anonymous => setup.map(|s| s.base_url.trim().to_string()),
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
fn toggle(ctx: SettingsCtx, kind: ProviderKind, mut configuring: State<Configuring>) {
    // One edit, not two: the flip and the re-point are adjacent writes to the same `settings.ai`,
    // and two would wake the pane twice — which is two full rebuilds of the row list per press.
    ctx.edit(|settings| {
        let setup = settings.ai.providers.entry(kind).or_default();
        setup.enabled = !setup.enabled;
        repoint(&mut settings.ai);
    });

    let mut keys = ctx.ai_keys;

    // **Switching a provider off takes its key with it — at Apply, not here.**
    //
    // Enabled and configured are one state: a provider you have turned off is one you are not
    // using, and leaving its credential in the OS keystore keeps a secret alive for something
    // nothing will ask again. What happens now is only that the intent is *recorded* — Apply is
    // still the one commit point, and Cancel still discards it with the rest of the window's
    // editing state.
    //
    // And the models it reported go with the key, for the same reason they go when a URL
    // changes: the answer described a request made with a credential that is on its way out.
    if !ctx.draft.peek().ai.is_enabled(kind) {
        keys.write().set(kind, String::new());
        ctx.forget_provider(kind);
        return;
    }

    // **And switching back on takes it back.** The removal is pending, not done, so changing your
    // mind before Apply has to be able to change it back — otherwise a stray toggle silently
    // queues the deletion of a key that is still perfectly good, leaves the provider enabled and
    // credential-less, and blocks Apply on a state the user never asked for.
    //
    // Only an *empty* pending entry is dropped: one carrying a key is a paste, and a paste
    // survives being toggled around.
    if keys.peek().touched(kind) && keys.peek().get(kind).trim().is_empty() {
        keys.write().forget(kind);
    }

    // **Switching on something that cannot answer asks for what it needs.** A provider that is
    // enabled and useless is otherwise announced only by a subline the user has no reason to read,
    // so the question comes to them rather than waiting to be found.
    //
    // `missing` is the same judgement the footer blocks Apply on, asked one gesture earlier —
    // which is what stops the dialog appearing for a provider Apply is perfectly happy with, or
    // failing to appear for one it is not.
    let draft = ctx.draft.peek();
    if missing(&draft.ai, &keys.peek(), kind).is_some() {
        drop(draft);
        configuring.set(Some(kind));
        return;
    }
    drop(draft);

    // **Switching on something that *can* answer asks it what it serves.** Enabling a provider
    // is the moment a person is setting it up and expecting it to reach out, so it is the right
    // point of use for the fetch — the alternative is a picker that is empty until they happen
    // to open it. `needs_refresh` is the same staleness question the pickers ask, so a provider
    // toggled off and on again with a fresh listing costs no round trip.
    if ctx.listings.peek().needs_refresh(kind) {
        probe::refresh(ctx, Ask::from_draft(ctx, kind));
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
