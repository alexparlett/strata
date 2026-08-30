//! **LOCATION** — where this table's files are: the local disk, or one of the project's object
//! stores (W7 · 04) — and, behind the second answer, the **TYPE** / **CONNECTION** pair that says
//! which store.
//!
//! **The choice is explicit, never inferred from a typed path** (spec §4). An earlier draft of
//! this feature read the first source's scheme and switched the mode under the user; that guess
//! is wrong precisely when it matters (a local path that happens to start `s3` , a bucket name
//! pasted into the local box), and the whole point of the data source picker is that a table's
//! store is the one the user chose rather than one parsed out of a string.
//!
//! **TYPE filters, CONNECTION chooses.** The two are kept in step by the draft itself
//! ([`ConfigureDraft::set_provider`]) rather than by these components, so the picker can only ever
//! show a data source it also offers. A provider with none says so in a line under the picker
//! instead of opening an empty dropdown — "no data sources" and "the list has not loaded" look
//! identical in an empty menu, and only one of them is worth acting on.
//!
//! **New data source… sets the project window's own slot.** Opening the editor needs that window's
//! handles, and there is deliberately no second open path — the pane's `+`, its empty-state CTA,
//! a row's *Edit data source* and this item all set [`SourceRequest`] and stop
//! (`project::views::source_launch`). The window that opens is a child of the *project*
//! window, not of this one, so it outlives a Configure window closed while it is up, and the
//! data source it saves lands in the store this picker is already reading.

use freya::prelude::*;
use freya::radio::{use_radio, use_radio_station, RadioStation};
use strata_engine::{SourceInfo, SourceMode};

use crate::apps::project::contexts::EngineCtx;
use strata_model::SourceDef;

use crate::apps::configure::model::{sources_for, Where};
use crate::apps::configure::ConfigureCtx;
use crate::apps::project::SourceRequest;
use crate::apps::project::{ProjChan, ProjectState};
use crate::apps::source::SourceTarget;
use crate::components::form::{form_theme, Row, FIELD_HEIGHT};
use crate::components::icon::{Icon, IconName};
use crate::components::metrics::{SP_3, SP_4};
use crate::components::segmented_toggle::{SegmentedToggle, ToggleSegment};
use crate::components::typography::{Caption, MonoValue, Prose};

/// The gap between the TYPE pill and the CONNECTION picker beside it — the identity row's own
/// column gap, because this is the same two-controls-on-one-line shape.
const COLUMN_GAP: f32 = SP_4;
/// The gap between the picker and the line that says its provider has no data sources.
const EMPTY_GAP: f32 = SP_3;
/// The glyph beside *New data source…*, and the gap to its label.
const ITEM_ICON: f32 = 12.;
const ITEM_GAP: f32 = SP_3;

/// The **LOCATION** segmented control: Local · Remote · Internal.
///
/// **Answers, not technologies**, where the canvas says *Local disk* / *Object store*. "Object
/// store" is the implementation's word — the thing DataFusion registers and this app calls a
/// data source — and a reader who has never met it cannot tell which of the answers is theirs.
/// Answering the row's own question in one word each also makes them read as the choice they are;
/// everything that follows (TYPE, CONNECTION, a bucket-relative path, a column list) explains
/// itself from there.
///
/// **Internal is the third answer and not a second surface** (IT-01). Creating a table Strata
/// stores is the same question this window already asks — what is it called, and what is in it —
/// with a different answer to *where*, so it belongs in this control rather than behind a menu
/// on the catalog's `+`. What changes below it is which sections have anything to ask: an internal
/// table has no store, no paths, no format options and no partitions, and declares its columns
/// instead.
///
/// Text segments, like the data source editor's PROVIDER pill next door, rather than the canvas's
/// glyph-plus-label: the two windows' pills should read as one control, and the labels here say
/// the whole thing on their own.
#[derive(PartialEq)]
pub struct Location;

impl Component for Location {
    fn render(&self) -> impl IntoElement {
        let ctx = use_consume::<ConfigureCtx>();
        let station = use_radio_station::<ProjectState, ProjChan>();
        let (location, editing) = (
            ctx.draft.read().location,
            ctx.target.read().editing().is_some(),
        );

        let segment = |label: &'static str, wants: Where| {
            ToggleSegment::text(label)
                .selected(location == wants)
                .enabled(wants != Where::Internal || !editing)
                .on_press(move |_| {
                    let sources = sources_at_press(station);
                    ctx.edit(move |draft| draft.set_location(wants, &sources));
                })
        };

        Row::new("LOCATION").child(
            SegmentedToggle::new()
                .form()
                .child(segment("Local", Where::Local))
                .child(segment("Remote", Where::Remote))
                .child(segment("Internal", Where::Internal)),
        )
    }
}

/// **TYPE** and **CONNECTION** — which object store, once LOCATION says there is one.
///
/// Always mounted, drawing nothing on the local disk: a section that comes and goes as a *child*
/// keeps this form's row list the same length either way, which is what the differ needs (see
/// `views::hive`, which is shaped the same way for the same reason).
#[derive(PartialEq)]
pub struct ObjectStore;

impl Component for ObjectStore {
    fn render(&self) -> impl IntoElement {
        let ctx = use_consume::<ConfigureCtx>();
        let remote = ctx.draft.read().remote();

        rect().width(Size::fill()).maybe_child(remote.then(|| {
            rect()
                .width(Size::fill())
                .horizontal()
                .content(Content::Flex)
                .cross_align(Alignment::Start)
                .spacing(COLUMN_GAP)
                .child(rect().child(ProviderFilter))
                .child(rect().width(Size::flex(1.)).child(SourcePicker))
        }))
    }
}

/// The project's sources, cloned out of the store — **subscribed**, so a data source added
/// while this window is open appears in the picker without a reopen. For the one component that
/// renders the list.
///
/// The defs themselves rather than a projection of them: the two questions asked here (which
/// data sources does this provider serve, and which provider serves this URL) are both answered
/// from `SourceDef`, and a second shape would be a second thing to keep true.
fn use_sources() -> Vec<SourceDef> {
    use_radio::<ProjectState, ProjChan>(ProjChan::Sources)
        .read()
        .sources
        .iter()
        .map(|row| row.def.clone())
        .collect()
}

/// The same list, read **at the press** — for the two pills, which do not render a data source and
/// only need one to hand to the draft.
///
/// A station rather than [`use_data sources`]: a subscribed read would clone every def (each with
/// its client-option map) on every render of a section that re-renders per keystroke, to serve a
/// handler that runs on a click. It would also wake these two on a channel neither of them draws
/// anything from.
fn sources_at_press(station: RadioStation<ProjectState, ProjChan>) -> Vec<SourceDef> {
    station
        .peek()
        .sources
        .iter()
        .map(|row| row.def.clone())
        .collect()
}

/// **TYPE** — the provider whose data sources the picker offers. Its labels are
/// the registrants' own labels, the same table the pane's row badge and the data source editor's own
/// picker read.
///
/// **Store-mode kinds only, not every registrant.** This row asks which data source a set of *files*
/// is read through, and a database source registers no object store — offering one would put
/// "read these parquet files through my Postgres data source" in the pill, and leave the
/// CONNECTION picker under it empty with nothing saying why. `ALL` is a fixed-length const, so
/// nothing about a new provider arm makes this loop notice on its own; the narrower list is what
/// says out loud which question is being asked.
#[derive(PartialEq)]
struct ProviderFilter;

impl Component for ProviderFilter {
    fn render(&self) -> impl IntoElement {
        let ctx = use_consume::<ConfigureCtx>();
        let station = use_radio_station::<ProjectState, ProjChan>();
        let engine = use_consume::<EngineCtx>();
        let current = ctx.draft.read().kind.clone();

        let mut pill = SegmentedToggle::new().form();
        for info in stores(&engine) {
            pill = pill.child(
                ToggleSegment::text(info.label)
                    .selected(info.kind == current)
                    .on_press(move |_| {
                        let sources = sources_at_press(station);
                        ctx.edit(move |draft| draft.set_provider(info.kind, &sources));
                    }),
            );
        }
        Row::new("TYPE").child(pill)
    }
}

/// **CONNECTION** — which of this provider's object stores the table reads through, plus the way
/// to add one without leaving the window.
#[derive(PartialEq)]
struct SourcePicker;

impl Component for SourcePicker {
    fn render(&self) -> impl IntoElement {
        let form = form_theme();
        let ctx = use_consume::<ConfigureCtx>();
        let mut request = use_consume::<SourceRequest>();
        let sources = use_sources();
        let engine = use_consume::<EngineCtx>();
        let (kind, chosen) = {
            let draft = ctx.draft.read();
            (draft.kind.clone(), draft.source.clone())
        };
        let label = stores(&engine)
            .into_iter()
            .find(|info| info.kind == kind)
            .map(|info| info.label.to_string())
            .unwrap_or_default();
        let offered = sources_for(&sources, &kind);

        let mut options: Vec<Element> = offered
            .iter()
            .map(|url| {
                let url = url.clone();
                MenuItem::new()
                    .selected(Some(&url) == chosen.as_ref())
                    .on_press({
                        let url = url.clone();
                        move |_| {
                            let url = url.clone();
                            ctx.edit(move |draft| draft.source = Some(url));
                        }
                    })
                    .child(MonoValue::new(url.clone()))
                    .into()
            })
            .collect();
        options.push(
            MenuItem::new()
                .on_press(move |_| request.set(Some(SourceTarget::New)))
                .child(
                    rect()
                        .horizontal()
                        .cross_align(Alignment::Center)
                        .spacing(ITEM_GAP)
                        .child(Icon::new(IconName::Plus).size(ITEM_ICON))
                        .child(Prose::new("New data source…")),
                )
                .into(),
        );

        Row::new("CONNECTION")
            .required()
            .child(
                rect()
                    .width(Size::fill())
                    .height(Size::px(FIELD_HEIGHT))
                    .child(
                        Select::new()
                            .selected_item(
                                MonoValue::new(match &chosen {
                                    Some(url) => url.clone(),
                                    None => "Select a data source…".to_string(),
                                })
                                .text_overflow(TextOverflow::Ellipsis),
                            )
                            .children(options),
                    ),
            )
            .maybe_child(offered.is_empty().then(|| {
                rect()
                    .width(Size::fill())
                    .padding(Gaps::new(EMPTY_GAP, 0., 0., 0.))
                    .child(
                        Caption::new(match label.is_empty() {
                            true => "No data sources yet. Add one to continue.".to_string(),
                            false => format!("No {label} data sources yet. Add one to continue."),
                        })
                        .color(form.hint_color)
                        .width(Size::fill())
                        .wrap(),
                    )
            }))
    }
}

/// The kinds a table's files can be read through — every **Store**-mode registrant, which is the
/// narrower question the retired `OBJECT_STORES` constant answered.
///
/// Asked of the registry rather than listed here, so a store an embedder registers is offered on
/// the same terms as a shipped one: a table reads *files*, and a source that answers with a
/// catalog has none to read.
fn stores(engine: &EngineCtx) -> Vec<SourceInfo> {
    engine
        .sources()
        .registrants()
        .into_iter()
        .filter(|info| info.mode == SourceMode::Store)
        .collect()
}
