//! The **SQL language service** — one analysis layer over the SQL buffer that backs both
//! autocomplete and validation. Its face is `service`: [`analyze`] and [`complete`], and
//! nothing else here is a door.
//!
//! - `service` — the two entries, and the invariants they hold (read it first).
//! - `oracle` — **what a name resolves to**, the one authority every rung asks.
//! - [`lex`] — tokenise via DataFusion's own `sqlparser` (byte spans + kinds), under the engine's
//!   configured `datafusion.sql_parser.dialect`.
//! - [`context`] — split statements + classify the caret's clause context.
//! - [`name`] — writing a name back **into** SQL: four rules, four types.
//! - [`symbols`] — the [`Catalog`] snapshot and in-statement alias resolution.
//! - [`lint`] — the token-level tier: parens, the keyword-typo lint, the tokens-only relation rung.
//! - [`spans`] — folding a fault onto a byte range of the buffer.
//! - [`resolve`] — the AST name resolver behind [`analyze`]: every unknown table/column in a
//!   parsed statement, multi-error and mid-edit tolerant, leaving types and arity to the dry-plan.
//! - [`qualify`] — bare-name resolution across the connected databases, in front of both
//!   the router and the planner, so a statement is judged and run with the names it reaches.
//! - [`complete`](mod@complete) — ranked completions for a caret position.
//!
//! Completion resolves against a [`Catalog`] snapshot the consumer holds and rebuilds when the
//! catalog generation moves; [`analyze`] runs engine-side so its faults are the *same* errors a
//! Run would hit.

pub(crate) mod complete;
pub(crate) mod context;
mod fuzzy;
pub(crate) mod lex;
pub(crate) mod lint;
pub mod name;
pub(crate) mod oracle;
pub(crate) mod qualify;
pub(crate) mod resolve;
mod service;
pub(crate) mod spans;
pub mod symbols;

pub use complete::{Completion, CompletionKind};
pub use name::{ResultColumn, SessionName, WorkspaceName};
pub(crate) use resolve::unwrap_statement;
pub(crate) use service::analyze;
pub use service::complete;
pub use symbols::{Catalog, DatabaseSym, LangBundle, PreparedSym, RelationSym, SchemaSym};

/// Which registry a function came from — the docs-panel header word, and (for the
/// caller) a coarse category. `Default` is `Scalar` so a name-only [`FunctionSym`]
/// (e.g. the `From<&str>` test constructor) is a plain scalar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FnKind {
    /// A scalar function.
    #[default]
    Scalar,
    /// An aggregate function.
    Aggregate,
    /// A window function.
    Window,
}

impl FnKind {
    /// The docs-panel header noun (`"scalar function"`, …).
    pub fn label(self) -> &'static str {
        match self {
            FnKind::Scalar => "scalar function",
            FnKind::Aggregate => "aggregate function",
            FnKind::Window => "window function",
        }
    }
}

/// One registered SQL function (built-in or UDF), enriched from the engine registry
/// for the language service (S7 completion detail, docs panel, signature help). The
/// signature/return rendering is done **engine-side** at registry-snapshot time
/// (`Engine::new`, which is the only place DataFusion is touched) so the UI never
/// depends on DataFusion's types.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FunctionSym {
    /// The function's name.
    pub name: String,
    /// Which registry it came from.
    pub kind: FnKind,
    /// The function's overloads, each an ordered list of parameter labels — a type
    /// display (`"Float64"`) or, when the registry names them, the parameter name.
    /// A trailing `"…"` element marks a variadic tail (an unbounded final param).
    /// An empty inner vec is a nullary form (`now()`). Empty outer vec = arity
    /// unknown (a `UserDefined` signature); consumers render `name(…)`.
    pub signatures: Vec<Vec<String>>,
    /// Return-type display, when the signature resolves one without concrete
    /// arguments (`Some("Float64")`); `None` for arg-dependent or window returns.
    pub ret: Option<String>,
    /// One-line description from the registry's documentation, when present.
    pub description: Option<String>,
    /// `true` for a function **this session created** (`CREATE FUNCTION`) —
    /// what `DROP FUNCTION |` filters on, marked at registry-snapshot time from the
    /// one authority (the `Functions` registry's created-name set).
    pub created: bool,
}

/// The variadic-tail marker used inside [`FunctionSym::signatures`] parameter lists.
pub const VARIADIC: &str = "…";

impl FunctionSym {
    /// The compact parameter form for the completion `detail` column — the shape
    /// with the optional tail bracketed, name omitted (the row label already is the
    /// name): `(Float64[, Int64])`, `(str, …)`, `()`.
    pub fn detail(&self) -> String {
        format!("({})", self.arity_form())
    }

    /// The parameter shape shared by [`detail`](Self::detail) and the signature
    /// popup fallback: required params, then each optional/variadic tail param
    /// bracketed. Built by treating the shortest overload as the required prefix and
    /// the longest as the full form.
    fn arity_form(&self) -> String {
        if self.signatures.is_empty() {
            return VARIADIC.to_string();
        }
        let shortest = self.signatures.iter().map(Vec::len).min().unwrap_or(0);
        let longest = self
            .signatures
            .iter()
            .max_by_key(|s| s.len())
            .cloned()
            .unwrap_or_default();
        let mut out = String::new();
        for (i, p) in longest.iter().enumerate() {
            if i < shortest {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(p);
            } else {
                out.push_str(&format!("[, {p}]"));
            }
        }
        out
    }

    /// The docs-panel body (the string a `docs_for` callback returns for a function
    /// row): the kind header, every overload as `name(params) → ret`, then the
    /// description. Bounded — at most [`Self::DOC_OVERLOADS`] overloads are listed.
    pub fn doc(&self) -> String {
        let max = Self::DOC_OVERLOADS;
        let ret = self
            .ret
            .as_ref()
            .map(|r| format!(" → {r}"))
            .unwrap_or_default();
        let mut lines = vec![self.kind.label().to_string(), String::new()];
        if self.signatures.is_empty() {
            lines.push(format!("{}(…){ret}", self.name));
        } else {
            for params in self.signatures.iter().take(max) {
                lines.push(format!("{}({}){ret}", self.name, params.join(", ")));
            }
            if self.signatures.len() > max {
                lines.push("…".to_string());
            }
        }
        if let Some(desc) = &self.description {
            lines.push(String::new());
            lines.push(desc.clone());
        }
        lines.join("\n")
    }

    const DOC_OVERLOADS: usize = 8;
}

impl From<&str> for FunctionSym {
    /// A name-only symbol (scalar, no signatures) — the ergonomic constructor for
    /// tests and any caller that only has a name.
    fn from(name: &str) -> Self {
        FunctionSym {
            name: name.to_string(),
            ..Default::default()
        }
    }
}

/// The engine's registered functions (built-ins + any UDFs), by category —
/// enriched to [`FunctionSym`]s (name + overload signatures + return type). Walked from the
/// engine's registry at startup (`Engine::new`) and again by any statement that moves that
/// registry (`CREATE FUNCTION` / `DROP FUNCTION`), held on the per-window
/// [`Engine`](crate::Engine); folded into a [`Catalog`] for completion + validation +
/// signature help.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FunctionCatalog {
    /// The scalar functions.
    pub scalar: Vec<FunctionSym>,
    /// The aggregate functions.
    pub aggregate: Vec<FunctionSym>,
    /// The window functions.
    pub window: Vec<FunctionSym>,
}

impl FunctionCatalog {
    /// Whether `name` (case-insensitive) is a registered function of any category.
    pub fn contains(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    /// The symbol for `name` (case-insensitive), across every category.
    pub fn get(&self, name: &str) -> Option<&FunctionSym> {
        self.all().find(|f| f.name.eq_ignore_ascii_case(name))
    }

    /// All symbols across categories (for a pooled completion fallback).
    pub fn all(&self) -> impl Iterator<Item = &FunctionSym> {
        self.scalar
            .iter()
            .chain(self.aggregate.iter())
            .chain(self.window.iter())
    }
}

#[cfg(test)]
mod function_sym_tests {
    use super::*;

    fn sym(name: &str, sigs: &[&[&str]], ret: Option<&str>) -> FunctionSym {
        FunctionSym {
            name: name.into(),
            kind: FnKind::Scalar,
            signatures: sigs
                .iter()
                .map(|o| o.iter().map(ToString::to_string).collect())
                .collect(),
            ret: ret.map(String::from),
            description: None,
            created: false,
        }
    }

    #[test]
    fn detail_brackets_the_optional_tail() {
        let round = sym(
            "round",
            &[&["Float64"], &["Float64", "Int64"]],
            Some("Float64"),
        );
        assert_eq!(round.detail(), "(Float64[, Int64])");
    }

    #[test]
    fn detail_of_fixed_arity_is_plain() {
        assert_eq!(sym("lower", &[&["Utf8"]], None).detail(), "(Utf8)");
    }

    #[test]
    fn detail_nullary_and_unknown_arity() {
        assert_eq!(sym("now", &[&[]], Some("Timestamp")).detail(), "()");
        let ud = FunctionSym::from("udf");
        assert_eq!(ud.detail(), "(…)");
    }

    #[test]
    fn doc_lists_every_overload_with_the_return_type() {
        let round = sym(
            "round",
            &[&["Float64"], &["Float64", "Int64"]],
            Some("Float64"),
        );
        let doc = round.doc();
        assert!(doc.starts_with("scalar function"), "{doc}");
        assert!(doc.contains("round(Float64) → Float64"), "{doc}");
        assert!(doc.contains("round(Float64, Int64) → Float64"), "{doc}");
    }

    #[test]
    fn doc_appends_the_description() {
        let mut f = sym("lower", &[&["Utf8"]], Some("Utf8"));
        f.description = Some("Lowercases a string.".into());
        assert!(f.doc().ends_with("Lowercases a string."));
    }
}
