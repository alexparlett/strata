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
//! ## The model list is what a Test actually returned
//!
//! The dropdown offers the models the provider reported, which is the same call the Providers
//! page's Test makes (`probe`) — one request serving both. It **also takes a typed name**,
//! because a list can 401, a gateway can serve no `/models`, and a private deployment can carry
//! a name no list mentions; a picker that could not be typed into would make those unreachable.
//!
//! ## Effort is the model's own rungs, in the canvas's shell
//!
//! The canvas draws a fixed four-way that dims for a non-reasoning model. The interaction is
//! kept; the ladder is not. `efforts(kind, model)` answers per **model** — three rungs for most,
//! five for the newest Claude family, none at all for a model that does not reason — because
//! `genai` clamps or forwards a rung the model will not take, and a segment naming a rung that
//! was not what got sent is the lie this whole table exists to prevent.

use freya::prelude::*;
use strata_agent::assistant::{efforts, info, label};
use strata_core::ai::{Effort, ProviderKind};

use crate::apps::settings::views::Pane;
use crate::apps::settings::{settings_theme, Anchor, SettingsCtx};
use crate::components::form::{Form, ValueField};
use crate::components::segmented_toggle::{SegmentedToggle, ToggleSegment};
use crate::components::typography::{Control, Prose};

/// The canvas's control column (`max-width: 420px`), and its label gutter.
const CONTROL_WIDTH: f32 = 420.;

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

        let offered: Vec<ProviderKind> = ai.enabled().collect();
        if offered.is_empty() {
            return Pane::new(
                rect().width(Size::fill()).child(
                    Prose::new(
                        "No providers are enabled. Turn one on in AI > Providers to set what a \
                         new chat starts with.",
                    )
                    .width(Size::fill())
                    .wrap()
                    .color(theme.hint_color),
                ),
            );
        }

        // The default may name a provider that was just disabled on the other page, or none at
        // all — `enabled()` is the authority, and the picker shows the head of it rather than a
        // stale name.
        let current = ai
            .default_provider
            .filter(|kind| ai.is_enabled(*kind))
            .or_else(|| offered.first().copied());

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
                offered
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

        // The model box: a typed name, with what the provider reported offered beneath it.
        let model_buf = use_state({
            let seed = ai.default_model.clone();
            move || seed
        });
        use_side_effect(move || {
            let typed = model_buf.read().clone();
            if ctx.draft.peek().ai.default_model != typed {
                ctx.edit(move |settings| settings.ai.default_model = typed);
            }
        });

        // The kind's current-model hint, from the table — a placeholder, never a default: an
        // empty box means "no model chosen", which is a state the send refuses by name.
        let example = current.map_or("", |kind| info(kind).model_example);

        let listed: Vec<String> = current
            .map(|kind| probes.get(kind).models().to_vec())
            .unwrap_or_default();

        let model = rect()
            .width(Size::px(CONTROL_WIDTH))
            .spacing(6.)
            .child(
                ValueField::new(model_buf)
                    .width(Size::fill())
                    .placeholder(example),
            )
            .child(match listed.is_empty() {
                // Said out loud: an empty list is not the same as a provider with no models,
                // and the way to fill it is a Test on the other page.
                true => Prose::new("Test the provider in AI > Providers to list its models.")
                    .width(Size::fill())
                    .wrap()
                    .color(theme.hint_color),
                false => Prose::new(format!("Offered: {}", listed.join(", ")))
                    .width(Size::fill())
                    .wrap()
                    .color(theme.hint_color),
            });

        // The rungs this model actually offers — empty is a real answer, and the control says
        // which model has no reasoning setting rather than dimming with no explanation.
        let typed_model = model_buf.read().clone();
        let rungs: &[Effort] = current.map_or(&[], |kind| efforts(kind, &typed_model));

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
        use_side_effect(move || {
            let model = model_buf.read().clone();
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
                    Prose::new(match typed_model.trim().is_empty() {
                        true => "Choose a model to see its reasoning settings.".to_string(),
                        false => format!("'{typed_model}' has no reasoning effort setting."),
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
                .child(Anchor::AiEffort.row().child(effort)),
        )
    }
}
