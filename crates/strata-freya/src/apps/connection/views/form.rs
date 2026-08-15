//! The editor's fields, in the canvas's order: **PROVIDER**, the authority box,
//! **AUTHENTICATION** and whatever that mode refers to, then S3's **REGION** and **ENDPOINT**,
//! then the standing note about where credentials actually come from.
//!
//! A database replaces the address box with **URL** and **DATABASE** — the two halves of
//! `ConnectionDef::address`, split here because a server and the database on it are two things
//! Postgres names separately — and follows them with **CATALOG**, **USER**, **PASSWORD** and
//! **SSL MODE**. CATALOG is the odd one out and says so: it is Strata's prefix for the
//! connection, not anything the server has.
//!
//! Built from `components::form` — a [`Form`] of [`Row`]s (AGENTS.md §3), so the label register,
//! the `REQUIRED` markers and the rhythm between rows are the app's rather than this window's.
//!
//! **Which rows exist depends on the provider, and only on the provider.** HTTP is anonymous by
//! construction and has no region, so it shows the authority box and nothing else; GCS has no
//! region or endpoint; a database has no region, endpoint, auth mode or client options, because
//! those are object-store vocabulary. Rows are not shipped disabled: a control that cannot mean
//! anything for the chosen provider is not a control (the same call the Configure window makes
//! about its LOCATION toggle).
//!
//! **A field's error is not painted on the field.** The canvas reddens the region box and writes
//! a line under it; here the one thing that says why Save is off is the footer, and it is the
//! same value that disables the button ([`super::footer`]) — which is what stops a form from
//! having two accounts of its own validity that can disagree. The label still carries `REQUIRED`,
//! because that is a fact about the field rather than a verdict on what is in it.

use freya::prelude::*;
use strata_engine::store::ClientKey;
use strata_model::{PgPassword, PgSslMode, ProviderId};

use crate::apps::connection::model::{GcsAuthId, S3AuthId};
use crate::apps::connection::{ConnectionCtx, PasswordRow};
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
/// The database boxes whose value is a short, known-shaped word rather than free text — at the
/// region box's width, which is the same judgement about the same kind of value. USER is not one
/// of them: it sits beside PASSWORD, which fills, and two credential boxes of different widths
/// read as a mistake.
const CATALOG_WIDTH: f32 = REGION_WIDTH;
const SSL_WIDTH: f32 = REGION_WIDTH;
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
            .child(ProviderPicker { key: DiffKey::None }.key(format!("provider·{scope}")));
        form = match provider {
            ProviderId::Postgres => form
                .child(PgUrl { key: DiffKey::None }.key(format!("url·{scope}")))
                .child(PgDatabase { key: DiffKey::None }.key(format!("database·{scope}")))
                .child(CatalogName { key: DiffKey::None }.key(format!("catalog·{scope}")))
                .child(UserField { key: DiffKey::None }.key(format!("user·{scope}")))
                .child(PasswordField { key: DiffKey::None }.key(format!("password·{scope}")))
                .child(Ssl { key: DiffKey::None }.key(format!("ssl·{scope}"))),
            _ => form.child(Authority { key: DiffKey::None }.key(format!("authority·{scope}"))),
        };
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
/// [`ProviderId::ALL`], because this is the picker that constant is for. The narrower question —
/// which connection a set of *files* reads through — belongs to the Configure window's LOCATION
/// pill, which is what [`ProviderId::OBJECT_STORES`] answers.
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
        let (label, hint, placeholder) = {
            let draft = ctx.draft.read();
            (
                draft.address_label(),
                match draft.provider {
                    ProviderId::Http => Some(
                        "The whole origin, scheme included. A path belongs to the table that \
                         reads it",
                    ),
                    _ => None,
                },
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

        Row::new(label).required().map(hint, Row::hint).child(
            ValueField::new(text)
                .width(Size::fill())
                .placeholder(placeholder),
        )
    }
}

/// **URL** — the server, `host:port`. The port is never assumed: a def reading `db.internal`
/// while it means `:5432` shows one thing and connects to another.
///
/// Half of `ConnectionDef::address`, which stays one `host:port/database` string — the two boxes
/// are a form split, so `parse_pg_address` remains the only parse of that grammar.
#[derive(PartialEq)]
struct PgUrl {
    key: DiffKey,
}

impl KeyExt for PgUrl {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for PgUrl {
    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }

    fn render(&self) -> impl IntoElement {
        let ctx = use_consume::<ConnectionCtx>();
        let text = use_state({
            let initial = ctx.draft.peek().pg_server().to_string();
            move || initial
        });
        use_side_effect(move || {
            let mut text = text;
            let typed = text.read().clone();
            ctx.edit(move |draft| draft.set_pg_server(typed));
            let stored = ctx.draft.peek().pg_server().to_string();
            text.set_if_modified(stored);
        });

        Row::new("URL")
            .required()
            .hint("The server, as you would dial it. The port is not assumed")
            .child(
                ValueField::new(text)
                    .width(Size::fill())
                    .placeholder("localhost:5432"),
            )
    }
}

/// **DATABASE** — the database on that server, which is the other half of the address and the
/// thing Postgres itself calls a database.
#[derive(PartialEq)]
struct PgDatabase {
    key: DiffKey,
}

impl KeyExt for PgDatabase {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for PgDatabase {
    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }

    fn render(&self) -> impl IntoElement {
        let ctx = use_consume::<ConnectionCtx>();
        let text = use_state({
            let initial = ctx.draft.peek().pg_database().to_string();
            move || initial
        });
        use_side_effect(move || {
            let mut text = text;
            let typed = text.read().clone();
            ctx.edit(move |draft| draft.set_pg_database(typed));
            let stored = ctx.draft.peek().pg_database().to_string();
            text.set_if_modified(stored);
        });

        Row::new("DATABASE")
            .required()
            .hint("One database per connection. Two databases on one server are two connections")
            .child(
                ValueField::new(text)
                    .width(Size::fill())
                    .placeholder("appdb"),
            )
    }
}

/// **CATALOG** — the prefix Strata addresses this connection by, since SQL cannot address
/// `postgres://host:5432/analytics`.
///
/// Strata's name for the connection rather than anything the server has: `PgStore::catalog`, the
/// top of `catalog.schema.table`. The user's choice, not derived from [`PgDatabase`], because two
/// servers' `analytics` would derive one prefix. What it may be is `PgStore::check_catalog` plus
/// the project-wide clash the footer asks `check_catalog_name` about, so the field and the
/// registration cannot disagree.
#[derive(PartialEq)]
struct CatalogName {
    key: DiffKey,
}

impl KeyExt for CatalogName {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for CatalogName {
    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }

    fn render(&self) -> impl IntoElement {
        let ctx = use_consume::<ConnectionCtx>();
        let text = use_state({
            let initial = ctx.draft.peek().pg.catalog.clone();
            move || initial
        });
        use_side_effect(move || {
            let catalog = text.read().clone();
            ctx.edit(move |draft| draft.pg.catalog = catalog);
        });

        Row::new("CATALOG")
            .required()
            .hint(
                "The catalog prefix Strata queries this connection by: 'pg' makes a table \
                   'pg.public.orders'",
            )
            .child(
                ValueField::new(text)
                    .width(Size::px(CATALOG_WIDTH))
                    .placeholder("pg"),
            )
    }
}

/// **USER** — the role this connection logs in as, and half its identity: two roles over one
/// database are two connections, with two sets of visible schemas. Its own row rather than
/// userinfo in the address box, which the address rules refuse, since a
/// `postgres://reader:hunter2@…` pasted into one box would put a password in the committed
/// `project.json`.
#[derive(PartialEq)]
struct UserField {
    key: DiffKey,
}

impl KeyExt for UserField {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for UserField {
    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }

    fn render(&self) -> impl IntoElement {
        let ctx = use_consume::<ConnectionCtx>();
        let text = use_state({
            let initial = ctx.draft.peek().pg.user.clone();
            move || initial
        });
        use_side_effect(move || {
            let user = text.read().clone();
            ctx.edit(move |draft| draft.pg.user = user);
        });

        Row::new("USER")
            .required()
            .hint(
                "The role to log in as. Part of the connection's identity, so changing it is a \
                   different connection",
            )
            .child(ValueField::new(text).width(Size::fill()))
    }
}

/// **PASSWORD** — the one control here whose state is about *this machine* rather than the def.
///
/// The settings window's API-key marker is honest because it minted the reference when it stored
/// one; a committed expectation says nothing about the machine reading it, so this row reports
/// the mount probe ([`PasswordRow`]) instead. The two clearing gestures are kept apart for the
/// same reason: *remove from this machine* is local, while *this connection uses no password*
/// edits the shared def and would break the colleague who has one.
///
/// They stack rather than sitting side by side, because both are offered at once and their two
/// sentences are wider than this window at its minimum size — and a torin child paints outside
/// its box rather than clipping.
#[derive(PartialEq)]
struct PasswordField {
    key: DiffKey,
}

impl KeyExt for PasswordField {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for PasswordField {
    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }

    fn render(&self) -> impl IntoElement {
        let form = form_theme();
        let ctx = use_consume::<ConnectionCtx>();
        let mut revealed = use_state(|| false);

        let text = ctx.password;
        use_side_effect(move || {
            let typed = !text.read().trim().is_empty();
            if typed {
                let mut removed = ctx.password_removed;
                removed.set(false);
            }
            let expected = *ctx.password_expected.read();
            let now = match typed {
                true => PgPassword::Keystore,
                false => expected,
            };
            ctx.edit(move |draft| draft.pg.password = now);
        });

        let row = PasswordRow::of(
            *ctx.password_expected.read(),
            !ctx.password.read().trim().is_empty(),
            *ctx.password_removed.read(),
            &ctx.password_probe.read(),
        );

        Row::new("PASSWORD")
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
                                true => "Hide the password",
                                false => "Show the password",
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
                        Prose::new(row.note())
                            .color(form.hint_color)
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
                                        let mut removed = ctx.password_removed;
                                        let mut text = text;
                                        text.set(String::new());
                                        removed.set(true);
                                    })
                                    .child(Control::new("Remove from this machine"))
                            }))
                            .maybe_child(row.offers_disuse().then(|| {
                                Button::new()
                                    .flat()
                                    .on_press(move |_: Event<PressEventData>| {
                                        let mut expected = ctx.password_expected;
                                        let mut removed = ctx.password_removed;
                                        let mut text = text;
                                        text.set(String::new());
                                        expected.set(PgPassword::None);
                                        removed.set(true);
                                        ctx.edit(|draft| draft.pg.password = PgPassword::None);
                                    })
                                    .child(Control::new("This connection uses no password"))
                            })),
                    ),
            ))
    }
}

/// **SSL MODE** and its **ROOT CERTIFICATE** — libpq's vocabulary in libpq's spellings, because
/// the value is handed to the driver as written. The certificate row is shown for the two
/// verifying modes only and is optional there: blank means the driver's own trust store.
#[derive(PartialEq)]
struct Ssl {
    key: DiffKey,
}

impl KeyExt for Ssl {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for Ssl {
    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }

    fn render(&self) -> impl IntoElement {
        let ctx = use_consume::<ConnectionCtx>();
        let mode = ctx.draft.read().pg.sslmode;

        let options: Vec<Element> = PgSslMode::ALL
            .into_iter()
            .map(|option| {
                MenuItem::new()
                    .selected(option == mode)
                    .on_press(move |_| ctx.edit(move |draft| draft.pg.sslmode = option))
                    .child(MonoValue::new(option.as_str()))
                    .into()
            })
            .collect();

        Row::new("SSL MODE")
            .hint("Handed to the driver as written. 'prefer' encrypts when the server offers it")
            .child(
                rect()
                    .width(Size::px(SSL_WIDTH))
                    .height(Size::px(FIELD_HEIGHT))
                    .child(
                        Select::new()
                            .selected_item(MonoValue::new(mode.as_str()))
                            .children(options),
                    ),
            )
            .maybe_child(mode.verifies().then(|| qualifier(RootCertificate)))
    }
}

/// **ROOT CERTIFICATE** — the certificate the two verifying modes read, as a path and never as
/// the file's contents ([`ServiceAccountFile`]'s rule). Optional: blank is the machine's own
/// trust store, which is the whole answer for a managed server behind a public CA.
#[derive(PartialEq)]
struct RootCertificate;

impl Component for RootCertificate {
    fn render(&self) -> impl IntoElement {
        let ctx = use_consume::<ConnectionCtx>();
        let cert = ctx.draft.read().pg.sslrootcert.clone();

        Row::new("ROOT CERTIFICATE")
            .hint("Blank uses this machine's own trust store")
            .child(
                PathField::file(cert, &["pem", "crt", "cer"])
                    .placeholder("/path/to/root.pem")
                    .dialog_title("Choose a root certificate")
                    .on_change(move |path: String| {
                        ctx.edit(|draft| draft.pg.sslrootcert = path);
                    }),
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
