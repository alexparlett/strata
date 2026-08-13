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
use strata_core::engine::store::ClientKey;
use strata_model::ProviderId;

use crate::apps::connection::model::{GcsAuthId, S3AuthId};
use crate::apps::connection::ConnectionCtx;
use crate::components::divider::Divider;
use crate::components::form::{
    form_theme, Form, Note, PathField, Row, ValueField, FIELD_HEIGHT, LABEL_GAP,
};
use crate::components::icon::IconName;
use crate::components::metrics::{
    EMPTY_TABLE_HEIGHT, ERROR_STRIPE, SP_1, SP_3, SP_4, TABLE_HEAD_HEIGHT,
};
use crate::components::segmented_toggle::{SegmentedToggle, ToggleSegment};
use crate::components::tones::tones;
use crate::components::tool_button::ToolButton;
use crate::components::typography::{Caption, Control, MonoValue, Prose};
use crate::components::window::window_theme;
use crate::theme::{use_roles, Role};

/// The gap between a control and the thing that qualifies it — an auth pill and the reference it
/// turned on, an endpoint box and its Allow-HTTP switch (canvas `var(--sp-4)`). Inside the row,
/// because a qualifier is what its control's answer *means* rather than a second question.
const QUALIFIER_GAP: f32 = SP_4;
/// The region box, which holds `ap-southeast-1` and never anything longer.
const REGION_WIDTH: f32 = 180.;
/// The profile picker (canvas `width: 220px`).
const PROFILE_WIDTH: f32 = 220.;
/// The client-option table, in the two list editors' own numbers: the properties grid's header
/// strip and cell inset, a row tall enough to hold a field, and the source-path list's toolbar gap
/// and stack gap. The key column is fixed because an option name is a known width and the value is
/// whatever the user types.
const CELL_INSET: f32 = SP_4;
const OPTION_ROW: f32 = 38.;
/// The stripe down an invalid row's leading edge, and how wide the suggestion panel stands — a
/// client option's name is long and the box it hangs off is a third of a narrow window.
const SUGGEST_WIDTH: f32 = 300.;
/// How many offers the panel grows to before it scrolls, and what one of them stands at.
///
/// **Three**, because an offer here is two lines (the name means little without the sentence under
/// it) and a panel taller than that covers the table it is being typed into. The rest are still
/// offered — the panel scrolls to them — so nothing is cut from the answer, only from the view.
/// A *maximum*, so one match is one row of panel rather than three rows of empty.
const SUGGEST_ROWS: f32 = 3.;
const SUGGEST_ROW_HEIGHT: f32 = 46.;
pub const OPTION_KEY_WIDTH: f32 = 210.;
const TOOL_GAP: f32 = SP_3;
const STACK_GAP: f32 = SP_3;

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

        let scope = provider.label();
        let mut form = Form::new()
            .child(ProviderPicker { key: DiffKey::None }.key(format!("provider·{scope}")))
            .child(Authority { key: DiffKey::None }.key(format!("authority·{scope}")));
        if matches!(provider, ProviderId::S3 | ProviderId::Gcs) {
            form = form.child(Auth { key: DiffKey::None }.key(format!("auth·{scope}")));
        }
        if provider == ProviderId::S3 {
            form = form
                .child(RegionField { key: DiffKey::None }.key(format!("region·{scope}")))
                .child(Endpoint { key: DiffKey::None }.key(format!("endpoint·{scope}")));
        }
        if provider.is_object_store() {
            form = form.child(ClientOptions { key: DiffKey::None }.key(format!("client·{scope}")));
        }
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
///
/// **[`ProviderId::OBJECT_STORES`] until DB-04 builds the database form.** The `Postgres` arm
/// exists on the model and in the engine (DB-02), but this window has no rows for a catalog
/// name, a user, an SSL mode or a password yet — so offering it would be a picker option that
/// produces a def nothing here can fill in or correct. A def that already names one still opens
/// and round-trips (`ConnectionDraft` carries its settings verbatim); what is missing is the
/// ability to *choose* it, and that arrives with the fields, in the task that owns them.
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
        for id in ProviderId::OBJECT_STORES {
            pill = pill.child(
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
                    .child(MonoValue::new(name))
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
                            .on_toggle(move |()| {
                                ctx.edit(|draft| draft.allow_http = !draft.allow_http);
                            }),
                    )
                    .child(Prose::new(
                        "Allow plain HTTP - for an S3-compatible endpoint on this machine",
                    )),
            ))
    }
}

/// **CLIENT OPTIONS** — `object_store`'s own `ClientConfigKey` map, as a table.
///
/// Every provider's store is built on the **same HTTP client**, and each builder routes a
/// `Client(..)` key into the same `ClientOptions` (`aws/builder.rs`, `gcp/builder.rs`), so this is
/// the one section that does not change with the picker: a proxy, a timeout or a user agent
/// applies to a signed S3 request exactly as it does to a public HTTP one.
///
/// **Built the way this app's other two list editors are** — Settings ▸ Engine's properties grid
/// and the Configure window's source paths (`apps/settings/views/engine/table.rs`,
/// `apps/configure/views/paths.rs`): Freya's built-in `Table`, a `ToolButton` toolbar above it
/// acting on the **selected** row, the empty state *inside* the table so it still reads as one,
/// and bare fields in the cells. Two columns, so it carries a header — where the single-column
/// path list does not, because its section label already names the column.
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
        Row::new("CLIENT OPTIONS")
            .hint(
                "Applied to this connection's HTTP client, whichever provider serves it: all \
                 three object stores are built on one client",
            )
            .child(
                rect()
                    .width(Size::fill())
                    .vertical()
                    .spacing(STACK_GAP)
                    .child(OptionToolbar)
                    .child(OptionTable),
            )
    }
}

/// Add · remove, in the source-path toolbar's shape and at its size — a `ToolButton` pair over
/// the table, acting on the selected row, rather than a control per row.
#[derive(PartialEq)]
struct OptionToolbar;

impl Component for OptionToolbar {
    fn render(&self) -> impl IntoElement {
        let win = window_theme();
        let error = tones().error;
        let ctx = use_consume::<ConnectionCtx>();
        let selected = *ctx.selected_option.read();

        rect()
            .horizontal()
            .cross_align(Alignment::Center)
            .spacing(TOOL_GAP)
            .child(
                ToolButton::new(IconName::Plus, "Add option")
                    .outlined()
                    .color(win.icon_color)
                    .on_press(move |_| {
                        let mut slot = ctx.selected_option;
                        let mut added = *slot.peek();
                        ctx.edit(|draft| {
                            added = Some(draft.client_config.add(String::new(), String::new()));
                        });
                        slot.set(added);
                    }),
            )
            .child(
                ToolButton::new(IconName::Minus, "Remove option")
                    .outlined()
                    .color(error)
                    .enabled(selected.is_some())
                    .on_press(move |_| {
                        let Some(id) = selected else { return };
                        let mut slot = ctx.selected_option;
                        let mut next = selected;
                        ctx.edit(|draft| next = draft.client_config.remove(id));
                        slot.set(next);
                    }),
            )
    }
}

/// The table: a header strip over the option rows, or the empty state in their place.
#[derive(PartialEq)]
struct OptionTable;

impl Component for OptionTable {
    fn render(&self) -> impl IntoElement {
        let form = form_theme();
        let ctx = use_consume::<ConnectionCtx>();
        let rows = ctx.draft.read().client_config.clone();

        if rows.is_empty() {
            return option_table().child(
                rect()
                    .width(Size::fill())
                    .height(Size::px(EMPTY_TABLE_HEIGHT))
                    .center()
                    .child(
                        Prose::new("No client options. The defaults suit most connections.")
                            .color(form.hint_color),
                    ),
            );
        }

        let mut body = TableBody::new();
        for row in rows.rows() {
            body = body.child(
                OptionRow {
                    id: row.id,
                    selected: Some(row.id) == *ctx.selected_option.read(),
                    invalid: rows.names_a_client_option(row.id) == Some(false),
                    key: DiffKey::None,
                }
                .key(row.id),
            );
        }

        option_table().child(body)
    }
}

/// The framed table and its header, **column split included** — one construction site, because
/// both of this section's branches need it and only one of them used to have it.
///
/// `TableRow` reads its split from the `TableConfig` its `Table` provides and falls back to an
/// equal share per cell, so an empty-state table built without `column_widths` laid the header out
/// 50/50 and then jumped to this split the instant the first row was added. A shared constructor
/// is the fix rather than a second `column_widths` call: the two branches cannot disagree about a
/// value neither of them writes.
fn option_table() -> Table {
    Table::new()
        .column_widths(vec![Size::px(OPTION_KEY_WIDTH), Size::flex(1.)])
        .child(TableHead::new().child(OptionHead))
}

/// The `Option` / `Value` strip. A `TableRow` so it shares the column widths and the rule under
/// it, with its hover fill pinned to its own background — a header is not a row you can pick.
#[derive(PartialEq)]
struct OptionHead;

impl Component for OptionHead {
    fn render(&self) -> impl IntoElement {
        let win = window_theme();
        let form = form_theme();
        let head = win.panel_background;

        let cell = |label: &'static str| {
            TableCell::new()
                .height(Size::px(TABLE_HEAD_HEIGHT))
                .padding(Gaps::new(0., CELL_INSET, 0., CELL_INSET))
                .main_align(Alignment::Start)
                .child(Control::new(label).color(form.label_color))
        };

        TableRow::new()
            .theme(TableThemePartial {
                row_background: Some(head.into()),
                hover_row_background: Some(head.into()),
                ..Default::default()
            })
            .child(cell("Option"))
            .child(cell("Value"))
    }
}

/// One option: its name, its value, and the fill that says it is the selected row.
///
/// **The name is a bare field with an attached suggestion panel**, the properties grid's cell for
/// cell — not a dropdown. A closed list is not a reason to reach for one: the grid types the same
/// kind of thing, the panel offers what is left of the catalogue while the box has focus, and a
/// field takes a paste, a partial match and a name from a newer `object_store` where a `Select`
/// takes none of them.
#[derive(PartialEq)]
struct OptionRow {
    id: u64,
    selected: bool,
    invalid: bool,
    key: DiffKey,
}

impl KeyExt for OptionRow {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for OptionRow {
    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }

    fn render(&self) -> impl IntoElement {
        let win = window_theme();
        let tones = tones();
        let roles = use_roles();
        let ctx = use_consume::<ConnectionCtx>();
        let id = self.id;

        let name = use_state(|| {
            ctx.draft
                .peek()
                .client_config
                .name_of(id)
                .unwrap_or_default()
        });
        let value = use_state(|| {
            ctx.draft
                .peek()
                .client_config
                .value_of(id)
                .unwrap_or_default()
        });
        let a11y_id = use_a11y();
        let focus = use_focus(a11y_id);
        let value_id = use_a11y();
        let value_focus = use_focus(value_id);

        use_side_effect(move || {
            let typed = name.read().clone();
            if ctx.draft.peek().client_config.name_of(id).as_deref() != Some(typed.as_str()) {
                ctx.edit(move |draft| draft.client_config.set_key(id, typed));
            }
        });
        use_side_effect(move || {
            let typed = value.read().clone();
            if ctx.draft.peek().client_config.value_of(id).as_deref() != Some(typed.as_str()) {
                ctx.edit(move |draft| draft.client_config.set_value(id, typed));
            }
        });

        let mut slot = ctx.selected_option;
        use_side_effect(move || {
            let focused = focus() != Focus::Not || value_focus() != Focus::Not;
            if focused && *slot.peek() != Some(id) {
                slot.set(Some(id));
            }
        });

        let name_color = match ctx.draft.read().client_config.names_a_client_option(id) {
            Some(false) => tones.error,
            _ => roles.get(Role::Text),
        };

        let suggestions: Vec<&'static ClientKey> = match focus() {
            Focus::Not => Vec::new(),
            _ => ctx.draft.read().client_config.suggestions(id),
        };
        let mut offers = rect().width(Size::fill()).vertical();
        for entry in suggestions.iter().copied() {
            offers = offers.child(
                MenuButton::new()
                    .on_press(move |_: Event<PressEventData>| {
                        let mut name = name;
                        name.set(entry.name.to_string());
                    })
                    .child(SuggestionRow { entry }),
            );
        }
        let menu = Menu::new().min_width(Size::px(SUGGEST_WIDTH)).child(
            ScrollView::new()
                .latch_wheel()
                .height(Size::auto())
                .max_height(Size::px(SUGGEST_ROWS * SUGGEST_ROW_HEIGHT))
                .child(offers),
        );

        let fill = match self.selected {
            true => win.row_selected_background,
            false => Color::TRANSPARENT,
        };

        TableRow::new()
            .theme(TableThemePartial {
                row_background: Some(fill.into()),
                hover_row_background: Some(fill.into()),
                ..Default::default()
            })
            .on_press(move |_: Event<PressEventData>| slot.set(Some(id)))
            .child(
                TableCell::new()
                    .height(Size::px(OPTION_ROW))
                    .padding(Gaps::new(0., CELL_INSET, 0., 0.))
                    .main_align(Alignment::Start)
                    .child(
                        rect()
                            .expanded()
                            .horizontal()
                            .content(Content::Flex)
                            .cross_align(Alignment::Center)
                            .spacing(CELL_INSET - ERROR_STRIPE)
                            .child(
                                rect()
                                    .width(Size::px(ERROR_STRIPE))
                                    .height(Size::fill())
                                    .background(match self.invalid {
                                        true => tones.error,
                                        false => Color::TRANSPARENT,
                                    }),
                            )
                            .child(
                                Attached::new(
                                    rect().width(Size::flex(1.)).color(name_color).child(
                                        ValueField::new(name)
                                            .bare()
                                            .placeholder("timeout")
                                            .height(Size::px(OPTION_ROW))
                                            .width(Size::fill())
                                            .a11y_id(a11y_id),
                                    ),
                                )
                                .bottom()
                                .align_start()
                                .maybe_child((!suggestions.is_empty()).then_some(menu)),
                            )
                            .child(Divider::vertical().color(roles.get(Role::Border))),
                    ),
            )
            .child(
                TableCell::new()
                    .height(Size::px(OPTION_ROW))
                    .padding(Gaps::new(0., CELL_INSET, 0., CELL_INSET))
                    .main_align(Alignment::Start)
                    .child(
                        ValueField::new(value)
                            .bare()
                            .placeholder("30s")
                            .height(Size::px(OPTION_ROW))
                            .width(Size::fill())
                            .a11y_id(value_id),
                    ),
            )
    }
}

/// One catalogue offer: the option, and what it does.
#[derive(PartialEq)]
struct SuggestionRow {
    entry: &'static ClientKey,
}

impl Component for SuggestionRow {
    fn render(&self) -> impl IntoElement {
        let form = form_theme();

        rect()
            .width(Size::fill())
            .vertical()
            .spacing(SP_1)
            .child(MonoValue::new(self.entry.name))
            .child(
                Caption::new(self.entry.what)
                    .color(form.hint_color)
                    .width(Size::fill())
                    .wrap(),
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
