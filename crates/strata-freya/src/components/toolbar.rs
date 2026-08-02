//! The app's **chrome row**: a header or toolbar that degrades instead of spilling when its panel
//! runs out of room (P5-06).
//!
//! A row is `[ leading run (flexible, ellipsizes) ][ items (fold tail-first) ][ pinned tail ]`.
//! As the row narrows the leading run gives up its width first; then items fold, one at a time
//! from the tail, into a single `⋯` [`Menu`]; then the ones that cannot be a menu row are dropped
//! outright. The pinned tail never folds, because it holds the control that gets the user *out* of
//! the squeeze (a panel's collapse ×, a pane's Prev/Next).
//!
//! This is IntelliJ's `AUTO_LAYOUT_POLICY` — *"if there is not enough space for items on the
//! toolbar, they are hidden under a chevron"* — and it is one policy applied to every row rather
//! than a breakpoint per surface. That matters here because the shell has **no usability
//! minimums** (see `apps::project::views::shell`): there is no row for which "it always fits" can
//! be argued, so every row needs the same answer.
//!
//! **An item is declared once.** A [`ToolbarItem`] knows how wide it is, how it draws inline and
//! how it draws as a menu row, so the overflow menu is a second *rendering* of the row rather
//! than a second copy of it — which is the drift this component exists to prevent. The fold point
//! is arithmetic over those widths ([`fold_plan`]), not a constant: adding a button moves it
//! automatically.
//!
//! The measured width lives in local state, never in the session store. A fold verdict is derived,
//! transient and per-mount — the same reason the theme is deliberately not stored (AGENTS.md §2) —
//! and `Chan::LayoutSize` has no subscribers by design, so it could not drive a re-render anyway.

use std::cmp::Reverse;

use freya::prelude::*;

use crate::components::divider::Divider;
use crate::components::icon::{Icon, IconName};
use crate::components::tool_button::TOOL_SIZE;
use crate::components::typography::Prose;
use crate::keymap::KeyHint;
use strata_core::config::Command;

/// A separator's painted width. It is a 1px rule, but it carries the row's spacing on both sides
/// like any other item, so the arithmetic treats it as an ordinary member.
const SEPARATOR_W: f32 = 1.;

/// A panel header's control box, against a toolbar's [`TOOL_SIZE`].
const HEADER_CONTROL_SIZE: f32 = 24.;

/// What a [`Toolbar`] shows in one slot.
#[derive(PartialEq, Clone)]
pub enum ToolbarItem {
    /// An icon action: a 28×28 button in the row, a labelled row in the overflow menu. The one
    /// kind the toolbar can render both ways itself, which is why it is the default shape.
    Action(ToolbarAction),
    /// A rule between two groups. Shown inline; dropped rather than folded, since a separator
    /// inside a menu that has lost one of the groups it separated is noise.
    Separator,
    /// Something the toolbar cannot rebuild as a menu row: a segmented toggle, a `Select`, a
    /// popover trigger that has to stay the anchor its panel measures. It states its width and
    /// supplies its own folded form, or `None` to be dropped once it no longer fits.
    Custom {
        width: f32,
        inline: Element,
        folded: Option<Element>,
    },
    /// An item that outranks the plain tail-first order: it folds only once everything of a lower
    /// rank already has. Wraps any of the above.
    Ranked(u8, Box<ToolbarItem>),
}

impl ToolbarItem {
    /// Fold this item only after everything of a lower rank has gone. Default rank is `0`, so a
    /// row that states nothing folds purely from the tail.
    pub fn rank(self, rank: u8) -> Self {
        match self {
            Self::Ranked(_, inner) => Self::Ranked(rank, inner),
            other => Self::Ranked(rank, Box::new(other)),
        }
    }

    fn inner(&self) -> &Self {
        match self {
            Self::Ranked(_, inner) => inner.inner(),
            other => other,
        }
    }

    fn fold_rank(&self) -> u8 {
        match self {
            Self::Ranked(rank, _) => *rank,
            _ => 0,
        }
    }

    /// How much room this item needs in a row whose controls are `control` wide.
    fn width(&self, control: f32) -> f32 {
        match self.inner() {
            Self::Action(_) => control,
            Self::Separator => SEPARATOR_W,
            Self::Custom { width, .. } => *width,
            Self::Ranked(..) => unreachable!("inner() unwraps every rank"),
        }
    }

    /// Whether this item survives folding. A separator does not, and a `Custom` says so itself.
    fn folds(&self) -> bool {
        match self.inner() {
            Self::Action(_) => true,
            Self::Separator => false,
            Self::Custom { folded, .. } => folded.is_some(),
            Self::Ranked(..) => unreachable!("inner() unwraps every rank"),
        }
    }
}

/// An icon action, in the one declaration both of its renderings come from.
#[derive(PartialEq, Clone)]
pub struct ToolbarAction {
    icon: IconName,
    /// Names the action in the tooltip and, once folded, in the menu row. Required: an icon-only
    /// button has no accessible name of its own.
    label: String,
    hint: Option<Command>,
    enabled: bool,
    danger: bool,
    active: bool,
    on_press: Option<EventHandler<Event<PressEventData>>>,
}

impl ToolbarAction {
    pub fn new(icon: IconName, label: impl Into<String>) -> Self {
        Self {
            icon,
            label: label.into(),
            hint: None,
            enabled: true,
            danger: false,
            active: false,
            on_press: None,
        }
    }

    /// The command whose live chord this action wears — in the tooltip inline, and as the menu
    /// row's right-aligned hint once folded.
    pub fn hint(mut self, command: Command) -> Self {
        self.hint = Some(command);
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Destructive dress on hover (the results pane's Clear). A tone, not a different control.
    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }

    /// The action's own state is **on** — it holds something open (the results pane's Find, while
    /// its popover is showing). Wears the comp's accent dress inline.
    ///
    /// Inline only: `MenuButton` carries no selected state (that is `MenuItem`'s), and a folded
    /// action whose panel is open needs no marker anyway — the panel itself is on screen.
    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    pub fn on_press(mut self, f: impl Into<EventHandler<Event<PressEventData>>>) -> Self {
        self.on_press = Some(f.into());
        self
    }

    /// Fold this action only after everything of a lower rank has gone — the `ToolbarItem` builder
    /// of the same name, reachable without spelling the conversion out at the call site.
    pub fn rank(self, rank: u8) -> ToolbarItem {
        ToolbarItem::from(self).rank(rank)
    }
}

impl From<ToolbarAction> for ToolbarItem {
    fn from(action: ToolbarAction) -> Self {
        Self::Action(action)
    }
}

/// What `items` cost in a row `spacing` wide-gapped, counting only those flagged in `keep`, plus
/// the `⋯` trigger when anything has folded.
fn run_width(
    items: &[ToolbarItem],
    keep: &[bool],
    spacing: f32,
    control: f32,
    overflowing: bool,
) -> f32 {
    let mut total = 0.;
    let mut slots = 0usize;
    for (item, keep) in items.iter().zip(keep) {
        if *keep {
            total += item.width(control);
            slots += 1;
        }
    }
    // The trigger is a control like any other in this row, so it costs what they cost.
    if overflowing {
        total += control;
        slots += 1;
    }
    total + spacing * slots.saturating_sub(1) as f32
}

/// Which of `items` stay in the row, given the width available to them.
///
/// Folding order is **lowest [`rank`](ToolbarItem::rank) first, and from the tail within a rank**.
/// With every rank left at its default that is plain tail-first, which is Swing's rule and what
/// the RustRover screenshots show: the far end of a toolbar is where the least-used actions sit.
/// A surface raises the rank of the controls that have to outlive the rest — the pager's Prev and
/// Next, which are the whole point of a pager.
///
/// `available` is what is left for the items after the row's padding, its pinned tail and the
/// leading run's own floor. The `⋯` trigger only charges for itself once something has actually
/// folded, so a row that fits exactly does not fold an item to make room for a menu holding it.
fn fold_plan(items: &[ToolbarItem], available: f32, spacing: f32, control: f32) -> Vec<bool> {
    let mut keep = vec![true; items.len()];
    if run_width(items, &keep, spacing, control, false) <= available {
        return keep;
    }

    // Something has to go, so the trigger is part of the bill from here on.
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by_key(|&i| (items[i].fold_rank(), Reverse(i)));

    for i in order {
        keep[i] = false;
        if run_width(items, &keep, spacing, control, true) <= available {
            break;
        }
    }
    keep
}

/// A chrome row that folds rather than spills. See the module docs.
#[derive(PartialEq)]
pub struct Toolbar {
    leading: Option<Element>,
    leading_min: f32,
    items: Vec<ToolbarItem>,
    pinned: Option<Element>,
    pinned_width: f32,
    spacing: f32,
    padding: f32,
    height: f32,
    control: f32,
    flat: bool,
    background: Option<Color>,
    overflow_label: &'static str,
}

impl Default for Toolbar {
    fn default() -> Self {
        Self::new()
    }
}

impl Toolbar {
    pub fn new() -> Self {
        Self {
            leading: None,
            leading_min: 0.,
            items: Vec::new(),
            pinned: None,
            pinned_width: 0.,
            spacing: 8.,
            padding: 10.,
            height: 38.,
            control: TOOL_SIZE,
            flat: false,
            background: None,
            overflow_label: "More actions",
        }
    }

    /// A **panel header**'s dress rather than a toolbar's: 24px flat controls instead of 28px
    /// outlined ones.
    ///
    /// The distinction is the row's, not each action's, because the whole point is that the `⋯`
    /// trigger matches the cluster it joins — a 28px outlined chevron dropped into a header's
    /// 24px flat controls reads as a different kind of thing, which is exactly the drift this
    /// component exists to stop.
    pub fn header(mut self) -> Self {
        self.control = HEADER_CONTROL_SIZE;
        self.flat = true;
        self
    }

    /// The run at the head of the row: a title, a filter box, a status readout, or a control that
    /// has to stay put because it is the surface's whole point (the editor's Run).
    ///
    /// **Whether the items pack left or sit at the far end is this element's choice**, not a
    /// separate setting: pass a `Size::flex(1.)` node and it takes the slack, pushing the items
    /// right; pass a hugging one and everything packs to the left behind it.
    ///
    /// `min` is what the run is charged in the fold arithmetic — its own width when it cannot
    /// give (a fixed control), or the smallest it is worth showing at when it can ellipsize. Pass
    /// `0.` for a run that may vanish entirely.
    pub fn leading(mut self, leading: impl IntoElement, min: f32) -> Self {
        self.leading = Some(leading.into_element());
        self.leading_min = min;
        self
    }

    /// The tail that never folds, and how wide it is. This is the control that gets the user out
    /// of a squeeze, so it outranks every action in the row.
    pub fn pinned(mut self, pinned: impl IntoElement, width: f32) -> Self {
        self.pinned = Some(pinned.into_element());
        self.pinned_width = width;
        self
    }

    pub fn item(mut self, item: impl Into<ToolbarItem>) -> Self {
        self.items.push(item.into());
        self
    }

    pub fn separator(mut self) -> Self {
        self.items.push(ToolbarItem::Separator);
        self
    }

    pub fn spacing(mut self, spacing: f32) -> Self {
        self.spacing = spacing;
        self
    }

    pub fn padding(mut self, padding: f32) -> Self {
        self.padding = padding;
        self
    }

    pub fn height(mut self, height: f32) -> Self {
        self.height = height;
        self
    }

    pub fn background(mut self, background: Color) -> Self {
        self.background = Some(background);
        self
    }

    /// The `⋯` trigger's tooltip. Names what folded, so "More actions" is only the default.
    pub fn overflow_label(mut self, label: &'static str) -> Self {
        self.overflow_label = label;
        self
    }
}

impl Component for Toolbar {
    fn render(&self) -> impl IntoElement {
        // The row's measured width. Local, per-mount and derived -- see the module docs on why it
        // is deliberately nowhere near the session store.
        let mut measured = use_state(|| f32::INFINITY);
        let theme = use_theme();
        let (border, danger, accent) = {
            let t = theme.read();
            (t.colors().border, t.colors().error, t.colors().primary)
        };

        // What the items may spend: the row, less its padding, its pinned tail and the floor the
        // leading run keeps.
        //
        // The head's gap is charged unconditionally, because the row always *has* a head — the
        // caller's leading run, or the flex spacer that stands in for it below. Charging it only
        // for a stated `leading` left a spacer-headed row one gap short of what it lays out.
        let tail_gap = if self.pinned.is_some() {
            self.spacing
        } else {
            0.
        };
        let available = *measured.read()
            - self.padding * 2.
            - self.leading_min
            - self.pinned_width
            - tail_gap
            - self.spacing;
        let keep = fold_plan(&self.items, available, self.spacing, self.control);
        let folded: Vec<ToolbarItem> = self
            .items
            .iter()
            .zip(&keep)
            .filter(|(item, keep)| !**keep && item.folds())
            .map(|(item, _)| item.inner().clone())
            .collect();

        // Kept items render in their declared order, whatever order they would have folded in.
        let inline = self
            .items
            .iter()
            .zip(&keep)
            .filter(|(_, keep)| **keep)
            .map(|(item, _)| match item.inner() {
                ToolbarItem::Action(action) => {
                    action_button(action, danger, accent, self.control, self.flat).into_element()
                }
                ToolbarItem::Separator => Divider::vertical()
                    .length(Size::px(18.))
                    .color(border)
                    .into_element(),
                ToolbarItem::Custom { inline, .. } => inline.clone(),
                ToolbarItem::Ranked(..) => unreachable!("inner() unwraps every rank"),
            });

        rect()
            .width(Size::fill())
            .height(Size::px(self.height))
            .horizontal()
            // Flex, so the leading run is what absorbs the slack rather than the row hugging its
            // content and pushing the tail off the panel.
            .content(Content::Flex)
            .cross_align(Alignment::Center)
            .spacing(self.spacing)
            .padding((0., self.padding))
            .maybe(self.background.is_some(), |el| {
                el.background(self.background.unwrap_or(Color::TRANSPARENT))
            })
            .on_sized(move |e: Event<SizedEventData>| {
                measured.set_if_modified(e.area.width());
            })
            .maybe_child(self.leading.clone())
            // With no leading run the items still have to be pushed to the tail, or a flex row
            // spreads them across the whole width.
            .maybe(self.leading.is_none(), |el| {
                el.child(rect().width(Size::flex(1.)))
            })
            .children(inline.collect::<Vec<_>>())
            .maybe_child((!folded.is_empty()).then(|| {
                OverflowMenu {
                    items: folded,
                    label: self.overflow_label,
                    control: self.control,
                    flat: self.flat,
                }
                .into_element()
            }))
            .maybe_child(self.pinned.clone())
    }
}

/// One action, drawn in the row: the app's 28×28 outline icon button wearing its label as a
/// tooltip.
fn action_button(
    action: &ToolbarAction,
    danger: Color,
    accent: Color,
    control: f32,
    flat: bool,
) -> impl IntoElement {
    let button = Button::new()
        .maybe(flat, Button::flat)
        .height(Size::px(control))
        .width(Size::px(control))
        .enabled(action.enabled)
        .maybe(action.danger, |b| {
            b.hover_background(danger.with_a(38))
                .hover_border_fill(danger.with_a(115))
                .hover_color(danger)
        })
        // The comp's `on` dress: accent icon over accent-tinted fill and border (13% / 55%).
        .maybe(action.active, |b| {
            b.background(accent.with_a(33))
                .border_fill(accent.with_a(140))
                .color(accent)
        })
        .map(action.on_press.clone(), |b, f| b.on_press(f))
        .child(Icon::new(action.icon).size(15.));

    TooltipContainer::new(Tooltip::new_text(action.label.clone()))
        .position(AttachedPosition::Bottom)
        .child(button)
}

/// The `⋯` trigger and the menu of everything that no longer fits.
#[derive(PartialEq)]
struct OverflowMenu {
    items: Vec<ToolbarItem>,
    label: &'static str,
    control: f32,
    flat: bool,
}

impl Component for OverflowMenu {
    fn render(&self) -> impl IntoElement {
        let mut open = use_state(|| false);

        let rows = self.items.iter().filter_map(|item| match item.inner() {
            ToolbarItem::Action(action) => {
                let on_press = action.on_press.clone();
                Some(
                    MenuButton::new()
                        .enabled(action.enabled)
                        .on_press(move |e: Event<PressEventData>| {
                            // The toolbar closes its own menu: a press lands *inside* it, so
                            // nothing else would.
                            open.set(false);
                            if let Some(on_press) = &on_press {
                                on_press.call(e);
                            }
                        })
                        .child(menu_row(&action.label, action.hint))
                        .into_element(),
                )
            }
            ToolbarItem::Separator => None,
            ToolbarItem::Custom { folded, .. } => folded.clone(),
            ToolbarItem::Ranked(..) => unreachable!("inner() unwraps every rank"),
        });

        let menu = Menu::new()
            .min_width(Size::px(MENU_WIDTH))
            .on_close(move |_| open.set(false))
            .children(rows.collect::<Vec<_>>());

        Attached::new(
            TooltipContainer::new(Tooltip::new_text(self.label))
                .position(AttachedPosition::Bottom)
                .child(
                    Button::new()
                        .maybe(self.flat, Button::flat)
                        .height(Size::px(self.control))
                        .width(Size::px(self.control))
                        .on_press(move |_| open.toggle())
                        .child(Icon::new(IconName::Dots).size(15.)),
                ),
        )
        .bottom()
        .align_end()
        .maybe_child(open().then(|| menu))
    }
}

/// Room for a folded action's label plus its right-aligned chord. The `Attached` overlay's
/// available width is the trigger's 28px, so without a floor every row would squeeze and clip.
const MENU_WIDTH: f32 = 200.;

/// A folded action's row: its label, and the chord it answers to when it has one.
fn menu_row(label: &str, hint: Option<Command>) -> impl IntoElement {
    rect()
        .horizontal()
        .width(Size::fill())
        .cross_align(Alignment::Center)
        .main_align(Alignment::SpaceBetween)
        .spacing(16.)
        .child(Prose::new(label.to_string()))
        .maybe_child(hint.map(KeyHint))
}

#[cfg(test)]
mod tests {
    use freya::prelude::{rect, IntoElement};

    use super::{fold_plan, ToolbarAction, ToolbarItem, HEADER_CONTROL_SIZE, SEPARATOR_W};
    use crate::components::icon::IconName;
    use crate::components::tool_button::TOOL_SIZE;

    const SPACING: f32 = 8.;
    /// The default toolbar control, which is also what the `⋯` trigger costs.
    const OVERFLOW_W: f32 = TOOL_SIZE;

    fn actions(n: usize) -> Vec<ToolbarItem> {
        (0..n)
            .map(|_| ToolbarItem::Action(ToolbarAction::new(IconName::Save, "Save")))
            .collect()
    }

    /// The width `n` items occupy with nothing folded.
    fn run(n: usize) -> f32 {
        TOOL_SIZE * n as f32 + SPACING * (n as f32 - 1.)
    }

    /// How many items the plan keeps inline.
    fn kept(items: &[ToolbarItem], available: f32) -> usize {
        fold_plan(items, available, SPACING, TOOL_SIZE)
            .iter()
            .filter(|k| **k)
            .count()
    }

    /// The indices the plan keeps, in order.
    fn kept_indices(items: &[ToolbarItem], available: f32) -> Vec<usize> {
        fold_plan(items, available, SPACING, TOOL_SIZE)
            .into_iter()
            .enumerate()
            .filter(|(_, keep)| *keep)
            .map(|(i, _)| i)
            .collect()
    }

    #[test]
    fn everything_stays_inline_when_it_fits() {
        let items = actions(4);
        assert_eq!(kept(&items, run(4)), 4);
        assert_eq!(kept(&items, 10_000.), 4);
    }

    /// The `⋯` trigger is only charged for once something has actually folded, so a row that fits
    /// exactly does not fold an item to make room for a menu holding that one item.
    #[test]
    fn an_exact_fit_does_not_fold() {
        let items = actions(4);
        assert_eq!(kept(&items, run(4)), 4);
        assert!(
            kept(&items, run(4) - 1.) < 4,
            "one pixel short and it folds"
        );
    }

    /// Folding runs from the tail, and the row gives up one more item at a time as it narrows.
    #[test]
    fn items_fold_from_the_tail_as_the_row_narrows() {
        let items = actions(6);
        // What `n` inline items cost once the `⋯` trigger is on the bill beside them.
        let with_trigger = |n: usize| run(n) + SPACING + OVERFLOW_W;

        assert_eq!(kept_indices(&items, run(6)), vec![0, 1, 2, 3, 4, 5]);
        assert_eq!(kept_indices(&items, with_trigger(4)), vec![0, 1, 2, 3]);
        assert_eq!(kept_indices(&items, with_trigger(3)), vec![0, 1, 2]);
        assert_eq!(kept_indices(&items, with_trigger(1)), vec![0]);
    }

    /// The trigger is exactly as wide as the button it replaces, so folding a *single* item buys
    /// nothing: the row goes straight from all-inline to two folded. Asserted rather than left to
    /// be rediscovered, because it looks like an off-by-one until you do the arithmetic.
    #[test]
    fn folding_one_item_would_save_nothing_so_it_never_happens() {
        let items = actions(6);
        assert_eq!(
            kept(&items, run(6) - 1.),
            4,
            "five inline plus a trigger costs exactly what six inline costs"
        );
    }

    /// At a stub width nothing survives inline: the whole row is the `⋯`.
    #[test]
    fn a_stub_width_folds_everything() {
        let items = actions(6);
        assert_eq!(kept(&items, OVERFLOW_W), 0);
        assert_eq!(kept(&items, 0.), 0);
        assert_eq!(
            kept(&items, -50.),
            0,
            "a row squeezed past nothing still resolves rather than panicking"
        );
    }

    /// Adding a button moves the fold point on its own -- the arithmetic is over the item list,
    /// so no call site carries a breakpoint that could drift out of date.
    #[test]
    fn the_fold_point_follows_the_item_list() {
        let width = run(5);
        assert_eq!(kept(&actions(5), width), 5, "five fit exactly");
        assert!(
            kept(&actions(6), width) < 6,
            "a sixth button folds at the same width, with nothing restated"
        );
    }

    /// A separator is narrower than an action, and is dropped rather than folded -- it is only
    /// meaningful between two groups that are both still on show.
    #[test]
    fn a_separator_is_thin_and_never_folds() {
        let items = vec![
            ToolbarItem::Action(ToolbarAction::new(IconName::Save, "Save")),
            ToolbarItem::Separator,
            ToolbarItem::Action(ToolbarAction::new(IconName::Trash, "Clear")),
        ];
        assert_eq!(items[1].width(TOOL_SIZE), SEPARATOR_W);
        assert!(!items[1].folds());
        assert!(items[0].folds());

        let full = TOOL_SIZE * 2. + SEPARATOR_W + SPACING * 2.;
        assert_eq!(kept(&items, full), 3);
    }

    /// A `Custom` item states its own width and whether it has a folded form at all.
    #[test]
    fn a_custom_item_states_its_width_and_whether_it_folds() {
        let dropped = ToolbarItem::Custom {
            width: 110.,
            inline: rect().into_element(),
            folded: None,
        };
        assert_eq!(dropped.width(TOOL_SIZE), 110.);
        assert!(!dropped.folds(), "no folded form means it is dropped");

        let keeps = ToolbarItem::Custom {
            width: 64.,
            inline: rect().into_element(),
            folded: Some(rect().into_element()),
        };
        assert!(keeps.folds());
    }

    /// A header row folds against **its own** control size, not a toolbar's.
    ///
    /// The `⋯` trigger joins the cluster it folds into, so a 24px flat header must charge 24 for
    /// it and for every action — charging a toolbar's 28 would fold a header early and then draw a
    /// mismatched trigger in the gap it made.
    #[test]
    fn a_header_row_folds_against_its_own_control_size() {
        let items = actions(4);
        // Four 24px controls with 8px gaps: 4*24 + 3*8 = 120.
        let header_run = HEADER_CONTROL_SIZE * 4. + SPACING * 3.;

        assert_eq!(
            fold_plan(&items, header_run, SPACING, HEADER_CONTROL_SIZE)
                .iter()
                .filter(|k| **k)
                .count(),
            4,
            "all four fit a header row at their own size"
        );
        assert!(
            fold_plan(&items, header_run, SPACING, TOOL_SIZE)
                .iter()
                .filter(|k| **k)
                .count()
                < 4,
            "the same width folds when the controls are charged as a toolbar's"
        );
    }

    /// A ranked item outlives the plain tail-first order: the pager's Prev and Next survive while
    /// the page-size dropdown and the jump box around them fold away.
    #[test]
    fn a_higher_rank_outlives_the_items_ahead_of_it() {
        let action = || ToolbarItem::Action(ToolbarAction::new(IconName::Save, "Save"));
        // Ranked at 1 and 4, so the survivors are neither at the head nor at the tail -- position
        // cannot be what keeps them.
        let items = vec![
            action(),
            action().rank(1),
            action(),
            action(),
            action().rank(1),
            action(),
        ];

        let with_trigger = |n: usize| run(n) + SPACING + OVERFLOW_W;

        assert_eq!(kept_indices(&items, run(6)), vec![0, 1, 2, 3, 4, 5]);
        assert_eq!(
            kept_indices(&items, 170.),
            vec![0, 1, 4],
            "the unranked tail goes first; both ranked items are still there"
        );
        assert_eq!(
            kept_indices(&items, with_trigger(2)),
            vec![1, 4],
            "and the last things standing are the two that were ranked"
        );
    }

    /// Ranking still resolves at a stub width -- rank orders the folding, it does not exempt.
    #[test]
    fn a_rank_orders_folding_rather_than_preventing_it() {
        let items = vec![
            ToolbarItem::Action(ToolbarAction::new(IconName::Save, "Save")).rank(3),
            ToolbarItem::Action(ToolbarAction::new(IconName::Trash, "Clear")),
        ];
        assert_eq!(kept(&items, 0.), 0, "a rank is not a pin");
    }
}
