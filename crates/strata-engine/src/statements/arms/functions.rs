//! **SQL functions** — `CREATE FUNCTION` for SQL-bodied scalar macros, and the
//! `DROP FUNCTION` that takes one back. `docs/STATEMENTS_SPEC.md` §6.6.
//!
//! Both run **natively** over DataFusion's own [`FunctionFactory`] seam, which is the framework's
//! shape for "what a created function *is*": a `CreateFunction` in, a `ScalarUDF` out.
//!
//! What is ours is [`Definition::read`] — the one judgement of a `CREATE FUNCTION`, called by the
//! arm for the sentence the user reads and by the factory to build from, so a form the arm accepts
//! is one the factory can build by construction. The two refusals that must answer *before*
//! planning are [`unsupported_clause`] and [`supported_language`].
//!
//! **The body is an expression over the arguments and nothing else.** DataFusion plans it against
//! an empty schema with the arguments as *placeholder* types, so its planner accepts `RETURN $1 + 1`
//! while the standard `RETURN x + 1` fails name resolution. [`bind_parameters`] rewrites that bare
//! form before planning, so all three spellings land on one planned body and
//! [`SqlMacro::simplify`] has one substitution to make. [`Definition::check`] then refuses a body
//! that reached anywhere else — a bare column or a subquery is a hidden dependency on a table
//! nothing persists. `AS '<string>'` is refused for a related reason: under the Postgres form `AS`
//! takes a string literal, so `AS 'x + 1'` would create a function returning the *text* `x + 1`.
//!
//! **A built-in is fenced off** because DataFusion's registry cannot tell one from a session's own,
//! and its `DROP FUNCTION` deregisters across **all five** registries at once — so
//! `DROP FUNCTION abs` would take the built-in away with nothing able to put it back.
//! [`Functions::created`] names the difference and both statements refuse toward it.
//! `engine::registered_function` therefore asks all five: three are one method call away and two
//! are not, and asking only those three left `array_filter` and its higher-order siblings reading
//! as free names.
//!
//! **Nothing persists.** A created function dies with the engine, which is what makes the report's
//! "for this session" true by construction. A `FunctionDef` list in `project.json` is the noted
//! extension, deliberately not scaffolded.

use std::collections::HashMap;
use std::ops::ControlFlow;
use std::sync::Arc;

use async_trait::async_trait;
use datafusion::arrow::datatypes::DataType;
use datafusion::common::tree_node::{Transformed, TreeNode, TreeNodeRecursion};
use datafusion::common::{internal_err, plan_datafusion_err, Result as DFResult};
use datafusion::execution::context::{FunctionFactory, RegisterFunction};
use datafusion::execution::SessionState;
use datafusion::logical_expr::simplify::{ExprSimplifyResult, SimplifyContext};
use datafusion::logical_expr::{
    Cast, ColumnarValue, CreateFunction, CreateFunctionBody, DdlStatement, Documentation,
    DropFunction, Expr, LogicalPlan, OperateFunctionArg, ScalarFunctionArgs, ScalarUDF,
    ScalarUDFImpl, Signature, Volatility,
};
use datafusion::sql::parser::Statement as DFStatement;
use datafusion::sql::sqlparser::ast::{
    visit_expressions_mut, CreateFunction as SqlCreateFunction,
    CreateFunctionBody as SqlCreateFunctionBody, DropFunction as SqlDropFunction, Expr as SqlExpr,
    Ident, Statement as SqlStatement, Value,
};

use crate::policy::Principal;
use crate::statements::ctx::StmtCtx;
use crate::statements::pipeline::Qualified;
use crate::statements::report::{StatementOutcome, StoreEffect};
use crate::statements::StmtKind;
use crate::{fold_ident, registered_function};

/// DataFusion's seam for `CREATE FUNCTION`, installed on every engine (`build_context`).
///
/// Stateless: everything it needs is in the statement DataFusion hands it, and the *policy* —
/// which names may be taken, what the report says — belongs to the arm, which is the only thing
/// that knows a statement was typed at all.
#[derive(Debug, Default)]
pub struct StrataFunctionFactory;

#[async_trait]
impl FunctionFactory for StrataFunctionFactory {
    async fn create(
        &self,
        _state: &SessionState,
        statement: CreateFunction,
    ) -> DFResult<RegisterFunction> {
        let definition = Definition::read(&statement).map_err(|e| plan_datafusion_err!("{e}"))?;
        Ok(RegisterFunction::Scalar(Arc::new(
            ScalarUDF::new_from_impl(SqlMacro::new(definition)?),
        )))
    }
}

/// `CREATE FUNCTION name(args) RETURNS type RETURN <expression>` — judged, then dispatched
/// natively so DataFusion's own factory call is what registers it.
///
/// Two judgements, in the order the user meets them. The **clause** check reads the parsed
/// statement, because DataFusion's planner silently drops most of what `CREATE FUNCTION` can
/// carry (`STRICT`, `SECURITY DEFINER`, `SET …`) — a clause read by nobody is a clause silently
/// ignored, which is the rule `views::definition` keeps from the same position. The **shape**
/// check is [`Definition::read`] over the planned statement, where the body is an `Expr` and the
/// argument types are resolved; it is the factory's own judgement, run here so its refusal reaches
/// the user as a plain sentence rather than wrapped in a planner error.
pub async fn create(
    cx: &StmtCtx,
    _who: &Principal,
    stmt: &Qualified,
) -> Result<StatementOutcome, String> {
    let (ctx, functions) = (&cx.ctx, &cx.functions);
    let DFStatement::Statement(mut s) = (**stmt).clone() else {
        return Err(not_a_function(StmtKind::CreateFunction));
    };
    let SqlStatement::CreateFunction(function) = s.as_mut() else {
        return Err(not_a_function(StmtKind::CreateFunction));
    };
    unsupported_clause(function)?;
    bind_parameters(function);

    let plan = ctx
        .state()
        .statement_to_plan(DFStatement::Statement(s))
        .await
        .map_err(|e| e.to_string())?;
    let LogicalPlan::Ddl(DdlStatement::CreateFunction(creating)) = &plan else {
        return Err(format!(
            "{} did not plan as a function",
            StmtKind::CreateFunction.label()
        ));
    };
    let name = Definition::read(creating)?.name;
    let or_replace = creating.or_replace;

    let replacing = match (functions.created(&name), registered_function(ctx, &name)) {
        (true, _) if !or_replace => {
            return Err(format!(
                "Function '{name}' already exists. Use CREATE OR REPLACE FUNCTION"
            ))
        }
        (true, _) => true,
        (false, true) => return Err(built_in(&name, "redefined")),
        (false, false) => false,
    };

    ctx.execute_logical_plan(plan)
        .await
        .map_err(|e| e.to_string())?;
    functions.settle(ctx, &name, true);

    let verb = match replacing {
        true => "replaced",
        false => "created",
    };
    Ok(StatementOutcome {
        message: format!("Function '{name}' {verb} for this session"),
        count: None,
        effect: Some(StoreEffect::FunctionsChanged),
    })
}

/// `DROP FUNCTION name` — judged off the parsed statement, then dispatched natively on the
/// **folded** name.
///
/// The plan is what resolves the name and refuses a *qualified* one in DataFusion's own words, but
/// it is not a gate on the rest of the statement: `DROP FUNCTION`'s planner arm takes
/// `func_desc.first()` and never looks at the length, the `DROP BEHAVIOR` or a `FunctionDesc`'s
/// argument list, so everything past the first name is discarded in silence
/// ([`unsupported_drop_clause`] — the same rule `create`'s clause check keeps).
///
/// The name the plan carries is the identifier *verbatim*, so the plan is re-formed around
/// [`fold_ident`]'s answer — the same folding [`create`] registered under, or `DROP FUNCTION
/// AddOne` would fail to find the function `CREATE FUNCTION AddOne` made.
pub async fn drop(
    cx: &StmtCtx,
    _who: &Principal,
    stmt: &Qualified,
) -> Result<StatementOutcome, String> {
    let (ctx, functions) = (&cx.ctx, &cx.functions);
    {
        let DFStatement::Statement(s) = &**stmt else {
            return Err(not_a_function(StmtKind::DropFunction));
        };
        let SqlStatement::DropFunction(dropping) = s.as_ref() else {
            return Err(not_a_function(StmtKind::DropFunction));
        };
        unsupported_drop_clause(dropping)?;
    }
    let plan = ctx
        .state()
        .statement_to_plan((**stmt).clone())
        .await
        .map_err(|e| e.to_string())?;
    let LogicalPlan::Ddl(DdlStatement::DropFunction(dropping)) = plan else {
        return Err(format!(
            "{} did not plan as a function drop",
            StmtKind::DropFunction.label()
        ));
    };
    let name = fold_ident(&dropping.name);
    if !functions.created(&name) {
        if registered_function(ctx, &name) {
            return Err(built_in(&name, "dropped"));
        }
        return match dropping.if_exists {
            true => Ok(StatementOutcome {
                message: format!("Function '{name}' does not exist"),
                count: None,
                effect: None,
            }),
            false => Err(format!("Function '{name}' does not exist")),
        };
    }

    ctx.execute_logical_plan(LogicalPlan::Ddl(DdlStatement::DropFunction(DropFunction {
        name: name.clone(),
        ..dropping
    })))
    .await
    .map_err(|e| e.to_string())?;
    functions.settle(ctx, &name, false);

    Ok(StatementOutcome {
        message: format!("Function '{name}' dropped"),
        count: None,
        effect: Some(StoreEffect::FunctionsChanged),
    })
}

/// The clauses a `CREATE FUNCTION` can carry that DataFusion's planner **drops** — refused by
/// name, so a statement that asks for something Strata does not do fails rather than succeeding
/// as something else.
///
/// **Exhaustive over the parsed statement, with no `..`**, for the reason `views::definition` is:
/// a clause sqlparser learns later has to be a compile error here rather than a promise silently
/// broken. Most of these are unreachable under the `generic` dialect, whose `CREATE FUNCTION`
/// parser hard-codes them absent — but the dialect is a Settings key, and `mssql` sets `OR ALTER`
/// while `bigquery` sets `OPTIONS` and `REMOTE WITH CONNECTION`.
fn unsupported_clause(function: &SqlCreateFunction) -> Result<(), String> {
    let SqlCreateFunction {
        or_alter,
        or_replace: _,
        temporary: _,
        if_not_exists,
        name: _,
        args: _,
        return_type: _,
        behavior: _,
        function_body,
        language,
        called_on_null,
        parallel,
        security,
        set_params,
        using,
        determinism_specifier,
        options,
        remote_connection,
    } = function;

    supported_language(language.as_ref())?;
    if *if_not_exists {
        return Err(
            "CREATE FUNCTION IF NOT EXISTS is not supported. Use CREATE OR REPLACE FUNCTION".into(),
        );
    }
    if matches!(
        function_body,
        Some(
            SqlCreateFunctionBody::AsBeforeOptions { .. }
                | SqlCreateFunctionBody::AsAfterOptions(_)
        )
    ) {
        return Err(
            "A function body given with AS is not supported. Use RETURN <expression>".into(),
        );
    }
    for (present, clause) in [
        (*or_alter, "OR ALTER"),
        (called_on_null.is_some(), "STRICT and CALLED ON NULL INPUT"),
        (parallel.is_some(), "PARALLEL"),
        (security.is_some(), "SECURITY"),
        (!set_params.is_empty(), "SET"),
        (using.is_some(), "USING"),
        (determinism_specifier.is_some(), "DETERMINISTIC"),
        (options.is_some(), "OPTIONS"),
        (remote_connection.is_some(), "REMOTE WITH CONNECTION"),
    ] {
        if present {
            return Err(format!("CREATE FUNCTION does not support {clause}"));
        }
    }
    Ok(())
}

/// Whether Strata runs a body written in `language` — absent or `SQL`, since a body is an
/// expression this engine plans.
///
/// Its own function because it is asked twice from opposite sides of planning, and it is one rule:
/// [`unsupported_clause`] asks it off the parsed statement, which is where the answer is *reachable*
/// (a non-SQL body does not survive planning), and [`Definition::read`] asks it off the planned one,
/// which is what keeps the factory closed to a caller that did not come through the arm. The
/// planner passes `language` through verbatim, so both see the same `Ident`.
fn supported_language(language: Option<&Ident>) -> Result<(), String> {
    match language {
        Some(language) if !language.value.eq_ignore_ascii_case("sql") => Err(format!(
            "LANGUAGE '{}' is not supported. Functions are SQL expressions",
            language.value
        )),
        _ => Ok(()),
    }
}

/// The parts of a `DROP FUNCTION` that DataFusion's planner **discards** — refused by name, for
/// the reason [`unsupported_clause`] refuses `CREATE FUNCTION`'s dropped clauses, and with a
/// sharper edge: `statement.rs`'s `DropFunction` arm does `if let Some(desc) = func_desc.first()`
/// with no length check and binds `drop_behavior: _`, while sqlparser parses the comma-separated
/// list in every dialect. So `DROP FUNCTION a, b` planned to a drop of `a` alone and reported
/// success — a statement half discarded, under a sentence that named only the half that happened.
///
/// **Exhaustive with no `..`**, like its sibling. An argument list is refused rather than ignored
/// because it *reads* as an overload selector and cannot be one: a created function has exactly one
/// signature, and DataFusion never looks at `FunctionDesc::args` either.
fn unsupported_drop_clause(dropping: &SqlDropFunction) -> Result<(), String> {
    let SqlDropFunction {
        if_exists: _,
        func_desc,
        drop_behavior,
    } = dropping;

    if func_desc.len() > 1 {
        return Err("DROP FUNCTION takes one function name".into());
    }
    if drop_behavior.is_some() {
        return Err("DROP FUNCTION does not support CASCADE and RESTRICT".into());
    }
    if func_desc.iter().any(|desc| desc.args.is_some()) {
        return Err(
            "DROP FUNCTION does not support an argument list. A name is one function".into(),
        );
    }
    Ok(())
}

/// The wording for a name the registry already holds and this session did not create. One
/// sentence for both statements, because it is one fact: DataFusion has no way to give a built-in
/// back once it is gone.
fn built_in(name: &str, verb: &str) -> String {
    format!("'{name}' is a built-in function and cannot be {verb}")
}

/// The router said this was function DDL and sqlparser parses it as `CreateFunction`. Anything
/// else is the two disagreeing.
fn not_a_function(kind: StmtKind) -> String {
    format!("{} did not parse as a function", kind.label())
}

/// A `CREATE FUNCTION` reduced to what Strata builds from it — **the one judgement of the
/// statement**, read by the arm for its refusals and by the factory to build the UDF.
struct Definition {
    /// The registered name, [`fold_ident`]ed — DataFusion's planner takes the identifier
    /// verbatim on both statements, so folding here is what makes `CREATE FUNCTION AddOne` and
    /// `SELECT addone(…)` name one function.
    name: String,
    /// Each argument's declared type, and its name where the statement gave one — through
    /// [`declared_name`], the same reading [`bind_parameters`] matched the body against, so the
    /// signature every surface renders spells the argument the way the user declared it. (A
    /// `CREATE FUNCTION` is either all-named or all-positional; DataFusion's planner refuses the
    /// mixture.)
    params: Vec<(Option<String>, DataType)>,
    return_type: DataType,
    volatility: Volatility,
    /// The body **as planned**, in which every reference to an argument is already a positional
    /// `Placeholder("$n")` — [`bind_parameters`] said the bare spelling in that vocabulary before
    /// planning, and DataFusion's own planner folded `$name` onto the same form.
    body: Expr,
}

impl Definition {
    fn read(creating: &CreateFunction) -> Result<Definition, String> {
        let CreateFunction {
            or_replace: _,
            temporary: _,
            name,
            args,
            return_type,
            params,
            schema: _,
        } = creating;
        let CreateFunctionBody {
            language,
            behavior,
            function_body,
        } = params;

        supported_language(language.as_ref())?;
        let Some(body) = function_body else {
            return Err("CREATE FUNCTION requires a body. Add RETURN <expression>".into());
        };
        let Some(return_type) = return_type else {
            return Err("CREATE FUNCTION requires a return type. Add RETURNS <type>".into());
        };

        let args = args.as_deref().unwrap_or_default();
        if args.iter().any(|arg| arg.default_expr.is_some()) {
            return Err("A function argument's default value is not supported".into());
        }
        let params: Vec<(Option<String>, DataType)> = args
            .iter()
            .map(
                |OperateFunctionArg {
                     name, data_type, ..
                 }| { (name.as_ref().map(declared_name), data_type.clone()) },
            )
            .collect();

        Definition::check(name, body, params.len())?;
        Ok(Definition {
            name: fold_ident(name),
            body: body.clone(),
            params,
            return_type: return_type.clone(),
            volatility: behavior.unwrap_or(Volatility::Volatile),
        })
    }

    /// Refuse a body that reaches outside its own arguments.
    ///
    /// After [`bind_parameters`] and DataFusion's own planning, every argument reference in the
    /// body is a positional `Placeholder("$n")` — whichever of the three spellings the user
    /// wrote. So the check is three facts: a placeholder names an argument that exists, there is
    /// no bare `Column`, and there is no subquery.
    ///
    /// The last two are one rule read from either end. A body that resolves a name against
    /// something other than its own arguments has a **hidden dependency on a table**: nothing
    /// persists the body, so a `DROP TABLE` cannot name it, and the plan the subquery inlined goes
    /// on reading a table the user believes the function has nothing to do with. The subquery is
    /// the arm that catches it today, and it is named separately because `Expr::apply` does not
    /// descend into the plan a subquery carries. The `Column` arm has no reachable spelling under
    /// DataFusion 54 — an identifier that binds to no argument fails planning first — and is kept
    /// because it is the *rule*: if a later DataFusion resolves an unbound name some other way,
    /// this refuses it rather than letting the caller's schema answer.
    fn check(name: &str, body: &Expr, arity: usize) -> Result<(), String> {
        body.apply(|expr| match expr {
            Expr::Column(column) => Err(plan_datafusion_err!(
                "A function body reads only its arguments, and '{column}' is not one of '{name}'s"
            )),
            Expr::ScalarSubquery(_) | Expr::InSubquery(_) | Expr::Exists(_) => Err(
                plan_datafusion_err!("A function body cannot contain a subquery"),
            ),
            Expr::AggregateFunction(_) | Expr::WindowFunction(_) => Err(plan_datafusion_err!(
                "A function body cannot contain an aggregate or window function"
            )),
            Expr::Placeholder(holder) => match position_of(&holder.id) {
                Some(position) if position < arity => Ok(TreeNodeRecursion::Continue),
                _ => Err(plan_datafusion_err!(
                    "'{}' is not an argument of '{name}'",
                    holder.id
                )),
            },
            _ => Ok(TreeNodeRecursion::Continue),
        })
        .map(|_| ())
        .map_err(|e| e.message().to_string())
    }
}

/// Rewrite each **bare** reference to an argument in the body into DataFusion's own `$name`
/// placeholder, on the parsed statement, before it is planned.
///
/// `CREATE FUNCTION add_one(x BIGINT) RETURNS BIGINT RETURN x + 1` is the standard SQL spelling
/// and the one a user writes, and DataFusion cannot plan it: the body is planned against an
/// **empty schema**, so a bare `x` fails name resolution outright ("No field named x"). What its
/// planner does support is a placeholder — positional `$1`, or `$x` matched by name against the
/// declared argument list — so this is the bare form *said in that vocabulary*, and every
/// spelling then lands on the same planned body.
///
/// Emitted as `$` plus the argument's **declared** identifier verbatim, because that is the
/// string DataFusion matches against (`create_placeholder_expr` compares it to the field name it
/// built from `Ident::value`). Matching is [`param_name`]'s, so an unquoted `X` in the body finds
/// the argument `x` the same way the planner would have resolved a column.
///
/// An identifier matching no argument is left exactly as written, so it stays DataFusion's error
/// to report and keeps naming itself.
fn bind_parameters(function: &mut SqlCreateFunction) {
    let declared: HashMap<String, String> = function
        .args
        .iter()
        .flatten()
        .filter_map(|arg| arg.name.as_ref())
        .map(|ident| (declared_name(ident), ident.value.clone()))
        .collect();
    if declared.is_empty() {
        return;
    }
    let _ = visit_expressions_mut(function, |expr| {
        if let SqlExpr::Identifier(ident) = expr {
            if let Some(declared) = declared.get(&param_name(ident)) {
                *expr = SqlExpr::Value(Value::Placeholder(format!("${declared}")).into());
            }
        }
        ControlFlow::<()>::Continue(())
    });
}

/// The name an identifier resolves to inside a function body — the planner's own identifier
/// normalization, which folds an unquoted identifier and takes a quoted one verbatim. Not
/// [`fold_ident`], which parses its input as a table reference and would take `a.b` apart.
fn param_name(ident: &Ident) -> String {
    match ident.quote_style {
        Some(_) => ident.value.clone(),
        None => ident.value.to_lowercase(),
    }
}

/// The same, for an identifier in the **argument list** — where sqlparser's Postgres
/// `CREATE FUNCTION` parser leaves a quoted name's quote characters inside the value and reports
/// `quote_style: None`, so a declared `"My Arg"` arrives as the seven-character string `"My Arg"`.
/// Unwrapped here so a declared name compares like any other identifier; the placeholder
/// [`bind_parameters`] emits still carries the value **verbatim**, because that is the string
/// DataFusion built its own field name from and matches against.
fn declared_name(ident: &Ident) -> String {
    if ident.quote_style.is_some() {
        return ident.value.clone();
    }
    match ident
        .value
        .strip_prefix('"')
        .and_then(|inner| inner.strip_suffix('"'))
    {
        Some(inner) => inner.replace("\"\"", "\""),
        None => ident.value.to_lowercase(),
    }
}

/// The zero-based argument a `$n` placeholder names, or `None` for anything that is not one.
fn position_of(id: &str) -> Option<usize> {
    id.strip_prefix('$')?.parse::<usize>().ok()?.checked_sub(1)
}

/// A SQL-bodied scalar function: the body, and the argument list it is written over.
///
/// It has no `invoke_with_args` implementation because it never runs — [`simplify`](Self::simplify)
/// replaces the call with its own body, which is what a macro *is*. That hook is called
/// unconditionally by the `SimplifyExpressions` optimizer rule, volatility included, so the
/// substitution happens once per plan rather than once per batch.
#[derive(Debug, PartialEq, Eq, Hash)]
struct SqlMacro {
    name: String,
    signature: Signature,
    return_type: DataType,
    /// Positionally, so a `Placeholder("$n")` in the body indexes straight into the call's
    /// arguments.
    body: Expr,
    documentation: Documentation,
}

impl SqlMacro {
    fn new(definition: Definition) -> DFResult<SqlMacro> {
        let Definition {
            name,
            params,
            return_type,
            volatility,
            body,
        } = definition;
        let types: Vec<DataType> = params.iter().map(|(_, t)| t.clone()).collect();
        let names: Option<Vec<String>> = params.iter().map(|(n, _)| n.clone()).collect();
        let signature = Signature::exact(types, volatility);
        let signature = match names {
            Some(names) if !names.is_empty() => signature.with_parameter_names(names)?,
            _ => signature,
        };
        let call = params
            .iter()
            .map(|(argument, data_type)| match argument {
                Some(argument) => argument.clone(),
                None => data_type.to_string(),
            })
            .collect::<Vec<_>>()
            .join(", ");
        let documentation = Documentation::builder(
            Default::default(),
            "SQL function created in this session",
            format!("{name}({call})"),
        )
        .build();
        Ok(SqlMacro {
            body: Expr::Cast(Cast::new(Box::new(body), return_type.clone())),
            name,
            signature,
            return_type,
            documentation,
        })
    }
}

impl ScalarUDFImpl for SqlMacro {
    fn name(&self) -> &str {
        &self.name
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn return_type(&self, _args: &[DataType]) -> DFResult<DataType> {
        Ok(self.return_type.clone())
    }

    /// The whole implementation: the call becomes the body with its arguments substituted in.
    fn simplify(&self, args: Vec<Expr>, _info: &SimplifyContext) -> DFResult<ExprSimplifyResult> {
        let body = self.body.clone().transform(|expr| match expr {
            Expr::Placeholder(ref holder) => {
                match position_of(&holder.id).and_then(|p| args.get(p)) {
                    Some(arg) => Ok(Transformed::yes(arg.clone())),
                    None => internal_err!("function '{}' has no argument {}", self.name, holder.id),
                }
            }
            other => Ok(Transformed::no(other)),
        })?;
        Ok(ExprSimplifyResult::Simplified(body.data))
    }

    fn invoke_with_args(&self, _args: ScalarFunctionArgs) -> DFResult<ColumnarValue> {
        internal_err!(
            "function '{}' is a SQL expression and should have been simplified away",
            self.name
        )
    }

    fn documentation(&self) -> Option<&Documentation> {
        Some(&self.documentation)
    }
}

#[cfg(test)]
mod tests {

    use crate::sql::{complete, Catalog, CompletionKind};
    use crate::{Engine, RunOutcome, RunRows, RunTag, StatementReport, StoreEffect, WsId};

    /// Run one statement and take its report — anything else is a test asking the wrong question.
    async fn statement(eng: &Engine, sql: &str) -> Result<StatementReport, String> {
        match eng
            .ws(WsId(1))
            .run(RunTag(1), sql.into(), 10)
            .await
            .map_err(|e| e.to_string())?
        {
            RunOutcome::Statement(report) => Ok(report),
            RunOutcome::Rows(..) => panic!("{sql} ran as a query"),
        }
    }

    /// The values a query returns, as text.
    async fn read(eng: &Engine, sql: &str) -> Vec<Vec<String>> {
        let RunOutcome::Rows(RunRows { output, .. }) = eng
            .ws(WsId(2))
            .run(RunTag(2), sql.into(), 100)
            .await
            .unwrap_or_else(|e| panic!("{sql}: {e}"))
        else {
            panic!("{sql} did not return rows");
        };
        output
            .rows
            .into_iter()
            .map(|row| row.into_iter().map(|cell| cell.text).collect())
            .collect()
    }

    /// The error a Run fails with. `RunOutcome` carries a `RecordBatch` and derives no `Debug`,
    /// so the success arm is named rather than unwrapped.
    async fn run_err(eng: &Engine, sql: &str) -> String {
        match eng.ws(WsId(3)).run(RunTag(3), sql.into(), 10).await {
            Err(e) => e.to_string(),
            Ok(_) => panic!("{sql} succeeded"),
        }
    }

    /// What completion offers for `prefix`, through the language service's own snapshot of the
    /// engine — which is the thing the editor rebuilds on a catalog epoch, not the registry.
    fn offered(eng: &Engine, prefix: &str) -> Vec<(String, String)> {
        let catalog = Catalog::build([], [], eng.lang().bundle(), "generic".into());
        complete(&catalog, prefix, prefix.len(), false)
            .into_iter()
            .filter(|c| c.kind == CompletionKind::Function)
            .map(|c| (c.label, c.detail.unwrap_or_default()))
            .collect()
    }

    /// **A created function runs, completes, and says its scope.** The four surfaces the task
    /// names, in one chain, because they are one fact seen four ways: the registry executes it,
    /// the catalog walk puts it in the completion pool with the signature the argument list
    /// declared, `SHOW FUNCTIONS` enumerates it, and the report says the one thing the user
    /// cannot see anywhere else — that it dies with the engine (spec §8).
    #[tokio::test]
    async fn a_created_function_runs_completes_and_says_it_is_session_scoped() {
        let eng = Engine::builder().build();
        assert!(offered(&eng, "SELECT add_o").is_empty(), "not yet");

        let report = statement(
            &eng,
            "CREATE FUNCTION add_one(x BIGINT) RETURNS BIGINT RETURN x + 1",
        )
        .await
        .expect("created");
        assert_eq!(
            report.message,
            "Function 'add_one' created for this session"
        );
        assert_eq!(report.count, None, "creating a function moves no rows");
        assert_eq!(report.effect, Some(StoreEffect::FunctionsChanged));

        assert_eq!(read(&eng, "SELECT add_one(41)").await, vec![vec!["42"]]);
        assert_eq!(
            read(&eng, "SELECT add_one(n) FROM (VALUES (1), (2)) AS v(n)").await,
            vec![vec!["2"], vec!["3"]],
            "and over a column, not only a constant"
        );
        assert_eq!(
            offered(&eng, "SELECT add_o"),
            vec![("add_one".to_string(), "(x)".to_string())],
            "with the argument's own name, which is what `with_parameter_names` buys"
        );
        assert_eq!(
            read(&eng, "SHOW FUNCTIONS LIKE 'add_one'")
                .await
                .first()
                .map(|row| row[0].clone()),
            Some("add_one".to_string())
        );

        let dropped = |eng: &Engine| {
            let catalog = Catalog::build([], [], eng.lang().bundle(), "generic".into());
            complete(&catalog, "DROP FUNCTION ", 14, false)
                .into_iter()
                .map(|c| c.label)
                .collect::<Vec<_>>()
        };
        assert_eq!(dropped(&eng), vec!["add_one"]);
        statement(&eng, "DROP FUNCTION add_one")
            .await
            .expect("dropped");
        assert!(dropped(&eng).is_empty(), "and the offer follows the drop");
    }

    /// **Every spelling of an argument reaches the same body.** DataFusion plans a function body
    /// against an empty schema, so it accepts a positional `$1` and a named `$x` and refuses the
    /// bare `x` that is the standard SQL — and the standard SQL is what a user writes.
    /// `bind_parameters` says the bare form in the planner's own vocabulary, so all three land on
    /// one planned body; the case-insensitive match is the same rule an unquoted column follows.
    #[tokio::test]
    async fn every_spelling_of_an_argument_reaches_the_same_body() {
        let eng = Engine::builder().build();
        for (n, body) in [
            (1, "a * 10 + b"),
            (2, "$a * 10 + $b"),
            (3, "$1 * 10 + $2"),
            (4, "A * 10 + $2"),
        ] {
            statement(
                &eng,
                &format!("CREATE FUNCTION f{n}(a BIGINT, b BIGINT) RETURNS BIGINT RETURN {body}"),
            )
            .await
            .unwrap_or_else(|e| panic!("{body}: {e}"));
            assert_eq!(
                read(&eng, &format!("SELECT f{n}(4, 2)")).await,
                vec![vec!["42"]],
                "{body}"
            );
        }

        statement(
            &eng,
            r#"CREATE FUNCTION spaced("My Arg" BIGINT) RETURNS BIGINT RETURN "My Arg" + 1"#,
        )
        .await
        .expect("created");
        assert_eq!(read(&eng, "SELECT spaced(41)").await, vec![vec!["42"]]);
        assert_eq!(
            offered(&eng, "SELECT spac"),
            vec![("spaced".to_string(), "(My Arg)".to_string())]
        );
    }

    /// **The declared return type is the call's type.** `RETURNS INT` over a body that plans as
    /// `Int64` has to answer `Int32`, or the simplified expression disagrees with the function's
    /// own `return_type` and the fault surfaces deep in the optimizer instead of as an answer.
    #[tokio::test]
    async fn the_declared_return_type_is_what_the_call_has() {
        let eng = Engine::builder().build();
        statement(
            &eng,
            "CREATE FUNCTION narrow(x BIGINT) RETURNS INT RETURN x + 1",
        )
        .await
        .expect("created");
        assert_eq!(
            read(&eng, "SELECT arrow_typeof(narrow(1)), narrow(1)").await,
            vec![vec!["Int32", "2"]]
        );
    }

    /// **A drop takes the function out of execution and out of completion**, and honours
    /// `IF EXISTS` — which is the difference between a statement that reports a no-op and one
    /// that failed.
    #[tokio::test]
    async fn a_drop_removes_it_from_execution_and_from_completion() {
        let eng = Engine::builder().build();
        statement(
            &eng,
            "CREATE FUNCTION add_one(x BIGINT) RETURNS BIGINT RETURN x + 1",
        )
        .await
        .expect("created");

        let report = statement(&eng, "DROP FUNCTION add_one")
            .await
            .expect("dropped");
        assert_eq!(report.message, "Function 'add_one' dropped");
        assert_eq!(report.effect, Some(StoreEffect::FunctionsChanged));
        assert!(run_err(&eng, "SELECT add_one(1)")
            .await
            .contains("Invalid function 'add_one'"));
        assert!(offered(&eng, "SELECT add_o").is_empty());

        let missing = statement(&eng, "DROP FUNCTION IF EXISTS add_one")
            .await
            .expect("reported");
        assert_eq!(missing.message, "Function 'add_one' does not exist");
        assert_eq!(missing.effect, None, "nothing for the store to fold");
        assert_eq!(
            statement(&eng, "DROP FUNCTION add_one")
                .await
                .expect_err("gone"),
            "Function 'add_one' does not exist"
        );
    }

    /// **A name is folded, and taking one twice needs `OR REPLACE`** — the rule a view keeps,
    /// for the reason a view keeps it: a silent replacement is a definition the user did not
    /// write. The folding matters on its own, because DataFusion's planner takes the identifier
    /// verbatim on both statements, so an unfolded `AddOne` would register under a name
    /// `SELECT addone(…)` could never resolve.
    #[tokio::test]
    async fn a_name_is_folded_and_replacing_one_is_asked_for() {
        let eng = Engine::builder().build();
        statement(
            &eng,
            "CREATE FUNCTION AddOne(x BIGINT) RETURNS BIGINT RETURN x + 1",
        )
        .await
        .expect("created");
        assert_eq!(read(&eng, "SELECT AddOne(41)").await, vec![vec!["42"]]);
        assert_eq!(read(&eng, "SELECT addone(41)").await, vec![vec!["42"]]);

        assert_eq!(
            statement(
                &eng,
                "CREATE FUNCTION addone(x BIGINT) RETURNS BIGINT RETURN x + 2"
            )
            .await
            .expect_err("taken"),
            "Function 'addone' already exists. Use CREATE OR REPLACE FUNCTION"
        );
        assert_eq!(
            read(&eng, "SELECT addone(41)").await,
            vec![vec!["42"]],
            "and the refusal left it alone"
        );

        let replaced = statement(
            &eng,
            "CREATE OR REPLACE FUNCTION AddOne(x BIGINT) RETURNS BIGINT RETURN x + 2",
        )
        .await
        .expect("replaced");
        assert_eq!(
            replaced.message,
            "Function 'addone' replaced for this session"
        );
        assert_eq!(read(&eng, "SELECT addone(40)").await, vec![vec!["42"]]);
        statement(&eng, "DROP FUNCTION AddOne")
            .await
            .expect("dropped");
    }

    /// **A built-in is neither redefined nor dropped.** DataFusion's registry cannot tell one
    /// from a session's own function and its `DROP FUNCTION` deregisters across every registry at
    /// once, so either statement would take a built-in away for the rest of the session with
    /// nothing able to put it back. The aggregate is here beside the scalar because that is the
    /// half a scalar-only check would miss.
    #[tokio::test]
    async fn a_built_in_is_neither_redefined_nor_dropped() {
        let eng = Engine::builder().build();
        for name in ["abs", "count", "row_number"] {
            for sql in [
                format!("CREATE FUNCTION {name}(x BIGINT) RETURNS BIGINT RETURN 0"),
                format!("CREATE OR REPLACE FUNCTION {name}(x BIGINT) RETURNS BIGINT RETURN 0"),
            ] {
                assert_eq!(
                    statement(&eng, &sql).await.expect_err("refused"),
                    format!("'{name}' is a built-in function and cannot be redefined"),
                    "{sql}"
                );
            }
            assert_eq!(
                statement(&eng, &format!("DROP FUNCTION {name}"))
                    .await
                    .expect_err("refused"),
                format!("'{name}' is a built-in function and cannot be dropped")
            );
        }
        assert_eq!(
            read(&eng, "SELECT abs(-1), count(1)").await,
            vec![vec!["1", "1"]]
        );
    }

    /// **The forms that are not a SQL function**, each refused in the app's own register and each
    /// leaving nothing behind. Three classes: a body Strata does not run (another language, no
    /// body at all, or `AS`, which takes a *string literal* in this dialect family and would
    /// create a function returning the text of the expression); a body that reaches outside its
    /// arguments; and a clause DataFusion's planner would drop on the floor.
    #[tokio::test]
    async fn the_forms_that_are_not_a_sql_function_refuse() {
        let eng = Engine::builder().build();
        statement(&eng, "CREATE VIEW v AS SELECT 1 AS a")
            .await
            .expect("created");

        for (sql, message) in [
            (
                "CREATE FUNCTION f(x BIGINT) RETURNS BIGINT LANGUAGE python RETURN x",
                "LANGUAGE 'python' is not supported. Functions are SQL expressions",
            ),
            (
                "CREATE FUNCTION f(x BIGINT) RETURNS BIGINT LANGUAGE python RETURN np_abs(x)",
                "LANGUAGE 'python' is not supported. Functions are SQL expressions",
            ),
            (
                "CREATE FUNCTION f(x BIGINT) RETURNS BIGINT LANGUAGE js RETURN y",
                "LANGUAGE 'js' is not supported. Functions are SQL expressions",
            ),
            (
                "CREATE FUNCTION f(x BIGINT) RETURNS BIGINT",
                "CREATE FUNCTION requires a body. Add RETURN <expression>",
            ),
            (
                "CREATE FUNCTION f(x BIGINT) RETURN x",
                "CREATE FUNCTION requires a return type. Add RETURNS <type>",
            ),
            (
                "CREATE FUNCTION f(x BIGINT) RETURNS BIGINT AS 'x + 1'",
                "A function body given with AS is not supported. Use RETURN <expression>",
            ),
            (
                "CREATE FUNCTION f(x BIGINT) RETURNS BIGINT RETURN x + $3",
                "'$3' is not an argument of 'f'",
            ),
            (
                "CREATE FUNCTION f() RETURNS BIGINT RETURN (SELECT max(a) FROM v)",
                "A function body cannot contain a subquery",
            ),
            (
                "CREATE FUNCTION f(x BIGINT) RETURNS BIGINT RETURN sum(x)",
                "A function body cannot contain an aggregate or window function",
            ),
            (
                "CREATE FUNCTION f(x BIGINT) RETURNS BIGINT RETURN row_number() OVER ()",
                "A function body cannot contain an aggregate or window function",
            ),
            (
                "CREATE FUNCTION f(x BIGINT) RETURNS BIGINT STRICT RETURN x",
                "CREATE FUNCTION does not support STRICT and CALLED ON NULL INPUT",
            ),
            (
                "CREATE FUNCTION f(x BIGINT) RETURNS BIGINT SECURITY DEFINER RETURN x",
                "CREATE FUNCTION does not support SECURITY",
            ),
        ] {
            assert_eq!(
                statement(&eng, sql).await.expect_err("refused"),
                message,
                "{sql}"
            );
        }
        assert!(statement(
            &eng,
            "CREATE FUNCTION f(x BIGINT) RETURNS BIGINT RETURN y + 1"
        )
        .await
        .expect_err("refused")
        .contains('y'));
        assert!(
            !offered(&eng, "SELECT f")
                .iter()
                .any(|(label, _)| label == "f"),
            "a refusal creates nothing"
        );
    }

    /// **A created function is known to the diagnostics pass too, not only to completion.**
    /// Those are two different readers of the swap: completion resolves against the language
    /// service's `Catalog` snapshot, while `Lang::analyze` dry-plans against the live
    /// `SessionContext` and takes the catalog by handle for its lexical lints. A squiggle left
    /// under a call the very same buffer can Run is the disagreement the epoch bump exists to
    /// prevent, and it is worth pinning from this end because the app-side wiring
    /// (`FunctionsChanged` -> `catalog_settled`) is the session layer's and is not re-tested here.
    #[tokio::test]
    async fn a_created_function_stops_being_a_diagnostic() {
        let eng = Engine::builder().build();
        let sql = "SELECT add_one(41)";
        assert!(
            !eng.lang().analyze(sql.into()).await.is_empty(),
            "unknown before it is created"
        );

        statement(
            &eng,
            "CREATE FUNCTION add_one(x BIGINT) RETURNS BIGINT RETURN x + 1",
        )
        .await
        .expect("created");
        assert_eq!(
            eng.lang().analyze(sql.into()).await,
            vec![],
            "and clean the moment it is"
        );

        statement(&eng, "DROP FUNCTION add_one")
            .await
            .expect("dropped");
        assert!(
            !eng.lang().analyze(sql.into()).await.is_empty(),
            "unknown again once it is gone"
        );
    }

    /// **The built-in fence covers every registry `DROP FUNCTION` clears, not the three that are
    /// easy to ask.** DataFusion's `drop_function` deregisters scalar, aggregate, window, table
    /// *and* higher-order in one go, and `array_filter` / `array_transform` / `array_any_match` are
    /// registered **only** as higher-order — so a fence that asked three registries read those
    /// three names as free, let a session take one, and then let the matching `DROP FUNCTION`
    /// destroy the built-in for the rest of the session.
    ///
    /// A higher-order name's loss is not observable from SQL *today* — the default `generic`
    /// dialect parses no lambda, and `SHOW FUNCTIONS` does not enumerate that registry — which is
    /// precisely why the fence asks the registry rather than "is it callable": the dialect is a
    /// Settings key, and a predicate that happens to be right because a name has a scalar twin
    /// (`range`, below) is one DataFusion can invalidate without telling us. The table function is
    /// the half that *is* observable, and it is the last assertion.
    #[tokio::test]
    async fn the_built_in_fence_covers_the_registries_a_drop_clears() {
        let eng = Engine::builder().build();
        for name in [
            "array_filter",
            "array_transform",
            "array_any_match",
            "range",
        ] {
            assert_eq!(
                statement(
                    &eng,
                    &format!("CREATE FUNCTION {name}(x BIGINT) RETURNS BIGINT RETURN 0")
                )
                .await
                .expect_err("refused"),
                format!("'{name}' is a built-in function and cannot be redefined")
            );
            assert_eq!(
                statement(&eng, &format!("DROP FUNCTION {name}"))
                    .await
                    .expect_err("refused"),
                format!("'{name}' is a built-in function and cannot be dropped")
            );
        }
        assert_eq!(
            read(&eng, "SELECT * FROM range(1, 3)").await,
            vec![vec!["1"], vec!["2"]],
            "the table function still resolves and still runs"
        );
    }

    /// **What a `DROP FUNCTION` carries that DataFusion's planner discards.** Its planner arm takes
    /// `func_desc.first()` with no length check and binds `drop_behavior: _`, while sqlparser
    /// parses the comma-separated list in every dialect — so `DROP FUNCTION a, b` planned as a drop
    /// of `a` alone and reported success for a statement half of which never happened. Refused off
    /// the parsed statement, the same rule `CREATE FUNCTION`'s clause check keeps.
    ///
    /// The second half is the assertion that matters: both functions are still callable.
    #[tokio::test]
    async fn a_drop_refuses_what_its_planner_would_discard() {
        let eng = Engine::builder().build();
        for n in [1, 2] {
            statement(
                &eng,
                &format!("CREATE FUNCTION f{n}(x BIGINT) RETURNS BIGINT RETURN x + {n}"),
            )
            .await
            .expect("created");
        }

        for (sql, message) in [
            (
                "DROP FUNCTION f1, f2",
                "DROP FUNCTION takes one function name",
            ),
            (
                "DROP FUNCTION f1 CASCADE",
                "DROP FUNCTION does not support CASCADE and RESTRICT",
            ),
            (
                "DROP FUNCTION f1(BIGINT)",
                "DROP FUNCTION does not support an argument list. A name is one function",
            ),
        ] {
            assert_eq!(
                statement(&eng, sql).await.expect_err("refused"),
                message,
                "{sql}"
            );
        }
        assert_eq!(
            read(&eng, "SELECT f1(1), f2(1)").await,
            vec![vec!["2", "3"]],
            "and every refusal left both functions alone"
        );
    }

    /// **A restart clears the created functions and leaves the built-ins exactly as they were.**
    /// True by construction: the remount builds a new `Engine`, which walks a fresh registry into
    /// a fresh `Functions`. The second assertion is the other half of the swap — a catalog that
    /// re-walks on every statement would be a cost, and one that never re-walks would be the bug
    /// this task exists to fix, so the equality is what says the built-in set is untouched.
    #[tokio::test]
    async fn a_restart_clears_created_functions_and_leaves_the_built_ins_alone() {
        let eng = Engine::builder().build();
        let built_in = eng.lang().functions();
        statement(
            &eng,
            "CREATE FUNCTION add_one(x BIGINT) RETURNS BIGINT RETURN x + 1",
        )
        .await
        .expect("created");
        assert_eq!(
            eng.lang().functions().scalar.len(),
            built_in.scalar.len() + 1,
            "the created function, and nothing else, joined the pool"
        );
        statement(&eng, "DROP FUNCTION add_one")
            .await
            .expect("dropped");
        assert_eq!(
            eng.lang().functions(),
            built_in,
            "and the drop puts the catalog back exactly as it was"
        );

        statement(
            &eng,
            "CREATE FUNCTION add_one(x BIGINT) RETURNS BIGINT RETURN x + 1",
        )
        .await
        .expect("created");
        let restarted = Engine::builder().build();
        assert!(run_err(&restarted, "SELECT add_one(1)")
            .await
            .contains("Invalid function 'add_one'"));
        assert_eq!(
            restarted.lang().functions(),
            built_in,
            "a fresh engine is the built-in set again"
        );
    }
}
