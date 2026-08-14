//! The database connection's **Schemas…** picker (DB-05) — which of a connection's schemas the
//! tree, the inspector and completion show.
//!
//! **Display only**, and that is what makes this dialog legitimate at all. Registration exposes
//! every schema the connection can reach; [`PgStore::schemas`] scopes what Strata *shows*, so a
//! change here needs no reconnect, invalidates no plan and cannot break a query that already
//! names a schema this list leaves out.
//!
//! Which is also why the write is **not** `upsert_connection`: that replaces the row with a fresh
//! `Reg::Loading`, and nothing would answer it short of a whole-catalog re-scan — a permanent
//! spinner over a change that touched no engine state. The store's def-in-place write
//! ([`ProjectState::update_connection_def`]) keeps the row's verdict, which is still true.
//!
//! The offer is [`Engine::db_listing`]'s **scoped and tagged** answer and nothing derived beside
//! it, so this picker, the tree and completion cannot disagree about what a connection shows. A
//! connection that is not live has no enumeration to offer: the picker then lists the def's own
//! schemas with the connection's failure named, rather than an unexplained empty list.

use freya::components::ScrollView;
use freya::prelude::*;
use freya::radio::{use_radio, use_radio_station, RadioStation};
use std::collections::BTreeSet;

use strata_core::engine::db::SchemaVisibility;
use strata_model::{ConnectionDef, PgStore, Provider};

use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::state::{persisted_defs, use_report, ProjChan, ProjectState, ReportCtx};
use crate::components::dialog::{CheckboxRow, Dialog, DialogHeader};
use crate::components::icon::{Icon, IconName};
use crate::components::metrics::{SP_2, SP_4};
use crate::components::tones::tones;
use crate::components::typography::{Caption, Control, MonoValue, Prose, Title};
use crate::theme::{use_roles, Role};

/// What a schema the def enables and the server does not have says on its row.
const MISSING: &str = "not in the connection";

/// The slot a trigger sets to ask for the picker — a connection's `ConnectionDef::url()`.
/// Provided at the project root, like every other dialog's.
pub type SchemasRequest = State<Option<String>>;

/// One row of the picker: a schema, whether it is enabled, and what the connection says about it.
#[derive(Clone, PartialEq)]
struct Offer {
    name: String,
    enabled: bool,
    /// The server does not have it (or the role cannot see it) — enabled, and answering nothing.
    missing: bool,
}

/// What the picker offers for `def`, and the failure to name if there is one.
///
/// Live: the connection's own tagged enumeration. Not live: the def's `schemas`, so the user can
/// still take one off a connection that is refusing to connect *because* of it.
fn offers(engine: &EngineCtx, def: &ConnectionDef, pg: &PgStore) -> (Vec<Offer>, bool) {
    let Some((_, schemas)) = engine.db_listing(def) else {
        let offers = pg
            .schemas
            .iter()
            .map(|name| Offer {
                name: name.clone(),
                enabled: true,
                missing: false,
            })
            .collect();
        return (offers, false);
    };
    let offers = schemas
        .into_iter()
        .map(|schema| Offer {
            name: schema.name,
            enabled: schema.visibility != SchemaVisibility::NotEnabled,
            missing: schema.visibility == SchemaVisibility::EnabledButMissing,
        })
        .collect();
    (offers, true)
}

/// Write `schemas` onto the connection's def and persist — the whole of what Apply does.
fn apply(
    project: RadioStation<ProjectState, ProjChan>,
    report: ReportCtx,
    url: &str,
    schemas: Vec<String>,
) {
    let mut project = project;
    let mut p = project.write_channel(ProjChan::Connections);
    p.update_connection_def(url, |def| {
        if let Provider::Postgres(pg) = &mut def.provider {
            pg.schemas = schemas;
        }
    });
    persisted_defs(&p, report);
}

/// Mounted at the window root beside the other dialogs, on the same terms: while open, its key
/// barrier precedes every feature listener in document order. Esc cancels, Enter applies.
///
/// **The draft is seeded when the picker opens, and the seed is dropped when it closes** — which
/// is what makes Cancel discard. This dialog is mounted for the window's life, so a draft kept
/// past a close comes back on the next open of the same connection and Apply then commits the
/// edit the user cancelled. Clearing the *seed* rather than the draft keeps the seeding condition
/// one comparison, and re-reads the def, so a schema that appeared or vanished server-side since
/// the last open is picked up too.
#[derive(PartialEq)]
pub struct SchemasPicker {
    pub target: SchemasRequest,
}

impl Component for SchemasPicker {
    fn render(&self) -> impl IntoElement {
        let mut slot = self.target;
        let url = slot.read().clone();
        let engine = use_consume::<EngineCtx>();
        let radio = use_radio::<ProjectState, ProjChan>(ProjChan::Connections);
        let project = use_radio_station::<ProjectState, ProjChan>();
        let report = use_report();
        let roles = use_roles();
        let accent = roles.get(Role::Accent);
        let warning = tones().warning;

        let mut draft = use_state(BTreeSet::<String>::new);
        let mut seeded = use_state(|| None::<String>);

        let Some(url) = url else {
            seeded.set_if_modified(None);
            return rect().into_element();
        };
        let Some((def, pg)) = radio
            .read()
            .connections
            .iter()
            .find(|c| c.def.url() == url)
            .and_then(|c| match &c.def.provider {
                Provider::Postgres(pg) => Some((c.def.clone(), pg.clone())),
                _ => None,
            })
        else {
            slot.set(None);
            return rect().into_element();
        };

        let (offers, live) = offers(&engine, &def, &pg);
        if seeded.peek().as_deref() != Some(url.as_str()) {
            seeded.set(Some(url.clone()));
            draft.set(
                offers
                    .iter()
                    .filter(|o| o.enabled)
                    .map(|o| o.name.clone())
                    .collect(),
            );
        }

        let title = rect()
            .width(Size::fill())
            .vertical()
            .child(Title::new("Schemas").color(roles.get(Role::Text)))
            .child(
                MonoValue::new(url.clone())
                    .color(accent)
                    .text_overflow(TextOverflow::Ellipsis),
            );

        let note = match live {
            true => {
                "Which schemas this connection shows. Every schema stays queryable by name; \
                     this scopes what the tree and completion offer."
            }
            false => {
                "This connection is not connected, so it cannot say which schemas it has. \
                      These are the ones it is set to show."
            }
        };

        let rows = offers.into_iter().map(|offer| {
            let name = offer.name.clone();
            let on = draft.read().contains(&name);
            CheckboxRow::new(offer.name.clone(), on)
                .on_toggle(move |_: Event<PressEventData>| {
                    let mut set = draft.write();
                    if !set.remove(&name) {
                        set.insert(name.clone());
                    }
                })
                .maybe(offer.missing, |row| {
                    row.trailing(
                        rect()
                            .horizontal()
                            .cross_align(Alignment::Center)
                            .spacing(SP_2)
                            .child(Icon::new(IconName::Warning).color(warning).size(13.))
                            .child(Caption::new(MISSING).color(warning)),
                    )
                })
        });

        let apply_now = move || {
            let mut slot = slot;
            apply(
                project,
                report,
                &url,
                draft.peek().iter().cloned().collect(),
            );
            slot.set(None);
        };
        let apply_on_enter = apply_now.clone();

        Dialog::new()
            .on_dismiss(move |()| slot.set(None))
            .on_confirm(move |()| apply_on_enter())
            .header(DialogHeader::new(IconName::Folder, accent, title))
            .body(
                rect()
                    .width(Size::fill())
                    .vertical()
                    .spacing(SP_4)
                    .child(Prose::new(note).color(roles.get(Role::TextMuted)).wrap())
                    .child(
                        ScrollView::new()
                            .height(Size::auto())
                            .max_height(Size::px(220.))
                            .child(rect().width(Size::fill()).vertical().children(rows)),
                    ),
            )
            .action(
                Button::new()
                    .flat()
                    .on_press(move |_| slot.set(None))
                    .child(Control::new("Cancel")),
            )
            .action(
                Button::new()
                    .filled()
                    .on_press(move |_| apply_now())
                    .child(Control::new("Apply")),
            )
            .into_element()
    }
}
