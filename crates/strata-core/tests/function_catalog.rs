//! The enriched function catalog against the **real** DataFusion registry (P2-22):
//! `Engine::new` must snapshot every built-in into a `FunctionSym` with sensible
//! overload signatures + (where resolvable) a return type — what the autocomplete
//! detail is rendered from. Structural assertions only — exact type spellings belong
//! to DataFusion and would be brittle to pin.

use strata_core::engine::sql::FunctionCatalog;
use strata_core::engine::Engine;

fn functions() -> FunctionCatalog {
    Engine::new(Default::default()).functions().clone()
}

#[test]
fn every_category_is_populated() {
    let f = functions();
    assert!(f.scalar.len() > 100, "scalar built-ins enumerated: {}", f.scalar.len());
    assert!(!f.aggregate.is_empty(), "aggregates enumerated");
    assert!(!f.window.is_empty(), "window fns enumerated");
    // Names are unique-per-category and every sym carries its own name.
    assert!(f.scalar.iter().all(|s| !s.name.is_empty()));
}

#[test]
fn round_has_a_two_argument_overload_and_a_detail() {
    let f = functions();
    let round = f.get("round").expect("round is registered");
    // round(x) and round(x, places) — at least one binary overload.
    assert!(
        round.signatures.iter().any(|o| o.len() == 2),
        "round has a 2-arg overload: {:?}",
        round.signatures
    );
    // The completion detail is the bracketed arity form, not the flat "function".
    assert!(round.detail().starts_with('('), "arity detail: {}", round.detail());
    assert!(round.detail().contains("[, "), "optional 2nd arg bracketed: {}", round.detail());
    // The docs body names the function and its category.
    assert!(round.doc().contains("round("), "{}", round.doc());
    assert!(round.doc().starts_with("scalar function"));
}

#[test]
fn concat_is_variadic() {
    let f = functions();
    let concat = f.get("concat").expect("concat is registered");
    // A variadic tail is marked by the trailing ellipsis parameter.
    assert!(
        concat.signatures.iter().any(|o| o.last().map(String::as_str) == Some("…")),
        "concat renders a variadic tail: {:?}",
        concat.signatures
    );
}

#[test]
fn aggregate_return_type_resolves() {
    let f = functions();
    // count is a stable aggregate whose return type resolves argument-free (Int64).
    let count = f.get("count").expect("count is registered");
    assert!(count.ret.is_some(), "count resolves a return type");
}
