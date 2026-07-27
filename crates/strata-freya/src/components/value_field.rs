//! The app's standard **value input**: a mono text box at a size the surface dictates.
//!
//! Freya's `Input` is the control; this is the dress and the two rules every value box in the
//! app wants on top of it.
//!
//! **A stated height.** `Input` sizes itself by its content — the text line box plus its layout
//! theme's inner margin — so a bare one cannot stand at the height a row needs (the fork grew
//! [`Input::height`] for exactly this). A form puts boxes beside controls of fixed size, so the
//! height is a property of the layout, not of the text that happens to be in it.
//!
//! **A length cap that is enforced on the box, not on the way out.** A field capped only where
//! its value is *read* shows one thing and means another — "ab" in a one-character quote field
//! that quotes with `a`. Here the cap trims the bound state itself, so what is on screen is
//! what the caller will read.
//!
//! It binds the caller's `State<String>` rather than owning one: the caller already has the
//! value (it is editing something), and a component that owned it would need a second effect
//! to push changes back. Watch the state for changes; this only ever writes it to enforce the
//! cap.

use freya::prelude::*;

use crate::components::typography::InputTypography;

/// The canvas's standard field box — the height a value input stands at unless a surface says
/// otherwise (a box beside a taller control, say).
pub const FIELD_HEIGHT: f32 = 30.;

#[derive(PartialEq)]
pub struct ValueField {
    value: State<String>,
    placeholder: Option<&'static str>,
    width: Size,
    height: Size,
    /// Characters the box will hold; anything beyond is trimmed from the state itself.
    max_len: Option<usize>,
    align: TextAlign,
    /// A glyph inside the box, before the text — a filter's magnifier, a unit marker.
    leading: Option<Element>,
    enabled: bool,
}

impl ValueField {
    /// A field bound to `value`, filling its parent at the standard height.
    pub fn new(value: State<String>) -> Self {
        Self {
            value,
            placeholder: None,
            width: Size::fill(),
            height: Size::px(FIELD_HEIGHT),
            max_len: None,
            align: TextAlign::default(),
            leading: None,
            enabled: true,
        }
    }

    pub fn placeholder(mut self, placeholder: &'static str) -> Self {
        self.placeholder = Some(placeholder);
        self
    }

    pub fn width(mut self, width: impl Into<Size>) -> Self {
        self.width = width.into();
        self
    }

    /// Stand the box at a specific height — beside a control the surface has already sized.
    pub fn height(mut self, height: impl Into<Size>) -> Self {
        self.height = height.into();
        self
    }

    /// Cap what the box will hold (see the module doc — the cap is enforced on the state).
    pub fn max_len(mut self, max_len: usize) -> Self {
        self.max_len = Some(max_len);
        self
    }

    pub fn align(mut self, align: TextAlign) -> Self {
        self.align = align;
        self
    }

    /// A glyph before the text, inside the box — `Input`'s own leading slot, so it scrolls and
    /// focuses as one control rather than sitting in a hand-drawn strip beside it.
    pub fn leading(mut self, leading: impl IntoElement) -> Self {
        self.leading = Some(leading.into_element());
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
}

impl Component for ValueField {
    fn render(&self) -> impl IntoElement {
        let value = self.value;
        let max_len = self.max_len;
        // Trim in place, so the box can never show more than the caller will read. Guarded, or
        // the write would wake every reader of this state on each keystroke.
        use_side_effect(move || {
            let Some(max_len) = max_len else {
                return;
            };
            let raw = value.read().clone();
            let capped: String = raw.chars().take(max_len).collect();
            if capped != raw {
                let mut value = value;
                value.set(capped);
            }
        });

        InputTypography::mono(
            Input::new(self.value)
                .width(self.width.clone())
                .height(self.height.clone())
                .text_align(self.align)
                .enabled(self.enabled)
                .compact()
                .maybe(self.placeholder.is_some(), |el| {
                    el.placeholder(self.placeholder.unwrap_or_default())
                })
                .map(self.leading.clone(), |el, leading| el.leading(leading)),
        )
    }
}

/// A **bounded number field** — the same box, over a number the parent owns.
///
/// Where [`ValueField`] binds a string the caller already holds, a number cannot be bound
/// directly: half-typed text is not a number, and an emptied box is not zero. So this owns the
/// text buffer, and reports a value only when the box parses — clamping it to the range, because
/// a control that shows one number and hands over another is worse than one that corrects
/// itself. An unparseable box is left alone: the parent keeps the last good value.
///
/// **It reports changes, it does not re-read the parent.** `on_change` fires only when the
/// clamped value actually differs from the last one reported, tracked in state rather than
/// captured — `use_side_effect` builds its closure once, so a captured comparison value freezes
/// at the first render and the field can never be typed back to where it started. That bug is
/// why this comparison lives where it does.
#[derive(PartialEq)]
pub struct NumberField {
    value: u32,
    min: u32,
    max: u32,
    width: Size,
    height: Size,
    on_change: Option<EventHandler<u32>>,
}

impl NumberField {
    /// A field showing `value`, accepting `min..=max`.
    pub fn new(value: u32, min: u32, max: u32) -> Self {
        Self {
            value,
            min,
            max,
            width: Size::fill(),
            height: Size::px(FIELD_HEIGHT),
            on_change: None,
        }
    }

    pub fn width(mut self, width: impl Into<Size>) -> Self {
        self.width = width.into();
        self
    }

    pub fn height(mut self, height: impl Into<Size>) -> Self {
        self.height = height.into();
        self
    }

    /// Called with each new in-range value the box settles on.
    pub fn on_change(mut self, on_change: impl Into<EventHandler<u32>>) -> Self {
        self.on_change = Some(on_change.into());
        self
    }
}

impl Component for NumberField {
    fn render(&self) -> impl IntoElement {
        let value = self.value;
        let text = use_state(move || value.to_string());
        // What was last handed to the caller. In state, not captured — see the type doc.
        let mut reported = use_state(move || value);

        let (min, max) = (self.min, self.max);
        let on_change = self.on_change.clone();
        use_side_effect(move || {
            let Ok(parsed) = text.read().trim().parse::<u32>() else {
                return;
            };
            let clamped = parsed.clamp(min, max);
            if clamped != *reported.peek() {
                reported.set(clamped);
                if let Some(on_change) = &on_change {
                    on_change.call(clamped);
                }
            }
        });

        ValueField::new(text)
            .width(self.width.clone())
            .height(self.height.clone())
    }
}
