//! The **profiling vocabulary**: where a catalog profile's aggregate runs, what it computes for a
//! column of each [`Kind`], and what it therefore leaves out.
//!
//! The scan itself is the engine's (`strata_engine::profile`) — it is a `DataFrame` aggregate over
//! a relation. What lives here is the part every surface has to agree with it about: the inspector
//! renders the footnote beside the numbers, and the confirm dialog says in advance which set a
//! target will get.

use strata_model::{Kind, StatKey};

/// **Where a profile's aggregate will run** — one value deciding the whole shape of the scan,
/// because both halves turn on the same fact.
///
/// A workspace entry's aggregate is executed by DataFusion over files it read itself, so every
/// expression DataFusion has is available. A relation inside a data source's catalog is
/// the opposite: `datafusion-federation` sweeps the aggregate into **one remote statement**, the
/// unparser renders it into SQL, and the server runs it — so an expression the server does not
/// have is not a slower plan but a failed one, and there is no per-expression fallback to catch it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Profiled {
    /// A table or view the workspace registered, scanned by DataFusion itself.
    Workspace,
    /// A relation inside a data source's catalog, aggregated by the server.
    Database,
}

impl Profiled {
    /// The facts a scan computes for a column of `kind`.
    ///
    /// **Two facts are dropped for a database, and neither is a preference.** A federated subplan
    /// has no per-expression fallback: an aggregate the server does not have fails the *whole*
    /// scan, so anything that might not be there has to go.
    ///
    /// **The median**, on a numeric column. DataFusion's is `approx_percentile_cont`, a DF-only
    /// aggregate with no `PostgreSQL` spelling — and DataFusion 54's `PostgreSqlDialect` offers
    /// scalar-function overrides only, so there is nowhere to teach it one. Postgres's own
    /// `percentile_cont(0.5) WITHIN GROUP (ORDER BY x)` is an ordered-set aggregate the unparser
    /// has no expression to emit, so it is not a substitution either.
    ///
    /// **Min and max, on a string column**, which is subtler and was missed on the first pass.
    /// `Kind` is read from the *mapped Arrow* type, and DB-02 maps every type the Postgres
    /// connector cannot represent to `Utf8` (`UnsupportedTypeAction::String`) — so `Kind::Str` is
    /// not "a text column", it is "text, **or `jsonb`, or `xml`, or `PostGIS` geometry**". Postgres
    /// documents `min`/`max` as available for numeric, string, date/time and enum types plus a
    /// short list of others, and `json`/`jsonb` is not among them: `min(<jsonb>)` is
    /// *function does not exist*, and it takes the scan of the whole relation with it. Since the
    /// mapping is lossy in exactly the way that matters, the server type cannot be recovered here,
    /// so the bound has to be drawn at the kind.
    ///
    /// The distinct count survives that cut deliberately: it is the fact profiling exists for, and
    /// it needs only an equality operator, which `jsonb` has. A type with **no** equality operator
    /// at all (`xml` is the one in practice) still fails the scan — that is the workstream's
    /// accepted "fails loudly at execute time" envelope, and it is a rare type where `jsonb` is a
    /// common one DB-02 went out of its way to support.
    ///
    /// [`stats_footnote`] states both omissions where the numbers are. Everything kept is pinned
    /// by the rendered SQL of `strata_engine::profile`'s tests and by the integration test's
    /// `EXPLAIN`.
    pub fn wanted(self, kind: Kind) -> &'static [StatKey] {
        const NUM: [StatKey; 5] = [
            StatKey::Distinct,
            StatKey::Min,
            StatKey::Max,
            StatKey::Mean,
            StatKey::Median,
        ];
        const ORDERED: [StatKey; 3] = [StatKey::Distinct, StatKey::Min, StatKey::Max];
        match (self, kind) {
            (Profiled::Workspace, Kind::Num) => &NUM[..],
            (Profiled::Database, Kind::Num) => &NUM[..4],
            (Profiled::Workspace, Kind::Str) => &ORDERED[..],
            (Profiled::Database, Kind::Str) => &ORDERED[..1],
            (_, Kind::Ts) => &ORDERED[..],
            (_, Kind::Bool | Kind::Struct | Kind::List | Kind::Map) => &[][..],
        }
    }
}

/// What a scan of `at` leaves out, when that is anything — the zone's footnote, so an absent
/// MEDIAN is a stated omission rather than a fact the user is left to wonder about. `None` where
/// nothing was dropped.
///
/// The wording names *what is missing*, not whose fault it is: the server has both a median and a
/// string minimum, and what cannot carry them is the one statement this scan is unparsed into.
/// Saying "the database has no aggregate for it" would tell the user something false about their
/// own server.
pub fn stats_footnote(at: Profiled) -> Option<&'static str> {
    match at {
        Profiled::Workspace => None,
        Profiled::Database => Some(
            "A scan on a database runs as one statement on the server, which leaves out medians, \
             and minimums and maximums of text columns.",
        ),
    }
}
