//! The data source's **Schemas…** picker (DB-05) — which of a data source's schemas the
//! tree, the inspector and completion show.
//!
//! **No reconnect**, and that is what makes this dialog legitimate at all. Registration exposes
//! every schema the data source can reach; [`SourceDef::schemas`] scopes what Strata *shows*, so a
//! change here rebuilds no pool, invalidates no plan and cannot break a query that already names
//! a schema this list leaves out.
//!
//! It is **not** display only, and has not been since DB-09: an unqualified name searches the
//! schemas a data source shows, so this press moves what `orders` means. Two things follow, and
//! both are Apply's ([`apply`]) — the session is told
//! ([`Sources::show_schemas`](strata_engine::Sources::show_schemas)), and the **catalog
//! generation** the window holds is re-read, because diagnostics are a reconciliation against that
//! number and completion's snapshot
//! is keyed on it. Without the bump the tree redraws while every open tab keeps the verdict it
//! had, and the popup goes on offering names that have stopped resolving.
//!
//! Which is also why the write is **not** `upsert_source`: that replaces the def at its sorted
//! slot, on a key this edit must not move, and asks for a registration nothing here needs. The
//! store's def-in-place write ([`ProjectState::update_source_def`]) leaves the engine's verdict
//! for this data source exactly where it was, which is still true.
//!
//! The offer is [`Sources::listing`](strata_engine::Sources::listing)'s **scoped and tagged**
//! answer and nothing derived beside it, so this picker, the tree and completion cannot disagree
//! about what a data source shows — and this is the one surface that sees a schema the data source
//! does *not* show, taking one back being what it is for. A data source that is not live has no
//! enumeration to offer: the picker then lists the def's own schemas with the data source's failure
//! named, rather than an unexplained empty list.

use freya::components::ScrollView;
use freya::prelude::*;
use freya::radio::{use_radio, use_radio_station, RadioStation};
use std::collections::BTreeSet;

use strata_engine::sources::{SchemaVisibility, SourceDetail};
use strata_model::SourceDef;

use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::state::{
    catalog_settled, persisted_defs, use_catalog, use_registrations, use_report, Catalog, ProjChan,
    ProjectState, RegistrationsCtx, ReportCtx,
};
use crate::components::dialog::{CheckboxRow, Dialog, DialogHeader};
use crate::components::icon::{Icon, IconName};
use crate::components::metrics::{SP_2, SP_4};
use crate::components::tones::tones;
use crate::components::typography::{Caption, Control, MonoValue, Prose, Title};
use crate::theme::{use_roles, Role};

/// What a schema the def enables and the server does not have says on its row.
const MISSING: &str = "not in the data source";

/// The slot a trigger sets to ask for the picker — a data source's own name.
/// Provided at the project root, like every other dialog's.
pub type SchemasRequest = State<Option<String>>;

/// One row of the picker: a schema, whether it is enabled, and what the data source says about it.
#[derive(Clone, PartialEq)]
struct Offer {
    name: String,
    enabled: bool,
    /// The server does not have it (or the role cannot see it) — enabled, and answering nothing.
    missing: bool,
}

/// What the picker offers for the data source called `name`, and whether the offer came from the
/// data source itself.
///
/// Live: the data source's own tagged enumeration, off the one snapshot every surface reads —
/// **including the schemas it does not show**, which is what this dialog is for and what no other
/// surface may draw. Not live: the def's `schemas`, so the user can still take one off a
/// data source that is refusing to connect *because* of it.
fn offers(engine: &EngineCtx, name: &str, source: &SourceDef) -> (Vec<Offer>, bool) {
    let listing = engine.sources().listing();
    let live = listing.source(name).filter(|source| source.live());
    let Some(SourceDetail::Catalog { schemas, .. }) = live.map(|source| &source.detail) else {
        let offers = source
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
        .iter()
        .map(|schema| Offer {
            name: schema.name.clone(),
            enabled: schema.visibility != SchemaVisibility::NotEnabled,
            missing: schema.visibility == SchemaVisibility::EnabledButMissing,
        })
        .collect();
    (offers, true)
}

/// Write `schemas` onto the data source's def, tell the session, persist, and adopt the engine's
/// catalog generation — the whole of what Apply does.
///
/// Both of the last two are because this press moves what an unqualified name resolves to: the
/// session learns the new set without a reconnect (`Sources::show_schemas`), and the surfaces that
/// answer about names re-derive on that number and nothing else — every tab's diagnostics through
/// `stale_tabs`, and the completion snapshot through its key. The discrete catalog mutation
/// [`catalog_settled`] exists for, exactly as a Forget is.
fn apply(
    project: RadioStation<ProjectState, ProjChan>,
    engine: &EngineCtx,
    catalog: Catalog,
    registrations: RegistrationsCtx,
    report: ReportCtx,
    url: &str,
    schemas: Vec<String>,
) {
    let mut project = project;
    {
        let mut p = project.write_channel(ProjChan::Sources);
        p.update_source_def(url, |def| def.schemas.clone_from(&schemas));
        engine.sources().show_schemas(url, &schemas);
        persisted_defs(&p, report);
    }
    catalog_settled(catalog, registrations, engine);
}

/// Mounted at the window root beside the other dialogs, on the same terms: while open, its key
/// barrier precedes every feature listener in document order. Esc cancels, Enter applies.
///
/// **The draft is seeded when the picker opens, and the seed is dropped when it closes** — which
/// is what makes Cancel discard. This dialog is mounted for the window's life, so a draft kept
/// past a close comes back on the next open of the same data source and Apply then commits the
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
        let catalog = use_catalog();
        let registrations = use_registrations();
        let radio = use_radio::<ProjectState, ProjChan>(ProjChan::Sources);
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
        let Some(source) = radio
            .read()
            .sources
            .iter()
            .find(|c| c.named() == url)
            .cloned()
        else {
            slot.set(None);
            return rect().into_element();
        };

        let (offers, live) = offers(&engine, &url, &source);
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
                "Which schemas this data source shows. Every schema stays queryable by name; \
                     this scopes what the tree and completion offer."
            }
            false => {
                "This data source is not connected, so it cannot say which schemas it has. \
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
                &engine,
                catalog,
                registrations,
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use strata_engine::sources::postgres::Pg;
    use strata_engine::{CatalogGen, Registrations, SourceKind};

    use freya::radio::RadioStation;
    use freya_testing::TestingRunner;
    use strata_core::project::ProjectDefs;
    use strata_core::theme::load;
    use strata_model::SourceDef;

    use super::*;
    use crate::apps::project::state::{CatalogState, Log, PersistFaults};
    use crate::theme::strata_theme;

    fn temp_root() -> PathBuf {
        std::env::temp_dir().join(format!("strata-schemas-{}", std::process::id()))
    }

    /// One data source showing both of its schemas.
    fn source() -> SourceDef {
        SourceDef {
            name: "analytics".into(),
            kind: Pg::NAME.to_string(),
            config: [("user", "reader"), ("sslmode", "disable")]
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            schemas: vec!["public".into(), "analytics".into()],
            ..Default::default()
        }
    }

    fn app() -> impl IntoElement {
        use_init_theme(|| strata_theme(&load("midnight")));
        let target = use_consume::<SchemasRequest>();
        rect().expanded().child(SchemasPicker { target })
    }

    type Handles = (
        SchemasRequest,
        RadioStation<ProjectState, ProjChan>,
        Catalog,
        EngineCtx,
    );

    fn runner() -> (TestingRunner, Handles) {
        let root = temp_root();
        let conn = source();
        TestingRunner::new(
            app,
            (900., 700.).into(),
            move |r| {
                let engine = r.provide_root_context(EngineCtx::default);
                let catalog = r.provide_root_context(|| {
                    State::create(CatalogState::Settled(CatalogGen::default()))
                });
                r.provide_root_context(|| State::create(Registrations::default()));
                let target = r.provide_root_context(|| State::create(None::<String>));
                let project = r.provide_root_context(|| {
                    let defs = ProjectDefs {
                        name: "test".into(),
                        sources: vec![conn.clone()],
                        ..ProjectDefs::default()
                    };
                    RadioStation::<ProjectState, ProjChan>::create(ProjectState::from_defs(
                        defs,
                        root.clone(),
                    ))
                });
                r.provide_root_context(|| State::create(Log::default()));
                r.provide_root_context(|| State::create(PersistFaults::default()));
                (target, project, catalog, engine)
            },
            1.,
        )
    }

    fn texts(runner: &TestingRunner) -> Vec<String> {
        runner.find_many(|_, element| Label::try_downcast(element).map(|l| l.text.to_string()))
    }

    fn click_text(runner: &mut TestingRunner, text: &str) {
        let area = runner
            .find_many(|node, element| {
                Label::try_downcast(element)
                    .filter(|l| l.text == text)
                    .map(|_| node.layout().area)
            })
            .into_iter()
            .max_by(|a, b| a.min_y().total_cmp(&b.min_y()))
            .unwrap_or_else(|| panic!("no text run {text:?} in the tree: {:?}", texts(runner)));
        let point = (
            (area.min_x() + area.width() / 2.) as f64,
            (area.min_y() + area.height() / 2.) as f64,
        );
        runner.move_cursor(point);
        runner.click_cursor(point);
        runner.sync_and_update();
        runner.sync_and_update();
    }

    /// **Applying moves the catalog generation** (DB-09) — without it the tree redraws while
    /// every open tab keeps squiggling against the schemas that were shown a moment ago.
    #[test]
    fn applying_a_schema_change_moves_the_catalog_generation() {
        let (mut runner, (mut target, project, catalog, engine)) = runner();
        runner.sync_and_update();
        target.set(Some(source().named()));
        runner.sync_and_update();
        runner.sync_and_update();

        click_text(&mut runner, "analytics");
        click_text(&mut runner, "Apply");

        assert_eq!(
            project.peek().sources[0].clone(),
            SourceDef {
                schemas: vec!["public".into()],
                ..source()
            },
            "the def keeps only the schemas still ticked"
        );
        assert_eq!(
            catalog.peek().generation(),
            Some(engine.catalog().generation()),
            "the window is at the number the engine minted"
        );
        assert_ne!(
            catalog.peek().generation(),
            Some(CatalogGen::default()),
            "and it moved, so the surfaces that answer about names re-derive"
        );
    }
}
