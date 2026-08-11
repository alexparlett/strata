//! **Settings ▸ AI ▸ Chat** — what a new chat starts with: provider · model · effort.
//!
//! The runtime half of the split this workstream is built on. AI ▸ Providers says which brains
//! *exist*; this says which one a new conversation opens on and what it opens asking. Each chat
//! can then change its own model and effort without touching these (AS-04).
//!
//! **Only enabled providers are offered**, so the pane can never name a brain it has no
//! credential for — and when none are enabled it says so and points at the page that fixes it,
//! rather than drawing three dead controls.
//!
//! ## The model is picked from what the provider serves (AS-06)
//!
//! A `Select`, not a box: `genai` prescribes no models — a name is an opaque string that goes
//! into the request payload — so there is no static list a free-text box would be protecting the
//! user from, and no reason for one, because the provider can be asked. A typed name buys
//! nothing and costs a turn: `gpt-5-turbo-imaginry` is accepted by every layer we own and
//! refused by the vendor, after the send, in a transcript.
//!
//! What it offers is `Listings::offer` — what the provider last reported, **plus the pick in
//! hand**, because the list endpoint is not the chat endpoint and a strict picker over an empty
//! answer would strand a setup that works. The list itself is the app-global satellite, so it
//! survives the window and the run of the app; opening this page refreshes it in the background
//! when it is stale, and the stale list stays usable throughout.
//!
//! The names are **unfiltered**, deliberately: OpenAI's list carries `whisper-1` and `dall-e-3`
//! beside the chat models, and tidying that with a static name list here would be the
//! prescribed-model table this design avoids — it would hide a new chat model on the day it
//! ships. A non-chat pick fails on the first send in the provider's own words.
//!
//! ## Effort is the model's own rungs, in the canvas's shell
//!
//! The canvas draws a fixed four-way that dims for a non-reasoning model. The interaction is
//! kept; the ladder is not. `efforts(kind, model)` answers per **model** — three rungs for most,
//! five for the newest Claude family, none at all for a model that does not reason — because
//! `genai` clamps or forwards a rung the model will not take, and a segment naming a rung that
//! was not what got sent is the lie this whole table exists to prevent.

use freya::prelude::*;
use strata_agent::assistant::{efforts, label};
use strata_core::ai::{Effort, ProviderKind, CHATS_MIN};

use crate::apps::settings::views::ai::probe::{self, FromDraft};
use crate::apps::settings::views::Pane;
use crate::apps::settings::{settings_theme, Anchor, SettingsCtx};
use crate::components::form::{Form, NumberField};
use crate::components::segmented_toggle::{SegmentedToggle, ToggleSegment};
use crate::components::typography::{Control, Prose};
use crate::state::{needs_asking, Ask, Probe, Probes};

/// The canvas's control column (`max-width: 420px`), and its label gutter.
const CONTROL_WIDTH: f32 = 420.;

/// A numeric field's width — Settings > System's own, so the two retention caps in the app are
/// set in fields of the same size.
const FIELD_WIDTH: f32 = 130.;

#[derive(PartialEq)]
pub struct ChatPane;

impl Component for ChatPane {
    fn render(&self) -> impl IntoElement {
        let ctx = use_consume::<SettingsCtx>();
        let theme = settings_theme();

        let (ai, probes) = {
            let draft = ctx.draft.read();
            (draft.ai.clone(), ctx.probes.read().clone())
        };

        // **Retention is not a provider question**, so this row is built before the check below
        // and rendered on both sides of it: conversations a project has already stored are still
        // there — and still worth being able to bound — when every provider has been turned off.
        //
        // Saturating, not `as`: a hand-edited config holding more than a u32 should show the
        // biggest number the field can offer rather than wrap round to a small one.
        let retention = Anchor::AiChatLimit.row().child(
            NumberField::new(
                ai.max_chats.try_into().unwrap_or(u32::MAX),
                CHATS_MIN as u32,
                u32::MAX,
            )
            .width(Size::px(FIELD_WIDTH))
            .unit("conversations")
            .on_change(move |chats: u32| ctx.edit(move |s| s.ai.max_chats = chats as usize)),
        );

        let enabled: Vec<ProviderKind> = ai.enabled().collect();
        if enabled.is_empty() {
            return Pane::new(
                Form::new()
                    .preferences()
                    .child(
                        Prose::new(
                            "No providers are enabled. Turn one on in AI > Providers to set what \
                             a new chat starts with.",
                        )
                        .width(Size::fill())
                        .wrap()
                        .color(theme.hint_color),
                    )
                    .child(retention),
            );
        }

        // The default may name a provider that was just disabled on the other page, or none at
        // all — `enabled()` is the authority, and the picker shows the head of it rather than a
        // stale name.
        let current = ai
            .default_provider
            .filter(|kind| ai.is_enabled(*kind))
            .or_else(|| enabled.first().copied());

        // **What the pane resolved is what the draft says**, on `repoint`'s terms: in the draft,
        // on the page the user is looking at, before Apply.
        //
        // Displaying a fallback without committing it is a pane that shows "Anthropic" and
        // commits `None` — and AS-04, reading the committed value, would then report that nothing
        // is configured. That state is reachable rather than theoretical: this branch renamed the
        // persisted field, so a config written by an earlier build loads with providers enabled
        // and no default at all.
        //
        // It is also what lets everything below trust `default_provider`, which is what the
        // effort check reads.
        let picked = use_reactive(&current);
        use_side_effect(move || {
            let resolved = *picked.read();
            if resolved.is_some() && ctx.draft.peek().ai.default_provider != resolved {
                ctx.edit(move |settings| settings.ai.default_provider = resolved);
            }
        });

        // Each item carries the `ProviderKind` it selects rather than being matched back by its
        // label, so the picker cannot set one provider while displaying another.
        let provider = Select::new()
            .selected_item(Control::new(
                current
                    .map(|kind| label(kind).to_string())
                    .unwrap_or_default(),
            ))
            .children(
                enabled
                    .iter()
                    .map(|kind| {
                        let kind = *kind;
                        MenuItem::new()
                            .selected(current == Some(kind))
                            .on_press(move |_| {
                                ctx.edit(move |settings| settings.ai.default_provider = Some(kind));
                            })
                            .child(Control::new(label(kind)))
                            .into()
                    })
                    .collect::<Vec<Element>>(),
            );

        // **Opening this page refreshes a stale list, in the background.**
        //
        // Not at launch: dialling every configured provider on every start spends a round trip
        // and puts a key on the wire for a session that mostly never opens a model picker, and
        // a read the user waits for has to be an *arm* rather than a freeze — which at startup
        // has no surface to be an arm on. Here it is neither: the cached list renders
        // immediately and is usable throughout, and the answer replaces it when it lands.
        //
        // **Guarded on the probe, which is what makes it one attempt.** A refresh that failed
        // leaves the listing absent, so the staleness question alone would ask again on every
        // repaint; `Probe::Untested` is true exactly once per provider per window, and a
        // deliberate Test is the way to ask again. `refresh` holds the in-flight guard itself,
        // so a fetch already running for this kind is left alone.
        use_side_effect(move || {
            let Some(kind) = *picked.read() else {
                return;
            };
            // Subscribed on purpose: the settled write lands here, and `needs_asking` then stops
            // it going round again.
            let _ = ctx.listings.read();
            if needs_asking(ctx.listings, ctx.probes, kind) {
                probe::refresh(ctx, Ask::from_draft(ctx, kind));
            }
        });

        // **The offer is what the provider reported plus the pick in hand** — one rule, in
        // `Listings::offer`, because the composer footer (AS-04) picks from the same list.
        //
        // **Trimmed once, here, so every reader agrees about the name.** `offer` inserts the
        // pick trimmed (an all-whitespace one is "nothing chosen", not a blank row), so a
        // padded value would match no offered row: the dropdown would open with nothing ticked
        // and `efforts` would be asked about a name no rule matches, quietly dropping a
        // reasoning model's ladder. That value is reachable rather than theoretical — the
        // free-text box this `Select` replaces wrote its raw contents, so an existing config
        // can hold one. Picking anything rewrites it clean.
        let chosen = ai.default_model.trim().to_string();
        let offered = match current {
            Some(kind) => ctx.listings.read().offer(kind, &chosen),
            None => Vec::new(),
        };

        let model = rect()
            .width(Size::px(CONTROL_WIDTH))
            .spacing(6.)
            .child(
                Select::new()
                    .selected_item(Control::new(match chosen.is_empty() {
                        // An invitation, never an example name: a model shown in the closed
                        // control reads as the value, and this one would be a model nobody
                        // picked. (The table's `model_example` went with the free-text box it
                        // was the placeholder for.)
                        true => "Choose a model".to_string(),
                        false => chosen.clone(),
                    }))
                    .children(
                        offered
                            .iter()
                            .map(|name| {
                                let name = name.clone();
                                MenuItem::new()
                                    .selected(name == chosen)
                                    .on_press({
                                        let name = name.clone();
                                        move |_| {
                                            let name = name.clone();
                                            ctx.edit(move |settings| {
                                                settings.ai.default_model = name;
                                            });
                                        }
                                    })
                                    .child(Control::new(name))
                                    .into()
                            })
                            .collect::<Vec<Element>>(),
                    ),
            )
            .maybe_child(unlisted(current, &offered, &probes).map(|said| {
                Prose::new(said)
                    .width(Size::fill())
                    .wrap()
                    .color(theme.hint_color)
            }));

        // The rungs this model actually offers — empty is a real answer, and the control says
        // which model has no reasoning setting rather than dimming with no explanation.
        let rungs: &[Effort] = current.map_or(&[], |kind| efforts(kind, &chosen));

        // **A rung the model no longer offers is dropped, not kept out of sight.**
        //
        // Reasoning is a *model* capability, so changing the model changes the ladder — and a
        // rung left behind is not merely stale: `Brain::resolve` refuses a `Selection` carrying
        // one (`NoSuchEffort`) before a socket opens, so every new chat seeded from these
        // defaults would fail its first send. Worse, the control that set it is gone by then, so
        // Settings offers no way to clear it.
        //
        // In the draft, on the pane the user is looking at, for `repoint`'s reason: a default
        // repaired silently at read time is a setting that changed with nobody told.
        //
        // **Both axes, and the resolved provider.** A ladder is `(provider, model)`, so this has
        // to re-run when either moves: reading the draft with `peek` and subscribing to the model
        // alone left a rung behind when the *provider* changed (Anthropic `claude-opus-5` at
        // XHigh, switched to OpenAI, whose ladder for that name is empty — control gone, value
        // kept, every new chat refused). And it reads `picked` rather than `default_provider`
        // because the controls above resolve through it: validating against a value the user
        // cannot see is how a press on a live segment gets reverted the instant it lands.
        //
        // The model half is a `use_reactive` of the draft's own value now that the picker writes
        // it directly: reading the draft inside would subscribe this effect to every field of it.
        let named = use_reactive(&chosen);
        use_side_effect(move || {
            let model = named.read().clone();
            let Some(kind) = *picked.read() else {
                return;
            };
            let Some(effort) = ctx.draft.peek().ai.default_effort else {
                return;
            };
            if !efforts(kind, &model).contains(&effort) {
                ctx.edit(|settings| settings.ai.default_effort = None);
            }
        });

        let effort: Element = match rungs.is_empty() {
            true => rect()
                .child(
                    Prose::new(match chosen.is_empty() {
                        true => "Choose a model to see its reasoning settings.".to_string(),
                        false => format!("'{chosen}' has no reasoning effort setting."),
                    })
                    .width(Size::fill())
                    .wrap()
                    .color(theme.hint_color),
                )
                .into(),
            false => rungs
                .iter()
                .fold(SegmentedToggle::new().form(), |toggle, rung| {
                    let rung = *rung;
                    toggle.child(
                        ToggleSegment::text(rung.label())
                            .selected(ai.default_effort == Some(rung))
                            .on_press(move |_: Event<PressEventData>| {
                                ctx.edit(move |settings| {
                                    // Pressing the rung already set clears it — "no preference"
                                    // is a real value (the model's own default) and otherwise
                                    // there would be no way back to it.
                                    settings.ai.default_effort = match settings.ai.default_effort {
                                        Some(current) if current == rung => None,
                                        _ => Some(rung),
                                    };
                                });
                            }),
                    )
                })
                .into(),
        };

        Pane::new(
            Form::new()
                .preferences()
                .child(Anchor::AiProvider.row().child(provider))
                .child(Anchor::AiModel.row().child(model))
                .child(Anchor::AiEffort.row().child(effort))
                .child(retention),
        )
    }
}

/// **What to say under a picker with nothing in it — and nothing when it has something.**
///
/// A dropdown that opens empty explains itself or it is a dead control, and the four reasons it
/// can be empty are genuinely different: nobody has asked yet, somebody is asking now, the
/// provider was asked and would not answer, or it answered and serves nothing. The third is the
/// one that matters most and it names the provider, because "could not be reached" under a
/// picker that offers no other clue is a sentence about nothing.
///
/// A *populated* picker says nothing at all. Listing the offered names beneath a control that
/// offers them was the free-text box's caption and has no job now.
fn unlisted(current: Option<ProviderKind>, offered: &[String], probes: &Probes) -> Option<String> {
    let kind = current?;
    if !offered.is_empty() {
        return None;
    }
    let name = label(kind);
    Some(match probes.get(kind) {
        Probe::Testing => format!("Asking {name} which models it serves."),
        // The provider's own words, already bounded — the same sentence the Providers page's
        // Test shows, because it is the same request.
        Probe::Failed { why } => format!("{name} could not be reached: {why}"),
        // Reached, and serving nothing. A real answer, and a different one from a failure.
        Probe::Verified { .. } => format!("{name} reports no models."),
        Probe::Untested => {
            format!("No models listed for {name}. Test it in AI > Providers to ask again.")
        }
    })
}
