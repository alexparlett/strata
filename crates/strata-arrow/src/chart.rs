//! The chart bounds the read and the control offering it have to share. The chart read itself is
//! the engine's (`strata_engine::SnapshotReads::chart`); what lives here is the number both sides state.

/// The most bins a request may ask for. A histogram is a *picture* of a distribution, and
/// past a couple of hundred bars there are more bins than the canvas has columns of pixels.
/// It also keeps a bin count that arrived as a number from allocating against it.
///
/// Public because the surface offering the control has to bound its input by the same number
/// the read clamps to — a box that accepts 5 000 and a read that quietly answers 200 is a
/// control that shows one thing and means another.
pub const MAX_BINS: usize = 200;
