//! Profile cost-confirm (D4 / U15) — an always-mounted host reading
//! `overlays::profile_confirm`.
//!
//! Every *first* profile of a table comes through here, from both entry points (the
//! inspector's "Profile table" button and the sidebar's table context menu) — the same
//! action shouldn't warn from one place and not the other. Only the PROFILE zone's ↻
//! re-scan skips it, being an explicit re-run of something already chosen.
//!
//! No figures. The canvas gates this on `files > 50` and quotes "248 files · ~186 MB",
//! but file count is a backwards proxy for cost — one 10GB Parquet file trips nothing
//! while sixty small ones trip it — so the honest version names the shape of the work
//! and leaves the arithmetic out.

use dioxus::prelude::*;

use crate::action::{dispatch, Action};
use crate::ui::components::{Button, ButtonVariant, Dialog, Icon, Readout, Title};
use crate::ui::icons::{IconName, IconSize};

#[component]
pub fn ProfileConfirmHost() -> Element {
    let Some(name) = crate::overlays::OVERLAYS
        .resolve()
        .read()
        .profile_confirm
        .clone()
    else {
        return rsx! {};
    };
    let go = name.clone();
    let is_view = crate::project::is_view(&name);

    rsx! {
        Dialog { on_close: move |_| crate::overlays::close_profile_confirm(), card_class: "confirm".to_string(), z: 80,
            div { class: "confirm-head",
                div { class: "confirm-ico", Icon { name: IconName::Chart, size: IconSize::Px(20) } }
                div { style: "flex:1;min-width:0;",
                    Title { class: "confirm-title", "Profile " span { class: "nm", "{name}" } "?" }
                    Readout { class: "confirm-body",
                        // A view's cost is its whole query, not a file scan — saying
                        // "reads every file" would understate a join.
                        {if is_view {
                            "Profiling runs the view's query in full to compute distinct counts, means and distributions — reading whatever the query reads, which may be several tables. Distinct counts can't be merged, so none of it can be shortcut. The result is cached until the view changes."
                        } else {
                            "Profiling reads every file to compute distinct counts, means and distributions. Distinct counts can't be merged across files, so this is a full scan. The result is cached until the table changes."
                        }}
                    }
                }
            }
            div { class: "confirm-foot",
                Button { variant: ButtonVariant::Secondary, onclick: move |_| crate::overlays::close_profile_confirm(), "Cancel" }
                Button {
                    variant: ButtonVariant::Primary,
                    onclick: move |_| dispatch(Action::ConfirmProfileTable(go.clone())),
                    "Profile"
                }
            }
        }
    }
}
