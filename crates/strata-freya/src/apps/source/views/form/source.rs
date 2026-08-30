//! A **source's** form: its name, its address, and one row per key its kind declared.
//!
//! Nothing here is named after a source. The rows come from
//! [`SourceInfo::keys`](strata_engine::SourceInfo) — the declaration the registry handed the
//! draft — and each [`Field`] has one dress, so a kind the engine gains is a kind this form
//! already edits: an `sslmode` picker and a root-certificate path are `PostgreSQL`'s
//! *declarations*, not this module's knowledge.
//!
//! What a value may **be** is never asked here. Per-key validation is what the `Field` implies —
//! a choice is a `Select`, so an illegal word is unreachable; a required box is refused empty —
//! and every other rule belongs to the kind, asked by
//! [`connect`](strata_engine::DataSource::connect), whose refusal lands on the data source's own
//! row. An address is the same bargain one row up: the kind's rule, reached through
//! `sources().check_address` and said in the footer.

use freya::prelude::*;
use strata_engine::{Field, SourceInfo, SourceSetting};
use strata_model::SecretRef;

use crate::apps::source::model::{noun, SecretProbe, SecretRow};
use crate::apps::source::SourceCtx;
use crate::components::form::{
    form_theme, Form, PathField, Row, Section, ValueField, FIELD_HEIGHT,
};
use crate::components::icon::IconName;
use crate::components::metrics::SP_3;
use crate::components::tones::tones;
use crate::components::tool_button::ToolButton;
use crate::components::typography::{Control, MonoValue, Prose};

use super::{qualifier, NARROW_WIDTH};

/// The gap between the sentence under a secret box and the press below it.
const TOOL_GAP: f32 = SP_3;

/// Every row a source has: the handle, then the kind's declared keys in the order it declared
/// them — the address among them — under whatever [`SourceSetting::group`] headings it asked for,
/// and finally the read-only switch, for a kind that says it can be written to.
///
/// A key the draft does not `show` gets no row: a setting another
/// setting's answer has made irrelevant is not a control, which is the call the whole form makes
/// about a source's rows one level up. A heading is emitted when the group **changes**, and the
/// conformance body refuses a declaration that returns to a group it left, so one heading is
/// printed once.
///
/// Each declared row is keyed by its own key, so a change of kind rebuilds every box rather than
/// handing one key's buffer to the row that took its place.
///
/// `ctx` is **handed in and not consumed here**: a helper on a conditional path may call no
/// hooks at all.
pub(super) fn rows(
    form: Form,
    ctx: SourceCtx,
    keys: &'static [SourceSetting],
    registrant: &Option<SourceInfo>,
    scope: &str,
) -> Form {
    let mut form = form.child(NameField { key: DiffKey::None }.key(format!("name·{scope}")));
    let mut heading: Option<&'static str> = None;
    for declared in keys
        .iter()
        .filter(|declared| ctx.draft.read().shows(declared))
    {
        if declared.group.is_some() && declared.group != heading {
            let label = declared.group.unwrap_or_default();
            form = form.child(Section::new(label).key(label));
        }
        heading = declared.group;
        form = form.child(
            KeyField {
                declared: *declared,
                key: DiffKey::None,
            }
            .key(declared.key),
        );
    }
    match registrant.as_ref().is_some_and(|info| info.writable) {
        true => form.child(ReadOnly { key: DiffKey::None }.key(format!("read_only·{scope}"))),
        false => form,
    }
}

/// **NAME** — the handle: what every surface calls this data source, and the catalog its relations
/// are addressed under (`lake.public.orders`).
///
/// One field for both, because they are one thing: a second "display name" beside the identifier
/// is two names for one source and a question about which one a confirm quotes.
///
/// **The user writes it, and nothing derives it.** Two sources may hold identical settings and
/// differ only here — four servers behind one tunnel that differ in credentials and in nothing
/// else — so a name is the one thing a source cannot be given for free. A rename is a
/// store-funnel operation: what this box writes is a draft, and Save is what moves the dependent
/// table refs and this machine's keystore entries with it.
#[derive(PartialEq)]
struct NameField {
    key: DiffKey,
}

impl KeyExt for NameField {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for NameField {
    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }

    fn render(&self) -> impl IntoElement {
        let ctx = use_consume::<SourceCtx>();
        let text = use_state({
            let initial = ctx.draft.peek().name.clone();
            move || initial
        });
        use_side_effect(move || {
            let name = text.read().clone();
            ctx.edit(move |draft| draft.name = name);
        });
        Row::new("NAME")
            .required()
            .hint(
                "What this data source is called, and the catalog its tables are queried by: \
                 'lake' makes a table 'lake.public.orders'",
            )
            .child(ValueField::new(text).width(Size::px(NARROW_WIDTH)))
    }
}

/// One declared key, in the dress its [`Field`] asks for — **the one place that decision is
/// made**, so a shape cannot be routed one way here and another way where the rows are built.
///
/// A dress per `Field` and no fallthrough: a shape the form cannot draw is a `Field` the engine
/// has not declared, which is a compile error rather than a silent blank — or, worse, a secret
/// drawn as a plain box and written into the def.
#[derive(PartialEq)]
struct KeyField {
    declared: SourceSetting,
    key: DiffKey,
}

impl KeyExt for KeyField {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for KeyField {
    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }

    fn render(&self) -> impl IntoElement {
        let ctx = use_consume::<SourceCtx>();
        let declared = self.declared;
        let name = declared.key;
        let value = ctx.draft.read().value(name);

        let row = Row::new(declared.label)
            .maybe(declared.required, Row::required)
            .map(declared.hint, Row::hint);
        match declared.field {
            Field::Secret => SecretField {
                declared,
                key: DiffKey::None,
            }
            .into_element(),
            Field::Choice(options) => {
                let chosen: Vec<Element> = options
                    .iter()
                    .map(|option| {
                        MenuItem::new()
                            .selected(*option == value)
                            .on_press(move |_| {
                                ctx.edit(move |draft| draft.set(name, (*option).to_string()));
                            })
                            .child(MonoValue::new(*option))
                            .into()
                    })
                    .collect();
                row.child(
                    rect()
                        .width(Size::px(NARROW_WIDTH))
                        .height(Size::px(FIELD_HEIGHT))
                        .child(
                            Select::new()
                                .selected_item(MonoValue::new(value.as_str()))
                                .children(chosen),
                        ),
                )
                .into_element()
            }
            Field::Path => row
                .child(
                    PathField::file(value, &[])
                        .dialog_title("Choose a file")
                        .map(declared.placeholder, PathField::placeholder)
                        .on_change(move |path: String| {
                            ctx.edit(move |draft| draft.set(name, path));
                        }),
                )
                .into_element(),
            Field::Flag => row
                .child(
                    Switch::new()
                        .toggled(value.trim() == "true")
                        .on_toggle(move |()| {
                            ctx.edit(move |draft| {
                                let on = draft.value(name).trim() == "true";
                                draft.set(name, (!on).to_string());
                            });
                        }),
                )
                .into_element(),
            Field::Text => row
                .child(TextKey {
                    declared,
                    key: DiffKey::None,
                })
                .into_element(),
        }
    }
}

/// A free-text key's box — its own component so it owns a buffer, which a `match` arm inside
/// another component's render cannot (the hook count has to be fixed per render).
#[derive(PartialEq)]
struct TextKey {
    declared: SourceSetting,
    key: DiffKey,
}

impl KeyExt for TextKey {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for TextKey {
    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }

    fn render(&self) -> impl IntoElement {
        let ctx = use_consume::<SourceCtx>();
        let name = self.declared.key;
        let text = use_state({
            let initial = ctx.draft.peek().value(name);
            move || initial
        });
        use_side_effect(move || {
            let typed = text.read().clone();
            ctx.edit(move |draft| draft.set(name, typed));
        });

        ValueField::new(text)
            .width(Size::fill())
            .map(self.declared.placeholder, ValueField::placeholder)
    }
}

/// A [`Field::Secret`] key — the one control here whose state is about *this machine* rather than
/// the def.
///
/// The settings window's API-key marker is honest because it minted the reference when it stored
/// one; a committed expectation says nothing about the machine reading it, so this row reports
/// the mount probe ([`SecretRow`]) instead. Its one press is local for the same reason: *remove
/// from this machine* leaves the def's expectation standing, so a colleague keeps their own.
///
/// **The tone is the row's own answer, not a second judgement** ([`SecretRow::fault`]): the
/// sentence is painted in the error tone exactly where it is describing something wrong — a def
/// that recorded a secret over a machine that has no entry for it — and in the hint colour
/// wherever it is simply stating what is true here.
///
/// The box's buffer and the window's map are **mirrored rather than shared**, because a
/// `ValueField` binds one `State<String>` while the window holds a value per declared key; both
/// effects are idempotent, so the pair settles rather than ringing. And what the def expects with
/// the box empty is kept apart from what is typed into it, or a stray keystroke would commit an
/// expectation nothing holds.
#[derive(PartialEq)]
struct SecretField {
    declared: SourceSetting,
    key: DiffKey,
}

impl KeyExt for SecretField {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for SecretField {
    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }

    fn render(&self) -> impl IntoElement {
        let form = form_theme();
        let tones = tones();
        let ctx = use_consume::<SourceCtx>();
        let mut revealed = use_state(|| false);
        let name = self.declared.key;
        let noun = noun(&self.declared);

        let stored = ctx
            .secret_values
            .read()
            .get(name)
            .cloned()
            .unwrap_or_default();
        let text = use_state({
            let initial = stored.clone();
            move || initial
        });
        let shared = use_reactive(&stored);
        use_side_effect(move || {
            let mut text = text;
            text.set_if_modified(shared.read().clone());
        });
        use_side_effect(move || {
            let typed = text.read().clone();
            ctx.set_secret(name, typed);
        });

        use_side_effect(move || {
            let typed = ctx
                .secret_values
                .read()
                .get(name)
                .is_some_and(|value| !value.trim().is_empty());
            if typed {
                ctx.keep_secret(name);
            }
            let expects = typed
                || SecretRow::of(
                    ctx.secret_expected.read().contains(name),
                    false,
                    ctx.secret_removed.read().contains(name),
                    ctx.secret_probes
                        .read()
                        .get(name)
                        .unwrap_or(&SecretProbe::Absent),
                )
                .keeps_expectation();
            ctx.edit(move |draft| match expects {
                true => {
                    draft
                        .secrets
                        .entry(name.to_string())
                        .or_insert_with(SecretRef::mint);
                }
                false => {
                    draft.secrets.remove(name);
                }
            });
        });

        let row = SecretRow::of(
            ctx.secret_expected.read().contains(name),
            ctx.secret_values
                .read()
                .get(name)
                .is_some_and(|typed| !typed.trim().is_empty()),
            ctx.secret_removed.read().contains(name),
            ctx.secret_probes
                .read()
                .get(name)
                .unwrap_or(&SecretProbe::Absent),
        );

        Row::new(self.declared.label)
            .maybe(self.declared.required, Row::required)
            .child(
                rect()
                    .width(Size::fill())
                    .horizontal()
                    .cross_align(Alignment::Center)
                    .spacing(SP_3)
                    .content(Content::Flex)
                    .child(
                        ValueField::new(text)
                            .width(Size::flex(1.))
                            .masked(!*revealed.read()),
                    )
                    .child(
                        ToolButton::new(
                            IconName::Eye,
                            match *revealed.read() {
                                true => "Hide the value",
                                false => "Show the value",
                            },
                        )
                        .outlined()
                        .on_press(move |_: Event<PressEventData>| {
                            let shown = *revealed.peek();
                            revealed.set(!shown);
                        }),
                    ),
            )
            .child(qualifier(
                rect()
                    .width(Size::fill())
                    .vertical()
                    .spacing(SP_3)
                    .child(
                        Prose::new(row.note(&noun))
                            .color(match row.fault() {
                                true => tones.error,
                                false => form.hint_color,
                            })
                            .width(Size::fill())
                            .wrap(),
                    )
                    .child(
                        rect()
                            .vertical()
                            .cross_align(Alignment::Start)
                            .spacing(TOOL_GAP)
                            .maybe_child(row.offers_removal().then(|| {
                                Button::new()
                                    .flat()
                                    .on_press(move |_: Event<PressEventData>| {
                                        ctx.forget_secret_here(name);
                                    })
                                    .child(Control::new("Remove from this machine"))
                            })),
                    ),
            ))
    }
}

/// **READ ONLY** — whether statements may change what this data source holds (DB-10). Every other
/// client calls it that, and it is **on** unless someone turns it off.
///
/// That default is the whole safety story: a data source is a read-only view until this says
/// otherwise, so shipping the write path changed nothing about any project already on disk. On
/// the **def** rather than beside it, because a data source is committed and shared and the answer
/// should be the same for a colleague.
///
/// **The editor's own row, not a declared one**, and the split is the point: the sentence beside
/// it is Strata's policy about Strata's gate, so a kind declaring it would be every kind
/// copy-pasting our words — while whether the source can be written to *at all* is the kind's
/// knowledge, and that is [`SourceKind::WRITABLE`](strata_engine::SourceKind::WRITABLE).
///
/// The sentence beside the switch names the two statements turning it off opens rather than
/// saying "writes": a reader has no reason to guess that `UPDATE` and `DELETE` are not among them.
#[derive(PartialEq)]
struct ReadOnly {
    key: DiffKey,
}

impl KeyExt for ReadOnly {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for ReadOnly {
    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }

    fn render(&self) -> impl IntoElement {
        let ctx = use_consume::<SourceCtx>();
        let read_only = ctx.draft.read().read_only;

        Row::new("READ ONLY")
            .hint(
                "Strata never changes what this data source holds. Turn it off to allow INSERT \
                 and CREATE TABLE AS SELECT",
            )
            .child(Switch::new().toggled(read_only).on_toggle(move |()| {
                ctx.edit(|draft| draft.read_only = !draft.read_only);
            }))
    }
}
