//! Session store hooks — window-root initialisation.

use freya::radio::{use_init_radio_station, RadioStation};

use super::{Chan, SessionState};

/// Initialise this window's Session store (opening one blank tab) and provide it via context.
/// Call once in the window root; returns the station for the root to read / drive.
pub fn use_init_session() -> RadioStation<SessionState, Chan> {
    use_init_radio_station::<SessionState, Chan>(|| {
        let mut s = SessionState::default();
        s.open_blank();
        s
    })
}
