//! The contract every [`DataSource`] keeps, as a body any registrant can be run through.
//!
//! This is the **generic ring**: what a source of any shape has to answer, whether it speaks SQL
//! or nothing like it. The **SQL ring** — pushdown proven, JSON exercised, one `compute_context`
//! per connection — is only meaningful against a real server, so it lives in the container suites
//! and is a shape to reproduce rather than a body to call. [`crate::guide::data_source`] says how.
//!
//! Available to embedders under the `testing` cargo feature.

use std::sync::Arc;

use datafusion::sql::TableReference;

use strata_model::SourceDef;

use super::source::{
    unsupported, DataSource, Located, SourceKind, SourceMode, SourceSetting, Sourced,
};
use crate::secrets::{MemSecrets, SecretProvider};
use crate::statements::Remote;

/// Runs `source` through the generic ring, panicking on the first thing it does not keep.
///
/// `def` has to be one `source` can connect: whatever address, credentials and settings it needs,
/// already filled in. A catalog source must hold at least one relation, which the body reads back
/// through the source's own provider.
///
/// What it asserts: the form the source declares can be drawn; connecting yields the mode the kind
/// declared; the handle names the kind it was registered under; the enumeration is non-empty; a
/// relation resolves to a provider; anything the source does not implement refuses in the trait's
/// own words rather than in an arm's; and [`SourceKind::WRITABLE`] agrees with whether
/// [`SourceCatalog::writer`](super::source::SourceCatalog::writer) is really there — in **both**
/// directions, so a source cannot say it
/// is read-only while holding a writer, or say it is writable and have none.
///
/// # Panics
///
/// On any of the above.
pub async fn conforms<S: DataSource + SourceKind>(source: S, def: &SourceDef) {
    conforms_with(source, def, Arc::new(MemSecrets::new())).await;
}

/// [`conforms`], for a source whose `connect` needs secrets a [`MemSecrets`] does not hold.
///
/// # Panics
///
/// As [`conforms`].
pub async fn conforms_with<S: DataSource + SourceKind>(
    source: S,
    def: &SourceDef,
    secrets: Arc<dyn SecretProvider>,
) {
    let mode = S::MODE;
    let kind = S::NAME;
    declares_a_drawable_form(kind, source.settings());
    let connected = source
        .connect(def, secrets)
        .await
        .unwrap_or_else(|why| panic!("'{kind}' could not connect the def it was given: {why}"));
    let catalog = match (connected, mode) {
        (Sourced::Catalog(catalog), SourceMode::Catalog) => catalog,
        (Sourced::Store { .. }, SourceMode::Store) => return,
        _ => panic!("'{kind}' connected as something other than the mode it declares"),
    };
    assert_eq!(catalog.kind(), kind, "the handle names its own kind");

    let listing = catalog.enumerate().await.expect("an enumeration");
    let (schema, relation) = listing
        .schemas()
        .values()
        .find_map(|schema| {
            let first = schema.relations.values().next()?;
            Some((schema.name.clone(), first.name.clone()))
        })
        .unwrap_or_else(|| panic!("'{kind}' enumerated nothing to read"));

    let at = Located {
        source: def.named(),
        identity: def.named(),
        relation: TableReference::partial(schema.clone(), relation.clone()),
    };
    let read = catalog
        .clone()
        .table_provider(&at)
        .await
        .unwrap_or_else(|why| panic!("'{kind}' would not read '{schema}.{relation}': {why}"));
    let schema_ref = read.schema();

    if let Err(why) = catalog.execute_text("SELECT 1").await {
        assert_eq!(
            why,
            unsupported(kind, "run a statement of its own"),
            "a refusal a source does not word itself is the trait's own"
        );
    }
    let target = Remote {
        source: def.named(),
        reference: TableReference::full(def.named(), schema, relation),
    };
    if let Err(why) = catalog.create_relation(&target, schema_ref.clone()).await {
        assert_eq!(why, unsupported(kind, "have relations created in it"));
    }
    match (S::WRITABLE, catalog.writer(read, &target, schema_ref)) {
        (false, Err(why)) => assert_eq!(
            why,
            unsupported(kind, "be written to"),
            "'{kind}' says it is not writable but refuses in its own words, so it has a writer it \
             is not admitting to"
        ),
        (false, Ok(_)) => panic!("'{kind}' says it is not writable and then wrote"),
        (true, Err(why)) => assert_ne!(
            why,
            unsupported(kind, "be written to"),
            "'{kind}' says it is writable and has no writer"
        ),
        (true, Ok(_)) => {}
    }
}

/// Asserts that the settings `kind` declares are ones a form can draw.
///
/// Three things nothing else checks, because none of them shows up as a failure anywhere: the
/// editor simply draws a form missing a setting the source needs, or drawing one twice.
///
/// A [`When`](super::source::When) naming a key that is not declared beside it hides its row
/// **forever** — the deciding value can never be typed, because there is no box to type it in. A
/// duplicate key gives one setting two rows, whose values overwrite each other. And a group
/// interrupted by another group's key prints its heading twice.
///
/// # Panics
///
/// On any of the above.
pub fn declares_a_drawable_form(kind: &str, keys: &[SourceSetting]) {
    let mut seen_groups: Vec<Option<&str>> = Vec::new();
    for declared in keys {
        assert_eq!(
            keys.iter()
                .filter(|other| other.key == declared.key)
                .count(),
            1,
            "'{kind}' declares '{}' twice",
            declared.key
        );
        if seen_groups.last() != Some(&declared.group) {
            assert!(
                !seen_groups.contains(&declared.group),
                "'{kind}' returns to the group {:?} after leaving it, so its heading is printed \
                 twice",
                declared.group
            );
            seen_groups.push(declared.group);
        }
        let Some(when) = declared.when else { continue };
        assert!(
            keys.iter().any(|other| other.key == when.key),
            "'{kind}' shows '{}' by '{}', which it does not declare, so the row can never appear",
            declared.key,
            when.key
        );
        assert!(
            !when.values.is_empty(),
            "'{kind}' shows '{}' by no value of '{}', which is the same as never",
            declared.key,
            when.key
        );
    }
}
