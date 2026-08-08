//! The editor's fields, in the canvas's order: **PROVIDER**, the authority box,
//! **AUTHENTICATION** and whatever that mode refers to, then S3's **REGION** and **ENDPOINT**,
//! then the standing note about where credentials actually come from.
//!
//! Built from `components::form` — a [`Form`] of [`Row`]s (AGENTS.md §3), so the label register,
//! the `REQUIRED` markers and the rhythm between rows are the app's rather than this window's.
//!
//! **Which rows exist depends on the provider, and only on the provider.** HTTP is anonymous by
//! construction and has no region, so it shows the authority box and nothing else; GCS has no
//! region or endpoint. Rows are not shipped disabled: a control that cannot mean anything for the
//! chosen provider is not a control (the same call the Configure window makes about its LOCATION
//! toggle).
//!
//! **A field's error is not painted on the field.** The canvas reddens the region box and writes
//! a line under it; here the one thing that says why Save is off is the footer, and it is the
//! same value that disables the button ([`super::footer`]) — which is what stops a form from
//! having two accounts of its own validity that can disagree. The label still carries `REQUIRED`,
//! because that is a fact about the field rather than a verdict on what is in it.

use freya::prelude::*;
use strata_core::engine::store::CLIENT_KEYS;
use strata_model::ProviderId;

use crate::apps::connection::model::{GcsAuthId, S3AuthId};
use crate::apps::connection::ConnectionCtx;
use crate::components::form::{Form, Note, PathField, Row, ValueField, FIELD_HEIGHT, LABEL_GAP};
use crate::components::icon::{Icon, IconName};
use crate::components::segmented_toggle::{SegmentedToggle, ToggleSegment};
use crate::components::typography::{MonoValue, Prose};

/// The gap between a control and the thing that qualifies it — an auth pill and the reference it
/// turned on, an endpoint box and its Allow-HTTP switch (canvas `var(--sp-4)`). Inside the row,
/// because a qualifier is what its control's answer *means* rather than a second question.
const QUALIFIER_GAP: f32 = 12.;
/// The region box, which holds `ap-southeast-1` and never anything longer.
const REGION_WIDTH: f32 = 180.;
/// The profile picker (canvas `width: 220px`).
const PROFILE_WIDTH: f32 = 220.;
/// A client-option row: tall enough for a field, with the ⨯ square at its end, and the gap
/// between the table and the button that adds to it.
const OPTION_ROW: f32 = 38.;
const OPTION_REMOVE: f32 = 24.;
const OPTION_GAP: f32 = 8.;

/// Every row this provider has, in canvas order.
#[derive(PartialEq)]
pub struct Fields;

impl Component for Fields {
    fn render(&self) -> impl IntoElement {
        let ctx = use_consume::<ConnectionCtx>();
        let (provider, note) = {
            let draft = ctx.draft.read();
            (draft.provider, draft.note())
        };

        // **Every row is keyed by the provider**, which makes switching one a clean
        // remove-and-add of the whole section — `OptionList::new`'s rule, for the crash it
        // documents. Keying only the rows that come and go is not enough: the ones that *stay*
        // (the client options, the note) sit at a different index under each provider, and
        // Freya's differ records a matched pair at a new index as **moved**, then unwraps a
        // `scope_id` the move left behind.
        //
        // Each key is scoped by the row as well, exactly as an option row's is scoped by its
        // format: two siblings keyed `"S3"` are a duplicate sibling key, which the differ panics
        // on rather than guessing at.
        let scope = provider.label();
        let mut form = Form::new()
            .child(ProviderPicker { key: DiffKey::None }.key(format!("provider·{scope}")))
            .child(Authority { key: DiffKey::None }.key(format!("authority·{scope}")));
        if provider != ProviderId::Http {
            form = form.child(Auth { key: DiffKey::None }.key(format!("auth·{scope}")));
        }
        if provider == ProviderId::S3 {
            form = form
                .child(RegionField { key: DiffKey::None }.key(format!("region·{scope}")))
                .child(Endpoint { key: DiffKey::None }.key(format!("endpoint·{scope}")));
        }
        // Every provider's store is built on one HTTP client, so this section is offered whatever
        // the picker says.
        form = form.child(ClientOptions { key: DiffKey::None }.key(format!("client·{scope}")));
        // Unlabelled, unlike every row above it: the note is a standing statement about the
        // whole form rather than an answer to a question, and a label over it would imply there
        // is something here to set.
        form.child(
            NoteRow {
                note,
                key: DiffKey::None,
            }
            .key(format!("note·{scope}")),
        )
    }
}

/// **PROVIDER** — explicit, never inferred from a typed URL scheme (spec §1). The one control
/// that decides which of the rows below exist.
#[derive(PartialEq)]
struct ProviderPicker {
    key: DiffKey,
}

impl KeyExt for ProviderPicker {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for ProviderPicker {
    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }

    fn render(&self) -> impl IntoElement {
        let ctx = use_consume::<ConnectionCtx>();
        let current = ctx.draft.read().provider;

        let mut pill = SegmentedToggle::new().form();
        for id in ProviderId::ALL {
            pill = pill.child(
                // `ProviderId::label`, not a word typed here: the pane's row badge names
                // providers from the same table, and a name written twice is a name that can
                // disagree.
                ToggleSegment::text(id.label())
                    .selected(id == current)
                    .on_press(move |_| ctx.edit(move |draft| draft.provider = id)),
            );
        }
        Row::new("PROVIDER").child(pill)
    }
}

/// The **bucket** (S3 / GCS) or **URL** (HTTP): the connection's authority, and half its
/// identity.
///
/// The box owns its buffer and reports per keystroke, the app's field contract — the thing that
/// commits this window is a `Button`, which moves focus and calls its handler in the same breath,
/// so a value waiting for blur would never reach the draft being saved.
#[derive(PartialEq)]
struct Authority {
    key: DiffKey,
}

impl KeyExt for Authority {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for Authority {
    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }

    fn render(&self) -> impl IntoElement {
        let ctx = use_consume::<ConnectionCtx>();
        let (label, http, placeholder) = {
            let draft = ctx.draft.read();
            (
                draft.address_label(),
                draft.provider == ProviderId::Http,
                match draft.provider {
                    ProviderId::Http => "https://aserver:8484",
                    _ => "my-bucket",
                },
            )
        };

        let text = use_state({
            let initial = ctx.draft.peek().address.clone();
            move || initial
        });
        // Report the keystroke, then put the box back in step with what was stored — which for
        // HTTP is what was typed, so the echo is a no-op there and this stays one rule. S3 and
        // GCS take their scheme from the picker, so a pasted `s3://acme-lake` is normalised away
        // and the box has to be corrected or it would show one thing and mean another (the rule a
        // length cap follows too). Written in the same effect and guarded, so the self-write
        // settles in one further pass; the draft is **peeked**, so this wakes on the box alone.
        use_side_effect(move || {
            let mut text = text;
            let typed = text.read().clone();
            ctx.edit(move |draft| draft.set_address(typed));
            let stored = ctx.draft.peek().address.clone();
            text.set_if_modified(stored);
        });

        Row::new(label)
            .required()
            .maybe(http, |row| {
                // There is no scheme chip and no scheme picker: `http` and `https` are two
                // different origins, and only the person typing knows which their server speaks.
                // So the box takes the whole URL, and the hint says what it may not carry —
                // which is the one thing about it that is not obvious.
                row.hint(
                    "The whole origin, scheme included. A path belongs to the table that reads it",
                )
            })
            .child(
                ValueField::new(text)
                    .width(Size::fill())
                    .placeholder(placeholder),
            )
    }
}

/// **AUTHENTICATION** — the provider's own modes, and the reference the chosen one carries.
///
/// One component for both providers rather than two, because the row is one row: a pill over an
/// optional reference field. What differs is the mode list and what the reference *is*, and both
/// of those are values.
#[derive(PartialEq)]
struct Auth {
    key: DiffKey,
}

impl KeyExt for Auth {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for Auth {
    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }

    fn render(&self) -> impl IntoElement {
        let ctx = use_consume::<ConnectionCtx>();
        let (provider, s3_auth, gcs_auth) = {
            let draft = ctx.draft.read();
            (draft.provider, draft.s3_auth, draft.gcs_auth)
        };

        let mut pill = SegmentedToggle::new().form();
        match provider {
            ProviderId::Gcs => {
                for id in GcsAuthId::ALL {
                    pill = pill.child(
                        ToggleSegment::text(id.label())
                            .selected(id == gcs_auth)
                            .on_press(move |_| ctx.edit(move |draft| draft.gcs_auth = id)),
                    );
                }
            }
            // S3 is the fall-through rather than an arm of its own, because HTTP never mounts
            // this row at all — it has no auth to choose (spec §6).
            _ => {
                for id in S3AuthId::ALL {
                    pill = pill.child(
                        ToggleSegment::text(id.label())
                            .selected(id == s3_auth)
                            .on_press(move |_| ctx.edit(move |draft| draft.s3_auth = id)),
                    );
                }
            }
        }

        // The reference is a `Row` of its own inside this one, so it carries its own label and
        // `REQUIRED` marker while staying part of the answer above it.
        Row::new("AUTHENTICATION")
            .child(pill)
            .maybe_child(
                (provider == ProviderId::S3 && s3_auth == S3AuthId::Profile)
                    .then(|| qualifier(ProfilePicker)),
            )
            .maybe_child(
                (provider == ProviderId::Gcs && gcs_auth == GcsAuthId::ServiceAccount)
                    .then(|| qualifier(ServiceAccountFile)),
            )
    }
}

/// Set a control's qualifier under it at [`QUALIFIER_GAP`] — the row's own child spacing is the
/// label gap, which is the distance to a *label*, not between two controls.
fn qualifier(child: impl IntoElement) -> Element {
    rect()
        .width(Size::fill())
        .padding(Gaps::new(QUALIFIER_GAP - LABEL_GAP, 0., 0., 0.))
        .child(child)
        .into_element()
}

/// **AWS PROFILE** — a picker over the profiles this machine's own AWS configuration defines
/// (spec §6), never a name typed from memory.
///
/// The list is `Engine::aws_profiles`, read once at the window's mount: profile *names*, and
/// nothing from inside a profile. Three states, and each says a different true thing —
/// unanswered, none found, and a list — because "no profiles" and "not read yet" look identical
/// in an empty dropdown and only one of them is worth acting on.
#[derive(PartialEq)]
struct ProfilePicker;

impl Component for ProfilePicker {
    fn render(&self) -> impl IntoElement {
        let ctx = use_consume::<ConnectionCtx>();
        let chosen = ctx.draft.read().profile.clone();
        let profiles = ctx.profiles.read().clone();

        let row = Row::new("AWS PROFILE").required();
        let Some(profiles) = profiles else {
            return row.child(Prose::new("Reading this machine's AWS configuration…"));
        };
        if profiles.is_empty() {
            // A dead end the app cannot fix, said plainly rather than as an empty dropdown: the
            // profiles are the user's own AWS setup, and Ambient is the mode that needs none.
            return row.child(Note::new(
                "No profiles are defined in this machine's AWS configuration (~/.aws/config). \
                 Add one there, or use Ambient, which resolves whatever this machine already has.",
            ));
        }

        let options: Vec<Element> = profiles
            .iter()
            .map(|name| {
                let name = name.clone();
                MenuItem::new()
                    .selected(name == chosen)
                    .on_press({
                        let name = name.clone();
                        move |_| {
                            let name = name.clone();
                            ctx.edit(move |draft| draft.profile = name);
                        }
                    })
                    .child(MonoValue::new(name.clone()))
                    .into()
            })
            .collect();

        row.child(
            rect()
                .width(Size::px(PROFILE_WIDTH))
                .height(Size::px(FIELD_HEIGHT))
                .child(
                    Select::new()
                        .selected_item(MonoValue::new(match chosen.is_empty() {
                            true => "Select a profile…".to_string(),
                            false => chosen,
                        }))
                        .children(options),
                ),
        )
    }
}

/// **SERVICE-ACCOUNT FILE** — a path, with the picker beside it, and never the key itself. The
/// hint is the canvas's own sentence, because it is the one line that says what Strata does *not*
/// do with the file.
#[derive(PartialEq)]
struct ServiceAccountFile;

impl Component for ServiceAccountFile {
    fn render(&self) -> impl IntoElement {
        let ctx = use_consume::<ConnectionCtx>();
        let path = ctx.draft.read().sa_path.clone();

        Row::new("SERVICE-ACCOUNT FILE")
            .required()
            .hint("A path to the key file. The JSON is never read into or stored by Strata")
            .child(
                PathField::file(path, &["json"])
                    .placeholder("/path/to/service-account.json")
                    .dialog_title("Choose a service-account key file")
                    .on_change(move |path: String| ctx.edit(|draft| draft.sa_path = path)),
            )
    }
}

/// **REGION** — required, and load-bearing: `object_store` does not derive a bucket's region
/// (arrow-rs#2795) and silently assumes `us-east-1`, which reads a different bucket's worth of
/// nothing. The hint says so, because a required field with no reason reads as bureaucracy.
#[derive(PartialEq)]
struct RegionField {
    key: DiffKey,
}

impl KeyExt for RegionField {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for RegionField {
    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }

    fn render(&self) -> impl IntoElement {
        let ctx = use_consume::<ConnectionCtx>();
        let text = use_state({
            let initial = ctx.draft.peek().region.clone();
            move || initial
        });
        use_side_effect(move || {
            let region = text.read().clone();
            ctx.edit(move |draft| draft.region = region);
        });

        Row::new("REGION")
            .required()
            .hint("S3 can't detect a bucket's region, and guessing it reads the wrong bucket")
            .child(
                ValueField::new(text)
                    .width(Size::px(REGION_WIDTH))
                    .placeholder("us-east-1"),
            )
    }
}

/// **ENDPOINT** and its **Allow HTTP** switch — the S3-compatible stores (R2 · MinIO · OSS ·
/// COS), which ride the S3 provider via an endpoint rather than each becoming a provider of
/// their own (spec, provider scope).
///
/// The switch qualifies the endpoint and means nothing without one — AWS itself is HTTPS — so it
/// is off while the box is empty, and `ConnectionDraft::def` drops it if the box is later
/// cleared.
#[derive(PartialEq)]
struct Endpoint {
    key: DiffKey,
}

impl KeyExt for Endpoint {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for Endpoint {
    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }

    fn render(&self) -> impl IntoElement {
        let ctx = use_consume::<ConnectionCtx>();
        let text = use_state({
            let initial = ctx.draft.peek().endpoint.clone();
            move || initial
        });
        use_side_effect(move || {
            let endpoint = text.read().clone();
            ctx.edit(move |draft| draft.endpoint = endpoint);
        });
        let (allow_http, has_endpoint) = {
            let draft = ctx.draft.read();
            (draft.allow_http, !draft.endpoint.trim().is_empty())
        };

        Row::new("ENDPOINT")
            .hint(
                "An S3-compatible endpoint: MinIO, Cloudflare R2, Alibaba OSS, Tencent COS. \
                 Blank means AWS itself",
            )
            .child(
                ValueField::new(text)
                    .width(Size::fill())
                    .placeholder("https://s3.example.com"),
            )
            .child(qualifier(
                rect()
                    .width(Size::fill())
                    .horizontal()
                    .cross_align(Alignment::Center)
                    .spacing(QUALIFIER_GAP)
                    .child(
                        Switch::new()
                            .toggled(allow_http)
                            .enabled(has_endpoint)
                            .on_toggle(move |_| {
                                ctx.edit(|draft| draft.allow_http = !draft.allow_http)
                            }),
                    )
                    // A *sibling* of the switch, never its parent: a built-in's press reaches
                    // its ancestors, so a pressable wrapper would take the same click and
                    // toggle back.
                    .child(Prose::new(
                        "Allow plain HTTP - for an S3-compatible endpoint on this machine",
                    )),
            ))
    }
}

/// **CLIENT OPTIONS** — `object_store`'s own `ClientConfigKey` map, as a table.
///
/// Every provider's store is built on one HTTP client and takes the same keys, so this is the
/// one section that does not change with the picker. It is the escape hatch for the things a
/// form cannot enumerate in advance: a proxy, a longer timeout, HTTP/1 against a server that
/// mishandles HTTP/2.
///
/// **Rows here, a map in the def** (AGENTS.md §2). The option is a `Select` over
/// [`CLIENT_KEYS`] rather than a text box, because the set is closed and small — which removes
/// the typo class outright and takes the autocomplete the Settings grid needs with it.
#[derive(PartialEq)]
struct ClientOptions {
    key: DiffKey,
}

impl KeyExt for ClientOptions {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for ClientOptions {
    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }

    fn render(&self) -> impl IntoElement {
        let ctx = use_consume::<ConnectionCtx>();
        let rows = ctx.draft.read().client_config.clone();

        Row::new("CLIENT OPTIONS")
            .hint("Applied to this connection's HTTP client, whichever provider serves it")
            .child(
                rect()
                    .width(Size::fill())
                    .vertical()
                    .spacing(OPTION_GAP)
                    .maybe_child((!rows.is_empty()).then(|| {
                        Table::new()
                            .column_widths(vec![
                                Size::flex(1.),
                                Size::flex(1.),
                                Size::px(OPTION_REMOVE),
                            ])
                            .child(ClientOptionRows)
                            .into_element()
                    }))
                    .child(
                        Button::new()
                            .outline()
                            .on_press(move |_: Event<PressEventData>| {
                                ctx.edit(|draft| {
                                    draft.client_config.add(String::new(), String::new());
                                })
                            })
                            .child(
                                rect()
                                    .horizontal()
                                    .cross_align(Alignment::Center)
                                    .spacing(6.)
                                    .child(Icon::new(IconName::Plus).size(12.))
                                    .child(Prose::new("Add option")),
                            ),
                    ),
            )
    }
}

/// The rows themselves. Its own component because `TableBody` is not `Clone`, so it cannot be
/// built in one scope and handed to `Table` as a value in another.
#[derive(PartialEq)]
struct ClientOptionRows;

impl Component for ClientOptionRows {
    fn render(&self) -> impl IntoElement {
        let ctx = use_consume::<ConnectionCtx>();
        let rows = ctx.draft.read().client_config.clone();

        let mut body = TableBody::new();
        for row in rows.rows() {
            body = body.child(
                ClientOptionRow {
                    id: row.id,
                    chosen: row.key.clone(),
                    value: row.value.clone(),
                    key: DiffKey::None,
                }
                .key(row.id),
            );
        }
        body
    }
}

/// One option: the key picker, its value, and the ⨯ that drops the row.
#[derive(PartialEq)]
struct ClientOptionRow {
    id: u64,
    chosen: String,
    value: String,
    key: DiffKey,
}

impl KeyExt for ClientOptionRow {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for ClientOptionRow {
    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }

    fn render(&self) -> impl IntoElement {
        let ctx = use_consume::<ConnectionCtx>();
        let id = self.id;

        // The box owns its buffer and reports per keystroke — the app's field contract, and the
        // one direction of travel the Settings grid holds for the same reason: the list wakes on
        // every keystroke, so writing it back would drag the cursor.
        let text = use_state({
            let seed = self.value.clone();
            move || seed
        });
        use_side_effect(move || {
            let value = text.read().clone();
            ctx.edit(move |draft| draft.client_config.set_value(id, value));
        });

        let chosen = self.chosen.clone();
        let options: Vec<Element> = CLIENT_KEYS
            .iter()
            .map(|key| {
                MenuItem::new()
                    .selected(key.name == chosen)
                    .on_press(move |_| {
                        ctx.edit(move |draft| draft.client_config.set_key(id, key.name.to_string()))
                    })
                    .child(
                        rect()
                            .vertical()
                            .spacing(2.)
                            .child(MonoValue::new(key.name))
                            .child(Prose::new(key.what)),
                    )
                    .into()
            })
            .collect();

        TableRow::new()
            .child(
                TableCell::new()
                    .height(Size::px(OPTION_ROW))
                    .main_align(Alignment::Start)
                    .child(
                        rect()
                            .width(Size::fill())
                            .height(Size::px(FIELD_HEIGHT))
                            .child(
                                Select::new()
                                    .selected_item(MonoValue::new(match self.chosen.is_empty() {
                                        true => "Choose an option…".to_string(),
                                        false => self.chosen.clone(),
                                    }))
                                    .children(options),
                            ),
                    ),
            )
            .child(
                TableCell::new()
                    .height(Size::px(OPTION_ROW))
                    .main_align(Alignment::Start)
                    .child(ValueField::new(text).width(Size::fill()).placeholder("30s")),
            )
            .child(
                TableCell::new()
                    .height(Size::px(OPTION_ROW))
                    .padding(Gaps::new_all(0.))
                    .child(
                        Button::new()
                            .flat()
                            .width(Size::px(OPTION_REMOVE))
                            .height(Size::px(OPTION_REMOVE))
                            .on_press(move |_: Event<PressEventData>| {
                                ctx.edit(move |draft| draft.client_config.remove(id))
                            })
                            .child(Icon::new(IconName::Close).size(12.)),
                    ),
            )
    }
}

/// The standing credentials note, as a keyed row — see [`Fields`] on why every child of that form
/// carries the provider in its key.
#[derive(PartialEq)]
struct NoteRow {
    note: &'static str,
    key: DiffKey,
}

impl KeyExt for NoteRow {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for NoteRow {
    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }

    fn render(&self) -> impl IntoElement {
        Note::new(self.note)
    }
}
