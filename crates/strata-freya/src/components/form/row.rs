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

use crate::components::form::{
    form_theme, Reveal, RevealScroll, Variant, CONTROL_GAP, HINT_GAP, LABEL_GAP,
};
use crate::components::icon::{Icon, IconName};
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
const FLASH_RADIUS: f32 = 8.;

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
        // Set once on the form (see the module doc); a bare row outside one is set in the
        // register the app's window forms use.
        let variant = use_try_consume::<Variant>().unwrap_or_default();

        // Being revealed: the ask (window-lived), the frame to scroll within (page-lived), our own
        // measured box, and the flash. All four hooks run whether or not this row has an anchor —
        // hook order is positional, so a row cannot pay for them conditionally.
        let anchor = self.anchor;
        let reveal = use_try_consume::<Reveal>();
        let scroll = use_try_consume::<RevealScroll>();
        let mut area = use_state(|| None::<Area>);
        // Dependent on the tint, so a theme change while this row is mounted re-arms the flash on
        // the new accent rather than freezing the one captured at mount.
        //
        // Two things that look like detail and are not. `OnChange::Finish`, because the default is
        // `Reset` — which sets `has_run_yet` *and* puts the value back to the animation's **origin**,
        // i.e. the tint, so switching theme on the Appearance pane left every row on it wearing a
        // permanent accent wash. Finishing lands on the destination instead, which is the invisible
        // end of the fade. And the destination is the tint at **zero alpha**, not `TRANSPARENT`:
        // `AnimColor` interpolates r, g, b and a independently, so fading to (0,0,0,0) drags the
        // wash's hue toward black on the way out instead of fading the accent out.
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
            // Our area lands a frame after the page mounts, so the first pass through here only
            // subscribes to it. Reading it *after* the ask is what keeps the later passes cheap:
            // torin re-emits `Sized` for every row on scroll, and by then the ask is cleared, so
            // this returns above without taking a subscription on the area at all.
            let Some(area) = *area.read() else {
                return;
            };
            if let Some(scroll) = scroll {
                scroll.reveal(area);
            }
            flash.run(AnimDirection::Forward);
            reveal.taken();
        });

        // Transparent until the flash has actually run: `AnimColor` sits at its origin — the tint —
        // before it is started, so an unflashed row would wear the wash permanently.
        let wash = match *flash.has_run_yet().read() {
            true => flash.get().value(),
            false => Color::TRANSPARENT,
        };

        // The label block. In the fields register the explanation hangs off a ⓘ beside the
        // label; in preferences it is a line of subtext under it, wrapped — those are full
        // sentences and the pane is narrow, so `Caption`'s default single-line cap would eat
        // the end of half of them.
        //
        // A preferences **title** wraps for the same reason: it is a sentence-case phrase and
        // some of them are whole clauses ("Confirm before closing a tab or window with a
        // running query"), which at the window's minimum width would otherwise be clipped
        // mid-word by the single-line default. A fields eyebrow stays capped — it is a short
        // uppercase label, and one that grew long would be the wrong label.
        // The marker reads as a small mono note beside the label in either register — quieter
        // than the label it qualifies, because it describes the field rather than naming it.
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
                    TooltipContainer::new(Tooltip::new(hint))
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
            // Label block and control side by side. `Content::Flex` is what makes the label's
            // `flex(1.)` divide the row rather than take its natural width — without it the
            // control is pushed off the surface.
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

        // The flash paints on the row itself rather than a wrapper, so it is the row's own box that
        // lights up and nothing about the form's rhythm changes. An anchorless row measures nothing.
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
        // The box comes off the form's own theme, not the base sheet. Reading `surface_primary`
        // there looked equivalent and is not: it is a *lower* tone than the window body, so the
        // note read as a hole punched in the surface while the panes beside it read as raised. A
        // component's dress is its own theme's (AGENTS.md §3), and the sheet is only for the
        // semantic ramp — which a note is not.
        rect()
            .width(Size::fill())
            .padding((12., 12.))
            .corner_radius(6.)
            .background(theme.note_background)
            .border(Border::new().width(1.).fill(theme.note_border_fill))
            .child(Prose::new(self.text.clone()).color(theme.note_color).wrap())
    }
}
