//! Enriching the SQL function catalog from the live DataFusion registry (F5), and holding it
//! **swappably** so a session-created function is in it (ED-09).
//!
//! This is the **only** place a [`FunctionSym`]'s signature/return strings are
//! produced — it touches DataFusion's `ScalarUDF`/`AggregateUDF`/`WindowUDF`
//! (`signature()`, `return_type()`, `documentation()`) and renders everything to
//! plain display strings at registry-snapshot time, so the language service and UI
//! never depend on DataFusion's type model.
//!
//! [`snapshot`] used to run exactly once, at `Engine::new`, into an immutable field — which was
//! true of the registry until `CREATE FUNCTION` could move it. [`Functions`] is that field made
//! swappable: the catalog is re-walked by the statement that changed the registry and by nothing
//! else, so the built-in set costs the same one walk it always did.

use std::collections::{BTreeSet, HashSet};
use std::iter;
use std::sync::{Arc, RwLock};

use datafusion::arrow::datatypes::DataType;
use datafusion::execution::registry::FunctionRegistry;
use datafusion::logical_expr::{AggregateUDF, ScalarUDF, Signature, TypeSignature, WindowUDF};
use datafusion::prelude::SessionContext;

use crate::sql::{FnKind, FunctionCatalog, FunctionSym, VARIADIC};

/// The engine's function catalog, plus which of its names this session **created**.
///
/// **Shared by handle** for the reason `InternalTables` and `SessionScope` are: the arms that move
/// it run inside the task `Engine::bookkeep` spawned, and that task must not hold the engine — the
/// engine's `Drop` is what aborts it. It holds values only, so it outlives an engine harmlessly,
/// and a fresh engine walks a fresh registry, which is what makes "a restart clears the created
/// functions" true by construction rather than by a teardown step somebody has to remember.
///
/// The two halves move together because they answer one question between them. The catalog is
/// what the completion row resolves against — the name, and the argument list as its dim detail,
/// which is where this codebase puts signature help; `created` is what
/// distinguishes a function this session made from a **built-in**, which `statements::arms::functions` needs
/// because DataFusion's registry cannot tell them apart and its `DROP FUNCTION` would deregister
/// either with nothing able to put a built-in back.
#[derive(Clone, Debug)]
pub struct Functions(Arc<RwLock<Registry>>);

#[derive(Debug)]
struct Registry {
    /// Handed out by the `Arc`, never cloned: the language service rebuilds its `Catalog`
    /// snapshot on every catalog epoch, and deep-copying ~1000 symbols per rebuild is the one
    /// thing that would make that pass felt.
    catalog: Arc<FunctionCatalog>,
    /// [`fold_ident`](crate::fold_ident)ed names of the functions this session created.
    created: BTreeSet<String>,
}

impl Functions {
    /// Walk `ctx`'s registry into the initial catalog — the built-in set, once per engine.
    /// Nothing is created yet, so no sym needs marking.
    pub(crate) fn new(ctx: &SessionContext) -> Functions {
        Functions(Arc::new(RwLock::new(Registry {
            catalog: Arc::new(snapshot(ctx)),
            created: BTreeSet::new(),
        })))
    }

    /// The catalog as it stands. An `Arc`, so a caller holding it across an await sees the set it
    /// asked for rather than one a concurrent `CREATE FUNCTION` moved underneath it.
    pub fn catalog(&self) -> Arc<FunctionCatalog> {
        Arc::clone(&self.0.read().unwrap().catalog)
    }

    /// Whether `name` (already folded) is a function **this session created** — `false` for a
    /// built-in and for a name nothing registered.
    pub fn created(&self, name: &str) -> bool {
        self.0.read().unwrap().created.contains(name)
    }

    /// Record what a `CREATE FUNCTION` / `DROP FUNCTION` settled, and re-walk the registry.
    ///
    /// Called **after** the dispatch that moved the registry, so the catalog is read from what
    /// DataFusion now holds rather than from what the statement claimed. The created set is
    /// mutated **in place, first** — a clone-and-swap would let two concurrent settles erase
    /// each other's record, and this set is the built-in fence's authority — and the syms are
    /// marked from the live set at swap time for the same reason. Only the walk itself runs
    /// with no lock held: it resolves every overload's return type, which is not work to do
    /// with the completion pool's reader shut out.
    pub(crate) fn settle(&self, ctx: &SessionContext, name: &str, created: bool) {
        {
            let mut registry = self.0.write().unwrap();
            match created {
                true => registry.created.insert(name.to_string()),
                false => registry.created.remove(name),
            };
        }
        let mut catalog = snapshot(ctx);
        let mut registry = self.0.write().unwrap();
        for sym in catalog
            .scalar
            .iter_mut()
            .chain(catalog.aggregate.iter_mut())
            .chain(catalog.window.iter_mut())
        {
            sym.created = registry.created.contains(&sym.name);
        }
        registry.catalog = Arc::new(catalog);
    }
}

/// Snapshot every registered function (built-ins + UDFs) into a [`FunctionCatalog`],
/// enriched with overload signatures + return type. The `created` marks are stamped
/// by [`Functions::settle`] from the live set **under its lock** — marking here from
/// a snapshot of the set is what would let two concurrent settles publish stale
/// marks. Names are sorted so the completion pool is stable.
fn snapshot(ctx: &SessionContext) -> FunctionCatalog {
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
    scalar.sort_by(|a, b| a.name.cmp(&b.name));
    aggregate.sort_by(|a, b| a.name.cmp(&b.name));
    window.sort_by(|a, b| a.name.cmp(&b.name));
    FunctionCatalog {
        scalar,
        aggregate,
        window,
    }
}

fn sorted(names: HashSet<String>) -> Vec<String> {
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
        created: false,
    }
}

fn aggregate_sym(udaf: &AggregateUDF) -> FunctionSym {
    FunctionSym {
        name: udaf.name().to_string(),
        kind: FnKind::Aggregate,
        signatures: signatures(udaf.signature()),
        ret: return_type(udaf.signature(), |args| udaf.return_type(args)),
        description: udaf.documentation().map(|d| d.description.clone()),
        created: false,
    }
}

fn window_sym(udwf: &WindowUDF) -> FunctionSym {
    FunctionSym {
        name: udwf.name().to_string(),
        kind: FnKind::Window,
        signatures: signatures(udwf.signature()),
        ret: None,
        description: udwf.documentation().map(|d| d.description.clone()),
        created: false,
    }
}

/// Render a signature's overloads to parameter-label lists, applying the registry's
/// parameter names when it provides a set matching an overload's arity.
fn signatures(sig: &Signature) -> Vec<Vec<String>> {
    let mut overloads = dedup(render(&sig.type_signature));
    if let Some(names) = &sig.parameter_names {
        for o in &mut overloads {
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
    let repeat = |label: &str, n: usize| vec![iter::repeat_n(label.to_string(), n).collect()];
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
