//! Adding SQL functions to an engine. See [`UdfPackage`].

use std::sync::Arc;

use datafusion::execution::FunctionRegistry;
use datafusion::logical_expr::{AggregateUDF, ScalarUDF, WindowUDF};
use datafusion::prelude::SessionContext;

/// Provides SQL functions to register on an engine.
///
/// Implement the methods for the kinds of function the package provides; the rest default to
/// empty. Add a package with [`EngineBuilder::with_udfs`](crate::EngineBuilder::with_udfs).
///
/// A function whose name is already registered replaces it, and the replacement is logged.
///
/// # Example
///
/// ```
/// use datafusion::logical_expr::ScalarUDF;
/// use strata_engine::UdfPackage;
///
/// struct Geo;
///
/// impl UdfPackage for Geo {
///     fn scalar(&self) -> Vec<ScalarUDF> {
///         Vec::new()
///     }
/// }
/// ```
pub trait UdfPackage: Send + Sync {
    /// Return the package's scalar functions
    fn scalar(&self) -> Vec<ScalarUDF> {
        Vec::new()
    }

    /// Return the package's aggregate functions
    fn aggregate(&self) -> Vec<AggregateUDF> {
        Vec::new()
    }

    /// Return the package's window functions
    fn window(&self) -> Vec<WindowUDF> {
        Vec::new()
    }
}

impl<T: UdfPackage + ?Sized> UdfPackage for Arc<T> {
    fn scalar(&self) -> Vec<ScalarUDF> {
        (**self).scalar()
    }

    fn aggregate(&self) -> Vec<AggregateUDF> {
        (**self).aggregate()
    }

    fn window(&self) -> Vec<WindowUDF> {
        (**self).window()
    }
}

impl<T: UdfPackage + ?Sized> UdfPackage for Box<T> {
    fn scalar(&self) -> Vec<ScalarUDF> {
        (**self).scalar()
    }

    fn aggregate(&self) -> Vec<AggregateUDF> {
        (**self).aggregate()
    }

    fn window(&self) -> Vec<WindowUDF> {
        (**self).window()
    }
}

/// Register every package's functions, in the order the packages were added.
///
/// One rule for all of them: a name the registry already holds is **replaced**, and replacing it
/// is warned about. `register_udf` and its siblings are silent about that, so a package could
/// otherwise take a built-in's name and destroy it for the session with nothing said. A function
/// that will not register is warned about rather than fatal — it names itself on the first query
/// that wanted it, and refusing to open the project over one is the worse trade.
pub(crate) fn register_packages(ctx: &SessionContext, packages: &[Arc<dyn UdfPackage>]) {
    let state = ctx.state_ref();
    let mut state = state.write();
    for package in packages {
        for udf in package.scalar() {
            let name = udf.name().to_string();
            note_registered(
                &name,
                state.register_udf(Arc::new(udf)).map(|old| old.is_some()),
            );
        }
        for udaf in package.aggregate() {
            let name = udaf.name().to_string();
            note_registered(
                &name,
                state.register_udaf(Arc::new(udaf)).map(|old| old.is_some()),
            );
        }
        for udwf in package.window() {
            let name = udwf.name().to_string();
            note_registered(
                &name,
                state.register_udwf(Arc::new(udwf)).map(|old| old.is_some()),
            );
        }
    }
}

/// Say what one registration did, when it is worth saying: it replaced something, or it failed.
fn note_registered(name: &str, outcome: datafusion::error::Result<bool>) {
    match outcome {
        Ok(true) => tracing::warn!("engine: '{name}' was already registered and has been replaced"),
        Ok(false) => {}
        Err(e) => tracing::warn!("engine: '{name}' could not be registered: {e}"),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::any::Any;

    use datafusion::arrow::datatypes::{DataType, FieldRef};
    use datafusion::error::Result as DFResult;
    use datafusion::functions_aggregate::count::Count;
    use datafusion::functions_window::row_number::RowNumber;
    use datafusion::logical_expr::function::{
        AccumulatorArgs, PartitionEvaluatorArgs, WindowUDFFieldArgs,
    };
    use datafusion::logical_expr::{
        Accumulator, AggregateUDFImpl, ColumnarValue, PartitionEvaluator, ScalarFunctionArgs,
        ScalarUDFImpl, Signature, Volatility, WindowUDFImpl,
    };
    use datafusion::scalar::ScalarValue;

    use super::*;
    use crate::udfs::StrataFunctions;
    use crate::Engine;

    /// The smallest honest function: one name, one constant.
    #[derive(Debug, PartialEq, Eq, Hash)]
    struct Answer {
        name: String,
        signature: Signature,
    }

    impl ScalarUDFImpl for Answer {
        fn name(&self) -> &str {
            &self.name
        }

        fn signature(&self) -> &Signature {
            &self.signature
        }

        fn return_type(&self, _args: &[DataType]) -> DFResult<DataType> {
            Ok(DataType::Int64)
        }

        fn invoke_with_args(&self, _args: ScalarFunctionArgs) -> DFResult<ColumnarValue> {
            Ok(ColumnarValue::Scalar(ScalarValue::Int64(Some(42))))
        }
    }

    /// `count` under another name, so the aggregate arm is driven by a function that works rather
    /// than by a stub.
    #[derive(Debug, PartialEq, Eq, Hash)]
    struct Tally {
        name: String,
        inner: Count,
    }

    impl AggregateUDFImpl for Tally {
        fn name(&self) -> &str {
            &self.name
        }

        fn signature(&self) -> &Signature {
            self.inner.signature()
        }

        fn return_type(&self, args: &[DataType]) -> DFResult<DataType> {
            self.inner.return_type(args)
        }

        fn accumulator(&self, args: AccumulatorArgs) -> DFResult<Box<dyn Accumulator>> {
            self.inner.accumulator(args)
        }
    }

    /// `row_number` under another name, for the window arm.
    #[derive(Debug, PartialEq, Eq, Hash)]
    struct Ordinal {
        name: String,
        inner: RowNumber,
    }

    impl WindowUDFImpl for Ordinal {
        fn name(&self) -> &str {
            &self.name
        }

        fn signature(&self) -> &Signature {
            self.inner.signature()
        }

        fn partition_evaluator(
            &self,
            args: PartitionEvaluatorArgs,
        ) -> DFResult<Box<dyn PartitionEvaluator>> {
            self.inner.partition_evaluator(args)
        }

        fn field(&self, args: WindowUDFFieldArgs) -> DFResult<FieldRef> {
            self.inner.field(args)
        }
    }

    /// An embedder's package, in the smallest form that registers something. `pub(crate)` because
    /// the builder's own tests need a package to hand to `with_udfs`.
    pub(crate) struct OnePackage(pub &'static str);

    impl UdfPackage for OnePackage {
        fn scalar(&self) -> Vec<ScalarUDF> {
            vec![ScalarUDF::from(Answer {
                name: self.0.to_string(),
                signature: Signature::nullary(Volatility::Immutable),
            })]
        }
    }

    /// A package offering one function of every kind a package can offer.
    struct EveryKind;

    impl UdfPackage for EveryKind {
        fn scalar(&self) -> Vec<ScalarUDF> {
            OnePackage("kind_scalar").scalar()
        }

        fn aggregate(&self) -> Vec<AggregateUDF> {
            vec![AggregateUDF::from(Tally {
                name: "kind_aggregate".to_string(),
                inner: Count::new(),
            })]
        }

        fn window(&self) -> Vec<WindowUDF> {
            vec![WindowUDF::from(Ordinal {
                name: "kind_window".to_string(),
                inner: RowNumber::new(),
            })]
        }
    }

    /// The clause every package keeps: an engine built with it resolves every name the package
    /// registered.
    fn conforms(package: impl UdfPackage + 'static, expected: &[&str]) {
        let engine = Engine::builder().with_udfs(package).build();
        let catalog = engine.functions();
        for name in expected {
            assert!(catalog.contains(name), "'{name}' should be registered");
        }
    }

    #[test]
    fn every_package_registers_what_it_names() {
        conforms(StrataFunctions, &["struct_keys", "regexp_extract_all"]);
        conforms(OnePackage("embedder_answer"), &["embedder_answer"]);
    }

    /// Strata's own package is the first one, and an embedder's is added rather than substituted.
    #[test]
    fn an_added_package_joins_the_built_ins_rather_than_replacing_them() {
        let engine = Engine::builder()
            .with_udfs(OnePackage("embedder_answer"))
            .build();
        let catalog = engine.functions();
        assert!(catalog.contains("struct_keys"));
        assert!(catalog.contains("embedder_answer"));
    }

    /// Every kind a package can offer reaches the registry it belongs in — the three loops
    /// [`register_packages`] runs, each asserted in its own category rather than across all of
    /// them.
    #[test]
    fn every_kind_of_function_a_package_offers_is_registered() {
        let engine = Engine::builder().with_udfs(EveryKind).build();
        let catalog = engine.functions();
        assert!(catalog.scalar.iter().any(|f| f.name == "kind_scalar"));
        assert!(catalog.aggregate.iter().any(|f| f.name == "kind_aggregate"));
        assert!(catalog.window.iter().any(|f| f.name == "kind_window"));
    }

    /// A package taking a name the registry already holds **replaces** it. The engine applies that
    /// rule for every package, so an embedder cannot register over a built-in by a route that
    /// skips it.
    #[test]
    fn a_package_that_takes_a_registered_name_replaces_it() {
        let engine = Engine::builder()
            .with_udfs(OnePackage("struct_keys"))
            .build();
        let udf = engine.ctx.udf("struct_keys").expect("still registered");
        assert!(
            (udf.inner().as_ref() as &dyn Any).is::<Answer>(),
            "the later package's function should be the one that resolves"
        );
        assert!(
            engine.functions().contains("struct_keys"),
            "and the catalog should still know the name"
        );
    }

    /// A package an embedder already shares reaches the slot.
    #[test]
    fn a_shared_package_is_a_package_too() {
        let engine = Engine::builder()
            .with_udfs(Arc::new(OnePackage("shared_answer")))
            .with_udfs(Box::new(OnePackage("boxed_answer")) as Box<dyn UdfPackage>)
            .build();
        let catalog = engine.functions();
        assert!(catalog.contains("shared_answer"));
        assert!(catalog.contains("boxed_answer"));
    }
}
