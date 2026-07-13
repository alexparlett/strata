//! Settings ▸ Engine page — the curated DataFusion options, on the strata-forms layer.
//!
//! [`EngineForm`] is the **UI-only** form (one field per option, keyed by its config id),
//! deliberately distinct from the persisted `Settings.engine`; [`engine_form_from`] /
//! [`engine_form_to`] seed from / map back to that map. The page reads the shared
//! `FormState` from context and renders grouped rows.

use std::collections::BTreeMap;

use dioxus::prelude::*;
use strata_forms::{validators, Form, FormState};

use crate::ui::components::{
    Button, ButtonVariant, Caption, Eyebrow, FieldKind, FormField, Icon, Micro, SelectOption,
    Strong,
};
use crate::ui::icons::{IconName, IconSize};

#[component]
pub(super) fn Engine() -> Element {
    let engine = use_context::<super::SettingsCtx>().engine;
    rsx! {
        div { class: "engine-note",
            span { class: "engine-note-ic", Icon { name: IconName::Info, size: IconSize::Sm } }
            Caption { "A curated subset of DataFusion's ConfigOptions, applied to every query. Execution, parser, optimizer & result-format changes take effect on the open window; memory & spill limits apply after reopening it." }
        }
        if !engine_form_to(&engine.data()).is_empty() {
            div { class: "row", style: "justify-content:flex-end;margin-bottom:var(--sp-4);",
                Button {
                    variant: ButtonVariant::Ghost,
                    onclick: move |_| engine.reset(engine_form_from(&BTreeMap::new())),
                    "Reset all ({engine_form_to(&engine.data()).len()})"
                }
            }
        }
        for (group, opts) in crate::engine_config::groups() {
            Eyebrow { class: "settings-sublabel engine-group", "{group}" }
            for opt in opts {
                {engine_row(opt, engine)}
            }
        }
    }
}

/// One row — label + description + key on the left, the [`FormField`] (bound to the form
/// by the option's key) on the right.
fn engine_row(
    opt: &'static crate::engine_config::EngineOption,
    form: FormState<EngineForm>,
) -> Element {
    rsx! {
        div { class: "engine-row",
            div { class: "engine-row-main",
                Strong { style: "display:block;", "{opt.label}" }
                Caption { style: "display:block;color:var(--dim2);margin-top:var(--sp-1);max-width:420px;", "{opt.desc}" }
                Micro { class: "engine-key", "{opt.key}" }
            }
            div { class: "engine-row-ctl",
                FormField { field: form.field(opt.key), kind: engine_field_kind(opt) }
            }
        }
    }
}

/// Map an engine option's [`crate::engine_config::EngineKind`] to the styled
/// [`FieldKind`] the [`FormField`] renders (toggle / dropdown / stepper / text).
fn engine_field_kind(opt: &'static crate::engine_config::EngineOption) -> FieldKind {
    use crate::engine_config::EngineKind;
    match opt.kind {
        EngineKind::Bool => FieldKind::Bool,
        EngineKind::Enum(choices) => {
            FieldKind::Select(choices.iter().map(|c| SelectOption::new(*c, *c)).collect())
        }
        EngineKind::Int { min, max, step } => FieldKind::Int { min, max, step },
        EngineKind::Text { placeholder, .. } => FieldKind::Text {
            placeholder: placeholder.to_string(),
            mono: true,
        },
    }
}

/// The **UI-only** form for the engine settings — a well-defined set of inputs, one field
/// per DataFusion option, distinct from the persisted `Settings.engine`. `#[derive(Form)]`
/// generates the validation impl from the fields; each field's id is its config key (via
/// `#[field(id = ..)]`), which is how the UI binds rows and how [`engine_form_to`] writes
/// overrides back. Counts are `usize`, the toggle a `bool`; string-valued options stay
/// `String` (the `FormValue` boundary converts to the string-y controls).
#[derive(Clone, PartialEq, Default, Form)]
pub(super) struct EngineForm {
    // 0 = one partition per core, so any whole number is valid (usize enforces ≥ 0).
    #[field(id = "datafusion.execution.target_partitions")]
    target_partitions: usize,
    #[field(id = "datafusion.execution.batch_size", validate = validators::at_least_one)]
    batch_size: usize,
    #[field(id = "datafusion.execution.time_zone", validate = validators::non_empty)]
    time_zone: String,
    // Blank = unlimited, so `size` allows empty; a non-empty value must be a byte size.
    #[field(id = "datafusion.runtime.memory_limit", validate = validators::size)]
    memory_limit: String,
    #[field(id = "datafusion.runtime.max_temp_directory_size", validate = validators::size)]
    max_temp_directory_size: String,
    #[field(id = "datafusion.sql_parser.dialect", validate = validators::non_empty)]
    sql_dialect: String,
    #[field(id = "datafusion.sql_parser.default_null_ordering", validate = validators::non_empty)]
    default_null_ordering: String,
    // The NULL-display text may be blank (renders nulls as empty), so no validator.
    #[field(id = "datafusion.format.null")]
    format_null: String,
    #[field(id = "datafusion.format.date_format", validate = validators::strftime)]
    date_format: String,
    #[field(id = "datafusion.format.timestamp_format", validate = validators::strftime)]
    timestamp_format: String,
    #[field(id = "datafusion.optimizer.prefer_hash_join")]
    prefer_hash_join: bool,
}

/// Seed the form from the persisted overrides: each field starts at its *effective* value
/// (override or catalog default), keyed by config id.
pub(super) fn engine_form_from(overrides: &BTreeMap<String, String>) -> EngineForm {
    let mut form = EngineForm::default();
    for opt in crate::engine_config::OPTIONS {
        if let Some(value) = crate::engine_config::effective(overrides, opt.key) {
            form.set_field(opt.key, &value);
        }
    }
    form
}

/// Map the form back to the persisted overrides map — `set_override` drops any value
/// equal to its default, so the map holds only real overrides.
pub(super) fn engine_form_to(form: &EngineForm) -> BTreeMap<String, String> {
    let mut overrides = BTreeMap::new();
    for opt in crate::engine_config::OPTIONS {
        if let Some(value) = form.get_field(opt.key) {
            crate::engine_config::set_override(&mut overrides, opt.key, value);
        }
    }
    overrides
}
