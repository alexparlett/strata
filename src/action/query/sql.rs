//! SQL editor buffer ops — format in place, clear. Split out of the action::query module.

use dioxus::prelude::*;

use crate::state::AppState;

/// Pretty-print the active tab's SQL in place.
pub fn format(_state: Signal<AppState>) {
    let cur = crate::session::active_sql();
    let out = sqlformat::format(
        &cur,
        &sqlformat::QueryParams::None,
        &sqlformat::FormatOptions::default(),
    );
    crate::session::set_sql(crate::session::active_id(), out);
}
/// Clear the active tab's SQL.
pub fn clear(_state: Signal<AppState>) {
    crate::session::set_sql(crate::session::active_id(), String::new());
}
