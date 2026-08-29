//! **The data sources, joined once** — the project's connection rows and the engine's
//! [`SourcesSnapshot`] folded into the one list the catalog tree draws.
//!
//! [`assemble`] is that join, and it is made **once, against one snapshot**, above the tree's
//! walk. Both halves of a connection's row — its badge and status from the project, its schemas
//! from the engine — then describe the same instant, where a lookup per row would be a moment
//! per row. What it produces is a plain value: the walk below it decides shape, and the rows
//! below that draw. Nothing here renders and nothing here reaches the engine.
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
use strata_model::ProviderId;

use super::{ProjectState, Reg};

/// One data source as the tree draws it, resolved.
///
/// Carries what a row paints and no more — the def itself is not cloned, a virtualized tree
/// copying every visible node on every walk. What opens underneath is
/// [`contents`](Self::contents).
#[derive(Clone, PartialEq, Debug)]
pub struct SourceNode {
    /// The connection's name — the handle every gesture addresses it by, and the tree key
    /// `conn/{name}` it is drawn under.
    pub name: String,
    /// Where it points, in its provider's own terms — what the row draws as its title.
    pub address: String,
    /// Which provider serves it, for the row's menu and the editor it opens.
    pub provider: ProviderId,
    /// The short word its row wears: the registered kind's own, asked of the kind so that a
    /// connection nothing has connected yet is still badged for what serves it.
    pub badge: String,
    /// The last pass has not answered for it yet.
    pub waiting: bool,
    /// What the last pass refused it with, if it did.
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

/// Join the project's connection rows with what the engine holds for each.
///
/// Ordered by the project's own rows, which is the order the pane draws and the user rearranges.
/// A connection the engine has not been told about still gets its node, carrying its row's
/// `Loading` and nothing else — a window's first frames are exactly that, and the tree has to
/// draw a project's connections before anything has registered them.
///
/// Bucket links are grouped in **one pass over the tables**: a scan per bucket would cost a
/// project with many tables and many connections their product.
pub fn assemble(project: &ProjectState, snapshot: &SourcesSnapshot) -> Vec<SourceNode> {
    let mut over: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for table in &project.tables {
        if let Some(connection) = table.def.connection.as_deref() {
            over.entry(connection)
                .or_default()
                .push(table.def.name.clone());
        }
    }
    project
        .connections
        .iter()
        .map(|row| {
            let name = row.def.named();
            let listing = snapshot.source(&name);
            let contents = match (row.def.catalog(), listing.map(|l| &l.detail)) {
                (Some(catalog), Some(SourceDetail::Catalog { schemas, .. })) => {
                    SourceContents::Catalog {
                        catalog,
                        schemas: shown(schemas),
                    }
                }
                (Some(catalog), _) => SourceContents::Catalog {
                    catalog,
                    schemas: Vec::new(),
                },
                (None, _) => SourceContents::Store {
                    tables: over.remove(name.as_str()).unwrap_or_default(),
                },
            };
            SourceNode {
                badge: match row.def.provider.source() {
                    Some(source) => snapshot.badge(&source.kind),
                    None => row.def.provider.id().label().to_string(),
                },
                address: row.def.address.clone(),
                provider: row.def.provider.id(),
                waiting: matches!(row.reg, Reg::Loading),
                problem: row.reg.error().map(str::to_owned),
                contents,
                name,
            }
        })
        .collect()
}

/// The namespaces a surface may draw: everything the connection shows, missing ones included, and
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
    use strata_engine::CatalogGen;
    use strata_model::{
        ConnectionDef, Provider, S3Auth, S3Store, SourceDef, SourceFormat, TableDef, TableOrigin,
    };

    use super::*;

    fn bucket(name: &str) -> ConnectionDef {
        ConnectionDef {
            address: "acme-lake".into(),
            name: name.into(),
            provider: Provider::S3(S3Store {
                region: "eu-west-2".into(),
                auth: S3Auth::Ambient,
                ..Default::default()
            }),
            client_config: Default::default(),
        }
    }

    fn database(name: &str) -> ConnectionDef {
        ConnectionDef {
            address: "db.internal:5432/analytics".into(),
            name: name.into(),
            provider: Provider::Source(SourceDef {
                kind: "postgres".into(),
                schemas: vec!["public".into()],
                ..Default::default()
            }),
            client_config: Default::default(),
        }
    }

    fn over(name: &str, connection: &str) -> TableDef {
        TableDef {
            name: name.into(),
            format: SourceFormat::Parquet,
            connection: Some(connection.into()),
            sources: vec![format!("{name}/")],
            partition_cols: Vec::new(),
            origin: TableOrigin::External,
        }
    }

    fn project() -> ProjectState {
        ProjectState::from_defs(
            ProjectDefs {
                name: "test".into(),
                tables: vec![over("events", "lake"), over("clicks", "lake")],
                connections: vec![bucket("lake"), database("analytics")],
                ..Default::default()
            },
            PathBuf::from("/tmp/strata-assemble"),
        )
    }

    /// A snapshot carrying `sources`, with one registrant so a `postgres` def has a badge to
    /// wear — the shape the engine hands over.
    fn snapshot(sources: Vec<SourceListing>) -> SourcesSnapshot {
        SourcesSnapshot {
            generation: CatalogGen::default(),
            sources,
            registrants: vec![SourceInfo {
                kind: "postgres",
                label: "PostgreSQL",
                badge: "PG",
                mode: SourceMode::Catalog,
                keys: &[],
                writable: true,
            }],
        }
    }

    /// The join, in one: the bucket takes its links from the store's table defs and the database
    /// takes its schemas from the engine, each wearing the badge the snapshot carries.
    #[test]
    fn a_bucket_takes_its_links_from_the_store_and_a_database_its_schemas_from_the_engine() {
        let nodes = assemble(
            &project(),
            &snapshot(vec![
                SourceListing {
                    name: "lake".into(),
                    live: true,
                    detail: SourceDetail::Store,
                },
                SourceListing {
                    name: "analytics".into(),
                    live: true,
                    detail: SourceDetail::Catalog {
                        catalog: "analytics".into(),
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
            "a schema the connection does not show is not a row"
        );
        assert_eq!(nodes[1].badge, "PG");
    }

    /// **A connection the engine has not been told about still draws.** That is every window's
    /// first frames, and a project whose open pass has not reached its connections yet must show
    /// them waiting rather than show nothing.
    #[test]
    fn a_connection_the_engine_has_not_answered_for_still_gets_its_node() {
        let nodes = assemble(&project(), &snapshot(Vec::new()));

        assert_eq!(nodes.len(), 2, "both connections");
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
