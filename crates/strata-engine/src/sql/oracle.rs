//! **What a name resolves to** — the one authority the language service asks.
//!
//! Four rungs of the mid-edit ladder used to answer this for themselves, and they did not agree:
//! [`qualify`](super::qualify) searched the connected databases, the resolver's prefetch asked
//! the session for a provider, the tokens-only rung asked
//! [`SessionContext::table_exist`] against the **workspace alone**, and the keyword-typo lint
//! asked its own narrower question. A name only a connected database holds therefore resolved for
//! the statement Run executed and did not resolve for the tokens rung judging the same buffer
//! mid-edit — a red squiggle on a statement that runs, which is the divergence the language
//! service exists to end.
//!
//! One type answers all four now: [`resolves`](NameOracle::resolves) (does this name reach a
//! relation), [`candidates`](NameOracle::candidates) (where a bare one reaches, for the rewrite
//! and for the ambiguity refusal) and [`columns`](NameOracle::columns) (what a relation's are, or
//! that they are unknowable). The rules are qualify's, stated once: the workspace wins, then the
//! schemas each data source **shows**.
//!
//! Built once per caller on purpose — the lint asks per identifier token on every keystroke, and
//! building this is a lock, a config read and a `Vec` per catalog.

use datafusion::common::TableReference;
use datafusion::prelude::SessionContext;
use datafusion::sql::sqlparser::ast::{Ident, ObjectName, ObjectNamePart};
use datafusion::sql::sqlparser::tokenizer::Span;

use crate::catalog_providers::shown_schemas;
use crate::ident::fold_ident;
use crate::sql::name::SessionName;

/// One relation, addressed the way the thing that holds it spells it: the catalog as the
/// data source registered it, the schema and the relation as the server does.
pub(crate) struct Address {
    catalog: String,
    schema: String,
    table: String,
}

impl Address {
    /// The name written back into the statement, **every part quoted** — the only rendering that
    /// means the same thing under either `enable_ident_normalization`, which would otherwise
    /// lower-case a server's `Orders`. [`rendered`](Self::rendered) is what a message uses.
    ///
    /// Every part carries the **bare name's** span, because the name does have a place in the
    /// buffer and a statement dispatched to a server is spliced out of it; the synthesized node's
    /// own [`Span::empty`] would say there is none.
    pub(crate) fn object_name(&self, span: Span) -> ObjectName {
        ObjectName(
            [&self.catalog, &self.schema, &self.table]
                .into_iter()
                .map(|part| ObjectNamePart::Identifier(Ident::with_quote_and_span('"', span, part)))
                .collect(),
        )
    }

    /// The address as a message prints it — [`SessionName::qualified`], because these three parts
    /// are a server's spelling and quoting them whole would name one relation with dots in it.
    pub(crate) fn rendered(&self) -> String {
        format!(
            "'{}'",
            SessionName::qualified([
                self.catalog.as_str(),
                self.schema.as_str(),
                self.table.as_str(),
            ])
        )
    }
}

/// What the session knows about one referenced relation's output columns.
pub(crate) enum Columns {
    /// Definitely not in the catalog.
    Missing,
    /// Exists (or the provider errored) but its columns are unavailable — stay quiet.
    Opaque,
    /// Fully known column names.
    Known(Vec<String>),
}

/// Where a bare name already resolves — this session's `datafusion.catalog.default_catalog` and
/// `default_schema`.
///
/// Read from the config rather than the crate's `CATALOG`/`SCHEMA`, because the question is
/// "would the planner have found it" and the planner asks the config — so a context built any
/// other way cannot have its own default read as a source.
struct Home {
    catalog: String,
    schema: String,
}

impl Home {
    fn of(ctx: &SessionContext) -> Self {
        let state = ctx.state_ref();
        let state = state.read();
        let catalog = &state.config_options().catalog;
        Home {
            catalog: catalog.default_catalog.clone(),
            schema: catalog.default_schema.clone(),
        }
    }
}

/// The one name authority — see the module docs.
pub(crate) struct NameOracle<'a> {
    ctx: &'a SessionContext,
    home: Home,
    /// Every registered catalog that is not [`Home`]'s, in its registered spelling.
    databases: Vec<String>,
}

impl<'a> NameOracle<'a> {
    pub(crate) fn of(ctx: &'a SessionContext) -> Self {
        let home = Home::of(ctx);
        let folded = fold_ident(&home.catalog);
        let databases = ctx
            .catalog_names()
            .into_iter()
            .filter(|name| fold_ident(name) != folded)
            .collect();
        NameOracle {
            databases,
            home,
            ctx,
        }
    }

    /// Whether this session holds any connected database at all — what lets a project with none
    /// skip the whole search.
    pub(crate) fn has_databases(&self) -> bool {
        !self.databases.is_empty()
    }

    /// Whether `reference` names a relation this session can reach.
    ///
    /// A **bare** name is asked qualify's own question — the workspace, then the connected
    /// databases — so the tokens-only rung and the typo lint agree with the statement pass about
    /// what is a known name. An ambiguous name answers `true`: it names relations, and the
    /// statement pass has the better sentence for what is wrong with it. A qualified one is the
    /// session's to resolve, and an error resolving it answers `true`, because every caller here
    /// is deciding whether to *report* a name and none of them may report one nobody could judge.
    pub(crate) fn resolves(&self, reference: TableReference) -> bool {
        match &reference {
            TableReference::Bare { table } => {
                self.home_has(table) || self.candidates(table).is_some()
            }
            _ => self.ctx.table_exist(reference).unwrap_or(true),
        }
    }

    /// Where a bare name resolves outside the workspace — `None` when the workspace has it (it
    /// wins) or when nothing does.
    ///
    /// **Scoped to the schemas each data source shows** ([`shown_schemas`]): a schema switched
    /// off neither captures a bare name nor collides with one in a schema left on, where a name
    /// written in full still resolves into any of them. `table_exist` throughout, so only a hit
    /// pays for `table_names` — and only to recover the server's spelling.
    pub(crate) fn candidates(&self, name: &str) -> Option<Vec<Address>> {
        if self.home_has(name) {
            return None;
        }
        let folded = fold_ident(name);
        let mut found = Vec::new();
        for catalog in &self.databases {
            let Some(provider) = self.ctx.catalog(catalog) else {
                continue;
            };
            let shown = shown_schemas(provider.as_ref());
            for schema in provider.schema_names() {
                if shown
                    .as_ref()
                    .is_some_and(|shown| !shown.contains(&fold_ident(&schema)))
                {
                    continue;
                }
                let Some(relations) = provider.schema(&schema) else {
                    continue;
                };
                if !relations.table_exist(name) {
                    continue;
                }
                let Some(table) = relations
                    .table_names()
                    .into_iter()
                    .find(|listed| fold_ident(listed) == folded)
                else {
                    continue;
                };
                found.push(Address {
                    catalog: catalog.clone(),
                    schema,
                    table,
                });
            }
        }
        (!found.is_empty()).then_some(found)
    }

    /// What the session knows about `reference`'s columns.
    ///
    /// The resolver's rung: a relation whose provider answers gives its field names, one the
    /// session definitely does not hold is [`Columns::Missing`], and anything else is
    /// [`Columns::Opaque`] — a scope holding one goes quiet rather than guess.
    pub(crate) async fn columns(&self, reference: TableReference) -> Columns {
        match self.ctx.table_provider(reference.clone()).await {
            Ok(provider) => Columns::Known(
                provider
                    .schema()
                    .fields()
                    .iter()
                    .map(|f| f.name().clone())
                    .collect(),
            ),
            Err(_) => match self.ctx.table_exist(reference) {
                Ok(false) => Columns::Missing,
                _ => Columns::Opaque,
            },
        }
    }

    /// Whether the bare name already resolves where it resolves today — the workspace's one
    /// schema, holding its tables, its views and the snapshot spool, which is what keeps
    /// `__snap_` names inside the fence that reserves them.
    fn home_has(&self, name: &str) -> bool {
        self.ctx
            .catalog(&self.home.catalog)
            .and_then(|catalog| catalog.schema(&self.home.schema))
            .is_some_and(|schema| schema.table_exist(name))
    }
}
