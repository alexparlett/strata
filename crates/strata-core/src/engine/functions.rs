//! Enriching the SQL function catalog from the live DataFusion registry (F5).
//!
//! This is the **only** place a [`FunctionSym`]'s signature/return strings are
//! produced — it touches DataFusion's `ScalarUDF`/`AggregateUDF`/`WindowUDF`
//! (`signature()`, `return_type()`, `documentation()`) and renders everything to
//! plain display strings at registry-snapshot time, so the language service and UI
//! never depend on DataFusion's type model. Called once per engine (`Engine::new`).

use datafusion::arrow::datatypes::DataType;
use datafusion::execution::registry::FunctionRegistry;
use datafusion::logical_expr::{
    AggregateUDF, ScalarUDF, Signature, TypeSignature, WindowUDF,
};
use datafusion::prelude::SessionContext;

use crate::engine::sql::{FnKind, FunctionCatalog, FunctionSym, VARIADIC};

/// Snapshot every registered function (built-ins + UDFs) into a [`FunctionCatalog`],
/// enriched with overload signatures + return type. Names are sorted so the
/// completion pool is stable.
pub(crate) fn snapshot(ctx: &SessionContext) -> FunctionCatalog {
    let mut scalar: Vec<FunctionSym> = sorted(ctx.udfs())
        .iter()
        .filter_map(|n| ctx.udf(n).ok())
        .map(|u| scalar_sym(&u))
        .collect();
    let mut aggregate: Vec<FunctionSym> = sorted(ctx.udafs())
        .iter()
        .filter_map(|n| ctx.udaf(n).ok())
        .map(|u| aggregate_sym(&u))
        .collect();
    let mut window: Vec<FunctionSym> = sorted(ctx.udwfs())
        .iter()
        .filter_map(|n| ctx.udwf(n).ok())
        .map(|u| window_sym(&u))
        .collect();
    // `sorted` orders the *names*; the filter_map preserves that order, but keep the
    // explicit sort so a future change to the pipeline can't silently unsort.
    scalar.sort_by(|a, b| a.name.cmp(&b.name));
    aggregate.sort_by(|a, b| a.name.cmp(&b.name));
    window.sort_by(|a, b| a.name.cmp(&b.name));
    FunctionCatalog {
        scalar,
        aggregate,
        window,
    }
}

fn sorted(names: std::collections::HashSet<String>) -> Vec<String> {
    let mut v: Vec<String> = names.into_iter().collect();
    v.sort();
    v
}

fn scalar_sym(udf: &ScalarUDF) -> FunctionSym {
    FunctionSym {
        name: udf.name().to_string(),
        kind: FnKind::Scalar,
        signatures: signatures(udf.signature()),
        ret: return_type(udf.signature(), |args| udf.return_type(args)),
        description: udf.documentation().map(|d| d.description.clone()),
    }
}

fn aggregate_sym(udaf: &AggregateUDF) -> FunctionSym {
    FunctionSym {
        name: udaf.name().to_string(),
        kind: FnKind::Aggregate,
        signatures: signatures(udaf.signature()),
        ret: return_type(udaf.signature(), |args| udaf.return_type(args)),
        description: udaf.documentation().map(|d| d.description.clone()),
    }
}

fn window_sym(udwf: &WindowUDF) -> FunctionSym {
    FunctionSym {
        name: udwf.name().to_string(),
        kind: FnKind::Window,
        signatures: signatures(udwf.signature()),
        // A window function's return type comes from `field(WindowUDFFieldArgs)` —
        // it needs the concrete input fields, so there is no honest argument-free
        // answer. Left unset rather than guessed.
        ret: None,
        description: udwf.documentation().map(|d| d.description.clone()),
    }
}

/// Render a signature's overloads to parameter-label lists, applying the registry's
/// parameter names when it provides a set matching an overload's arity.
fn signatures(sig: &Signature) -> Vec<Vec<String>> {
    let mut overloads = dedup(render(&sig.type_signature));
    if let Some(names) = &sig.parameter_names {
        for o in overloads.iter_mut() {
            if o.len() == names.len() {
                *o = names.clone();
            }
        }
    }
    overloads
}

/// Best-effort return type: feed the signature's own example argument types (the
/// same set `information_schema` uses) into `return_type`; `None` if the signature
/// admits no examples or the resolver declines them.
///
/// Crucially we only call the resolver with a **representative** argument set: a
/// non-empty example, or an empty one only when the signature genuinely takes zero
/// arguments. Several UDFs (`array_any_value`, …) index `arg_types[0]` unguarded
/// and *panic* on an empty slice, so a blind `return_type(&[])` on a non-nullary
/// function would crash engine construction.
fn return_type<F>(sig: &Signature, resolve: F) -> Option<String>
where
    F: Fn(&[DataType]) -> datafusion::error::Result<DataType>,
{
    let example = sig.type_signature.get_example_types().into_iter().next();
    let args = match example {
        Some(a) if !a.is_empty() => a,
        _ if sig.type_signature.supports_zero_argument() => Vec::new(),
        // No representative arguments — don't risk an unguarded resolver.
        _ => return None,
    };
    resolve(&args).ok().map(|t| short_type(&t))
}

/// One [`TypeSignature`] → its overloads, each a list of parameter labels. Mirrors
/// DataFusion's own `to_string_repr`, but keeps parameters **structured** (one Vec
/// element per argument) so signature help can highlight the active one without
/// re-splitting a joined string (arrow `DataType` displays such as
/// `Timestamp(Nanosecond, None)` contain commas). A trailing [`VARIADIC`] marks an
/// open-ended tail.
fn render(ts: &TypeSignature) -> Vec<Vec<String>> {
    use TypeSignature as TS;
    let repeat = |label: &str, n: usize| vec![std::iter::repeat_n(label.to_string(), n).collect()];
    match ts {
        TS::Nullary => vec![vec![]],
        TS::Exact(types) => vec![types.iter().map(short_type).collect()],
        TS::Coercible(coercions) => {
            vec![coercions.iter().map(ToString::to_string).collect()]
        }
        TS::Uniform(n, valid) => repeat(&join(valid), *n),
        TS::Variadic(types) => vec![vec![join(types), VARIADIC.to_string()]],
        TS::VariadicAny => vec![vec![VARIADIC.to_string()]],
        TS::Any(n) => repeat("any", *n),
        TS::Numeric(n) => repeat("numeric", *n),
        TS::String(n) => repeat("string", *n),
        TS::Comparable(n) => repeat("comparable", *n),
        TS::OneOf(sigs) => sigs.iter().flat_map(render).collect(),
        TS::ArraySignature(a) => vec![vec![a.to_string()]],
        // The function computes its own coercion — arity is genuinely unknown. No
        // overload; `FunctionSym::doc`/signature help fall back to `name(…)`.
        TS::UserDefined => vec![],
    }
}

fn join(types: &[DataType]) -> String {
    types.iter().map(short_type).collect::<Vec<_>>().join("/")
}

/// A **compact** display for an arrow type — the base variant, dropping the verbose
/// parameters that make signatures unreadable (`Timestamp(Nanosecond, "+TZ")` →
/// `Timestamp`, `Decimal128(38, 10)` → `Decimal`, `List(Field { … })` → `List`).
/// Plain scalar types keep their normal short display (`Utf8`, `Int64`).
fn short_type(t: &DataType) -> String {
    use DataType::*;
    match t {
        Timestamp(..) => "Timestamp".into(),
        Time32(_) | Time64(_) => "Time".into(),
        Date32 | Date64 => "Date".into(),
        Duration(_) => "Duration".into(),
        Interval(_) => "Interval".into(),
        Decimal128(..) | Decimal256(..) => "Decimal".into(),
        List(_) | LargeList(_) | FixedSizeList(..) => "List".into(),
        Struct(_) => "Struct".into(),
        Map(..) => "Map".into(),
        Dictionary(_, value) => short_type(value),
        // Plain scalars (Utf8, Int64, Boolean, …) display short already.
        other => other.to_string(),
    }
}

/// Drop duplicate overloads (a `OneOf` frequently repeats an arity across coercion
/// variants), then order by arity then lexically so the docs panel reads shortest
/// form first.
fn dedup(overloads: Vec<Vec<String>>) -> Vec<Vec<String>> {
    let mut out: Vec<Vec<String>> = Vec::new();
    for o in overloads {
        if !out.contains(&o) {
            out.push(o);
        }
    }
    out.sort_by(|a, b| a.len().cmp(&b.len()).then_with(|| a.cmp(b)));
    out
}
