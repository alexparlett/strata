//! **The data sources, joined once** — the project's data source defs and the engine's
//! [`SourcesSnapshot`] folded into the one list the catalog tree draws.
//!
//! [`assemble`] is that join, and it is made **once, against one snapshot**, above the tree's
//! walk. The def side is the project's (what the user wrote down, in the order they arranged it);
//! everything the row *says* — its badge, its schemas, whether it registered and what refused it
//! — is the engine's, taken under one read so each row describes the same instant where a lookup
//! per row would be a moment per row. What it produces is a plain value: the walk below it
//! decides shape, and the rows below that draw. Nothing here renders and nothing here reaches
//! the engine.
//!
//! **The verdict comes from the ledger, not from the snapshot**, though the snapshot carries one
//! too. Both are the same record read at different moments, and the window holds the ledger
//! already — so taking it from there is what makes a tree row and a Problems row describe one
//! instant rather than two. The snapshot's own copy is for a reader with no window to hold one:
//! the engine's `catalog_names`, and the agent's `list_tables`.
//!
//! ## The two halves are the same difference the tree draws
//!
//! An object store's contents are **declared** — a bucket cannot say what its tables are, so its
//! children are the workspace defs that name it, and those are the store's rows. A source's
//! contents are **discovered** — it answers for itself, so its schemas come from the connect-time
//! enumeration the snapshot carries, already scoped and tagged. One join, two arms, and no
//! surface re-derives either.

use std::collections::BTreeMap;

use strata_engine::sources::{SchemaListingView, SourceDetail, SourcesSnapshot};
use strata_engine::{Registrations, SourceMode};

use super::ProjectState;

/// One data source as the tree draws it, resolved.
///
/// Carries what a row paints and no more — the def itself is not cloned, a virtualized tree
/// copying every visible node on every walk. What opens underneath is
/// [`contents`](Self::contents).
#[derive(Clone, PartialEq, Debug)]
pub struct SourceNode {
    /// The data source's name — the handle every gesture addresses it by, and the tree key
    /// `conn/{name}` it is drawn under.
    pub name: String,
    /// Where it points, in its provider's own terms — what the row draws as its title.
    pub address: String,
    /// The short word its row wears: the registered kind's own, asked of the kind so that a
    /// data source nothing has connected yet is still badged for what serves it.
    pub badge: String,
    /// What connecting to it yields — the row's menu and the consequence a Forget spells out.
    /// From the registrants rather than from the data source's own row, so a def no pass has
    /// reached yet still answers for the kind that serves it.
    pub mode: SourceMode,
    /// The engine has not answered for it yet — every data source in a window's first frames,
    /// and one added since the last pass.
    ///
    /// Not "a pass is running": a re-scan does not un-answer what it is about to answer again, so
    /// a row keeps the verdict it has while the pass runs and the header's spinner is what says a
    /// pass is out.
    pub waiting: bool,
    /// What the engine refused it with, if it did.
    pub problem: Option<String>,
    pub contents: SourceContents,
}

/// What opens underneath a source.
#[derive(Clone, PartialEq, Debug)]
pub enum SourceContents {
    /// An object store: the workspace tables that read through it, alphabetically, as links back
    /// to their own rows.
    Store { tables: Vec<String> },
    /// A source's catalog: the name its relations are addressed by, and the namespaces it
    /// **shows** — [`NotEnabled`](strata_engine::sources::SchemaVisibility::NotEnabled) ones
    /// already dropped, since nothing in the tree may draw one.
    Catalog {
        catalog: String,
        schemas: Vec<SchemaListingView>,
    },
}

impl SourceNode {
    /// Whether this node has anything to open — a bucket with no table over it and a database
    /// that has not answered are both leaves.
    pub fn can_open(&self) -> bool {
        match &self.contents {
            SourceContents::Store { tables } => !tables.is_empty(),
            SourceContents::Catalog { schemas, .. } => !schemas.is_empty(),
        }
    }
}

/// Join the project's data source defs with what the engine holds for each.
///
/// Ordered by the project's own defs, which is the order the pane draws and the user rearranges.
/// A data source the engine has not been told about still gets its node, drawn as waiting and
/// carrying nothing else — a window's first frames are exactly that, and the tree has to draw a
/// project's data sources before anything has registered them.
///
/// Bucket links are grouped in **one pass over the tables**: a scan per bucket would cost a
/// project with many tables and many data sources their product.
pub fn assemble(
    project: &ProjectState,
    registrations: &Registrations,
    snapshot: &SourcesSnapshot,
) -> Vec<SourceNode> {
    let answers = &registrations.sources;
    let mut over: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for table in &project.tables {
        if let Some(source) = table.def.source.as_deref() {
            over.entry(source).or_default().push(table.def.name.clone());
        }
    }
    project
        .sources
        .iter()
        .map(|def| {
            let name = def.named();
            let listing = snapshot.source(&name);
            let catalogued = snapshot.mode(&def.kind) == Some(SourceMode::Catalog);
            let contents = match (catalogued, listing.map(|l| &l.detail)) {
                (true, Some(SourceDetail::Catalog { schemas, .. })) => SourceContents::Catalog {
                    catalog: name.clone(),
                    schemas: shown(schemas),
                },
                (true, _) => SourceContents::Catalog {
                    catalog: name.clone(),
                    schemas: Vec::new(),
                },
                (false, _) => SourceContents::Store {
                    tables: over.remove(name.as_str()).unwrap_or_default(),
                },
            };
            SourceNode {
                badge: snapshot.badge(&def.kind),
                mode: snapshot.mode(&def.kind).unwrap_or(SourceMode::Store),
                address: def.setting("address").to_string(),
                waiting: answers.of(&name).is_none(),
                problem: answers.problem(&name).map(str::to_owned),
                contents,
                name,
            }
        })
        .collect()
}

/// The namespaces a surface may draw: everything the data source shows, missing ones included, and
/// nothing it does not.
///
/// Dropped **here** rather than at each row, because a `NotEnabled` schema is not a thing the tree
/// or the picker's counts have any arm for — the Schemas… dialog is the one surface that sees
/// them, and it reads the snapshot itself.
fn shown(schemas: &[SchemaListingView]) -> Vec<SchemaListingView> {
    use strata_engine::sources::SchemaVisibility;
    schemas
        .iter()
        .filter(|schema| schema.visibility != SchemaVisibility::NotEnabled)
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use strata_core::project::ProjectDefs;
    use strata_engine::sources::source::{SourceInfo, SourceMode};
    use strata_engine::sources::{SchemaVisibility, SourceListing};
    use strata_engine::{CatalogGen, RegStatus};

    use crate::apps::project::state::Answered;
    use strata_model::{SourceDef, SourceFormat, TableDef, TableOrigin};

    use super::*;

    fn bucket(name: &str) -> SourceDef {
        SourceDef {
            name: name.into(),
            kind: "s3".into(),
            config: [("region".to_string(), "eu-west-2".to_string())]
                .into_iter()
                .collect(),
            ..Default::default()
        }
    }

    fn database(name: &str) -> SourceDef {
        SourceDef {
            config: [("address".to_string(), "db.internal:5432/analytics".into())]
                .into_iter()
                .collect(),
            name: name.into(),
            kind: "postgres".into(),
            schemas: vec!["public".into()],
            ..Default::default()
        }
    }

    fn over(name: &str, source: &str) -> TableDef {
        TableDef {
            name: name.into(),
            format: SourceFormat::Parquet,
            source: Some(source.into()),
            paths: vec![format!("{name}/")],
            partition_cols: Vec::new(),
            origin: TableOrigin::External,
        }
    }

    fn project() -> ProjectState {
        ProjectState::from_defs(
            ProjectDefs {
                name: "test".into(),
                tables: vec![over("events", "lake"), over("clicks", "lake")],
                sources: vec![bucket("lake"), database("analytics")],
                ..Default::default()
            },
            PathBuf::from("/tmp/strata-assemble"),
        )
    }

    /// A snapshot carrying `sources`, with the two registrants these fixtures name so each def
    /// has a badge and a mode to wear — the shape the engine hands over.
    fn snapshot(sources: Vec<SourceListing>) -> SourcesSnapshot {
        SourcesSnapshot {
            generation: CatalogGen::default(),
            sources,
            registrants: vec![
                SourceInfo {
                    kind: "postgres",
                    label: "PostgreSQL",
                    badge: "PG",
                    mode: SourceMode::Catalog,
                    settings: &[],
                    writable: true,
                    unique: &[],
                    scheme: None,
                },
                SourceInfo {
                    kind: "s3",
                    label: "S3",
                    badge: "S3",
                    mode: SourceMode::Store,
                    settings: &[],
                    writable: false,
                    unique: &[],
                    scheme: Some("s3"),
                },
            ],
        }
    }

    /// The join, in one: the bucket takes its links from the store's table defs and the database
    /// takes its schemas from the engine, each wearing the badge the snapshot carries.
    #[test]
    fn a_bucket_takes_its_links_from_the_store_and_a_database_its_schemas_from_the_engine() {
        let nodes = assemble(
            &project(),
            &Answered::default()
                .source_ready("lake")
                .source_ready("analytics")
                .read(),
            &snapshot(vec![
                SourceListing {
                    name: "lake".into(),
                    status: Some(RegStatus::Ready),
                    detail: SourceDetail::Store,
                },
                SourceListing {
                    name: "analytics".into(),
                    status: Some(RegStatus::Ready),
                    detail: SourceDetail::Catalog {
                        catalog: "analytics".into(),
                        writable: false,
                        schemas: vec![
                            SchemaListingView {
                                name: "public".into(),
                                relations: Vec::new(),
                                visibility: SchemaVisibility::Live,
                            },
                            SchemaListingView {
                                name: "private".into(),
                                relations: Vec::new(),
                                visibility: SchemaVisibility::NotEnabled,
                            },
                        ],
                    },
                },
            ]),
        );

        assert_eq!(
            nodes.iter().map(|n| n.name.as_str()).collect::<Vec<_>>(),
            ["lake", "analytics"],
            "in the project's own order"
        );
        assert_eq!(
            nodes[0].contents,
            SourceContents::Store {
                tables: vec!["events".into(), "clicks".into()]
            },
            "the workspace defs that name the bucket"
        );
        assert_eq!(nodes[0].badge, "S3");
        assert!(nodes[0].can_open());

        let SourceContents::Catalog { catalog, schemas } = &nodes[1].contents else {
            panic!("a database opens onto its schemas");
        };
        assert_eq!(catalog, "analytics");
        assert_eq!(
            schemas.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            ["public"],
            "a schema the data source does not show is not a row"
        );
        assert_eq!(nodes[1].badge, "PG");
        assert!(
            nodes
                .iter()
                .all(|node| !node.waiting && node.problem.is_none()),
            "and both wear the engine's verdict"
        );
    }

    /// **A data source the engine has not been told about still draws.** That is every window's
    /// first frames, and a project whose open pass has not reached its data sources yet must show
    /// them waiting rather than show nothing.
    #[test]
    fn a_source_the_engine_has_not_answered_for_still_gets_its_node() {
        let nodes = assemble(&project(), &Registrations::default(), &snapshot(Vec::new()));

        assert_eq!(nodes.len(), 2, "both data sources");
        assert!(nodes.iter().all(|node| node.waiting), "and both waiting");
        assert_eq!(nodes[0].badge, "S3", "badged from the def it does have");
        assert_eq!(
            nodes[1].contents,
            SourceContents::Catalog {
                catalog: "analytics".into(),
                schemas: Vec::new()
            },
            "a database says what it is addressed by before it has answered"
        );
        assert!(!nodes[1].can_open(), "with nothing to open under it");
    }
}
