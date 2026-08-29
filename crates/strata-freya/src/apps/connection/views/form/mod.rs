//! The editor's fields, in canvas order: **PROVIDER**, then the rows the chosen source declared.
//!
//! **Nothing here is named after a source, and there is no second dress beside the generic one.**
//! The picker offers what the registry serves, and every row below it comes from that
//! registrant's declaration ([`SourceInfo::keys`](strata_engine::SourceInfo)) — so registering a
//! `DataSource` puts a working editor in front of it, and a source this build does not serve has
//! no form here at all rather than a hand-written one.
//!
//! Rows are not shipped disabled: a control that cannot mean anything for the chosen source is
//! not a control (the same call the Configure window makes about its LOCATION toggle), and that
//! goes for a whole form as much as for one row.
//!
//! Built from `components::form` — a [`Form`] of [`Row`](crate::components::form::Row)s, so the
//! label register, the `REQUIRED` markers and the rhythm between rows are the app's rather than
//! this window's.
//!
//! **A field's error is not painted on the field.** The one thing that says why Save is off is
//! the footer, and it is the same value that disables the button ([`super::footer`]) — which is
//! what stops a form from having two accounts of its own validity that can disagree. The label
//! still carries `REQUIRED`, because that is a fact about the field rather than a verdict on what
//! is in it.

mod picker;
mod source;

use freya::prelude::*;

use crate::apps::connection::ConnectionCtx;
use crate::apps::project::contexts::EngineCtx;
use crate::components::form::{Form, LABEL_GAP};
use crate::components::metrics::SP_4;

/// The gap between a control and the thing that qualifies it — a secret box and the two presses
/// under it (canvas `var(--sp-4)`). Inside the row, because a qualifier is what its control's
/// answer *means* rather than a second question.
pub(super) const QUALIFIER_GAP: f32 = SP_4;
/// A box whose value is a short, known-shaped word rather than free text — a mode, a
/// connection's name. One number, because it is one judgement about one kind of value.
pub(super) const NARROW_WIDTH: f32 = 180.;

/// Every row the chosen source has, in canvas order.
///
/// The **declaration** travels on the draft, because the footer and the def projection need it
/// too; the registrant is looked up here for what decides only what is *drawn* — the mode, and
/// the address box's own hint. Every child carries the kind in its key, so a row that means
/// something different under two sources is a different node rather than a reused buffer —
/// `Row`'s own contract.
#[derive(PartialEq)]
pub struct Fields;

impl Component for Fields {
    fn render(&self) -> impl IntoElement {
        let ctx = use_consume::<ConnectionCtx>();
        let engine = use_consume::<EngineCtx>();
        let (kind, keys) = {
            let draft = ctx.draft.read();
            (draft.kind.clone(), draft.settings)
        };
        let registrant = engine
            .sources()
            .registrants()
            .into_iter()
            .find(|info| info.kind == kind);

        let scope = kind.as_str();
        let picker = picker::ProviderPicker { key: DiffKey::None }.key(format!("provider·{scope}"));
        source::rows(Form::new().child(picker), ctx, keys, &registrant, scope)
    }
}

/// Set a control's qualifier under it at [`QUALIFIER_GAP`] — the row's own child spacing is the
/// label gap, which is the distance to a *label*, not between two controls.
pub(super) fn qualifier(child: impl IntoElement) -> Element {
    rect()
        .width(Size::fill())
        .padding(Gaps::new(QUALIFIER_GAP - LABEL_GAP, 0., 0., 0.))
        .child(child)
        .into_element()
}
