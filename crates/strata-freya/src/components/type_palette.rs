//! The **type palette** — the seven categorical hues, one per [`Kind`], that every surface showing
//! a column's type wears: the datagrid header's dtype labels, the record view's field gutter, and
//! the catalog sidebar's column swatches.
//!
//! Named for [`Kind`], not for Arrow: `Str · Num · Bool · Ts · Struct · List · Map` is *Strata's*
//! seven-bucket display taxonomy, not Arrow's type system (which has dozens). `Kind::from_arrow`
//! derives it from an Arrow dtype, but the palette is keyed by the display kind, so an
//! Arrow-specific name would claim a precision it doesn't have.
//!
//! It is a shared **token group**, not a component: `%[no_ext]` so `define_theme!` emits the theme
//! types without the `…ThemePartialExt` trait that would need a component struct to hang off. It
//! registers under its own `"type_palette"` key like any other entry in the mapping table
//! (`theme/components.rs`), mapped once onto the `data_type.*` roles instead of repeated per
//! consumer — which is how the four copies had already drifted (Daylight's `datagrid` and
//! `record_view` were still carrying Midnight's neon ramp on a white background).
//!
//! Reading this from a component is *not* the cross-component theme read the surrounding code
//! warns against — nobody reaches into `datagrid`'s theme. It is one palette with one owner.
//!
//! One consumer borrows it for something other than dtypes: the EXPLAIN plan view maps operator
//! kinds, metric units and insight tones onto the same seven hues (see `explain_plan::palette`).
//! That is deliberate — it wants the app's categorical ramp, and it was already keeping a private
//! copy of exactly these values. A future chart ramp is its **own** group, not this one — which is
//! why the key is `type_palette` and not a bare `palette`.

use freya::components::{define_theme, get_theme};
use freya::prelude::Color;
use strata_model::Kind;

define_theme!(
    %[no_ext]
    %[component]
    pub TypePalette {
        %[fields]
        str_color: Color,
        num_color: Color,
        bool_color: Color,
        ts_color: Color,
        struct_color: Color,
        list_color: Color,
        map_color: Color,
    }
);

/// This window's resolved type palette. Call from a component's `render`.
pub fn type_palette() -> TypePaletteTheme {
    get_theme!(
        &None::<TypePaletteThemePartial>,
        TypePaletteThemePreference,
        "type_palette"
    )
}

/// The hue for a column [`Kind`] — the dtype label, the type swatch, the cell tint. The single
/// mapping; consumers pick where to paint it, never which colour a kind is.
pub fn kind_color(kind: Kind, t: &TypePaletteTheme) -> Color {
    match kind {
        Kind::Str => t.str_color,
        Kind::Num => t.num_color,
        Kind::Bool => t.bool_color,
        Kind::Ts => t.ts_color,
        Kind::Struct => t.struct_color,
        Kind::List => t.list_color,
        Kind::Map => t.map_color,
    }
}
