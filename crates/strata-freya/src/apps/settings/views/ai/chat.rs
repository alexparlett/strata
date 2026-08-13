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
//! **The model is picked from what the provider serves.** A `Select`, not a box: `genai`
//! prescribes no models, so there is no static list a free-text box would protect the user from
//! and no reason for one, because the provider can be asked. A typed name buys nothing and costs a
//! turn — accepted by every layer we own and refused by the vendor, after the send.
//!
//! It offers `Listings::offer`: what the provider last reported **plus the pick in hand**, because
//! the list endpoint is not the chat endpoint and a strict picker over an empty answer would strand
//! a working setup. The list is the app-global satellite, refreshed in the background when this
//! page opens and usable while stale.
//!
//! The names are **unfiltered**, deliberately: tidying `whisper-1` and `dall-e-3` out with a static
//! list here would be the prescribed-model table this design avoids, and would hide a new chat
//! model on the day it ships. A non-chat pick fails on the first send in the provider's own words.
//!
//! **Effort is the model's own rungs, in the canvas's shell.** The canvas draws a fixed four-way
//! that dims for a non-reasoning model; the interaction is kept and the ladder is not.
//! `efforts(kind, model)` answers per **model**, because `genai` clamps or forwards a rung the
//! model will not take, and a segment naming a rung that was not what got sent is the lie this
//! table exists to prevent.

use freya::prelude::*;
use strata_agent::assistant::{efforts, label};
use strata_core::ai::{Effort, ProviderKind, CHATS_MIN};

use crate::apps::settings::views::ai::probe::{self, FromDraft};
use crate::apps::settings::views::Pane;
use crate::apps::settings::{settings_theme, Anchor, SettingsCtx};
use crate::components::form::{Form, NumberField};
use crate::components::metrics::{SETTINGS_FIELD_WIDTH, SP_3};
use crate::components::segmented_toggle::{SegmentedToggle, ToggleSegment};
use crate::components::typography::{Control, Prose};
use crate::state::{needs_asking, Ask, Probe, Probes};

/// The canvas's control column (`max-width: 420px`), and its label gutter.
const CONTROL_WIDTH: f32 = 420.;

/// A numeric field's width — Settings > System's own, so the two retention caps in the app are
/// set in fields of the same size.

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

        let retention = Anchor::AiChatLimit.row().child(
            NumberField::new(
                ai.max_chats.try_into().unwrap_or(u32::MAX),
                CHATS_MIN as u32,
                u32::MAX,
            )
            .width(Size::px(SETTINGS_FIELD_WIDTH))
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

        let current = ai
            .default_provider
            .filter(|kind| ai.is_enabled(*kind))
            .or_else(|| enabled.first().copied());

        let picked = use_reactive(&current);
        use_side_effect(move || {
            let resolved = *picked.read();
            if resolved.is_some() && ctx.draft.peek().ai.default_provider != resolved {
                ctx.edit(move |settings| settings.ai.default_provider = resolved);
            }
        });

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

        use_side_effect(move || {
            let Some(kind) = *picked.read() else {
                return;
            };
            let _ = ctx.listings.read();
            if needs_asking(ctx.listings, ctx.probes, kind) {
                probe::refresh(ctx, Ask::from_draft(ctx, kind));
            }
        });

        let chosen = ai.default_model.trim().to_string();
        let offered = match current {
            Some(kind) => ctx.listings.read().offer(kind, &chosen),
            None => Vec::new(),
        };

        let model = rect()
            .width(Size::px(CONTROL_WIDTH))
            .spacing(SP_3)
            .child(
                Select::new()
                    .selected_item(Control::new(match chosen.is_empty() {
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

        let rungs: &[Effort] = current.map_or(&[], |kind| efforts(kind, &chosen));

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
        Probe::Failed { why } => format!("{name} could not be reached: {why}"),
        Probe::Verified { .. } => format!("{name} reports no models."),
        Probe::Untested => {
            format!("No models listed for {name}. Test it in AI > Providers to ask again.")
        }
    })
}
