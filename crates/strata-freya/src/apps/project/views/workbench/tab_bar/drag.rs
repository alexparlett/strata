//! Pure drag-reorder maths for the tab strip, deliberately kept out of `TabBar::render`: mapping a
//! pointer position to an insert slot and the edge-scroll step. No hooks, no elements — data in,
//! numbers out, so it's unit-testable and the render just wires handlers to it.

use freya::prelude::Area;

/// The insert slot for a pointer at horizontal position `x`, given the areas of the tabs that *stay*
/// in the strip during the drag (the dragged tab is excluded — `show_while_dragging(false)` collapses
/// it out), in visible order. Each tab splits at its horizontal midpoint: the left half inserts before
/// it, the right half after — so dragging past the last tab's midpoint yields `len` (drop at the end).
///
/// The result is an index into that dragged-excluded order, which is exactly what
/// [`SessionState::move_tab`](crate::apps::project::state::SessionState::move_tab) wants: it removes
/// the dragged tab and inserts at this index, so there's no off-by-one to undo (and it no-ops when the
/// index is the tab's own original gap).
pub fn insert_slot(x: f32, tab_areas: &[Area]) -> usize {
    for (i, a) in tab_areas.iter().enumerate() {
        if x < a.min_x() + a.width() / 2.0 {
            return i;
        }
    }
    tab_areas.len()
}

/// The horizontal scroll delta (px, signed) when a drag hovers within `margin` of a viewport edge;
/// `0.0` in the middle. Positive scrolls toward the start, negative toward the end — Freya's scroll-x
/// *grows* to reveal earlier content (same convention as `ScrollController::scroll_to_item`), the
/// opposite of a web `scrollLeft`. So hovering the left edge (reveal earlier tabs) is `+step`.
pub fn edge_scroll(x: f32, viewport: Area, margin: f32, step: f32) -> f32 {
    if x < viewport.min_x() + margin {
        step
    } else if x > viewport.max_x() - margin {
        -step
    } else {
        0.0
    }
}