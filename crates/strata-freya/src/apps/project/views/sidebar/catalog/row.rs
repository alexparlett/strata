//! The tree's **row shell**, and the three things every row kind shares: the status slot's
//! hold-back, the ⋮ trigger, and the fold plan that decides which of a row's optional marks
//! survive at width.
//!
//! The shell is the fork's [`TreeItem`] — depth guides, the disclosure slot, hover / selected
//! dress and, since it is now every row in this pane, the `Link` role and keyboard focus a
//! `SideBarItem` row carried before. What this adds is the app's own chevron in the arrow slot,
//! the catalog theme's dress, and the secondary press: a built-in exposes `on_press` only, and a
//! wrapper is where a right-click belongs anyway, so no row kind invents its own.
//!
//! **That wrapper is unconditional**, even on the six kinds with no menu to open. The tree is a
//! virtualized window whose slots are reused, and a root that changed shape with the row would put
//! two element kinds at one slot: the differ cannot pair those, so scrolling across the boundary
//! would rebuild the whole `TreeItem` and drop its hover and keyboard focus with it.
//!
//! [`use_status`] and [`fold_plan`] live here rather than beside one row kind because a workspace
//! entry, a database connection and an object store all wear the same status slot and the same
//! ranked fold.
//!
//! **A row takes no reconciliation key**, which is why it offers no way to set one: the tree is a
//! virtualized window whose children shift by one on every scroll step, so identity keys would make
//! each step a list of moves. The window reconciles positionally instead (see `view::TreeRow`).

use std::borrow::Cow;

use freya::components::{CircularLoader, Disclosure, TreeItem, TreeThemePartial};
use freya::prelude::*;

use super::CatalogTheme;
use crate::components::icon::{Icon, IconName};
use crate::components::metrics::{
    use_progress_hold, ROW_ACTION, R_1, SP_2, SP_3, SP_4, STATUS_DOT,
};
use crate::components::typography::scale;
use strata_core::util::clip;

/// Every row is one height, because a tree indents rather than sizing its levels apart.
pub const ROW_HEIGHT: f32 = 26.;
/// One level of indentation — the guide spacing too. The scale's `SP_4`, named rather than
/// restated: a database's relations sit four levels down and the pane is 288px wide, so this is
/// the widest step the tree can afford.
pub const INDENT: f32 = SP_4;

/// What the spinner says on hover (and to a screen reader).
const LOADING: &str = "Loading…";

/// How much of a refusal a tooltip will show, and where the rest of it is.
///
/// **The limit lives here because the limit is this surface's.** It used to live in the engine
/// (`register_error`'s 240-character cut), which meant a constraint belonging to a sidebar
/// overlay was applied to the string *every* consumer read — so the Problems drawer, which wraps,
/// and its copy button, which exists to put the message on the clipboard, both handed back a
/// sentence cut mid-clause. The engine's message is whole again; a tooltip that cannot hold it
/// says so and names the surface that can.
const TIP_CHARS: usize = 160;
const TIP_MORE: &str = "\nSee Problems for the full message.";

/// A status glyph wearing its message as a tooltip. Dropped below, like the rest of the app's
/// overlays, so it can't cover the row above it in a dense list.
pub fn tip(message: impl Into<Cow<'static, str>>) -> TooltipContainer {
    TooltipContainer::new(Tooltip::new_text(message)).position(AttachedPosition::Bottom)
}

/// The row's **⋮ trigger** — the menu's discoverable half: the right-click opens the same one,
/// but nothing on screen says so.
///
/// `stop_propagation` because it sits *inside* a pressable row — without it, opening the menu
/// would also toggle the row.
///
/// The menu is built lazily, per press: its labels are a snapshot of the moment it opens (see
/// `menu.rs`), so building one per render would be both wasteful and wrong.
pub fn actions_button(menu: impl Fn() -> Menu + 'static) -> impl IntoElement {
    TooltipContainer::new(Tooltip::new_text("Actions"))
        .position(AttachedPosition::Bottom)
        .child(
            Button::new()
                .flat()
                .width(Size::px(ROW_ACTION))
                .height(Size::px(ROW_ACTION))
                .on_press(move |e: Event<PressEventData>| {
                    e.stop_propagation();
                    ContextMenu::open(menu());
                })
                .child(Icon::new(IconName::Dots).size(15.)),
        )
}

/// One row of the tree.
#[derive(PartialEq)]
pub struct Row {
    depth: usize,
    disclosure: Disclosure,
    selected: bool,
    on_press: Option<EventHandler<Event<PressEventData>>>,
    on_toggle: Option<EventHandler<Event<PressEventData>>>,
    on_context_menu: Option<EventHandler<Event<PressEventData>>>,
    /// The run whose width the fold plan is measured against — everything but the ⋮.
    on_sized: Option<EventHandler<Event<SizedEventData>>>,
    /// What sits inside the measured run.
    children: Vec<Element>,
    /// What sits outside it, at the row's trailing edge — the ⋮, on the rows that have one.
    trailing: Option<Element>,
    theme: CatalogTheme,
}

impl Row {
    pub fn new(depth: usize, theme: CatalogTheme) -> Self {
        Self {
            depth,
            disclosure: Disclosure::Leaf,
            selected: false,
            on_press: None,
            on_toggle: None,
            on_context_menu: None,
            on_sized: None,
            children: Vec::new(),
            trailing: None,
            theme,
        }
    }

    /// Whether this row opens, and whether it is open.
    pub fn disclosure(mut self, disclosure: Disclosure) -> Self {
        self.disclosure = disclosure;
        self
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn on_press(mut self, handler: impl Into<EventHandler<Event<PressEventData>>>) -> Self {
        self.on_press = Some(handler.into());
        self
    }

    /// The disclosure arrow's own press. It does not also fire `on_press`.
    pub fn on_toggle(mut self, handler: impl Into<EventHandler<Event<PressEventData>>>) -> Self {
        self.on_toggle = Some(handler.into());
        self
    }

    pub fn on_context_menu(
        mut self,
        handler: impl Into<EventHandler<Event<PressEventData>>>,
    ) -> Self {
        self.on_context_menu = Some(handler.into());
        self
    }

    /// Measure the run the fold plan applies to — see [`fold_plan`].
    pub fn on_sized(mut self, handler: impl Into<EventHandler<Event<SizedEventData>>>) -> Self {
        self.on_sized = Some(handler.into());
        self
    }

    /// The ⋮, outside the measured run so folding never takes it.
    pub fn trailing(mut self, trailing: impl IntoElement) -> Self {
        self.trailing = Some(trailing.into_element());
        self
    }
}

impl ChildrenExt for Row {
    fn get_children(&mut self) -> &mut Vec<Element> {
        &mut self.children
    }
}

impl Component for Row {
    fn render(&self) -> impl IntoElement {
        let dress = TreeThemePartial::default()
            .arrow_fill(self.theme.chevron_color)
            .item_background(Color::TRANSPARENT)
            .hover_item_background(self.theme.row_hover_fill)
            .selected_item_background(self.theme.row_selected_fill)
            .selected_item_color(self.theme.name_color)
            .guide_fill(self.theme.rail_fill)
            .item_padding(Gaps::new(0., SP_2, 0., SP_2))
            .corner_radius(CornerRadius::new_all(R_1));

        let content = rect()
            .width(Size::flex(1.))
            .height(Size::fill())
            .horizontal()
            .content(Content::Flex)
            .cross_align(Alignment::Center)
            .overflow(Overflow::Clip)
            .spacing(SP_3)
            .map(self.on_sized.clone(), |el, handler| {
                el.on_sized(move |e: Event<SizedEventData>| handler.call(e))
            })
            .children(self.children.clone());

        let item = TreeItem::new()
            .depth(self.depth)
            .disclosure(self.disclosure)
            .selected(self.selected)
            .width(Size::fill())
            .theme(dress)
            .arrow(
                Icon::new(match self.disclosure {
                    Disclosure::Expanded => IconName::ChevronDown,
                    _ => IconName::ChevronRight,
                })
                .color(self.theme.chevron_color)
                .size(11.),
            )
            .map(self.on_press.clone(), |el, handler| {
                el.on_press(move |e| handler.call(e))
            })
            .map(self.on_toggle.clone(), |el, handler| {
                el.on_toggle(move |e| handler.call(e))
            })
            .child(content)
            .maybe_child(self.trailing.clone());

        rect()
            .width(Size::fill())
            .content(Content::Fit)
            .map(
                self.on_context_menu.clone(),
                |el, handler: EventHandler<Event<PressEventData>>| {
                    el.on_secondary_down(move |e: Event<PressEventData>| {
                        e.stop_propagation();
                        handler.call(e);
                    })
                },
            )
            .child(item)
    }
}

/// What the row's one trailing status column is saying. A type rather than a built element so
/// it can be compared, and because more than one row kind draws it.
#[derive(Clone, PartialEq)]
pub enum StatusMark {
    /// The registration has not answered and the hold has expired.
    Loading,
    /// The settled verdict — what is wrong with this row, in the words the store recorded.
    Problem(String),
}

impl StatusMark {
    /// The glyph, wearing its message as a tooltip *and* an a11y label, so the explanation is
    /// never mouse-only.
    pub fn glyph(&self, theme: &CatalogTheme) -> Element {
        match self {
            StatusMark::Loading => tip(LOADING)
                .child(CircularLoader::new().size(STATUS_DOT).a11y_alt(LOADING))
                .into_element(),
            StatusMark::Problem(reason) => {
                let shown = tip_text(reason);
                tip(shown.clone())
                    .child(
                        rect().a11y_alt(shown).child(
                            Icon::new(IconName::Warning)
                                .color(theme.warn_color)
                                .size(STATUS_DOT),
                        ),
                    )
                    .into_element()
            }
        }
    }
}

/// `reason` as much as this tooltip will show — whole when it fits, and otherwise clipped with
/// the pointer to where the rest is (see [`TIP_CHARS`]).
///
/// [`clip`](strata_core::util::clip) is the app's one clipping funnel, so this cannot disagree
/// with the grid or the value tree about where a clipped string stops.
fn tip_text(reason: &str) -> String {
    match clip(reason, TIP_CHARS) {
        Cow::Borrowed(whole) => whole.to_string(),
        Cow::Owned(cut) => format!("{cut}{TIP_MORE}"),
    }
}

/// What the row's status slot says, given that `owner`'s registration is `waiting` and last settled
/// on `problem` — **one hook, so the tree's row kinds cannot drift**.
///
/// The hold-back itself is [`use_progress_hold`]'s; what this adds is the other half of the rule:
/// across that gap the slot **holds its last settled verdict**, so a ↻ over a row that stays broken
/// does not blink its triangle off and back on.
///
/// **The held verdict is tagged with whose it is**, and `owner` is that tag — the row's node path,
/// or `None` on the row kinds that have no status at all. The tree is virtualized, so a scope is a
/// *slot*: scrolling hands this one a different row, and a verdict kept without saying whose it was
/// would be shown against a row it was never about, indefinitely, since a waiting row never settles
/// it. `None` is what stops the ten statusless kinds from *erasing* the tag as they pass through:
/// they are not waiting, so an untagged write would look like a settled "nothing is wrong".
///
/// Two limits the tag does not lift, both of them consequences of one cell per slot. Two status
/// rows sharing a slot overwrite each other's held verdict, so the second of them to be waiting
/// shows no triangle until its registration answers. And the hold-back *timer* is the slot's, keyed
/// on `waiting` alone: a statusless row passing through cancels it, so a row that has been loading
/// for a long time serves the hold again on the way back in rather than spinning at once.
pub fn use_status(
    owner: Option<&str>,
    waiting: bool,
    problem: Option<String>,
) -> Option<StatusMark> {
    let spinning = use_progress_hold(waiting);

    let held = use_state(|| None::<(String, Option<String>)>);
    use_side_effect_with_deps(
        &(owner.map(str::to_string), waiting, problem.clone()),
        move |(owner, waiting, problem)| {
            let Some(owner) = owner else {
                return;
            };
            if !*waiting {
                let mut held = held;
                held.set_if_modified(Some((owner.clone(), problem.clone())));
            }
        },
    );

    let settled = match (owner, waiting) {
        (Some(owner), true) => held
            .read()
            .as_ref()
            .filter(|(whose, _)| whose == owner)
            .and_then(|(_, verdict)| verdict.clone()),
        _ => problem,
    };

    match (spinning, settled) {
        (true, _) => Some(StatusMark::Loading),
        (false, Some(reason)) => Some(StatusMark::Problem(reason)),
        (false, None) => None,
    }
}

/// What each optional item costs the measured run: its own width plus the one gap it brings
/// with it.
const BADGE_SLOT: f32 = 63. + SP_3;
/// The **mark** slot's width on a row whose mark is an entity icon — [`fold_plan`]'s `mark`
/// argument for a workspace entry. A row whose mark is text passes that text's own width, and a
/// row with no mark passes `0.`: the slot is a budget, and budgeting a glyph's width for a
/// variable-width label ellipsizes the name while the plan believes nothing needs to fold.
pub const ICON_SLOT: f32 = 14. + SP_3;
const STATUS_SLOT: f32 = STATUS_DOT + SP_3;
/// Advance width of the mono face, as a fraction of its point size.
///
/// The name is [`MonoValue`](crate::components::typography::MonoValue) — a **monospace** role —
/// so its natural width is exactly its character count times one advance, and that is the whole
/// reason this fold can be arithmetic rather than a measurement: Freya lays the name out at the
/// width it is *given* (it is `Size::flex(1.)`), so nothing downstream can report what it would
/// have wanted. 0.6 em is JetBrains Mono's advance and the standard one for the genre; the point
/// size comes from the theme's own scale rather than this file, so retuning the type scale
/// retunes the fold with it.
const MONO_ADVANCE: f32 = 0.6;

/// One character of the row's mono face, in pixels.
///
/// **A hook**, through [`scale`], which is why it is resolved once by the row component rather than
/// by the row kinds that fold: a helper that reads the theme cannot be reached from a `match` arm
/// without making the scope's hook count a function of which row it drew (see `view::TreeRow`).
pub fn mono_advance() -> f32 {
    scale().data_value.size * MONO_ADVANCE
}

/// The natural width of `name` in the row's mono face, at [`mono_advance`]'s pitch.
pub fn name_width(name: &str, advance: f32) -> f32 {
    name.chars().count() as f32 * advance
}

/// Which of the row's optional items survive at a given width.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Folds {
    pub badge: bool,
    /// The row's **mark** — an entity icon on a workspace entry, the catalog name on a database.
    /// Named for the slot rather than for the first thing to occupy it, because its width is the
    /// caller's ([`fold_plan`]'s `mark`).
    pub mark: bool,
    pub status: bool,
}

/// The row's fold plan — `components::toolbar`'s policy, ranked for this row.
///
/// That component's row is `[ leading run (flexible, ellipsizes) ][ items (fold lowest-rank
/// first) ][ pinned tail ]`, and the space it offers the items is the row minus the **leading
/// run's floor**. So items fold while the leading run is still whole, and the leading run
/// ellipsizes only once they have all gone. A tree row is that shape: the name is the leading
/// run, its floor is its own natural width, and everything else folds — **in this order, least
/// informative first**:
///
/// 1. the badge — a marker the mark's own tint repeats, and the only item that is pure
///    reinforcement;
/// 2. the mark — decoration once the row's group already says what kind it is;
/// 3. the status glyph — information, not decoration, so it outranks both of the above and goes
///    last of the three.
///
/// `run_width` is the row **minus its indent, its chevron and its ⋮**, because those are what the
/// row is *used* by and are laid out outside the measured run: a row too narrow to show them is
/// worse than one showing nothing else. `mark` is the mark's own width plus its gap — [`ICON_SLOT`]
/// for a glyph, [`name_width`] plus a gap for a label, `0.` for a row that draws none — because a
/// budget stated for the wrong item folds the ranks above it to pay for room the row never needed.
/// The name never sets a floor of its own: below the point where everything has folded it goes on
/// ellipsizing, because a chrome row folds rather than spills.
pub fn fold_plan(run_width: f32, name_width: f32, badge: bool, mark: f32) -> Folds {
    let needs = |f: Folds| {
        name_width
            + if f.badge { BADGE_SLOT } else { 0. }
            + if f.mark { mark } else { 0. }
            + if f.status { STATUS_SLOT } else { 0. }
    };
    let mut folds = Folds {
        badge,
        mark: true,
        status: true,
    };
    if needs(folds) <= run_width {
        return folds;
    }
    folds.badge = false;
    if needs(folds) <= run_width {
        return folds;
    }
    folds.mark = false;
    if needs(folds) <= run_width {
        return folds;
    }
    folds.status = false;
    folds
}
