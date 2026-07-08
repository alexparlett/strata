//! SQL editor DDL policy — classifies a statement with DataFusion's bundled
//! parser (`DFParser`, which also understands `CREATE EXTERNAL TABLE` / `COPY`)
//! *before* `ctx.sql`, since DataFusion executes DDL eagerly.
//!
//! Policy (managed catalog, capture views):
//!   * allow  — SELECT / WITH / EXPLAIN / DESCRIBE / SHOW → run as a query
//!   * allow  — CREATE [OR REPLACE] VIEW / DROP VIEW      → captured into the project
//!   * block  — CREATE EXTERNAL TABLE / CREATE TABLE / CTAS / INSERT / UPDATE /
//!              DELETE / COPY / ALTER / DROP TABLE / TRUNCATE → point at the UI
//!   * block  — CREATE DATABASE / CREATE SCHEMA (hard)   → outside the model
//!
//! Default is *allow* (read-oriented statements fall through to DataFusion,
//! which executes or returns a proper error); writes/DDL are explicitly blocked.

use datafusion::sql::parser::{DFParser, Statement as DFStatement};
use datafusion::sql::sqlparser::ast::{CreateView, ObjectType, Statement as SqlStatement};
use serde_json::to_string;

#[derive(Debug, Clone)]
pub enum Decision {
    Query,
    CaptureView { name: String, sql: String },
    DropView { name: String },
    Block { reason: String },
}

pub fn classify(sql: &str) -> Decision {
    let statements = match DFParser::parse_sql(sql) {
        Ok(s) => s,
        // Unparseable → let the engine surface a proper syntax error.
        Err(_) => return Decision::Query,
    };
    let Some(stmt) = statements.into_iter().next() else {
        return Decision::Query;
    };

    match stmt {
        DFStatement::CreateExternalTable(_) => block(
            "Create tables through Table Config so their sources, format and partitioning are saved to the project.",
        ),
        DFStatement::CopyTo(_) => block("Use the Export dialog to write results to files."),
        DFStatement::Explain(_) => Decision::Query,
        DFStatement::Statement(inner) => classify_sql(*inner),
        DFStatement::Reset(_) => block(
            "Reset not supported",
        ),
    }
}

fn classify_sql(stmt: SqlStatement) -> Decision {
    match stmt {
        // views — allowed + captured
        SqlStatement::CreateView(CreateView { name, query, ..}) => Decision::CaptureView {
            name: last_ident(&name.to_string()),
            sql: query.to_string(),
        },
        SqlStatement::Drop {
            object_type: ObjectType::View,
            names,
            ..
        } => Decision::DropView {
            name: names
                .first()
                .map(|n| last_ident(&n.to_string()))
                .unwrap_or_default(),
        },

        // hard-blocked (outside the flat-catalog model)
        SqlStatement::CreateSchema { .. } | SqlStatement::CreateDatabase { .. } => block(
            "Schemas and databases aren't part of the project model — everything lives in one catalog.",
        ),

        // catalog changes routed to the UI
        SqlStatement::CreateTable(_) => block(
            "In-memory / CTAS tables aren't supported. Register files as an external table via Table Config, or save a query as a view.",
        ),
        SqlStatement::Insert(_) | SqlStatement::Update { .. } | SqlStatement::Delete(_) => {
            block("Data is read-only here. Use Export to write results to a file.")
        }
        SqlStatement::AlterTable { .. }
        | SqlStatement::Truncate { .. }
        | SqlStatement::Drop { .. } => {
            block("That statement isn't allowed from the editor — manage the catalog through the UI.")
        }

        // everything else (Query, Explain, ExplainTable/DESCRIBE, SHOW …) → allow
        _ => Decision::Query,
    }
}

fn block(reason: &str) -> Decision {
    Decision::Block {
        reason: reason.to_string(),
    }
}

/// Strip schema qualifiers/quoting: `catalog.schema.name` → `name`.
fn last_ident(name: &str) -> String {
    name.rsplit('.')
        .next()
        .unwrap_or(name)
        .trim_matches(|c| c == '"' || c == '`')
        .to_string()
}
