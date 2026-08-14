//! The window's footer — why Save is blocked on the left, Cancel and Save on the right.
//!
//! **This is the only thing in the window that writes anything.** Cancel just closes; nothing is
//! committed until Save.
//!
//! A database connection has a fifth step, and it comes **first**: whatever this machine's
//! keystore owes the password ([`password_ops`]). A def claiming `PgPassword::Keystore` over a
//! keystore that refused the write is a connection nothing can log in with.
//!
//! 1. writes the def onto the shared project store — removing the row it is **moving from**
//!    first, when the edit changed the bucket or the provider and therefore the connection's
//!    identity;
//! 2. persists through the funnel and **gates on the answer** — a connection the project file
//!    never heard about is gone on the next open, which is what P4-15 exists to stop being
//!    silent;
//! 3. drops the object store the old URL registered, the one call the scan driver cannot make:
//!    `engine::store::connect` only ever sees the def it is given, so nothing else would ever
//!    take that store back out (`Engine::disconnect` — the same call Forget makes);
//! 4. asks the project window's one scan driver for a whole-catalog pass, and leaves this window
//!    watching its row ([`super::use_watch_connection`]).
//!
//! **A whole-catalog pass, not a connection-shaped one.** `plan_scan` puts connections in
//! `ScanScope::All` alone, deliberately: the case that needs a re-connect is a region corrected
//! or an `aws sso login` run, which is exactly this window — and every table over the bucket was
//! registered against the store this save replaces, so re-registering the connection alone would
//! leave them answering from a store that is no longer there. Save is the ↻ the user would
//! otherwise press, with the def written first.

use freya::prelude::*;
use freya::radio::{use_radio_station, RadioStation};
use strata_core::engine::db::PG_PASSWORD;
use strata_core::secret::{migrate_derived, Secret, SecretError, SecretRef};
use strata_model::{check_catalog_name, ConnectionDef, Provider};

use crate::apps::connection::{ConnectionCtx, ConnectionTarget, Status};
use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::{log_event, use_report, LogLevel, ReportCtx};
use crate::apps::project::{
    persisted_defs, refresh_catalog, Catalog, CatalogRescan, ProjChan, ProjectState,
};
use crate::components::divider::Divider;
use crate::components::form::form_theme;
use crate::components::metrics::ACTION_HEIGHT;
use crate::components::metrics::{SP_4, SP_5};
use crate::components::typography::{Control, Path};
use crate::components::window::window_theme;
use crate::task::offload;

/// The strip's inset (canvas `padding: var(--sp-4) var(--sp-5)`).
const FOOTER_PADDING: Gaps = Gaps::new(SP_4, SP_5, SP_4, SP_5);

#[derive(PartialEq)]
pub struct Footer;

impl Component for Footer {
    fn render(&self) -> impl IntoElement {
        let win = window_theme();
        let form = form_theme();
        let ctx = use_consume::<ConnectionCtx>();
        let project = use_radio_station::<ProjectState, ProjChan>();
        let rescan = use_consume::<CatalogRescan>();
        let catalog = use_consume::<Catalog>();
        let engine = use_consume::<EngineCtx>();
        let report = use_report();
        let platform = use_hook(Platform::get);

        let busy = match *ctx.status.read() {
            Status::Storing => Some("Saving…"),
            Status::Connecting(_) => Some("Connecting…"),
            Status::Idle | Status::Failed(_) => None,
        };
        let scanning = catalog.read().is_scanning();
        let note = save_note(
            ctx.draft
                .read()
                .blocker()
                .or_else(|| url_clash(ctx, project))
                .or_else(|| catalog_clash(ctx, project)),
            scanning,
        );

        let cancel = {
            Button::new()
                .height(Size::px(ACTION_HEIGHT))
                .on_press(move |_: Event<PressEventData>| platform.close_current_window())
                .child(Control::new("Cancel"))
        };

        let save = Button::new()
            .filled()
            .height(Size::px(ACTION_HEIGHT))
            .enabled(busy.is_none() && note.is_none())
            .on_press({
                move |_: Event<PressEventData>| save(ctx, project, rescan, engine.clone(), report)
            })
            .child(Control::new(busy.unwrap_or("Save")));

        rect()
            .width(Size::fill())
            .vertical()
            .child(Divider::horizontal().color(win.border_fill))
            .child(
                rect()
                    .width(Size::fill())
                    .horizontal()
                    .content(Content::Flex)
                    .cross_align(Alignment::Center)
                    .spacing(SP_4)
                    .padding(FOOTER_PADDING)
                    .background(win.background)
                    .child(
                        rect().width(Size::flex(1.)).maybe_child(
                            note.filter(|_| busy.is_none()).map(|why| {
                                Path::new(why).color(form.hint_color).max_lines(2).wrap()
                            }),
                        ),
                    )
                    .child(cancel)
                    .child(save),
            )
    }
}

/// The one line the footer shows about why Save is off — and **the same value that disables it**,
/// so the button and its explanation cannot disagree.
///
/// `blocker` comes **first**, ahead of the re-scan, for the Configure footer's reason: a note
/// should name the next thing the user can *do*, and a blank region is fixable now while the scan
/// will very likely settle while they are fixing it.
fn save_note(blocker: Option<String>, scanning: bool) -> Option<String> {
    blocker.or_else(|| {
        scanning
            .then(|| "The catalog is being re-scanned. Save is available when it settles.".into())
    })
}

/// The one blocker the draft cannot see: a URL another connection already holds.
///
/// `upsert_connection` replaces on `url()`, so without this an edit that moved a bucket onto an
/// existing connection's would silently take that connection's def out from under it — the same
/// hazard the Configure window's name clash guards, one key along. On an edit the connection's
/// own URL does not clash with itself.
///
/// Matched case-**sensitively**, like every other connection lookup: a URL is not a SQL
/// identifier, and the object-store registry matches it verbatim.
fn url_clash(ctx: ConnectionCtx, project: RadioStation<ProjectState, ProjChan>) -> Option<String> {
    let url = ctx.draft.read().def().url();
    if ctx.target.read().editing() == Some(url.as_str()) {
        return None;
    }
    project
        .peek()
        .connections
        .iter()
        .any(|c| c.def.url() == url)
        .then(|| format!("'{url}' is already a connection in this project."))
}

/// The other blocker the draft cannot see: a **catalog name** another database connection in this
/// project already registers under.
///
/// `check_catalog_name` rather than a comparison written here, so the field refuses what
/// `engine::db::connect` refuses, in the same words. It folds, unlike [`url_clash`] beside it,
/// because a catalog name is a SQL identifier. The set is the project's *stored* defs where the
/// engine's is what is live — a connection that failed to connect reserves nothing.
fn catalog_clash(
    ctx: ConnectionCtx,
    project: RadioStation<ProjectState, ProjChan>,
) -> Option<String> {
    let def = ctx.draft.read().def();
    let existing: Vec<ConnectionDef> = project
        .peek()
        .connections
        .iter()
        .map(|c| c.def.clone())
        .collect();
    check_catalog_name(&existing, &def).err()
}

/// One thing Save owes this machine's keystore. A list rather than a verdict because the *order*
/// is the content of the answer: a migration has to reach the new slot before a put lands on it.
#[derive(Clone, PartialEq, Debug)]
enum PasswordOp {
    /// The identity moved, so the derived reference moved with it.
    Migrate(SecretRef, SecretRef),
    Put(SecretRef, Secret),
    /// *Remove from this machine*, the "uses no password" edit, or a connection that is no longer
    /// a database — in which case nothing would ever name the entry again.
    Delete(SecretRef),
}

/// Plan a save of `next` over `previous`, the def as this window opened it.
///
/// Pure, and called from Save rather than from `def()`: `blocker` assembles a def per keystroke,
/// so a keystore write there would be a platform call — on macOS a Keychain prompt — per frame.
fn password_ops(
    previous: Option<&ConnectionDef>,
    next: &ConnectionDef,
    typed: &str,
    removed: bool,
) -> Vec<PasswordOp> {
    let slot = |def: &ConnectionDef| SecretRef::derived(PG_PASSWORD, &def.url());
    let was = previous.filter(|def| matches!(def.provider, Provider::Postgres(_)));
    let mut ops = Vec::new();

    let Provider::Postgres(_) = next.provider else {
        ops.extend(was.map(|def| PasswordOp::Delete(slot(def))));
        return ops;
    };

    if let Some(old) = was.filter(|def| def.url() != next.url()) {
        ops.push(PasswordOp::Migrate(slot(old), slot(next)));
    }
    match Secret::new(typed) {
        Some(secret) => ops.push(PasswordOp::Put(slot(next), secret)),
        None if removed => ops.push(PasswordOp::Delete(slot(next))),
        None => {}
    }
    ops
}

/// Carry out `ops` in order, stopping at the first refusal. Blocking, so it runs on a worker; a
/// keystore that refuses is reported and the save does not happen, never answered by writing the
/// password somewhere else.
fn run_password_ops(ops: &[PasswordOp]) -> Result<(), SecretError> {
    for op in ops {
        match op {
            PasswordOp::Migrate(from, to) => migrate_derived(from, to)?,
            PasswordOp::Put(slot, secret) => slot.put(secret)?,
            PasswordOp::Delete(slot) => slot.delete()?,
        }
    }
    Ok(())
}

/// Write the def, persist it, drop what the old URL registered, and ask for the pass. See the
/// module doc.
///
/// **The bucket is not probed here**, and that is a decision with a measurement behind it. A
/// Save-time `Engine::check_connection` was built first, to refuse an unreachable bucket before
/// anything was written. It was withdrawn for two reasons that only showed up once it existed.
///
/// It is **redundant**: `store::connect` now asks the bucket itself, so the pass this Save asks
/// for already answers the same question, and this window already watches that row — `Failed`
/// keeps it open carrying the engine's own words (see the module doc). The probe was a second
/// round trip to learn what the first one was about to say, and the def being written in between
/// is exactly what already happens for a credential chain the server rejects.
///
/// And it was **expensive in the wrong place**: it put a network call with `object_store`'s ten
/// retries behind a button that three interaction tests press, taking this crate's suite from 7
/// seconds to 308. A UI test that dials out to a bucket nobody owns is a bad trade for a refusal
/// arriving a second earlier.
fn save(
    mut ctx: ConnectionCtx,
    project: RadioStation<ProjectState, ProjChan>,
    rescan: CatalogRescan,
    engine: EngineCtx,
    report: ReportCtx,
) {
    let def = ctx.draft.peek().def();
    let previous = ctx.target.peek().editing().and_then(|url| {
        project
            .peek()
            .connections
            .iter()
            .find(|c| c.def.url() == url)
            .map(|row| row.def.clone())
    });
    let ops = password_ops(
        previous.as_ref(),
        &def,
        &ctx.password.peek(),
        *ctx.password_removed.peek(),
    );
    if ops.is_empty() {
        commit(ctx, project, rescan, engine, report);
        return;
    }

    ctx.status.set(Status::Storing);
    spawn(async move {
        let landed = offload(move || run_password_ops(&ops)).await;
        match landed {
            Some(Ok(())) => commit(ctx, project, rescan, engine, report),
            Some(Err(why)) => ctx.status.set(Status::Failed(format!(
                "The password could not be written to this machine's keystore, so nothing was \
                 saved. {why}"
            ))),
            None => ctx.status.set(Status::Failed(
                "The password could not be written to this machine's keystore: a worker did not \
                 answer. Nothing was saved."
                    .into(),
            )),
        }
    });
}

/// The rest of Save, once this machine's keystore is in the state the def is about to claim.
///
/// Split out rather than inlined behind an `await`, so a save with no password work stays
/// synchronous: made asynchronous for everyone, the interaction tests that press this button
/// would assert a frame that has not happened yet.
fn commit(
    mut ctx: ConnectionCtx,
    mut project: RadioStation<ProjectState, ProjChan>,
    rescan: CatalogRescan,
    engine: EngineCtx,
    report: ReportCtx,
) {
    let def = ctx.draft.peek().def();
    let url = def.url();
    let moved_from = ctx
        .target
        .peek()
        .editing()
        .filter(|old| *old != url)
        .map(str::to_string);

    let landed = {
        let mut p = project.write_channel(ProjChan::Connections);
        if let Some(old) = &moved_from {
            p.remove_connection(old);
        }
        p.upsert_connection(def);
        persisted_defs(&p, report)
    };
    if !landed {
        ctx.status.set(Status::Failed(
            "The connection is registered, but the project file could not be written, so it will \
             be gone when this project is reopened."
                .into(),
        ));
    } else {
        ctx.status.set(Status::Connecting(url.clone()));
    }
    {
        let mut target = ctx.target;
        target.set(ConnectionTarget::Edit(url.clone()));
    }

    if let Some(old) = &moved_from {
        engine.disconnect(old);
        log_event(
            report.log,
            LogLevel::Info,
            format!("Moved connection '{old}' to '{url}'"),
        );
    }

    refresh_catalog(rescan);
}

#[cfg(test)]
mod tests {
    use strata_model::{PgPassword, PgStore, S3Store};

    use super::*;

    fn pg(address: &str, user: &str) -> ConnectionDef {
        ConnectionDef {
            address: address.into(),
            provider: Provider::Postgres(PgStore {
                catalog: "warehouse".into(),
                user: user.into(),
                password: PgPassword::Keystore,
                ..Default::default()
            }),
            client_config: Default::default(),
        }
    }

    fn slot(def: &ConnectionDef) -> SecretRef {
        SecretRef::derived(PG_PASSWORD, &def.url())
    }

    /// **Nothing typed, nothing moved, nothing pressed is no keystore call at all**, so an
    /// ordinary Save never raises a Keychain prompt for a password nobody touched.
    #[test]
    fn a_save_that_touches_no_password_asks_the_keystore_nothing() {
        let def = pg("db:5432/analytics", "reader");
        assert_eq!(password_ops(Some(&def), &def, "", false), []);
        assert_eq!(
            password_ops(None, &def, "  ", false),
            [],
            "blank is nothing"
        );

        let s3 = ConnectionDef {
            address: "acme-lake".into(),
            provider: Provider::S3(S3Store::default()),
            client_config: Default::default(),
        };
        assert_eq!(password_ops(Some(&s3), &s3, "hunter2", true), []);
    }

    /// **An identity move migrates the entry, and the migration comes before the put.** The
    /// reference is derived from the URL, so an edit to the address or the user moves the slot;
    /// run the other way round, a save that moved the identity *and* typed a new password would
    /// carry the old one over it.
    #[test]
    fn a_moved_identity_migrates_before_anything_lands_on_the_new_slot() {
        let old = pg("db:5432/analytics", "reader");
        let new = pg("db:5432/analytics", "writer");
        assert_eq!(
            password_ops(Some(&old), &new, "", false),
            [PasswordOp::Migrate(slot(&old), slot(&new))]
        );

        let ops = password_ops(Some(&old), &new, "hunter2", false);
        assert_eq!(
            ops,
            [
                PasswordOp::Migrate(slot(&old), slot(&new)),
                PasswordOp::Put(slot(&new), Secret::new("hunter2").unwrap()),
            ]
        );

        let moved_address = pg("db:5433/analytics", "reader");
        assert_eq!(
            password_ops(Some(&old), &moved_address, "", false),
            [PasswordOp::Migrate(slot(&old), slot(&moved_address))],
            "the address is the other half of the identity"
        );
    }

    /// **A typed password is trimmed to nothing or stored, never both** — `Secret::new`'s fork, so
    /// a cleared box cannot become an empty stored password.
    #[test]
    fn a_typed_password_is_stored_under_the_connections_own_slot() {
        let def = pg("db:5432/analytics", "reader");
        assert_eq!(
            password_ops(Some(&def), &def, " hunter2 ", false),
            [PasswordOp::Put(slot(&def), Secret::new("hunter2").unwrap())]
        );
        assert_eq!(
            password_ops(Some(&def), &def, "hunter2", true),
            [PasswordOp::Put(slot(&def), Secret::new("hunter2").unwrap())],
            "typing over a pending removal is the password you meant"
        );
    }

    /// **Every way of abandoning a password deletes this machine's entry**, including switching
    /// the provider away from the database arm — which drops `PgPassword` from the def entirely,
    /// so nothing would ever name that slot again.
    #[test]
    fn an_abandoned_password_is_deleted_from_this_machine() {
        let def = pg("db:5432/analytics", "reader");
        assert_eq!(
            password_ops(Some(&def), &def, "", true),
            [PasswordOp::Delete(slot(&def))],
            "remove from this machine"
        );

        let unused = ConnectionDef {
            provider: Provider::Postgres(PgStore {
                catalog: "warehouse".into(),
                user: "reader".into(),
                password: PgPassword::None,
                ..Default::default()
            }),
            ..def.clone()
        };
        assert_eq!(
            password_ops(Some(&def), &unused, "", true),
            [PasswordOp::Delete(slot(&def))],
            "this connection uses no password — the same slot, since the identity did not move"
        );

        let s3 = ConnectionDef {
            address: "acme-lake".into(),
            provider: Provider::S3(S3Store::default()),
            client_config: Default::default(),
        };
        assert_eq!(
            password_ops(Some(&def), &s3, "", false),
            [PasswordOp::Delete(slot(&def))],
            "and the slot deleted is the one the old def named, not the new URL's"
        );
    }

    #[test]
    fn an_actionable_blocker_outranks_the_re_scan() {
        let blocker = || Some("An S3 connection needs a region.".to_string());
        assert_eq!(save_note(blocker(), true), blocker());
        assert_eq!(save_note(blocker(), false), blocker());
    }

    #[test]
    fn a_re_scan_is_explained_once_it_is_the_only_thing_left() {
        let note = save_note(None, true).expect("a scanning footer says why");
        assert!(note.contains("re-scanned"), "{note}");
    }

    #[test]
    fn nothing_to_say_when_save_is_available() {
        assert_eq!(save_note(None, false), None);
    }
}
