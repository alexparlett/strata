//! The segmented toggle (design `segmented_toggle`): a general two/three-option accent-tint
//! segmented control — glyph or text segments in one bordered pill, the active segment an
//! accent-tint fill + accent content. First used as the results Table/Chart switcher (P2-07,
//! icons), then the plan view's Physical/Logical tabs (P2-05, text) — not specific to either,
//! hence the shared component + its own theme component.
//!
//! Shaped like Freya's built-in `SegmentedButton`/`ButtonSegment`: the pill is a container,
//! each [`ToggleSegment`] child carries its own `selected` + `on_press` — the caller owns the
//! selection state.
//!
//! **Two layouts, because the canvas draws two controls.** A [`Variant::Toolbar`] pill is the
//! compact one that sits in a toolbar: flush segments separated by 1px dividers, on the raised
//! `background`. A [`Variant::Form`] pill is the one a settings-style form uses: segments
//! rounded and inset, separated by a gap rather than a rule, on the recessed
//! `form_background`, and padded enough to stand beside a 30px text field. They are the same
//! control with the same dress — only the geometry and the surface differ — which is why this
//! is a variant rather than a second component.
//!
//! The variant is set **once, on the pill**, and reaches the segments through context: a caller
//! that had to remember it on every child would eventually forget one.

use freya::prelude::*;

use crate::components::icon::{Icon, IconName};
use crate::components::typography::Control;
use crate::theme::{use_roles, Role};

define_theme!(
    %[component]
    pub SegmentedToggle {
        %[fields]
        background: Color,
        /// The recessed surface a [`Variant::Form`] pill sits on — a form's controls sit
        /// *into* the pane, where a toolbar's sit on top of it.
        form_background: Color,
        border_fill: Color,
        divider_fill: Color,
        item_color: Color,
        item_active_background: Color,
        item_active_color: Color,
    }
);

/// The pill's own corner (canvas `--r-2`), shared by both layouts.
const PILL_RADIUS: f32 = 8.;
/// A form pill's container padding *and* the gap between its segments (canvas `--sp-1`) — one
/// number, because the inset around the segments and the inset between them are the same inset.
const INSET: f32 = 2.;
/// A toolbar segment's fixed box: the icon segment's 32×24, and the height its divider spans.
const TOOLBAR_SEGMENT_HEIGHT: f32 = 24.;
const TOOLBAR_ICON_WIDTH: f32 = 32.;

/// What a two-icon toolbar pill occupies: both segments plus the 1px divider between them. The
/// border is painted rather than laid out (torin has no notion of one), so it adds nothing.
///
/// Exposed because a pill in a [`crate::components::toolbar::Toolbar`]'s leading run has to
/// declare a width it cannot shrink below, and a call site guessing that number is how it drifts.
pub const TOOLBAR_TWO_ICON_WIDTH: f32 = TOOLBAR_ICON_WIDTH * 2. + 1.;
/// A toolbar text segment's side padding.
const TOOLBAR_TEXT_PADDING: f32 = 12.;
/// A form segment's corner (canvas `--r-1`) and its side padding (canvas `var(--sp-5)`).
const FORM_SEGMENT_RADIUS: f32 = 6.;
const FORM_SEGMENT_SIDE_PADDING: f32 = 16.;
/// A form segment's height: the canvas's `var(--sp-3)` above and below a 12.5px label. Stated
/// rather than left to the text metrics, because a control set beside a form pill is built to
/// it ([`SegmentedToggle::FORM_SEGMENT_HEIGHT`]).
const FORM_SEGMENT_HEIGHT: f32 = 33.;

/// Which of the canvas's two segmented controls this is — see the module doc.
#[derive(PartialEq, Clone, Copy, Default, Debug)]
pub enum Variant {
    /// The compact toolbar pill: flush segments, 1px dividers, raised surface.
    #[default]
    Toolbar,
    /// The roomier form pill: inset rounded segments, gaps instead of dividers, recessed
    /// surface. The canvas's `padding: var(--sp-1)` / `gap: var(--sp-1)` container around
    /// `padding: var(--sp-3) var(--sp-5)` buttons.
    Form,
}

/// The pill: a bordered container over its [`ToggleSegment`] children — interleaving a 1px
/// divider between them in [`Variant::Toolbar`], spacing them in [`Variant::Form`].
#[derive(PartialEq)]
pub struct SegmentedToggle {
    children: Vec<Element>,
    variant: Variant,
    theme: Option<SegmentedToggleThemePartial>,
}

impl Default for SegmentedToggle {
    fn default() -> Self {
        Self::new()
    }
}

impl SegmentedToggle {
    pub fn new() -> Self {
        Self {
            children: Vec::new(),
            variant: Variant::default(),
            theme: None,
        }
    }

    /// A form segment's height — what a control set *beside* a form pill is built to.
    ///
    /// The segments, deliberately, and not the pill's outer box: a field next to the pill reads
    /// as one of the row's controls, so it should match the buttons rather than the container
    /// they sit in. Public because guessing it at a call site (or restating the arithmetic) is
    /// how the two drift apart the first time either changes.
    pub const FORM_SEGMENT_HEIGHT: f32 = FORM_SEGMENT_HEIGHT;

    /// The roomier form layout (see [`Variant::Form`]). Applies to this pill's segments too —
    /// they read it from context, so it is set here and nowhere else.
    pub fn form(mut self) -> Self {
        self.variant = Variant::Form;
        self
    }
}

impl ChildrenExt for SegmentedToggle {
    fn get_children(&mut self) -> &mut Vec<Element> {
        &mut self.children
    }
}

impl Component for SegmentedToggle {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(
            &self.theme,
            SegmentedToggleThemePreference,
            "segmented_toggle"
        );

        // Scoped to this pill's subtree, so every segment under it lays out the same way
        // without the caller repeating itself.
        let variant = self.variant;
        use_provide_context(move || variant);

        let mut pill = rect()
            .horizontal()
            .cross_align(Alignment::Center)
            .corner_radius(PILL_RADIUS)
            .border(Border::new().width(1.).fill(theme.border_fill));
        pill = match self.variant {
            // Flush segments, so the pill clips them to its own corners.
            Variant::Toolbar => pill.background(theme.background).overflow(Overflow::Clip),
            // Inset segments carry their own corners, so the pill pads and spaces them
            // instead of clipping — clipping would eat the 2px inset.
            Variant::Form => pill
                .background(theme.form_background)
                .padding(Gaps::new(INSET, INSET, INSET, INSET))
                .spacing(INSET),
        };

        for (i, segment) in self.children.iter().enumerate() {
            // The divider is the toolbar layout's separator; the form layout's is the gap.
            if i > 0 && self.variant == Variant::Toolbar {
                pill = pill.child(
                    rect()
                        .width(Size::px(1.))
                        .height(Size::px(TOOLBAR_SEGMENT_HEIGHT))
                        .background(theme.divider_fill),
                );
            }
            pill = pill.child(segment.clone());
        }
        pill
    }
}

/// What a segment shows: a 15px glyph (the 32×24 icon segment) or a control-role text label
/// (the segment hugs it with 12px side padding).
#[derive(PartialEq, Clone)]
enum SegmentContent {
    Icon(IconName),
    Text(String),
}

/// One 24px-tall segment: a glyph or label wearing its tooltip `title`, the active dress
/// (accent tint + accent content) when `selected`, and the comp's soft hover (a 7% text-colour
/// overlay derived from its own theme) otherwise.
#[derive(PartialEq)]
pub struct ToggleSegment {
    content: SegmentContent,
    title: Option<String>,
    selected: bool,
    on_press: Option<EventHandler<Event<PressEventData>>>,
    theme: Option<SegmentedToggleThemePartial>,
}

impl ToggleSegment {
    pub fn new(icon: IconName) -> Self {
        Self {
            content: SegmentContent::Icon(icon),
            title: None,
            selected: false,
            on_press: None,
            theme: None,
        }
    }

    /// A text segment (`Control` typography) — e.g. the plan view's Physical/Logical tabs.
    pub fn text(label: impl Into<String>) -> Self {
        Self {
            content: SegmentContent::Text(label.into()),
            title: None,
            selected: false,
            on_press: None,
            theme: None,
        }
    }

    /// The tooltip this segment wears (the comp's `title=`).
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn on_press(mut self, on_press: impl Into<EventHandler<Event<PressEventData>>>) -> Self {
        self.on_press = Some(on_press.into());
        self
    }
}

impl Component for ToggleSegment {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(
            &self.theme,
            SegmentedToggleThemePreference,
            "segmented_toggle"
        );
        let hover = use_roles().get(Role::Text).with_a(18);
        let mut hovered = use_state(|| false);

        let background = if self.selected {
            theme.item_active_background
        } else if hovered() {
            hover
        } else {
            Color::TRANSPARENT
        };
        let on_press = self.on_press.clone();
        let color = if self.selected {
            theme.item_active_color
        } else {
            theme.item_color
        };
        // Set once on the pill (see the module doc); a bare segment outside one lays out as the
        // toolbar it was first written for.
        let variant = use_try_consume::<Variant>().unwrap_or_default();

        let segment = rect()
            .center()
            .background(background)
            .on_pointer_enter(move |_| hovered.set(true))
            .on_pointer_leave(move |_| hovered.set(false))
            .on_press(move |e| {
                if let Some(on_press) = &on_press {
                    on_press.call(e);
                }
            });
        // A form segment is sized by its padding and carries its own corner; a toolbar segment
        // is a fixed box clipped by the pill.
        let segment = match variant {
            Variant::Toolbar => segment.height(Size::px(TOOLBAR_SEGMENT_HEIGHT)),
            Variant::Form => segment
                .height(Size::px(FORM_SEGMENT_HEIGHT))
                .corner_radius(FORM_SEGMENT_RADIUS),
        };
        let segment = match (&self.content, variant) {
            (SegmentContent::Icon(icon), _) => segment
                .width(Size::px(TOOLBAR_ICON_WIDTH))
                .child(Icon::new(*icon).color(color).size(15.)),
            (SegmentContent::Text(label), Variant::Toolbar) => segment
                .padding((0., TOOLBAR_TEXT_PADDING))
                .child(Control::new(label.clone()).color(color)),
            (SegmentContent::Text(label), Variant::Form) => segment
                .padding((0., FORM_SEGMENT_SIDE_PADDING))
                .child(Control::new(label.clone()).color(color)),
        };
        match &self.title {
            Some(title) => TooltipContainer::new(Tooltip::new_text(title.clone()))
                .position(AttachedPosition::Bottom)
                .child(segment)
                .into_element(),
            None => segment.into_element(),
        }
    }
}
