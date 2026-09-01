//! **The registration ledger** — what this engine last answered for each def it was asked to
//! register, and how a caller reads it back.
//!
//! Registration outcomes are the engine's own decisions, so the engine retains them and every
//! embedder reads this one record rather than keeping its own. [`Catalog::registrations`](crate::Catalog::registrations) is the
//! read; [`sync`](crate::register::sync) is what prunes it.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use crate::generation::CatalogGen;
use crate::ident::fold_ident;
use crate::sources::source::{ConnectFault, ConnectRefusal};

/// What the engine last answered for one def, and nothing about how it was asked.
///
/// Two arms, not three: a def the engine has not answered for is **absent** from the
/// [`Ledger`], because "no answer yet" is a fact about the pass rather than about the def, and a
/// third arm here would let a caller store one. What "not yet" looks like on screen is the
/// scanning affordance the host already has.
///
/// The refusal is carried **whole** — a limit belongs to whichever surface has one, never to the
/// string every other surface reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegStatus {
    /// It registered: a table resolves, a view plans, a data source's store or catalog is on the
    /// session.
    Ready,
    /// The engine refused it. The def still exists; there is just nothing working behind it.
    Failed {
        /// Why, in the engine's own words.
        reason: String,
        /// The same refusal as a fact, where the thing that refused could tell — a **connect**'s
        /// own facet ([`ConnectFault`]), and `None` for every registration that is not one.
        ///
        /// A table and a view register against the session and fail in the planner's words, which
        /// nothing here is in a position to classify; a data source is a login, and a source that
        /// reads its server's codes can say which credential was refused. Carried beside the
        /// sentence rather than instead of it: the sentence is what every surface shows, and this
        /// is what one surface points with.
        fault: Option<ConnectFault>,
    },
}

impl RegStatus {
    /// A refusal with nothing to point at — the sentence alone.
    pub fn failed(reason: impl Into<String>) -> RegStatus {
        RegStatus::Failed {
            reason: reason.into(),
            fault: None,
        }
    }

    /// What the engine answered for `result`, discarding the payload — which is what a
    /// *status* is, and what registration **learned** is the answer's own.
    pub(crate) fn of<T, E: ToString>(result: &Result<T, E>) -> RegStatus {
        match result {
            Ok(_) => RegStatus::Ready,
            Err(e) => RegStatus::failed(e.to_string()),
        }
    }

    /// What the engine answered for a **connect**, which is the one registration whose refusal
    /// carries a facet.
    ///
    /// Its own constructor rather than a `ToString` through [`of`](Self::of): reading the sentence
    /// back off `Display` would drop exactly the half this exists to keep.
    pub(crate) fn of_connect<T>(result: &Result<T, ConnectRefusal>) -> RegStatus {
        match result {
            Ok(_) => RegStatus::Ready,
            Err(refusal) => RegStatus::Failed {
                reason: refusal.reason.clone(),
                fault: refusal.fault,
            },
        }
    }

    /// Whether the def registered.
    pub fn is_ready(&self) -> bool {
        matches!(self, RegStatus::Ready)
    }

    /// The refusal, if this is one — a host's problem row, and the sentence a tooltip clips.
    pub fn reason(&self) -> Option<&str> {
        match self {
            RegStatus::Failed { reason, .. } => Some(reason),
            RegStatus::Ready => None,
        }
    }

    /// The declared secret key whose value the server rejected, where the source said so.
    ///
    /// What the data source editor's row for that key keys on: the def expects a secret, this
    /// machine holds one, and the last connect was turned away over it. `None` for every other
    /// answer, including a failure this engine could not classify — an unrecognised refusal must
    /// read as *unknown*, never as a wrong password.
    pub fn rejected(&self) -> Option<&'static str> {
        match self {
            RegStatus::Failed {
                fault: Some(ConnectFault::Rejected { key }),
                ..
            } => Some(key),
            _ => None,
        }
    }
}

/// One def's entry in the [`Ledger`]: what the engine answered, and the generation it answered at.
///
/// The stamp is what makes a status readable as *this* answer rather than the one before it. A
/// gesture that asks for a registration keeps the generation it asked at and waits for an entry
/// stamped past it; without that, a re-save of a table that already registered reads its own
/// previous `Ready` as the answer to the question it has only just asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Registration {
    /// What the engine answered.
    pub status: RegStatus,
    /// The generation it answered at.
    pub generation: CatalogGen,
}

/// **The registration ledger**: what this engine last answered for each def it was asked to
/// register — data sources, tables and views alike.
///
/// Registration outcomes are the engine's own decisions, so the engine retains them and every
/// embedder reads this one record rather than keeping its own. The [`InternalTables`](crate::InternalTables) shape,
/// with the same limits: every funnel that registers a name notes what it answered, every funnel
/// that takes one out forgets it, and [`sync`](crate::register::sync) prunes to the names its
/// `CatalogSpec` holds — which is the only thing that can retire the entry of a def whose
/// registration *failed*, since no deregistration will ever report one.
///
/// **Two namespaces, because two names can be the same word.** The workspace catalog holds
/// tables and views in one namespace (a name is at most one of them), and a data source's name is
/// the handle its user gave it — so a bucket called `events` and a table called `events` are
/// different things, and one map keyed by name would land one answer on both.
///
/// It is not a second catalog. What a def *is* stays the host's (the store writes the defs);
/// this says only what happened when the engine was asked to register one.
#[derive(Clone, Debug, Default)]
pub struct Ledger {
    /// Tables and views, by [`fold_ident`]ed name.
    workspace: Arc<Mutex<BTreeMap<String, Registration>>>,
    /// Data sources, by [`fold_ident`]ed name.
    sources: Arc<Mutex<BTreeMap<String, Registration>>>,
}

impl Ledger {
    /// Record what registering the workspace def `name` answered.
    pub(crate) fn note(&self, name: &str, status: RegStatus, generation: CatalogGen) {
        note_in(&self.workspace, name, status, generation);
    }

    /// Forget the workspace def `name` — every funnel that deregisters one.
    pub(crate) fn forget(&self, name: &str) {
        self.workspace.lock().unwrap().remove(&fold_ident(name));
    }

    /// Record what connecting the data source `name` answered.
    pub(crate) fn note_source(&self, name: &str, status: RegStatus, generation: CatalogGen) {
        note_in(&self.sources, name, status, generation);
    }

    /// Forget the data source `name` — [`Sources::disconnect`](crate::Sources::disconnect).
    pub(crate) fn forget_source(&self, name: &str) {
        self.sources.lock().unwrap().remove(&fold_ident(name));
    }

    /// What the engine last answered for the data source `name`.
    pub(crate) fn source(&self, name: &str) -> Option<Registration> {
        self.sources.lock().unwrap().get(&fold_ident(name)).cloned()
    }

    /// Every answer this engine holds, both namespaces, taken under one read and stamped with
    /// the generation the caller read for it — [`Catalog::registrations`](crate::Catalog::registrations).
    pub(crate) fn registrations(&self, generation: CatalogGen) -> Registrations {
        Registrations {
            generation,
            workspace: Answers(self.workspace.lock().unwrap().clone()),
            sources: Answers(self.sources.lock().unwrap().clone()),
        }
    }

    /// Keep only the defs `desired` names — [`sync`](crate::register::sync)'s reconciliation, and the
    /// only thing that retires the entry of a def whose registration failed.
    pub(crate) fn retain(&self, workspace: &BTreeSet<String>, sources: &BTreeSet<String>) {
        self.workspace
            .lock()
            .unwrap()
            .retain(|held, _| workspace.contains(held));
        self.sources
            .lock()
            .unwrap()
            .retain(|held, _| sources.contains(held));
    }
}

/// One [`Ledger`] map's write, folded so the two namespaces cannot disagree about how a name is
/// keyed.
fn note_in(
    map: &Mutex<BTreeMap<String, Registration>>,
    name: &str,
    status: RegStatus,
    generation: CatalogGen,
) {
    map.lock()
        .unwrap()
        .insert(fold_ident(name), Registration { status, generation });
}

/// What the engine last answered for every def it was asked to register, read as of one moment —
/// [`Catalog::registrations`](crate::Catalog::registrations)'s answer, and the whole ledger.
///
/// Taken under one read and stamped with the [`generation`](Self::generation) it was taken at,
/// for [`SourcesSnapshot`](crate::sources::SourcesSnapshot)'s reason: a caller that asked per row would
/// be describing a different instant per row. Key a derived answer on the generation and re-derive
/// when [`Catalog::generation`](crate::Catalog::generation) stops matching it.
///
/// The two namespaces are reached separately ([`workspace`](Self::workspace) /
/// [`sources`](Self::sources)) because they are separate: a bucket called `events` and a table
/// called `events` are different things.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Registrations {
    /// The generation the ledger was read at.
    pub generation: CatalogGen,
    /// What the engine answered for the workspace catalog's tables and views.
    pub workspace: Answers,
    /// What the engine answered for the project's data sources.
    pub sources: Answers,
}

/// One namespace of the ledger, as a caller reads it — see [`Registrations`].
///
/// Opaque, because the keys are [`fold_ident`]ed and a map handed out raw would be one a caller
/// could put an unfolded name into.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Answers(BTreeMap<String, Registration>);

impl Answers {
    /// Answers a caller already holds, folded and stamped the way the ledger's own are.
    ///
    /// [`SourcesSnapshot`](crate::sources::SourcesSnapshot)'s affordance, for its reason: what this
    /// crate hands out is a **value**, and a consumer that can only ever receive one built by an
    /// engine cannot be exercised against the states it draws.
    pub fn recorded(
        answers: impl IntoIterator<Item = (String, RegStatus)>,
        generation: CatalogGen,
    ) -> Answers {
        Answers(
            answers
                .into_iter()
                .map(|(name, status)| (fold_ident(&name), Registration { status, generation }))
                .collect(),
        )
    }

    /// What the engine last answered for `name`, or `None` for a def no pass has reached.
    ///
    /// One lookup for a table and a view, because they are one namespace: the workspace catalog
    /// cannot hold a table and a view of the same name, so the kind adds nothing a caller does
    /// not already know from the row it is drawing.
    ///
    /// Folded, because a name reaches this from a def (whatever the user typed) and from the
    /// planner (which folds an unquoted identifier) alike.
    pub fn of(&self, name: &str) -> Option<&Registration> {
        self.0.get(&fold_ident(name))
    }

    /// What the engine last answered for `name`, without the generation it answered at.
    pub fn status(&self, name: &str) -> Option<&RegStatus> {
        self.of(name).map(|reg| &reg.status)
    }

    /// The refusal `name` last landed, if it landed one.
    pub fn problem(&self, name: &str) -> Option<&str> {
        self.of(name).and_then(|reg| reg.status.reason())
    }

    /// Whether `name` is registered right now.
    pub fn is_ready(&self, name: &str) -> bool {
        self.of(name).is_some_and(|reg| reg.status.is_ready())
    }

    /// Whether the engine has answered for `name` **since** `asked_at` — what a gesture that
    /// asked for a registration waits on, so a def's previous answer is never read as this one.
    ///
    /// A re-save of a table that already registered is the case: its row carries `Ready` from
    /// the pass before, and a caller that read the status alone would take that as the answer to
    /// the question it has only just asked.
    pub fn answered_since(&self, name: &str, asked_at: CatalogGen) -> Option<&RegStatus> {
        self.of(name)
            .filter(|reg| reg.generation > asked_at)
            .map(|reg| &reg.status)
    }
}

/// **The registration ledger's own claim**, as one checklist: every funnel that registers a def
/// records what the engine answered, every funnel that takes one out forgets it, and a pass
/// prunes the entries neither reported.
///
/// Kept together rather than beside each facade method for [`generation`]'s reason — the claim
/// is that nothing registers without being recorded, and a checklist is only checkable read
/// whole. Driven through the facade, because that is the surface a host has.
#[cfg(test)]
mod ledger_tests {
    use std::path::{Path, PathBuf};
    use std::{env, fs, process};

    use strata_model::{SourceDef, SourceFormat, ViewDef};

    use super::*;
    use crate::register::CatalogSpec;
    use crate::{Engine, RunTag, TableSpec, WsId};

    /// A scratch project folder per test, holding one two-column CSV.
    fn scratch(tag: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("strata_ledger_{}_{tag}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("t.csv"), "id,name\n1,a\n2,b\n").unwrap();
        dir
    }

    fn spec(root: &Path, name: &str, file: &str) -> TableSpec {
        TableSpec {
            name: name.into(),
            paths: vec![root.join(file).display().to_string()],
            format: SourceFormat::from_name("csv"),
            partitions: Vec::new(),
            source: None,
            internal: false,
        }
    }

    /// A bucket refused before any socket opens — an S3 data source with no region.
    fn unreachable(name: &str) -> SourceDef {
        SourceDef {
            config: [("address".to_string(), "acme-lake".to_string())]
                .into_iter()
                .collect(),
            kind: "s3".into(),
            name: name.into(),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn every_registration_is_recorded_and_every_removal_forgotten() {
        let root = scratch("gestures");
        let engine = Engine::builder().with_data_dir(&root).build();
        let catalog = engine.catalog();
        assert_eq!(
            catalog.registrations(),
            Registrations::default(),
            "an engine that has registered nothing has answered for nothing"
        );

        catalog
            .register(spec(&root, "t", "t.csv"))
            .await
            .expect("t");
        let refused = catalog
            .register(spec(&root, "gone", "missing.csv"))
            .await
            .expect_err("no such file");
        catalog
            .create_view("v".into(), "SELECT id FROM t".into())
            .await
            .expect("create view");
        let _ = engine.sources().connect(unreachable("lake")).await;

        let ledger = catalog.registrations();
        assert_eq!(ledger.workspace.status("t"), Some(&RegStatus::Ready));
        assert_eq!(ledger.workspace.status("v"), Some(&RegStatus::Ready));
        assert_eq!(
            ledger.workspace.problem("gone").map(str::to_string),
            Some(refused.to_string()),
            "the refusal is kept whole, in the engine's own words"
        );
        assert!(
            ledger
                .sources
                .problem("lake")
                .is_some_and(|why| why.contains("region")),
            "a data source's refusal is its own namespace's: {:?}",
            ledger.sources.problem("lake")
        );
        assert_eq!(
            ledger.workspace.status("nothing_of_the_sort"),
            None,
            "a name no pass reached is absent rather than a state of its own"
        );

        catalog.drop_view("v".into()).await.expect("drop view");
        let _ = catalog.deregister("t");
        let _ = engine.sources().disconnect("lake");

        let ledger = catalog.registrations();
        for gone in ["t", "v"] {
            assert_eq!(ledger.workspace.of(gone), None, "'{gone}' was taken out");
        }
        assert_eq!(ledger.sources.of("lake"), None, "and so was the bucket");
        assert!(
            ledger.workspace.problem("gone").is_some(),
            "the def that never registered is reported by no removal, so it is still here"
        );

        engine.catalog().sync(CatalogSpec::default(), |_| {}).await;
        assert_eq!(
            engine.catalog().registrations(),
            Registrations {
                generation: engine.catalog().generation(),
                ..Default::default()
            },
            "and a pass over a catalog that names nothing prunes it"
        );

        let _ = fs::remove_dir_all(&root);
    }

    /// **A gesture waits for its own answer, not for the one before it.** A re-registered def
    /// already carries `Ready` from the pass before, so a caller reading the status alone would
    /// take that as the answer to the question it has only just asked — which is a Configure Save
    /// closing its window over a registration that has not happened.
    #[tokio::test]
    async fn an_answer_is_told_from_the_one_before_it_by_the_generation() {
        let root = scratch("since");
        let engine = Engine::builder().with_data_dir(&root).build();
        let catalog = engine.catalog();
        catalog
            .register(spec(&root, "t", "t.csv"))
            .await
            .expect("t");

        let asked_at = catalog.generation();
        assert_eq!(
            catalog
                .registrations()
                .workspace
                .answered_since("t", asked_at),
            None,
            "the answer it already had is not this gesture's"
        );

        catalog
            .register(spec(&root, "t", "t.csv"))
            .await
            .expect("re-registered");

        assert_eq!(
            catalog
                .registrations()
                .workspace
                .answered_since("t", asked_at),
            Some(&RegStatus::Ready),
            "and the new one is"
        );

        let _ = fs::remove_dir_all(&root);
    }

    /// A view is upserted by a typed statement as much as by ⌘S, and both funnels record —
    /// `settle_effect` is the second one, and a fold that skipped it would leave the row a
    /// statement had just created reading as unanswered.
    #[tokio::test]
    async fn a_typed_statement_records_what_it_registered() {
        let root = scratch("typed");
        let engine = Engine::builder().with_data_dir(&root).build();
        engine
            .catalog()
            .register(spec(&root, "t", "t.csv"))
            .await
            .expect("t");
        engine
            .ws(WsId(1))
            .run(
                RunTag(1),
                "CREATE VIEW typed AS SELECT id FROM t".into(),
                10,
            )
            .await
            .expect("typed view DDL");

        assert_eq!(
            engine.catalog().registrations().workspace.status("typed"),
            Some(&RegStatus::Ready)
        );

        engine
            .ws(WsId(1))
            .run(RunTag(2), "DROP VIEW typed".into(), 10)
            .await
            .expect("typed drop");

        assert_eq!(
            engine.catalog().registrations().workspace.of("typed"),
            None,
            "and the drop forgets it"
        );

        let _ = fs::remove_dir_all(&root);
    }

    /// A pass keeps what its spec still names, whatever the answer was — the entry a host's row
    /// is joined against must survive a re-scan that re-answers it.
    #[tokio::test]
    async fn a_pass_keeps_what_it_still_names() {
        let root = scratch("retain");
        let engine = Engine::builder().with_data_dir(&root).build();
        let desired = CatalogSpec {
            tables: vec![
                spec(&root, "t", "t.csv"),
                spec(&root, "gone", "missing.csv"),
            ],
            views: vec![ViewDef {
                name: "v".into(),
                sql: "SELECT id FROM t".into(),
            }],
            ..Default::default()
        };

        engine.catalog().sync(desired.clone(), |_| {}).await;
        engine.catalog().sync(desired, |_| {}).await;

        let ledger = engine.catalog().registrations();
        assert_eq!(ledger.workspace.status("t"), Some(&RegStatus::Ready));
        assert_eq!(ledger.workspace.status("v"), Some(&RegStatus::Ready));
        assert!(ledger.workspace.problem("gone").is_some());

        let _ = fs::remove_dir_all(&root);
    }
}
