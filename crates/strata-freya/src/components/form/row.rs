//! One row of a [`Form`](super::Form): a label, its explanation, and the control.
//!
//! The control is the caller's **child** — this knows nothing about it, so a row wraps a
//! [`ValueField`](super::ValueField), a `Switch`, a `SegmentedToggle`, a `Select` or a [`Note`]
//! without changing shape. How the label and its explanation are *set* comes from the form's
//! [`Variant`], read from context; see the module doc.
//!
//! A row with an [`anchor`](Row::anchor) can also be **revealed** — brought into view and flashed
//! once, when something outside the form asks for it by name (see [`reveal`](super::reveal)).

use freya::animation::{use_animation_with_dependencies, AnimColor, AnimDirection, Ease, OnChange};
use freya::prelude::*;

use crate::components::divider::Divider;
use crate::components::form::{
    form_theme, Reveal, RevealScroll, Variant, CONTROL_GAP, HINT_GAP, LABEL_GAP,
};
use crate::components::icon::{Icon, IconName};
use crate::components::metrics::{R_1, R_2, SP_4};
use crate::components::typography::{Caption, Eyebrow, Meta, Prose, Strong};

/// The ⓘ that carries a fields row's explanation.
const HINT_SIZE: f32 = 12.;

/// How long a revealed row's flash takes to fade out (canvas `ps-setting-flash`, `1.5s ease-out`).
const FLASH_MS: u64 = 1500;

/// The flash's corner (canvas `border-radius: 8px`).
///
/// The canvas bleeds the wash 10px past either edge of the row (a spread box-shadow); ours stops at
/// the row's own box, because a torin child cannot paint outside the bounds its parent laid out for
/// it and inset-then-negative-margin would move every row on the surface to buy it.
const FLASH_RADIUS: f32 = R_2;

#[derive(PartialEq)]
pub struct Row {
    label: String,
    hint: Option<String>,
    required: bool,
    trailing: bool,
    anchor: Option<&'static str>,
    on_press: Option<EventHandler<Event<PressEventData>>>,
    children: Vec<Element>,
}

impl Row {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            hint: None,
            required: false,
            trailing: false,
            anchor: None,
            on_press: None,
            children: Vec::new(),
        }
    }

    /// Name this row, so something outside the form can ask for it: a [`Reveal`] carrying this
    /// anchor scrolls the row into view and flashes it once.
    ///
    /// `&'static str` and not a `String`, because an anchor is a name in the *code* — the Settings
    /// search hands over one its own index minted (`apps::settings::search::Anchor`), never
    /// anything a user typed.
    pub fn anchor(mut self, anchor: &'static str) -> Self {
        self.anchor = Some(anchor);
        self
    }

    /// Mark this row's value as **required** — the `REQUIRED` marker on the label line.
    ///
    /// On the label rather than on the control, and here rather than as a per-window label
    /// component, for the reason the row exists at all: it is one of the three things a row
    /// says about itself, beside its title and its explanation, and a window that drew its own
    /// would be a window whose label line drifts from every other one's. The marker sits
    /// between the label and the ⓘ, which is the canvases' order.
    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }

    /// This row's explanation — a hover tooltip in the fields register, inline subtext under
    /// the title in preferences. Absent = nothing, rather than an empty tooltip or a blank line.
    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// Put the control at the row's **trailing edge** instead of under the label block.
    ///
    /// For a control small enough to read as the row's answer to its own label — a `Switch` in
    /// the preferences register. Explicit per row rather than derived from the variant because
    /// it is the one presentation the canvases disagree about *within* a register (see the
    /// module doc's known divergences).
    pub fn trailing(mut self) -> Self {
        self.trailing = true;
        self
    }

    /// Make the label block activate the control, so the whole row acts as one target.
    ///
    /// The row is a **sibling** of the control, never its ancestor: a built-in's `on_press`
    /// does not stop propagation, so a pressable ancestor would take the same click and act
    /// twice — for a `Switch`, back to where it started.
    pub fn on_press(mut self, on_press: impl Into<EventHandler<Event<PressEventData>>>) -> Self {
        self.on_press = Some(on_press.into());
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
        let theme = form_theme();
        let variant = use_try_consume::<Variant>().unwrap_or_default();

        let anchor = self.anchor;
        let reveal = use_try_consume::<Reveal>();
        let scroll = use_try_consume::<RevealScroll>();
        let mut area = use_state(|| None::<Area>);
        let flash = use_animation_with_dependencies(&theme.reveal_background, |conf, tint| {
            conf.on_change(OnChange::Finish);
            AnimColor::new(*tint, tint.with_a(0))
                .time(FLASH_MS)
                .ease(Ease::Out)
        });

        use_side_effect(move || {
            let (Some(anchor), Some(reveal)) = (anchor, reveal) else {
                return;
            };
            if !reveal.wanted(anchor) {
                return;
            }
            let Some(area) = *area.read() else {
                return;
            };
            if let Some(scroll) = scroll {
                scroll.reveal(area);
            }
            flash.run(AnimDirection::Forward);
            reveal.taken();
        });

        let wash = match *flash.has_run_yet().read() {
            true => flash.get().value(),
            false => Color::TRANSPARENT,
        };

        let required = self
            .required
            .then(|| Meta::new("REQUIRED").color(theme.required_color));

        let label = match variant {
            Variant::Fields => rect()
                .horizontal()
                .cross_align(Alignment::Center)
                .spacing(LABEL_GAP)
                .child(Eyebrow::new(self.label.clone()).color(theme.label_color))
                .maybe_child(required)
                .maybe_child(self.hint.clone().map(|hint| {
                    TooltipContainer::new(Tooltip::new_text(hint))
                        .position(AttachedPosition::Top)
                        .child(
                            Icon::new(IconName::Info)
                                .size(HINT_SIZE)
                                .color(theme.hint_color),
                        )
                })),
            Variant::Preferences => rect()
                .vertical()
                .child(
                    rect()
                        .horizontal()
                        .cross_align(Alignment::Center)
                        .spacing(LABEL_GAP)
                        .child(
                            Strong::new(self.label.clone())
                                .color(theme.title_color)
                                .width(Size::flex(1.))
                                .wrap(),
                        )
                        .maybe_child(required),
                )
                .map(self.hint.clone(), |el, hint| {
                    el.child(rect().height(Size::px(HINT_GAP))).child(
                        Caption::new(hint)
                            .color(theme.hint_color)
                            .width(Size::fill())
                            .wrap(),
                    )
                }),
        };
        let label = label.map(self.on_press.clone(), |el, on_press| {
            el.on_press(move |e: Event<PressEventData>| on_press.call(e))
        });

        let gap = match variant {
            Variant::Fields => LABEL_GAP,
            Variant::Preferences => CONTROL_GAP,
        };

        let row = if self.trailing {
            let mut row = rect()
                .width(Size::fill())
                .horizontal()
                .content(Content::Flex)
                .cross_align(Alignment::Center)
                .spacing(CONTROL_GAP)
                .child(label.width(Size::flex(1.)));
            for child in &self.children {
                row = row.child(child.clone());
            }
            row
        } else {
            let mut row = rect()
                .width(Size::fill())
                .vertical()
                .spacing(gap)
                .child(label.width(Size::fill()));
            for child in &self.children {
                row = row.child(child.clone());
            }
            row
        };

        row.background(wash)
            .corner_radius(FLASH_RADIUS)
            .maybe(anchor.is_some(), |el| {
                el.on_sized(move |e: Event<SizedEventData>| area.set(Some(e.area)))
            })
    }
}

/// A form's explanatory block — a statement where a control would go.
///
/// Not a disabled control and not a hint: it is what a row says when there is nothing to set
/// (the export window's Arrow format, which has no write options at all). An empty row would
/// read as "still loading".
#[derive(PartialEq)]
pub struct Note {
    text: String,
}

impl Note {
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }
}

impl Component for Note {
    fn render(&self) -> impl IntoElement {
        let theme = form_theme();
        rect()
            .width(Size::fill())
            .padding((SP_4, SP_4))
            .corner_radius(R_1)
            .background(theme.note_background)
            .border(Border::new().width(1.).fill(theme.note_border_fill))
            .child(Prose::new(self.text.clone()).color(theme.note_color).wrap())
    }
}

/// A **section heading** inside a fields form: a group's name, with a rule running out from it.
///
/// For a form long enough that its rows want sorting into subjects — a data source declaring a
/// dozen settings, where "which of these are about SSL" is a question the reader should not have
/// to answer by reading every label. Set in the eyebrow register like a row's own label, because
/// it is the same kind of word about a bigger thing.
#[derive(PartialEq)]
pub struct Section {
    label: String,
    key: DiffKey,
}

impl Section {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            key: DiffKey::None,
        }
    }
}

impl KeyExt for Section {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for Section {
    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }

    fn render(&self) -> impl IntoElement {
        let theme = form_theme();

        rect()
            .width(Size::fill())
            .horizontal()
            .content(Content::Flex)
            .cross_align(Alignment::Center)
            .spacing(LABEL_GAP)
            .child(Eyebrow::new(self.label.clone()).color(theme.label_color))
            .child(
                rect()
                    .width(Size::flex(1.))
                    .child(Divider::horizontal().color(theme.divider_fill)),
            )
    }
}
