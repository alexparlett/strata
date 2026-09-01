//! The window's footer — why Save is blocked on the left, Cancel and Save on the right.
//!
//! **This is the only thing in the window that writes anything.** Cancel just closes; nothing is
//! committed until Save.
//!
//! A data source has a fifth step, and it comes **first**: whatever this machine's keystore
//! owes the secrets its kind declares ([`secret_ops`]). A def that expects one over a keystore
//! that refused the write is a data source nothing can log in with.
//!
//! 1. writes the def onto the shared project store — removing the row it is **moving from**
//!    first, when the edit changed the bucket or the provider and therefore the data source's
//!    identity;
//! 2. persists through the funnel and **gates on the answer** — a data source the project file
//!    never heard about is gone on the next open, which is what P4-15 exists to stop being
//!    silent;
//! 3. drops the object store the old URL registered, the one call the scan driver cannot make:
//!    `engine::sources::store::connect` only ever sees the def it is given, so nothing else would ever
//!    take that store back out (`Sources::disconnect` — the same call Forget makes);
//! 4. asks the project window's one scan driver for a whole-catalog pass, and leaves this window
//!    watching its row ([`super::use_watch_data source`]).
//!
//! **A whole-catalog pass, not a source-shaped one.** `plan_scan` puts data sources in
//! `ScanScope::All` alone, deliberately: the case that needs a re-connect is a region corrected
//! or an `aws sso login` run, which is exactly this window — and every table over the bucket was
//! registered against the store this save replaces, so re-registering the data source alone would
//! leave them answering from a store that is no longer there. Save is the ↻ the user would
//! otherwise press, with the def written first.

use freya::prelude::*;
use freya::radio::{use_radio_station, RadioStation};

use std::collections::{BTreeMap, BTreeSet};

use strata_engine::sources::put_secret_at;
use strata_model::{check_catalog_name, SecretRef, SourceDef};

use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::{log_event, use_report, LogLevel, ReportCtx};
use crate::apps::project::{
    catalog_settled, persisted_defs, refresh_catalog, Catalog, CatalogRescan, ProjChan,
    ProjectState,
};
use crate::apps::source::{SourceCtx, SourceTarget, Status};
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
        let ctx = use_consume::<SourceCtx>();
        let project = use_radio_station::<ProjectState, ProjChan>();
        let rescan = use_consume::<CatalogRescan>();
        let catalog = use_consume::<Catalog>();
        let engine = use_consume::<EngineCtx>();
        let report = use_report();
        let platform = use_hook(Platform::get);

        let busy = match *ctx.status.read() {
            Status::Storing => Some("Saving…"),
            Status::Connecting { .. } => Some("Connecting…"),
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
                move |_: Event<PressEventData>| save(ctx, project, rescan, catalog, engine.clone(), report)
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
fn address_refusal(ctx: SourceCtx, engine: &EngineCtx) -> Option<String> {
    let def = ctx.draft.read().def();
    let kind = def.kind.clone();
    engine
        .sources()
        .check_address(&kind, def.setting("address"))
        .err()
        .map(|why| why.to_string())
}

/// The blocker the draft cannot see: a name another data source already holds.
///
/// `upsert_source` replaces on the name, so without this an edit that renamed one data source
/// onto another's would silently take that data source's def out from under it — the same hazard
/// the Configure window's name clash guards, one section along. On an edit the data source's own
/// name does not clash with itself.
///
/// Matched case-**sensitively**, unlike [`catalog_clash`] below: a data source is addressed by
/// gesture, and the spelling the user typed is the one every surface shows back.
fn name_clash(ctx: SourceCtx, project: RadioStation<ProjectState, ProjChan>) -> Option<String> {
    let name = ctx.draft.read().def().named();
    if ctx.target.read().editing() == Some(name.as_str()) {
        return None;
    }
    project
        .peek()
        .sources
        .iter()
        .any(|c| c.named() == name)
        .then(|| format!("'{name}' is already a data source in this project."))
}

/// The other blocker the draft cannot see: a **catalog name** another data source in this
/// project already registers under.
///
/// [`check_catalog_name`] rather than a comparison written here, so the field refuses what
/// registration refuses, in the same words. It folds, unlike [`name_clash`] beside it,
/// because a catalog name is a SQL identifier. The set is the project's *stored* defs where the
/// engine's is what is live — a data source that failed to connect reserves nothing.
///
/// **The row this window opened on is dropped first**, exactly as [`name_clash`] drops it.
/// `check_catalog_name` skips the candidate by comparing URLs, and a data source's URL
/// carries its user — so editing the USER, the URL or the DATABASE moves the identity, the stale
/// row stops matching, and the draft clashes with the data source it is replacing. The footer then
/// quotes that data source's own old URL back and Save never re-enables.
fn catalog_clash(ctx: SourceCtx, project: RadioStation<ProjectState, ProjChan>) -> Option<String> {
    let def = ctx.draft.read().def();
    let editing = ctx.target.read().editing().map(str::to_string);
    let existing: Vec<SourceDef> = project
        .peek()
        .sources
        .iter()
        .filter(|c| editing.as_deref() != Some(c.named().as_str()))
        .cloned()
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
    /// A value was typed into a box, bound for the slot its key is filed under.
    Put { slot: SecretRef, value: String },
    /// *Remove from this machine*, for one declared key — or a key this save drops, whose slot
    /// only the **previous** def still names.
    Forget { slot: SecretRef },
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
    let was = previous.filter(|def| !def.secrets.is_empty());

    let mut ops = Vec::new();

    let mut keys: BTreeSet<&str> = next
        .secrets
        .keys()
        .into_iter()
        .map(String::as_str)
        .collect();
    keys.extend(
        was.iter()
            .flat_map(|def| def.secrets.keys())
            .map(String::as_str),
    );
    keys.extend(typed.keys().map(String::as_str));
    keys.extend(removed.iter().map(String::as_str));
    for key in keys {
        let Some(slot) = next
            .secret_slot(key)
            .or_else(|| was.and_then(|def| def.secret_slot(key)))
        else {
            continue;
        };
        match typed.get(key).map(|value| value.trim()).unwrap_or_default() {
            "" if removed.contains(key) || !next.secrets.expects(key) => {
                ops.push(SecretOp::Forget { slot });
            }
            "" => {}
            value => ops.push(SecretOp::Put {
                slot,
                value: value.to_string(),
            }),
        }
    }
    ops
}

/// Carry `ops` out in order, stopping at the first refusal. Blocking, so it runs on a worker; a
/// keystore that refuses is reported and the save does not happen, never answered by writing the
/// secret somewhere else.
fn run_secret_ops(ops: &[SecretOp]) -> Result<(), String> {
    for op in ops {
        match op {
            SecretOp::Put { slot, value } => put_secret_at(slot, value)?,
            SecretOp::Forget { slot } => put_secret_at(slot, "")?,
        }
    }
    Ok(())
}

/// Write the def, persist it, drop what the old URL registered, and ask for the pass. See the
/// module doc.
///
/// **The bucket is not probed here**, and that is a decision with a measurement behind it. A
/// Save-time `Engine::check_source` was built first, to refuse an unreachable bucket before
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
    mut ctx: SourceCtx,
    project: RadioStation<ProjectState, ProjChan>,
    rescan: CatalogRescan,
    catalog: Catalog,
    engine: EngineCtx,
    report: ReportCtx,
) {
    let def = ctx.draft.peek().def();
    let previous = ctx.target.peek().editing().and_then(|name| {
        project
            .peek()
            .sources
            .iter()
            .find(|c| c.named() == name)
            .cloned()
    });
    let ops = secret_ops(
        previous.as_ref(),
        &def,
        &ctx.secret_values.peek(),
        &ctx.secret_removed.peek(),
    );
    if ops.is_empty() {
        commit(ctx, project, rescan, catalog, engine, report);
        return;
    }

    ctx.status.set(Status::Storing);
    spawn(async move {
        let landed = offload(move || run_secret_ops(&ops)).await;
        match landed {
            Some(Ok(())) => commit(ctx, project, rescan, catalog, engine, report),
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
    mut ctx: SourceCtx,
    mut project: RadioStation<ProjectState, ProjChan>,
    rescan: CatalogRescan,
    catalog: Catalog,
    engine: EngineCtx,
    report: ReportCtx,
) {
    let asked_at = engine.catalog().generation();
    let def = ctx.draft.peek().def();
    let name = def.named();
    let moved_from = ctx
        .target
        .peek()
        .editing()
        .filter(|old| *old != name)
        .map(str::to_string);

    let landed = {
        let mut p = project.write_channel(ProjChan::Sources);
        match &moved_from {
            Some(old) => p.rename_source(old, def),
            None => p.upsert_source(def),
        }
        persisted_defs(&p, report)
    };
    if !landed {
        ctx.status.set(Status::Failed(
            "The data source is registered, but the project file could not be written, so it will \
             be gone when this project is reopened."
                .into(),
        ));
    } else {
        ctx.status.set(Status::Connecting {
            name: name.clone(),
            asked_at,
        });
    }
    {
        let mut target = ctx.target;
        target.set(SourceTarget::Edit(name.clone()));
    }

    if let Some(old) = &moved_from {
        catalog_settled(catalog, engine.sources().disconnect(old));
        log_event(
            report.log,
            LogLevel::Info,
            format!("Moved data source '{old}' to '{name}'"),
        );
    }

    refresh_catalog(rescan);
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use strata_model::{Secrets, SourceDef};

    use super::*;

    /// One source def: a kind, a name and the expectation of a `password`.
    fn source(name: &str, address: &str, kind: &str) -> SourceDef {
        SourceDef {
            config: [("address".to_string(), address.into())]
                .into_iter()
                .collect(),
            name: name.into(),
            kind: kind.into(),
            secrets: Secrets::Filed(BTreeMap::from([(
                "password".to_string(),
                SecretRef::derived("fixture", name),
            )])),
            ..Default::default()
        }
    }

    /// A source with two credentials, so a plan can be asked to keep them apart.
    fn two_credentials(name: &str) -> SourceDef {
        SourceDef {
            secrets: Secrets::Filed(BTreeMap::from([
                ("password".to_string(), SecretRef::derived("fixture", name)),
                (
                    "token".to_string(),
                    SecretRef::derived("fixture-token", name),
                ),
            ])),
            ..source(name, "db:5432/analytics", "test")
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

    /// **A data source that holds no secret is never a keystore call**, whatever moved — so
    /// renaming one, or changing its kind, raises no Keychain prompt and keeps Save synchronous.
    #[test]
    fn a_source_with_no_secrets_is_never_a_keystore_call() {
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

        let s3 = store();
        assert_eq!(
            secret_ops(Some(&s3), &s3, &BTreeMap::new(), &BTreeSet::new()),
            []
        );
    }

    /// **A rename owes the keystore nothing**, which is what recording the slot bought.
    ///
    /// The ref travels in the def, so the renamed source reads the entry it already had. It used
    /// to be derived from the name: a rename moved the slot on every machine while only the
    /// renaming one could move its own entry to follow, so every colleague who pulled the rename
    /// was left with a password under an id nothing would name again.
    #[test]
    fn a_rename_plans_no_keystore_work_at_all() {
        let old = source("warehouse", "db:5432/analytics", "test");
        let renamed = SourceDef {
            name: "depot".into(),
            ..old.clone()
        };
        assert_eq!(
            secret_ops(Some(&old), &renamed, &BTreeMap::new(), &BTreeSet::new()),
            [],
            "the slot is in the def, so nothing moves"
        );
        assert_eq!(
            secret_ops(
                Some(&old),
                &renamed,
                &typed(&[("password", "hunter2")]),
                &BTreeSet::new()
            ),
            [SecretOp::Put {
                slot: old.secret_slot("password").expect("a filed slot"),
                value: "hunter2".into()
            }],
            "and a value typed during a rename lands on the slot the def already names"
        );
    }

    /// **A key the save no longer expects is cleared from the slot the previous def named**, and
    /// nothing else is.
    ///
    /// The kind changing is the case that produces one: `def()` projects through the *new* kind's
    /// declaration, so a credential it does not declare drops out of the map. The old slot is the
    /// only name that entry ever had, so the previous def is what has to be asked for it.
    #[test]
    fn a_dropped_key_is_cleared_from_the_slot_the_previous_def_named() {
        let old = source("warehouse", "db:5432/analytics", "test");
        let dropped = SourceDef {
            secrets: Secrets::default(),
            ..old.clone()
        };
        assert_eq!(
            secret_ops(Some(&old), &dropped, &BTreeMap::new(), &BTreeSet::new()),
            [SecretOp::Forget {
                slot: old.secret_slot("password").expect("a filed slot")
            }]
        );

        let same_key = source("warehouse", "db:5432/analytics", "other");
        assert_eq!(
            secret_ops(Some(&old), &same_key, &BTreeMap::new(), &BTreeSet::new()),
            [],
            "a kind that declares the same key keeps the entry: the slot is the def's now, not \
             the kind's"
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
                slot: def.secret_slot("password").expect("a filed slot"),
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
                slot: def.secret_slot("password").expect("a filed slot"),
                value: "hunter2".into()
            }],
            "typing over a pending removal is the secret you meant"
        );
        let both = two_credentials("warehouse");
        assert_eq!(
            secret_ops(
                Some(&both),
                &both,
                &typed(&[("password", "hunter2"), ("token", "t-1")]),
                &BTreeSet::new()
            ),
            [
                SecretOp::Put {
                    slot: both.secret_slot("password").expect("a filed slot"),
                    value: "hunter2".into()
                },
                SecretOp::Put {
                    slot: both.secret_slot("token").expect("a filed slot"),
                    value: "t-1".into()
                }
            ],
            "one op per box, each on its own slot, never one for the data source"
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
                slot: def.secret_slot("password").expect("a filed slot")
            }],
            "remove from this machine"
        );

        let unused = SourceDef {
            secrets: Secrets::default(),
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
                slot: def.secret_slot("password").expect("a filed slot")
            }],
            "a save that stops expecting one"
        );

        assert_eq!(
            secret_ops(Some(&def), &store(), &BTreeMap::new(), &BTreeSet::new()),
            [SecretOp::Forget {
                slot: def.secret_slot("password").expect("a filed slot")
            }],
            "and a data source that is no longer a source keeps none of them — named one slot at \
             a time, because the previous def is what still knows where each one is"
        );
    }

    #[test]
    fn an_actionable_blocker_outranks_the_re_scan() {
        let blocker = || Some("This data source has no user.".to_string());
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
