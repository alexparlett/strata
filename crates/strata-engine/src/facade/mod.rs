//! The facade's six group handles and the accessors that mint them.
//!
//! Each handle borrows the engine and carries the identity its calls are about, so the id is
//! stated once and a new call has an obvious home. Two shape rules hold the whole module
//! together: a handle is `Copy` and every method takes `self`, because with `&self` an
//! `async fn`'s future would borrow a temporary and no caller could hold it; and the mapping is
//! **total**, which this module's test module reads the crate's own source to prove.

mod catalog;
mod lang;
mod snapshot;
mod sources;
mod work;
mod workspace;

pub use catalog::Catalog;
pub use lang::Lang;
pub use snapshot::SnapshotReads;
pub use sources::Sources;
pub use work::Work;
pub use workspace::Workspace;

use strata_model::SnapshotId;

use crate::{Engine, WsId};

impl Engine {
    /// Returns the reads of one immutable snapshot.
    ///
    /// A retired id is not refused here: its reads fail on their own terms, and
    /// [`live`](SnapshotReads::live) tells that apart from a fault.
    pub fn snapshot(&self, snapshot: SnapshotId) -> SnapshotReads<'_> {
        SnapshotReads {
            engine: self,
            snapshot,
        }
    }

    /// Returns one workspace's runs.
    ///
    /// A workspace is minted by being named: an id this engine has never seen is one with
    /// nothing in flight and no snapshot.
    pub fn ws(&self, ws: WsId) -> Workspace<'_> {
        Workspace { engine: self, ws }
    }

    /// Returns this engine's workspace catalog.
    pub fn catalog(&self) -> Catalog<'_> {
        Catalog { engine: self }
    }

    /// Returns this engine's data sources.
    pub fn sources(&self) -> Sources<'_> {
        Sources { engine: self }
    }

    /// Returns this engine's language service.
    pub fn lang(&self) -> Lang<'_> {
        Lang { engine: self }
    }

    /// Returns what this engine has in flight.
    pub fn work(&self) -> Work<'_> {
        Work { engine: self }
    }
}

/// The totality gate: **every public method of the facade is reached through exactly one group,
/// or is in the named root set.**
///
/// A grouping is only worth having if it is complete, and completeness is the thing that decays
/// silently — one `pub fn` added to `impl Engine` because that is where the field is, and the
/// facade has two shapes again. So the inventory below is written down and checked against the
/// source: a new public method fails this test until it is placed, which is the moment to decide
/// where it belongs rather than a year later.
///
/// Reading the text rather than the types is deliberate — Rust has no reflection over an inherent
/// impl, and the alternative (a trait per group, implemented to be enumerated) would shape the
/// API around its own test.
#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    /// The calls that belong to no group, because they are about the **engine** itself — how one
    /// is made, its identity, the project it writes into, and the config it runs with.
    const ROOT: &[&str] = &[
        "builder",
        "id",
        "set_data_dir",
        "set_config",
        "restart_owed",
        "overrides",
        "display",
        "formats",
    ];

    /// The six accessors, which this module adds to `Engine` and `lib.rs` therefore must not.
    const ACCESSORS: &[&str] = &["snapshot", "ws", "catalog", "sources", "lang", "work"];

    /// Every group, its source file, and the calls it carries.
    const GROUPS: &[(&str, &str, &[&str])] = &[
        (
            "SnapshotReads",
            include_str!("snapshot.rs"),
            &[
                "page",
                "chart",
                "trend",
                "export",
                "export_to",
                "pin",
                "live",
            ],
        ),
        (
            "Workspace",
            include_str!("workspace.rs"),
            &["run", "query", "explain", "cancel", "cleanup", "is_running"],
        ),
        (
            "Catalog",
            include_str!("catalog.rs"),
            &[
                "sync",
                "generation",
                "registrations",
                "register",
                "deregister",
                "spec",
                "table_meta",
                "table_spec",
                "detect_partitions",
                "create_view",
                "create_views",
                "drop_view",
                "drop_table",
                "is_internal",
                "profile",
                "cancel_profile",
            ],
        ),
        (
            "Sources",
            include_str!("sources.rs"),
            &[
                "connect",
                "disconnect",
                "listing",
                "show_schemas",
                "database_syms",
                "registrants",
                "check_address",
                "check_unique",
                "dependents",
                "describe_remote",
                "aws_profiles",
            ],
        ),
        (
            "Lang",
            include_str!("lang.rs"),
            &[
                "validate",
                "policy_verdicts",
                "column_type",
                "functions",
                "prepared",
            ],
        ),
        ("Work", include_str!("work.rs"), &["flag", "background"]),
    ];

    /// The `pub fn` / `pub async fn` names declared inside `impl <ty>` blocks in `src`.
    ///
    /// The impl header is matched at column 0 and the methods at one indent, which is what the
    /// whole crate is formatted as; a `pub fn` nested deeper (inside a body, or in a test module)
    /// is therefore not a facade method and is correctly invisible here.
    fn methods(src: &str, ty: &str) -> BTreeSet<String> {
        let mut found = BTreeSet::new();
        let mut inside = false;
        for line in src.lines() {
            if line.starts_with("impl") {
                inside = line.starts_with(&format!("impl {ty} "))
                    || line.starts_with(&format!("impl {ty}<"));
                continue;
            }
            if line == "}" {
                inside = false;
                continue;
            }
            let Some(rest) = line.strip_prefix("    pub ") else {
                continue;
            };
            let rest = rest.strip_prefix("async ").unwrap_or(rest);
            let Some(rest) = rest.strip_prefix("fn ") else {
                continue;
            };
            if inside {
                found.insert(
                    rest.split(|c: char| !(c.is_alphanumeric() || c == '_'))
                        .next()
                        .unwrap_or_default()
                        .to_string(),
                );
            }
        }
        found
    }

    fn declared(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|n| (*n).to_string()).collect()
    }

    /// Nothing is ungrouped: `impl Engine` offers the root set and the six accessors, and not
    /// one method more — anywhere.
    #[test]
    fn the_root_offers_the_groups_and_nothing_else() {
        assert_eq!(
            methods(include_str!("../lib.rs"), "Engine"),
            declared(ROOT),
            "a public method on Engine belongs on a group handle, or in ROOT because it is \
             about the engine itself"
        );
        assert_eq!(
            methods(include_str!("mod.rs"), "Engine"),
            declared(ACCESSORS),
            "the group accessors are the only thing this module adds to Engine"
        );
        for (ty, src, _) in GROUPS {
            assert!(
                methods(src, "Engine").is_empty(),
                "{ty}'s file adds a method to Engine; it belongs on the handle or in ROOT"
            );
        }
    }

    /// Each group carries exactly what it says it carries.
    #[test]
    fn every_group_is_its_inventory() {
        for (ty, src, inventory) in GROUPS {
            assert_eq!(
                methods(src, ty),
                declared(inventory),
                "{ty}'s public methods have moved on from the inventory above"
            );
        }
    }

    /// Exactly one group: no call is offered under the same name by two of them, so "which
    /// handle is `export` on" has one answer.
    #[test]
    fn the_groups_do_not_overlap() {
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        for (ty, _, inventory) in GROUPS {
            for name in *inventory {
                assert!(seen.insert(name), "'{name}' is offered twice, once by {ty}");
            }
        }
    }
}
