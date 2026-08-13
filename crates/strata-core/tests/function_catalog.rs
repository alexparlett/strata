//! The enriched function catalog against the **real** DataFusion registry (P2-22):
//! `Engine::new` must snapshot every built-in into a `FunctionSym` with sensible
//! overload signatures + (where resolvable) a return type — what the autocomplete
//! detail is rendered from. Structural assertions only — exact type spellings belong
//! to DataFusion and would be brittle to pin.

use std::sync::Arc;

use strata_core::engine::sql::FunctionCatalog;
use strata_core::engine::Engine;

fn functions() -> Arc<FunctionCatalog> {
    Engine::new(Default::default()).functions()
}

#[test]
fn every_category_is_populated() {
    let f = functions();
    assert!(
        f.scalar.len() > 100,
        "scalar built-ins enumerated: {}",
        f.scalar.len()
    );
    assert!(!f.aggregate.is_empty(), "aggregates enumerated");
    assert!(!f.window.is_empty(), "window fns enumerated");
    assert!(f.scalar.iter().all(|s| !s.name.is_empty()));
}

#[test]
fn round_has_a_two_argument_overload_and_a_detail() {
    let f = functions();
    let round = f.get("round").expect("round is registered");
    assert!(
        round.signatures.iter().any(|o| o.len() == 2),
        "round has a 2-arg overload: {:?}",
        round.signatures
    );
    assert!(
        round.detail().starts_with('('),
        "arity detail: {}",
        round.detail()
    );
    assert!(
        round.detail().contains("[, "),
        "optional 2nd arg bracketed: {}",
        round.detail()
    );
    assert!(round.doc().contains("round("), "{}", round.doc());
    assert!(round.doc().starts_with("scalar function"));
}

#[test]
fn concat_is_variadic() {
    let f = functions();
    let concat = f.get("concat").expect("concat is registered");
    assert!(
        concat
            .signatures
            .iter()
            .any(|o| o.last().map(String::as_str) == Some("…")),
        "concat renders a variadic tail: {:?}",
        concat.signatures
    );
}

#[test]
fn aggregate_return_type_resolves() {
    let f = functions();
    let count = f.get("count").expect("count is registered");
    assert!(count.ret.is_some(), "count resolves a return type");
}
