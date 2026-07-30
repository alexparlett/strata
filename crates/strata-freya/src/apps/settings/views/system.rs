//! **Settings ▸ Appearance & behaviour ▸ System** (P4-06, design `Settings.dc.html`) — the five
//! settings that shape how the app itself behaves: what it reopens, where the folder picker
//! starts, where a project opens, whether a running query is worth a question, and how much
//! query history a project keeps.
//!
//! Every control writes [`SettingsCtx::draft`] and stops there; the footer's Apply commits. As
//! on the data-display pane, each of these already has its reader waiting on the other side of
//! that commit — startup routing reads `reopen_on_startup`, `platform::pick_project_folder`
//! the default directory, `platform::open::decide` the open preference, the close confirm
//! `confirm_close_running`, and the history satellite `max_history` — so this task is the
//! control, not the wiring.
//!
//! **Opening a project** is the row worth naming. Until now the only thing that wrote
//! `open_pref` was the This/New prompt's "Remember, don't ask again", which is one-way in
//! practice: once remembered, nothing in the app put it back to Ask. This segmented control is
//! how that decision is undone.
//!
//! The history limit's floor is `strata_core::config`'s [`HISTORY_MIN`], which is the floor
//! `history_cap` already applies — the field offers exactly the range its consumer honours,
//! for the same reason the column-width field does.
//!
//! Each row is built from its [`Anchor`] (P4-09), which is where its title and subtext live.

use freya::prelude::*;
use strata_core::config::{OpenPref, HISTORY_MIN};

use crate::apps::settings::views::Pane;
use crate::apps::settings::{Anchor, SettingsCtx};
use crate::components::form::{DirectoryField, Form, NumberField};
use crate::components::segmented_toggle::{SegmentedToggle, ToggleSegment};

/// The canvas's numeric field (`width: 130px`) — the same box the data-display pane's are.
const FIELD_WIDTH: f32 = 130.;

/// The history limit is bounded below and not above: keeping more runs costs a longer
/// `history.jsonl` and nothing else, and the canvas offers no ceiling either. The field's own
/// type is the only bound there is.
const NO_HISTORY_CAP: u32 = u32::MAX;

#[derive(PartialEq)]
pub struct SystemPane;

impl Component for SystemPane {
    fn render(&self) -> impl IntoElement {
        let ctx = use_consume::<SettingsCtx>();
        // Read in a block: the guard has to be gone before anything below takes a write one on
        // the same `State`.
        let (reopen, default_dir, open_pref, confirm_close, max_history) = {
            let draft = ctx.draft.read();
            (
                draft.reopen_on_startup,
                draft.default_project_dir.clone(),
                draft.open_pref,
                draft.confirm_close_running,
                draft.max_history,
            )
        };

        let body = Form::new()
            .preferences()
            .child(
                Anchor::Reopen
                    .row()
                    .trailing()
                    .on_press(move |_: Event<PressEventData>| {
                        ctx.edit(|s| s.reopen_on_startup = !s.reopen_on_startup)
                    })
                    .child(Switch::new().toggled(reopen).on_toggle(move |_| {
                        ctx.edit(|s| s.reopen_on_startup = !s.reopen_on_startup)
                    })),
            )
            .child(
                Anchor::DefaultDir
                    .row()
                    // A plain folder box, and deliberately not one that resolves what it is
                    // given the way `platform::pick_project_folder` does: this is where the
                    // picker *starts*, which need not hold a project at all — it is usually
                    // the folder projects get made in.
                    .child(
                        // The canvas's placeholder is `~/data`; the example here is absolute
                        // instead, and the difference is not cosmetic. Every consumer hands
                        // this string to the picker's `set_directory` as-is — nothing expands
                        // a leading `~` — so an example in that form is one the app would
                        // silently ignore, from a field whose own browse button only ever
                        // writes absolute paths.
                        DirectoryField::new(default_dir)
                            .placeholder("/Users/you/data")
                            .dialog_title("Default project directory")
                            .on_change(move |dir: String| {
                                ctx.edit(|s| s.default_project_dir = dir)
                            }),
                    ),
            )
            .child(
                Anchor::OpenPref
                    .row()
                    .child(OpenPrefControl { pref: open_pref }),
            )
            .child(
                Anchor::ConfirmClose
                    .row()
                    .trailing()
                    .on_press(move |_: Event<PressEventData>| {
                        ctx.edit(|s| s.confirm_close_running = !s.confirm_close_running)
                    })
                    .child(Switch::new().toggled(confirm_close).on_toggle(move |_| {
                        ctx.edit(|s| s.confirm_close_running = !s.confirm_close_running)
                    })),
            )
            .child(
                // Saturating, not `as`: a hand-edited config holding more than a u32 should show
                // the biggest number the field can offer, not wrap round to a small one.
                Anchor::HistoryLimit.row().child(
                    NumberField::new(
                        max_history.try_into().unwrap_or(NO_HISTORY_CAP),
                        HISTORY_MIN as u32,
                        NO_HISTORY_CAP,
                    )
                    .width(Size::px(FIELD_WIDTH))
                    .unit("runs")
                    .on_change(move |runs: u32| ctx.edit(|s| s.max_history = runs as usize)),
                ),
            );

        Pane::new(body)
    }
}

/// Where a project opens: the three-segment pill over `Settings::open_pref`.
///
/// A segmented toggle and not a `Select`, because all three answers fit on one line and the
/// choice is the row's whole content — the canvas draws it as the pill for the same reason.
#[derive(PartialEq)]
struct OpenPrefControl {
    pref: OpenPref,
}

impl Component for OpenPrefControl {
    fn render(&self) -> impl IntoElement {
        let ctx = use_consume::<SettingsCtx>();
        let set = move |pref: OpenPref| ctx.edit(|s| s.open_pref = pref);
        let pref = self.pref;
        let segment = move |label: &'static str, value: OpenPref| {
            ToggleSegment::text(label)
                .selected(pref == value)
                .on_press(move |_| set(value))
        };

        // The form layout, and a hug-content parent for it — the same pair the data-display
        // pane's density pill needs: the pill hugs its segments, so dropped straight into the
        // row's fill-width column it would stretch across the pane.
        rect().horizontal().child(
            SegmentedToggle::new()
                .form()
                .child(segment("Ask every time", OpenPref::Ask))
                .child(segment("This window", OpenPref::This))
                .child(segment("New window", OpenPref::New)),
        )
    }
}
