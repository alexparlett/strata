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
use strata_core::ai::{BrainRef, Effort};

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

        let offered: Vec<BrainRef> = ai.enabled().collect();
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

        // The default may name a brain that was just disabled on the other page; `enabled()` is
        // the authority and the picker shows the head of it rather than a stale name.
        let current = ai
            .default_brain
            .filter(|brain| ai.is_enabled(brain))
            .or_else(|| offered.first().copied());

        // A built-in is named by its table row; a custom endpoint by its user — and a *new* one
        // has no name yet, since Add leaves the box empty for the placeholder to invite. It
        // still has to be pickable, so it says so rather than appearing as a blank line.
        let name_of = |brain: &BrainRef| match ai.setup(brain) {
            Some(setup) => match setup.name.map(str::trim) {
                None => label(setup.kind).to_string(),
                Some("") => "Unnamed endpoint".to_string(),
                Some(name) => name.to_string(),
            },
            None => String::new(),
        };

        // Each item carries the `BrainRef` it selects, so nothing is matched back by name — two
        // custom endpoints may share one, and a picker that resolved by label would then set the
        // wrong one silently.
        let provider = Select::new()
            .selected_item(Control::new(
                current.map(|brain| name_of(&brain)).unwrap_or_default(),
            ))
            .children(
                offered
                    .iter()
                    .map(|brain| {
                        let brain = *brain;
                        MenuItem::new()
                            .selected(current == Some(brain))
                            .on_press(move |_| {
                                ctx.edit(move |settings| settings.ai.default_brain = Some(brain));
                            })
                            .child(Control::new(name_of(&brain)))
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
        let example = current
            .and_then(|brain| ai.setup(&brain))
            .map_or("", |setup| info(setup.kind).model_example);

        let listed: Vec<String> = current
            .map(|brain| probes.get(&brain).models().to_vec())
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
        let rungs: &[Effort] = current
            .and_then(|brain| ai.setup(&brain))
            .map(|setup| efforts(setup.kind, &typed_model))
            .unwrap_or(&[]);

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
        use_side_effect(move || {
            let model = model_buf.read().clone();
            let draft = ctx.draft.peek();
            let Some(effort) = draft.ai.default_effort else {
                return;
            };
            let offered = draft
                .ai
                .default_brain
                .and_then(|brain| draft.ai.setup(&brain))
                .is_some_and(|setup| efforts(setup.kind, &model).contains(&effort));
            drop(draft);
            if !offered {
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
