//! Edit-menu commands (⌘A / ⌘C) routed by the current focus scope.
//!
//! The native menu is a dumb adapter — it dispatches `Action::SelectAll` / `Action::Copy`
//! and doesn't care where they land. *This* decides, off `menu::select_all_scope()`: the
//! results grid, or the focused text field (re-emitted as the native `selectAll:`/`copy:`
//! down the responder chain). The grid select-all is a `runs` mutation, so it lives here
//! with the other results-state actions, not in the grid render code.

use crate::menu::{select_all_scope, SelectAllScope};

/// ⌘A — select the whole result page (grid focused), or the focused text field.
pub fn select_all() {
    match select_all_scope() {
        SelectAllScope::Grid => select_all_grid(),
        // A text field holds focus (that's what set this scope) — re-emit the native
        // `selectAll:` so it selects the field's own text, the eval-free system Select All.
        SelectAllScope::Input => crate::window::send_select_all(),
        // The menu item is greyed outside those scopes, so this shouldn't fire — defensive.
        SelectAllScope::None => {}
    }
}

/// ⌘C — copy the grid selection (TSV, the paste-friendly default), or re-emit the native
/// `copy:` for the focused text field.
pub fn menu_copy() {
    match select_all_scope() {
        SelectAllScope::Grid => super::copy_selection(crate::engine::serialize::TextFormat::Tsv),
        SelectAllScope::Input | SelectAllScope::None => crate::window::send_copy(),
    }
}

/// Select every cell on the active tab's current result page. A `runs` mutation; dims are
/// recomputed from the run (this handler has no grid component scope) to match the grid's
/// page-local search filtering.
fn select_all_grid() {
    let ws = crate::session::active_id();
    if ws == 0 {
        return;
    }
    crate::runs::edit_existing(ws, |run| {
        let search = run.result_search.to_lowercase();
        let dims = run.result.as_ref().map(|result| {
            let nrows = result
                .rows
                .iter()
                .filter(|r| {
                    search.is_empty() || r.iter().any(|c| c.text.to_lowercase().contains(&search))
                })
                .count();
            (nrows, result.columns.len())
        });
        if let Some((nrows, ncols)) = dims {
            if nrows > 0 && ncols > 0 {
                run.sel = Some(crate::runs::Selection::Cell {
                    ar: 0,
                    ac: 0,
                    fr: nrows - 1,
                    fc: ncols - 1,
                });
            }
        }
    });
}
