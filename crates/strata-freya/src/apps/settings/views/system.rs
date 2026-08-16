//! **Settings ▸ Appearance & behaviour ▸ System** (P4-06, design `Settings.dc.html`) — the six
//! settings that shape how the app itself behaves: what it reopens, where the folder picker
//! starts, where a project opens, whether a running query is worth a question, whether the
//! updater asks GitHub at startup, and how much query history a project keeps.
//!
//! Every control writes [`SettingsCtx::draft`] and stops there; the footer's Apply commits. As
//! on the data-display pane, each of these already has its reader waiting on the other side of
//! that commit — startup routing reads `reopen_on_startup`, `platform::pick_project_folder`
//! the default directory, `platform::open::decide` the open preference, the close confirm
//! `confirm_close_running`, the updater's startup check `check_updates`, and the history
//! satellite `max_history` — so this task is the control, not the wiring.
//!
//! **Check for updates on startup** gates only the automatic check (UP-02/UP-03) — App ▸ Check
//! for Updates… and the launcher rail's action run whatever it says. That is in the title
//! rather than in subtext under it: "on startup" is the whole of what the row does, and a
//! sentence restating it is the near-duplicate wording this codebase merges. Hence the
//! empty hint in the index, like `Theme`'s.
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
use crate::components::form::{Form, NumberField, PathField};
use crate::components::metrics::SETTINGS_FIELD_WIDTH;
use crate::components::segmented_toggle::{SegmentedToggle, ToggleSegment};

/// The history limit is bounded below and not above: keeping more runs costs a longer
/// `history.jsonl` and nothing else, and the canvas offers no ceiling either. The field's own
/// type is the only bound there is.
const NO_HISTORY_CAP: u32 = u32::MAX;

#[derive(PartialEq)]
pub struct SystemPane;

impl Component for SystemPane {
    fn render(&self) -> impl IntoElement {
        let ctx = use_consume::<SettingsCtx>();
        let (reopen, default_dir, open_pref, confirm_close, check_updates, max_history) = {
            let draft = ctx.draft.read();
            (
                draft.reopen_on_startup,
                draft.default_project_dir.clone(),
                draft.open_pref,
                draft.confirm_close_running,
                draft.check_updates,
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
                        ctx.edit(|s| s.reopen_on_startup = !s.reopen_on_startup);
                    })
                    .child(Switch::new().toggled(reopen).on_toggle(move |()| {
                        ctx.edit(|s| s.reopen_on_startup = !s.reopen_on_startup);
                    })),
            )
            .child(
                Anchor::DefaultDir.row().child(
                    PathField::folder(default_dir)
                        .placeholder("/Users/you/data")
                        .dialog_title("Default project directory")
                        .on_change(move |dir: String| {
                            ctx.edit(|s| s.default_project_dir = dir);
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
                        ctx.edit(|s| s.confirm_close_running = !s.confirm_close_running);
                    })
                    .child(Switch::new().toggled(confirm_close).on_toggle(move |()| {
                        ctx.edit(|s| s.confirm_close_running = !s.confirm_close_running);
                    })),
            )
            .child(
                Anchor::CheckUpdates
                    .row()
                    .trailing()
                    .on_press(move |_: Event<PressEventData>| {
                        ctx.edit(|s| s.check_updates = !s.check_updates);
                    })
                    .child(Switch::new().toggled(check_updates).on_toggle(move |()| {
                        ctx.edit(|s| s.check_updates = !s.check_updates);
                    })),
            )
            .child(
                Anchor::HistoryLimit.row().child(
                    NumberField::new(
                        max_history.try_into().unwrap_or(NO_HISTORY_CAP),
                        HISTORY_MIN as u32,
                        NO_HISTORY_CAP,
                    )
                    .width(Size::px(SETTINGS_FIELD_WIDTH))
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

        rect().horizontal().child(
            SegmentedToggle::new()
                .form()
                .child(segment("Ask every time", OpenPref::Ask))
                .child(segment("This window", OpenPref::This))
                .child(segment("New window", OpenPref::New)),
        )
    }
}
