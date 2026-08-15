//! The **semantic tones** — `success` / `info` / `warning` / `error`, read off the role
//! vocabulary as one shared hook. A severity's colour follows the app-wide ramp wherever it
//! appears: Problems' glyphs, Events' dots, the status bar's state dot all paint
//! from these four, never from a surface's own theme. This is the one place that reads them —
//! three surfaces had grown three copies of the same four-slot read, with the fields in
//! different orders.

use freya::prelude::*;

use crate::theme::{use_roles, Role};

/// The four semantic tones, plus the clean state's tick (`ok` is the `success` role).
#[derive(Clone, Copy, PartialEq)]
pub struct Tones {
    pub error: Color,
    pub warning: Color,
    pub info: Color,
    pub ok: Color,
}

/// Resolve [`Tones`] from the active theme. A **hook** (one theme read), so call it exactly once
/// per render — like `type_palette`.
pub fn tones() -> Tones {
    let roles = use_roles();
    Tones {
        error: roles.get(Role::Error),
        warning: roles.get(Role::Warning),
        info: roles.get(Role::Info),
        ok: roles.get(Role::Success),
    }
}
