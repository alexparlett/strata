//! The window's footer — why Save is blocked on the left, Cancel and Save on the right.
//!
//! **This is the only thing in the window that writes anything.** Cancel just closes; nothing is
//! committed until Save.
//!
//! A source connection has a fifth step, and it comes **first**: whatever this machine's keystore
//! owes the secrets its kind declares ([`secret_ops`]). A def that expects one over a keystore
//! that refused the write is a connection nothing can log in with.
//!
//! 1. writes the def onto the shared project store — removing the row it is **moving from**
//!    first, when the edit changed the bucket or the provider and therefore the connection's
//!    identity;
//! 2. persists through the funnel and **gates on the answer** — a connection the project file
//!    never heard about is gone on the next open, which is what P4-15 exists to stop being
//!    silent;
//! 3. drops the object store the old URL registered, the one call the scan driver cannot make:
//!    `engine::sources::store::connect` only ever sees the def it is given, so nothing else would ever
//!    take that store back out (`Connections::disconnect` — the same call Forget makes);
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

use std::collections::{BTreeMap, BTreeSet};

use strata_engine::sources::{forget_secret, forget_secrets, migrate_secrets, put_secret};
use strata_model::{check_catalog_name, SourceDef};

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
                .or_else(|| address_refusal(ctx, &engine))
                .or_else(|| name_clash(ctx, project))
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

/// The blocker the draft cannot see: whether the **kind** accepts this address.
///
/// What an address means is the source's own rule, so it is asked of the registry rather than
/// re-stated here — the same reason the two clash checks below are the store's question rather
/// than the draft's. A kind nothing is registered for says so, which is the honest answer for a
/// def naming a source this build does not have.
fn address_refusal(ctx: ConnectionCtx, engine: &EngineCtx) -> Option<String> {
    let def = ctx.draft.read().def();
    let kind = def.kind.clone();
    engine
        .sources()
        .check_address(&kind, def.setting("address"))
        .err()
        .map(|why| why.to_string())
}

/// The blocker the draft cannot see: a name another connection already holds.
///
/// `upsert_connection` replaces on the name, so without this an edit that renamed one connection
/// onto another's would silently take that connection's def out from under it — the same hazard
/// the Configure window's name clash guards, one section along. On an edit the connection's own
/// name does not clash with itself.
///
/// Matched case-**sensitively**, unlike [`catalog_clash`] below: a connection is addressed by
/// gesture, and the spelling the user typed is the one every surface shows back.
fn name_clash(ctx: ConnectionCtx, project: RadioStation<ProjectState, ProjChan>) -> Option<String> {
    let name = ctx.draft.read().def().named();
    if ctx.target.read().editing() == Some(name.as_str()) {
        return None;
    }
    project
        .peek()
        .connections
        .iter()
        .any(|c| c.def.named() == name)
        .then(|| format!("'{name}' is already a connection in this project."))
}

/// The other blocker the draft cannot see: a **catalog name** another database connection in this
/// project already registers under.
///
/// [`check_catalog_name`] rather than a comparison written here, so the field refuses what
/// registration refuses, in the same words. It folds, unlike [`name_clash`] beside it,
/// because a catalog name is a SQL identifier. The set is the project's *stored* defs where the
/// engine's is what is live — a connection that failed to connect reserves nothing.
///
/// **The row this window opened on is dropped first**, exactly as [`name_clash`] drops it.
/// `check_catalog_name` skips the candidate by comparing URLs, and a database connection's URL
/// carries its user — so editing the USER, the URL or the DATABASE moves the identity, the stale
/// row stops matching, and the draft clashes with the connection it is replacing. The footer then
/// quotes that connection's own old URL back and Save never re-enables.
fn catalog_clash(
    ctx: ConnectionCtx,
    project: RadioStation<ProjectState, ProjChan>,
) -> Option<String> {
    let def = ctx.draft.read().def();
    let editing = ctx.target.read().editing().map(str::to_string);
    let existing: Vec<SourceDef> = project
        .peek()
        .connections
        .iter()
        .filter(|c| editing.as_deref() != Some(c.def.named().as_str()))
        .map(|c| c.def.clone())
        .collect();
    check_catalog_name(&existing, &def).err()
}

/// What Save owes this machine's keystore, as one call the engine performs.
///
/// A plan rather than a verdict because the *order* is the content of the answer: a rename has to
/// reach the new slots before a put lands on one. Where a slot is and what a rename does to it are
/// the engine's — this says only which of these things happened to which box.
#[derive(Clone, PartialEq, Eq, Debug)]
enum SecretOp {
    /// The connection was renamed, so its secrets move with it.
    Rename,
    /// A value was typed into `key`'s box.
    Put { key: String, value: String },
    /// *Remove from this machine*, or the "uses no …" edit, for one declared key.
    Forget { key: String },
    /// Everything the **previous** def kept here: a connection that is no longer a source, or one
    /// whose kind moved — in which case nothing would ever name the old slots again, since the
    /// family is the kind.
    ForgetAll,
}

/// Plan a save of `next` over `previous`, the def as this window opened it.
///
/// Pure, and called from Save rather than from `def()`: `blocker` assembles a def per keystroke,
/// so a keystore write there would be a platform call — on macOS a Keychain prompt — per frame.
///
/// Every key either the boxes or the def has an opinion about is walked in one order, so a put
/// and a forget for one key cannot both be planned.
fn secret_ops(
    previous: Option<&SourceDef>,
    next: &SourceDef,
    typed: &BTreeMap<String, String>,
    removed: &BTreeSet<String>,
) -> Vec<SecretOp> {
    // Only a def that actually **holds** something has anything to move or forget. A rename or a
    // dropped provider is a keystore operation for a connection with secrets and nothing at all
    // for one without — and planning it anyway sends every such save down the worker path, which
    // on macOS is a Keychain prompt raised over an empty slot.
    let was = previous.filter(|def| !def.secrets.is_empty());

    let mut ops = Vec::new();
    match was.map(|def| def.kind.trim()) {
        Some(kind) if kind != next.kind.trim() => ops.push(SecretOp::ForgetAll),
        _ if was.is_some_and(|def| def.named() != next.named()) => ops.push(SecretOp::Rename),
        _ => {}
    }

    let mut keys: BTreeSet<&str> = next.secrets.iter().map(String::as_str).collect();
    keys.extend(typed.keys().map(String::as_str));
    keys.extend(removed.iter().map(String::as_str));
    for key in keys {
        match typed.get(key).map(|value| value.trim()).unwrap_or_default() {
            "" if removed.contains(key) => ops.push(SecretOp::Forget {
                key: key.to_string(),
            }),
            "" => {}
            value => ops.push(SecretOp::Put {
                key: key.to_string(),
                value: value.to_string(),
            }),
        }
    }
    ops
}

/// Carry `ops` out in order, stopping at the first refusal. Blocking, so it runs on a worker; a
/// keystore that refuses is reported and the save does not happen, never answered by writing the
/// secret somewhere else.
fn run_secret_ops(
    ops: &[SecretOp],
    previous: Option<&SourceDef>,
    next: &SourceDef,
) -> Result<(), String> {
    for op in ops {
        match op {
            SecretOp::Rename => {
                if let Some(was) = previous {
                    migrate_secrets(was, next)?;
                }
            }
            SecretOp::Put { key, value } => put_secret(next, key, value)?,
            SecretOp::Forget { key } => forget_secret(next, key)?,
            SecretOp::ForgetAll => forget_secrets(previous.unwrap_or(next))?,
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
    let previous = ctx.target.peek().editing().and_then(|name| {
        project
            .peek()
            .connections
            .iter()
            .find(|c| c.def.named() == name)
            .map(|row| row.def.clone())
    });
    let ops = secret_ops(
        previous.as_ref(),
        &def,
        &ctx.secret_values.peek(),
        &ctx.secret_removed.peek(),
    );
    if ops.is_empty() {
        commit(ctx, project, rescan, engine, report);
        return;
    }

    ctx.status.set(Status::Storing);
    spawn(async move {
        let landed = offload(move || run_secret_ops(&ops, previous.as_ref(), &def)).await;
        match landed {
            Some(Ok(())) => commit(ctx, project, rescan, engine, report),
            Some(Err(why)) => ctx.status.set(Status::Failed(format!(
                "This machine's keystore could not be written, so nothing was saved. {why}"
            ))),
            None => ctx.status.set(Status::Failed(
                "This machine's keystore could not be written: a worker did not answer. Nothing \
                 was saved."
                    .into(),
            )),
        }
    });
}

/// The rest of Save, once this machine's keystore is in the state the def is about to claim.
///
/// Split out rather than inlined behind an `await`, so a save with no keystore work stays
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
    let name = def.named();
    let moved_from = ctx
        .target
        .peek()
        .editing()
        .filter(|old| *old != name)
        .map(str::to_string);

    let landed = {
        let mut p = project.write_channel(ProjChan::Connections);
        match &moved_from {
            Some(old) => p.rename_connection(old, def),
            None => p.upsert_connection(def),
        }
        persisted_defs(&p, report)
    };
    if !landed {
        ctx.status.set(Status::Failed(
            "The connection is registered, but the project file could not be written, so it will \
             be gone when this project is reopened."
                .into(),
        ));
    } else {
        ctx.status.set(Status::Connecting(name.clone()));
    }
    {
        let mut target = ctx.target;
        target.set(ConnectionTarget::Edit(name.clone()));
    }

    if let Some(old) = &moved_from {
        engine.sources().disconnect(old);
        log_event(
            report.log,
            LogLevel::Info,
            format!("Moved connection '{old}' to '{name}'"),
        );
    }

    refresh_catalog(rescan);
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use strata_model::SourceDef;

    use super::*;

    /// One source def: a kind, a name and the expectation of a `password`.
    fn source(name: &str, address: &str, kind: &str) -> SourceDef {
        SourceDef {
            config: [("address".to_string(), address.into())]
                .into_iter()
                .collect(),
            name: name.into(),
            kind: kind.into(),
            secrets: BTreeSet::from(["password".to_string()]),
            ..Default::default()
        }
    }

    fn store() -> SourceDef {
        SourceDef {
            config: [("address".to_string(), "acme-lake".into())]
                .into_iter()
                .collect(),
            kind: "s3".into(),
            name: "acme_lake".into(),
            ..Default::default()
        }
    }

    fn typed(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    fn removed(keys: &[&str]) -> BTreeSet<String> {
        keys.iter().map(|k| (*k).to_string()).collect()
    }

    /// **A connection that holds no secret is never a keystore call**, whatever moved — so
    /// renaming one, or changing its kind, raises no Keychain prompt and keeps Save synchronous.
    #[test]
    fn a_connection_with_no_secrets_is_never_a_keystore_call() {
        let bare = |name: &str, kind: &str| SourceDef {
            config: [("address".to_string(), "db:5432/analytics".into())]
                .into_iter()
                .collect(),
            name: name.into(),
            kind: kind.into(),
            ..Default::default()
        };
        let held = bare("warehouse", "test");
        for next in [bare("depot", "test"), bare("warehouse", "other"), store()] {
            assert_eq!(
                secret_ops(Some(&held), &next, &BTreeMap::new(), &BTreeSet::new()),
                [],
                "nothing is stored, so there is nothing to move or forget"
            );
        }
    }

    /// **Nothing typed, nothing moved, nothing pressed is no keystore call at all**, so an
    /// ordinary Save never raises a Keychain prompt for a secret nobody touched.
    #[test]
    fn a_save_that_touches_no_secret_asks_the_keystore_nothing() {
        let def = source("warehouse", "db:5432/analytics", "test");
        assert_eq!(
            secret_ops(Some(&def), &def, &BTreeMap::new(), &BTreeSet::new()),
            []
        );
        assert_eq!(
            secret_ops(None, &def, &typed(&[("password", "  ")]), &BTreeSet::new()),
            [],
            "blank is nothing"
        );

        // A kind that declares no secret has no box to type one into, so the map is empty by
        // construction — the editor keys it by the settings it drew.
        let s3 = store();
        assert_eq!(
            secret_ops(Some(&s3), &s3, &BTreeMap::new(), &BTreeSet::new()),
            []
        );
    }

    /// **A rename moves the entries, and the move comes before the put.** The slot is derived from
    /// the connection's name, so renaming one moves it; run the other way round, a save that
    /// renamed *and* typed a new value would carry the old one over it.
    #[test]
    fn a_rename_moves_the_entries_before_anything_lands_on_the_new_slots() {
        let old = source("warehouse", "db:5432/analytics", "test");
        let new = source("depot", "db:5432/analytics", "test");
        assert_eq!(
            secret_ops(Some(&old), &new, &BTreeMap::new(), &BTreeSet::new()),
            [SecretOp::Rename]
        );
        assert_eq!(
            secret_ops(
                Some(&old),
                &new,
                &typed(&[("password", "hunter2")]),
                &BTreeSet::new()
            ),
            [
                SecretOp::Rename,
                SecretOp::Put {
                    key: "password".into(),
                    value: "hunter2".into()
                }
            ]
        );

        let moved_address = source("warehouse", "db:5433/analytics", "test");
        assert_eq!(
            secret_ops(
                Some(&old),
                &moved_address,
                &BTreeMap::new(),
                &BTreeSet::new()
            ),
            [],
            "the address moved and the name did not, so the slot did not move"
        );
    }

    /// **A moved kind is a moved family, so the old slots are dropped rather than migrated.** The
    /// keystore family is `{kind}-{key}`, and nothing would ever name the old one again.
    #[test]
    fn a_moved_kind_forgets_what_the_old_kind_kept_here() {
        let old = source("warehouse", "db:5432/analytics", "test");
        let moved = source("warehouse", "db:5432/analytics", "other");
        assert_eq!(
            secret_ops(Some(&old), &moved, &BTreeMap::new(), &BTreeSet::new()),
            [SecretOp::ForgetAll]
        );
    }

    /// **A typed value is trimmed to nothing or stored, never both** — `Secret::new`'s fork, so a
    /// cleared box cannot become an empty stored secret. And a source with two credentials plans
    /// one op per box.
    #[test]
    fn a_typed_secret_is_stored_under_its_own_key() {
        let def = source("warehouse", "db:5432/analytics", "test");
        assert_eq!(
            secret_ops(
                Some(&def),
                &def,
                &typed(&[("password", " hunter2 ")]),
                &BTreeSet::new()
            ),
            [SecretOp::Put {
                key: "password".into(),
                value: "hunter2".into()
            }]
        );
        assert_eq!(
            secret_ops(
                Some(&def),
                &def,
                &typed(&[("password", "hunter2")]),
                &removed(&["password"])
            ),
            [SecretOp::Put {
                key: "password".into(),
                value: "hunter2".into()
            }],
            "typing over a pending removal is the secret you meant"
        );
        assert_eq!(
            secret_ops(
                Some(&def),
                &def,
                &typed(&[("password", "hunter2"), ("token", "t-1")]),
                &BTreeSet::new()
            ),
            [
                SecretOp::Put {
                    key: "password".into(),
                    value: "hunter2".into()
                },
                SecretOp::Put {
                    key: "token".into(),
                    value: "t-1".into()
                }
            ],
            "one op per box, never one for the connection"
        );
    }

    /// **Every way of abandoning a secret deletes this machine's entry** — and abandoning one of a
    /// source's credentials leaves the others alone, which is why the op names its key.
    #[test]
    fn an_abandoned_secret_is_deleted_from_this_machine() {
        let def = source("warehouse", "db:5432/analytics", "test");
        assert_eq!(
            secret_ops(Some(&def), &def, &BTreeMap::new(), &removed(&["password"])),
            [SecretOp::Forget {
                key: "password".into()
            }],
            "remove from this machine"
        );

        let unused = SourceDef {
            secrets: BTreeSet::new(),
            ..def.clone()
        };
        assert_eq!(
            secret_ops(
                Some(&def),
                &unused,
                &BTreeMap::new(),
                &removed(&["password"])
            ),
            [SecretOp::Forget {
                key: "password".into()
            }],
            "this connection uses no password"
        );

        assert_eq!(
            secret_ops(Some(&def), &store(), &BTreeMap::new(), &BTreeSet::new()),
            [SecretOp::ForgetAll],
            "and a connection that is no longer a source keeps none of them"
        );
    }

    #[test]
    fn an_actionable_blocker_outranks_the_re_scan() {
        let blocker = || Some("This connection has no user.".to_string());
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
