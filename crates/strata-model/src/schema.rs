//! Column/schema vocabulary: a column's visual [`Kind`], its [`ColumnInfo`], and the
//! [`Stat`]s known about it. Produced by the engine (footer) and by profiling (scan);
//! stored by the project; rendered by the UI — a leaf everyone depends down onto.

/// The visual "kind" of a column, driving dot/type/cell colours (matches the
/// Strata type→colour map).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Str,
    Num,
    Bool,
    Ts,
    Struct,
    List,
    Map,
}

impl Kind {
    /// Infer a kind from an Arrow `DataType` debug string (e.g. "Int64",
    /// "Utf8", "Timestamp(...)", "Struct(...)", "List(...)", "Map(...)").
    pub fn from_arrow(dtype: &str) -> Kind {
        let d = dtype;
        if d.starts_with("Struct") {
            Kind::Struct
        } else if d.starts_with("List")
            || d.starts_with("LargeList")
            || d.starts_with("FixedSizeList")
        {
            Kind::List
        } else if d.starts_with("Map") {
            Kind::Map
        } else if d.starts_with("Boolean") {
            Kind::Bool
        } else if d.starts_with("Timestamp") || d.starts_with("Date") || d.starts_with("Time") {
            Kind::Ts
        } else if d.starts_with("Int")
            || d.starts_with("UInt")
            || d.starts_with("Float")
            || d.starts_with("Decimal")
        {
            Kind::Num
        } else {
            Kind::Str
        }
    }

    /// CSS class for the small square dot (`d-num`, ...).
    pub fn dot_class(self) -> &'static str {
        match self {
            Kind::Str => "d-str",
            Kind::Num => "d-num",
            Kind::Bool => "d-bool",
            Kind::Ts => "d-ts",
            Kind::Struct => "d-struct",
            Kind::List => "d-list",
            Kind::Map => "d-map",
        }
    }

    /// CSS colour for the type swatch/dot (`var(--t-num)`, ...) — for the `Dot` component's
    /// `color` prop (inline fill, so it beats the base dot styling).
    pub fn dot_color(self) -> &'static str {
        match self {
            Kind::Str => "var(--t-str)",
            Kind::Num => "var(--t-num)",
            Kind::Bool => "var(--t-bool)",
            Kind::Ts => "var(--t-ts)",
            Kind::Struct => "var(--t-struct)",
            Kind::List => "var(--t-list)",
            Kind::Map => "var(--t-map)",
        }
    }

    /// CSS class for coloured type text (`t-num`, ...).
    pub fn text_class(self) -> &'static str {
        match self {
            Kind::Str => "t-str",
            Kind::Num => "t-num",
            Kind::Bool => "t-bool",
            Kind::Ts => "t-ts",
            Kind::Struct => "t-struct",
            Kind::List => "t-list",
            Kind::Map => "t-map",
        }
    }

    /// Extra CSS class for a result cell (`num`/`bool`/`ts`/`nested`), if any.
    pub fn cell_class(self) -> &'static str {
        match self {
            Kind::Num => "num",
            Kind::Bool => "bool",
            Kind::Ts => "ts",
            Kind::Struct | Kind::List | Kind::Map => "nested",
            Kind::Str => "",
        }
    }

    pub fn is_nested(self) -> bool {
        matches!(self, Kind::Struct | Kind::List | Kind::Map)
    }
}

/// What a column may be **encoded** as on a chart (`docs/CHART_SPEC.md` §3).
///
/// Sibling to [`Kind`], and deliberately a second taxonomy rather than a reading of that one:
/// `Kind` is what a column *looks* like (dot and cell colours) and is coarser on purpose — it
/// folds a union in with the strings, which is harmless for a swatch and wrong for an axis.
///
/// Resolved from the Arrow `DataType` itself, in the one place that still has it
/// (`strata_core::engine::catalog::chart_role`, called by `column_info`) — never from a name
/// and never from a type's *spelling*, which is a rendering of a type and not the type.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ChartRole {
    /// A number: a Y, either scatter axis, a histogram's value. Exactly the set the engine's
    /// own read will accept as a measure.
    Measure,
    /// A point on the calendar (a date or a timestamp): the default X, the reason a default
    /// mark is a line, and the only role a **stride** means anything to — which is why it is
    /// not the same variant as [`Clock`](Self::Clock). `date_bin(interval '1 day', …)` over a
    /// time of day is not a coarser reading of it, it is one bucket; DataFusion says so
    /// outright ("`DATE_BIN` stride for TIME input must be less than 1 day").
    ///
    /// **Nothing reads the distinction today.** It was split for a scaffold that wrote that
    /// SQL, and the scaffold was cut (`docs/CHART_SPEC.md` §8); the split was kept because
    /// chart-side bucketing needs it (§3) and the only way to recover it later is a type's
    /// *spelling*, which this taxonomy exists to rule out.
    Instant,
    /// A time of day, with no calendar under it. An axis and a series split like
    /// [`Instant`](Self::Instant) — it is ordered, and a line across it reads — but nothing a
    /// date stride can bin.
    Clock,
    /// A category: an X, or the column a series splits on.
    Dimension,
    /// Nested, opaque, or simply not a thing with an axis — offered nowhere.
    Other,
}

/// Which fact a [`Stat`] carries.
///
/// Keyed rather than positional so the two tiers can interlock: D4's profile surfaces
/// only what the source didn't already answer for free, by key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatKey {
    Nulls,
    Min,
    Max,
    Distinct,
    Mean,
    Median,
}

/// One fact about a column, ready to display.
///
/// Deliberately a **list**, not a fixed set of fields: which facts exist depends
/// entirely on where they came from. A Parquet footer yields nulls/min/max for nothing;
/// CSV and JSON yield literally none; D4's profile computes whatever the source didn't,
/// and adds distinct/mean/median besides. Fixed `Option` fields would bake the Parquet
/// shape into every source and leave the profile nowhere to put the same facts. Both
/// tiers emit this one shape, so the inspector renders a row per fact that genuinely
/// exists rather than a grid of blanks.
///
/// `exact` is false when the source truncated the value (Parquet does this to long
/// strings / binary routinely), making it a bound rather than the value — the inspector
/// marks those `~`. Computed facts are always exact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stat {
    pub key: StatKey,
    pub text: String,
    pub exact: bool,
}

/// One column of a table or view — its type, nullability, nested children, and the
/// facts read for free from the source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnInfo {
    pub name: String,
    pub dtype: String,
    pub kind: Kind,
    /// What a chart may encode this column as — see [`ChartRole`]. Carried here because it is
    /// a fact about the column's Arrow type, and this struct is what survives the boundary
    /// the type itself does not cross.
    pub role: ChartRole,
    pub nullable: bool,
    pub children: Vec<ColumnInfo>,
    /// Facts the source reports **for free** — read, never computed. Empty for any
    /// format without metadata to read, which is every format but Parquet (and Arrow).
    pub stats: Vec<Stat>,
}
