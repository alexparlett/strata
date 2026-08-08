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

use crate::components::form::{form_theme, FIELD_GAP};
use crate::components::icon::{Icon, IconName};
use crate::components::typography::{Body, InputTypography};

/// The canvas's standard field box — the height a value input stands at unless a surface says
/// otherwise (a box beside a taller control, say).
pub const FIELD_HEIGHT: f32 = 30.;

/// A [`DirectoryField`]'s browse button: the canvas's square beside the box, at the height
/// every value box in a form stands, and the glyph in it.
const BROWSE_WIDTH: f32 = 34.;
const BROWSE_ICON: f32 = 15.;

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
    /// Show the value as dots instead of characters — see [`ValueField::masked`].
    masked: bool,
    enabled: bool,
    /// No chrome of its own — see [`ValueField::bare`].
    bare: bool,
    /// The box's own id, when the caller needs to watch it — see [`ValueField::a11y_id`].
    a11y_id: Option<AccessibilityId>,
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
            masked: false,
            enabled: true,
            bare: false,
            a11y_id: None,
        }
    }

    /// Drop the box: no background, no border, no focus ring.
    ///
    /// For a field inside a container that already draws the chrome — a pane's header strip, a
    /// popover panel — where a second box inside the first reads as a mistake. The canvas
    /// writes these inputs as `background: transparent; border: none; outline: none` for the
    /// same reason.
    ///
    /// **It drops the focus ring too**, which is the whole point: a surface reaches for this
    /// when the container is what the user sees, and a ring appearing inside it would be the
    /// second box all over again. A field that wants to look like an input — its own box, its
    /// own focus ring — should simply not ask to be bare.
    pub fn bare(mut self) -> Self {
        self.bare = true;
        self
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

    /// Show the value as dots rather than characters — a secret the surface displays but does
    /// not want on screen by default (Settings ▸ Agent access's bearer token).
    ///
    /// `Input`'s own [`InputMode`], not a masked copy of the string, and the difference is the
    /// whole reason this is a passthrough: the state keeps the real value, so revealing it is a
    /// prop flip rather than a second source of truth to keep in step — and Freya's editable
    /// refuses to copy a masked box's contents to the clipboard, which a hand-masked string
    /// could not.
    pub fn masked(mut self, masked: bool) -> Self {
        self.masked = masked;
        self
    }

    /// Give the box an id the caller already holds, so it can watch the field's focus with
    /// `use_focus(id)`.
    ///
    /// `Input` has no blur prop and only reports on Enter, and losing focus is when a field
    /// whose text is *derived* from something else re-echoes it ([`NumberField`]). Owning the
    /// id is the only way to see that moment.
    pub fn a11y_id(mut self, a11y_id: AccessibilityId) -> Self {
        self.a11y_id = Some(a11y_id);
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

        // The caller's width goes on the **wrapper**, and the `Input` fills it.
        //
        // `InputTypography` is a `rect()` around the input, so it — not the input — is what a
        // parent lays out. Sizing only the input leaves the wrapper hugging whatever that
        // resolved to, which is invisible for a `px` width and wrong for a relative one: a
        // `flex(1.)` input inside a hugging wrapper is not a flex child of the row at all, so
        // the row divides nothing, the wrapper hugs the full width, and the control beside it
        // (a `DirectoryField`'s browse button) is pushed off the surface.
        InputTypography::mono(
            Input::new(self.value)
                .width(Size::fill())
                .height(self.height.clone())
                .text_align(self.align)
                .enabled(self.enabled)
                .maybe(self.masked, |el| el.mode(InputMode::new_password()))
                .compact()
                .map(self.a11y_id, |el, id| el.a11y_id(id))
                .maybe(self.placeholder.is_some(), |el| {
                    el.placeholder(self.placeholder.unwrap_or_default())
                })
                .map(self.leading.clone(), |el, leading| el.leading(leading))
                .maybe(self.bare, |el| {
                    el.background(Color::TRANSPARENT)
                        .focus_background(Color::TRANSPARENT)
                        .border_fill(Color::TRANSPARENT)
                        .focus_border_fill(Color::TRANSPARENT)
                }),
        )
        .width(self.width.clone())
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
///
/// **Reporting is per keystroke; the box is normalized when it is left.** Every accepted
/// keystroke reports, because the thing that commits a value is usually a `Button` and `Button`
/// moves focus and calls its handler in the same breath — a value that waited for blur would
/// never reach the draft being committed. But that leaves the box free to show something the
/// caller never received (`abc`, an empty box, `9999` where the max is 2000), so losing focus
/// re-echoes what was last reported. Echoed from `reported` and not from `value`, which keeps
/// the rule above: this still never re-reads the parent.
#[derive(PartialEq)]
pub struct NumberField {
    value: u32,
    min: u32,
    max: u32,
    width: Size,
    height: Size,
    /// What the number is measured in — see [`NumberField::unit`].
    unit: Option<&'static str>,
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
            unit: None,
            on_change: None,
        }
    }

    /// The unit this number is measured in (`px`, `rows`, `runs`), set beside the box.
    ///
    /// **Beside**, not inside it, which is the canvas's own arrangement in every form that has
    /// one: the unit labels the number rather than being part of what you type, so it neither
    /// scrolls with the text nor takes the field's focus. Absent for a bare count (the export
    /// window's compression level), which is why this is opt-in rather than a required word.
    pub fn unit(mut self, unit: &'static str) -> Self {
        self.unit = Some(unit);
        self
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
        let mut text = use_state(move || value.to_string());
        // What was last handed to the caller. In state, not captured — see the type doc.
        let mut reported = use_state(move || value);
        // The box's id is ours, so the effect below can see it lose focus.
        let a11y_id = use_a11y();
        let focus = use_focus(a11y_id);

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

        // Leaving the box is when it is made to agree with what it reported — see the type doc.
        // `reported` is peeked, not read: this must wake on focus alone, or the echo would land
        // mid-keystroke and overwrite what is being typed.
        use_side_effect(move || {
            if focus() == Focus::Not {
                text.set_if_modified(reported.peek().to_string());
            }
        });

        let box_ = ValueField::new(text)
            .width(self.width.clone())
            .height(self.height.clone())
            .a11y_id(a11y_id);

        // Without a unit the box *is* the control, so it is returned bare — a wrapper would
        // change how a caller's `width` lands in the row around it.
        match self.unit {
            None => box_.into_element(),
            Some(unit) => rect()
                .horizontal()
                .cross_align(Alignment::Center)
                .spacing(FIELD_GAP)
                .child(box_)
                .child(Body::new(unit).color(form_theme().hint_color))
                .into_element(),
        }
    }
}

/// What a [`PathField`]'s browse button opens, and therefore what the field names.
#[derive(PartialEq, Clone, Copy)]
enum Pick {
    Folder,
    /// One file, narrowed to the extensions the surface names — a picker offering every file on
    /// the disk for a field that takes one kind is a picker that finds the wrong one.
    File(&'static [&'static str]),
}

/// A **path** — a value box with the native picker beside it, over a folder
/// ([`folder`](PathField::folder)) or one file ([`file`](PathField::file)).
///
/// Two ways to set one value, so there is one buffer and both write it: the picker sets the
/// box, and what the box holds is what gets reported. A button that reached past the box into
/// the caller's state would leave the two free to disagree.
///
/// It follows [`NumberField`]'s contract, for the same reasons: it **owns the buffer** (`Input`
/// writes its bound state directly and has no on-change prop), it **reports per keystroke**
/// (the thing that commits a form is usually a `Button`, which moves focus and calls its
/// handler in the same breath, so a value that waited for blur would never reach what is being
/// committed), and it **does not re-read the caller** — `value` seeds the box and nothing more.
/// What is reported is tracked in state rather than captured, or the comparison would freeze at
/// the first render and a path typed back to its original could never be reported.
///
/// Unlike a number, every string a user can type is a legal path, so there is nothing to
/// normalize when the box is left — a path that does not exist yet is still the path they mean.
///
/// **One component for both**, because the two differ in the picker call and nothing else: the
/// box, the report contract, the browse button and its geometry are the same field. Two
/// components would be sixty duplicated lines and a second place for that contract to drift.
#[derive(PartialEq)]
pub struct PathField {
    value: String,
    pick: Pick,
    placeholder: Option<&'static str>,
    /// What the picker calls itself — see [`PathField::dialog_title`].
    dialog_title: &'static str,
    on_change: Option<EventHandler<String>>,
}

impl PathField {
    /// A field showing `value`, with a **folder** picker beside it.
    pub fn folder(value: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            pick: Pick::Folder,
            placeholder: None,
            dialog_title: "Choose a folder",
            on_change: None,
        }
    }

    /// A field showing `value`, with a **file** picker beside it, narrowed to `extensions`
    /// (bare, no dot — `["json"]`).
    pub fn file(value: impl Into<String>, extensions: &'static [&'static str]) -> Self {
        Self {
            value: value.into(),
            pick: Pick::File(extensions),
            placeholder: None,
            dialog_title: "Choose a file",
            on_change: None,
        }
    }

    pub fn placeholder(mut self, placeholder: &'static str) -> Self {
        self.placeholder = Some(placeholder);
        self
    }

    /// The title on the picker. Worth setting: the dialog is a window of its own, so what the
    /// row said is off screen by the time it is up, and "Choose a folder" is all it would
    /// otherwise say about which folder.
    pub fn dialog_title(mut self, dialog_title: &'static str) -> Self {
        self.dialog_title = dialog_title;
        self
    }

    /// Called with each new path the box settles on, typed or picked.
    pub fn on_change(mut self, on_change: impl Into<EventHandler<String>>) -> Self {
        self.on_change = Some(on_change.into());
        self
    }
}

impl Component for PathField {
    fn render(&self) -> impl IntoElement {
        let mut text = use_state({
            let value = self.value.clone();
            move || value
        });
        // What was last handed to the caller. Seeded from `value`, so the first pass reports
        // nothing — the caller already holds this.
        let mut reported = use_state({
            let value = self.value.clone();
            move || value
        });

        let on_change = self.on_change.clone();
        use_side_effect(move || {
            let current = text.read().clone();
            if current == *reported.peek() {
                return;
            }
            reported.set(current.clone());
            if let Some(on_change) = &on_change {
                on_change.call(current);
            }
        });

        let dialog_title = self.dialog_title;
        let pick = self.pick;
        rect()
            .width(Size::fill())
            .horizontal()
            .content(Content::Flex)
            .cross_align(Alignment::Center)
            .spacing(FIELD_GAP)
            .child(
                ValueField::new(text)
                    .width(Size::flex(1.))
                    .map(self.placeholder, |el, placeholder| {
                        el.placeholder(placeholder)
                    }),
            )
            .child(
                Button::new()
                    .outline()
                    .theme_layout(
                        ButtonLayoutThemePartial::default()
                            .width(Size::px(BROWSE_WIDTH))
                            .height(Size::px(FIELD_HEIGHT))
                            // The stated box *is* the size: the stock 6/12 padding would leave
                            // the glyph 10px to sit in, and a button clips its overflow.
                            .padding(Gaps::new_all(0.)),
                    )
                    .on_press(move |_| {
                        // Start where the box points, so browsing from a set path opens there
                        // rather than wherever the OS last left the panel. A file field starts
                        // at its file's *folder*: `set_directory` on a file path opens the
                        // panel at whatever the OS makes of it, which is usually nowhere.
                        let start = match pick {
                            Pick::Folder => text.peek().clone(),
                            Pick::File(_) => std::path::Path::new(&*text.peek())
                                .parent()
                                .map(|p| p.to_string_lossy().into_owned())
                                .unwrap_or_default(),
                        };
                        spawn(async move {
                            let mut dialog = rfd::AsyncFileDialog::new().set_title(dialog_title);
                            if !start.is_empty() {
                                dialog = dialog.set_directory(&start);
                            }
                            // Dismissing the dialog is a decision, not a failure — the box
                            // keeps what it had.
                            let picked = match pick {
                                Pick::Folder => dialog.pick_folder().await,
                                Pick::File(extensions) => {
                                    dialog.add_filter("", extensions).pick_file().await
                                }
                            };
                            if let Some(handle) = picked {
                                text.set(handle.path().to_string_lossy().into_owned());
                            }
                        });
                    })
                    .child(Icon::new(IconName::Folder).size(BROWSE_ICON)),
            )
    }
}
