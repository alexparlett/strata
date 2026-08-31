//! Column/schema vocabulary: a column's visual [`Kind`], its [`ColumnInfo`], and the
//! [`Stat`]s known about it. Produced by the engine (footer) and by profiling (scan);
//! stored by the project; rendered by the UI — a leaf everyone depends down onto.

/// The visual "kind" of a column, driving dot/type/cell colours (matches the
/// Strata type→colour map).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// Text.
    Str,
    /// A number.
    Num,
    /// A boolean.
    Bool,
    /// A date, a time or a timestamp.
    Ts,
    /// A struct.
    Struct,
    /// A list.
    List,
    /// A map.
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

    /// Whether a cell of this kind holds a value that expands.
    pub fn is_nested(self) -> bool {
        matches!(self, Kind::Struct | Kind::List | Kind::Map)
    }
}

/// What a column may be **encoded** as on a chart (`docs/CHART_SPEC.md` §3).
///
/// A second taxonomy rather than a reading of [`Kind`], which is coarser on purpose. Resolved from
/// the Arrow `DataType` itself (`strata_engine::catalog::chart_role`) — never from a name and
/// never from a type's *spelling*.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ChartRole {
    /// A number: a Y, either scatter axis, a histogram's value.
    Measure,
    /// A point on the calendar (a date or a timestamp): the default X, and the only role a
    /// **stride** means anything to — which is why it is not the same variant as
    /// [`Clock`](Self::Clock). A day-wide `date_bin` over a time of day is refused outright.
    Instant,
    /// A time of day, with no calendar under it. An axis and a series split like
    /// [`Instant`](Self::Instant), but nothing a date stride can bin.
    Clock,
    /// A category: an X, or the column a series splits on.
    Dimension,
    /// Nested, opaque, or simply not a thing with an axis — offered nowhere.
    Other,
}

/// Which fact a [`Stat`] carries. Keyed rather than positional so the two tiers interlock: the
/// profile surfaces only what the source did not already answer for free.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatKey {
    /// How many rows are null.
    Nulls,
    /// The smallest value.
    Min,
    /// The largest value.
    Max,
    /// How many distinct values there are.
    Distinct,
    /// The arithmetic mean.
    Mean,
    /// The median.
    Median,
}

/// One fact about a column, ready to display.
///
/// A **list**, not a fixed set of fields: which facts exist depends on where they came from, and
/// fixed `Option` fields would bake the Parquet shape into every source. Both tiers emit this one
/// shape, so the inspector renders a row per fact that exists rather than a grid of blanks.
///
/// `exact` is false when the source truncated the value, making it a bound rather than the value —
/// the inspector marks those `~`. Computed facts are always exact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stat {
    /// Which fact this is.
    pub key: StatKey,
    /// The value, formatted for display.
    pub text: String,
    /// Whether the value is the fact itself rather than a bound on it.
    pub exact: bool,
}

/// One column of a table or view — its type, nullability, nested children, and the
/// facts read for free from the source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnInfo {
    /// The column's name.
    pub name: String,
    /// Its Arrow type, as that type prints itself.
    pub dtype: String,
    /// The visual kind that type falls under.
    pub kind: Kind,
    /// What a chart may encode this column as — see [`ChartRole`]. Carried here because this
    /// struct is what survives the boundary the Arrow type itself does not cross.
    pub role: ChartRole,
    /// Whether the column admits nulls.
    pub nullable: bool,
    /// The columns nested inside it, for a struct, a list or a map.
    pub children: Vec<ColumnInfo>,
    /// Facts the source reports **for free** — read, never computed. Empty for every format but
    /// Parquet and Arrow.
    pub stats: Vec<Stat>,
}
