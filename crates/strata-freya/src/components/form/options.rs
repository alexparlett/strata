//! **Options as data** — the vocabulary a format-driven option list is described in, and the
//! one component per control *shape* that renders it.
//!
//! A surface hands over a `Vec<Group<E>>` and this renders whatever it is given: label, optional
//! hint, control. So adding an option is a row in a table rather than a new branch in a
//! component, which is what the export window's rebuild (P4-10 / D6) was actually for — the
//! Dioxus modal reached the same screen through hardcoded `match` arms per format.
//!
//! **Every option carries the edit it performs.** A [`Choice`] holds an `E`, a text field holds
//! `fn(String) -> E`. There is no key/value pairing for a view to get wrong: the only thing a
//! control can do is report the edit it was built with, and the surface's own `apply` is
//! exhaustive over `E`. The edit type is the surface's, because what an option *means* is: the
//! export window writes an `ExportDraft`, the Configure window a table def.
//!
//! **Each control shape is its own `Component`**, not a helper fn. The group list changes length
//! with the format, so rendering the stateful ones inline would call a *variable* number of
//! hooks per render and corrupt hook order. A component per shape gives each its own scope,
//! which is also what lets a text field keep an edit buffer at all.
//!
//! This started as the export window's `views/options.rs` and moved here with P4-11, its second
//! consumer. The only thing that changed in the move is where the write-back comes from: these
//! took the export window's context directly, and now take an `on_edit` handler, because a
//! shared control cannot know whose draft it is editing.

use freya::prelude::*;

use crate::components::form::{Form, Note, NumberField, Row, ValueField, FIELD_HEIGHT};
use crate::components::segmented_toggle::{SegmentedToggle, ToggleSegment};
use crate::components::typography::{MonoValue, Prose};

/// Field boxes, from the canvas: a one-character field, a short text field, a number, the
/// custom box beside a segmented control, and a select (the one control the canvas draws 32
/// tall rather than 30).
const CHAR_WIDTH: f32 = 48.;
const TEXT_WIDTH: f32 = 120.;
const NUM_WIDTH: f32 = 72.;
const CUSTOM_WIDTH: f32 = 62.;
const SELECT_WIDTH: f32 = 180.;
const SELECT_HEIGHT: f32 = 32.;

/// A control's write-back — the function that turns what the user entered into an edit.
///
/// A newtype rather than a bare `fn` pointer so the comparison is explicit: a derived `==` over
/// a function pointer warns, because pointer addresses are not guaranteed unique. `fn_addr_eq`
/// is the sanctioned comparison, and it is enough here — a group is keyed by its label, so two
/// groups with the same label always carry the same write-back.
#[derive(Debug)]
pub struct Make<T, E>(pub fn(T) -> E);

// Hand-written, all three: `derive` would bound them on `T: Clone`/`T: Copy`, and `T` here is
// the *argument* type — a `Make` is a bare function pointer and copies regardless of what it
// takes.
impl<T, E> Clone for Make<T, E> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T, E> Copy for Make<T, E> {}

impl<T, E> Make<T, E> {
    /// Build the edit for `value`.
    pub fn edit(&self, value: T) -> E {
        (self.0)(value)
    }
}

impl<T, E> PartialEq for Make<T, E> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::fn_addr_eq(self.0, other.0)
    }
}

/// One selectable value in a segmented control or a dropdown.
#[derive(Clone, PartialEq, Debug)]
pub struct Choice<E> {
    pub label: String,
    pub edit: E,
    pub selected: bool,
}

impl<E> Choice<E> {
    /// A choice that is selected when it equals `current` — the shape every pill in the app
    /// builds, spelled once.
    pub fn of(label: impl Into<String>, value: E, current: &E) -> Self
    where
        E: PartialEq + Clone,
    {
        Self {
            label: label.into(),
            selected: value == *current,
            edit: value,
        }
    }
}

/// A free-text control's current value and what typing in it does.
#[derive(Clone, PartialEq, Debug)]
pub struct TextField<E> {
    pub value: String,
    pub placeholder: &'static str,
    pub max_len: usize,
    /// What typing in this field does.
    pub make: Make<String, E>,
}

/// The control a group renders. One variant per shape the canvases draw.
#[derive(Clone, PartialEq, Debug)]
pub enum Control<E> {
    /// A pill of mutually exclusive values, optionally with a custom text field that shows
    /// only while the "custom" value is picked.
    Seg {
        options: Vec<Choice<E>>,
        custom: Option<TextField<E>>,
    },
    /// A switch, with the sentence beside it that says what it currently means.
    Toggle {
        on: bool,
        edit: E,
        hint: Option<String>,
    },
    /// A short free-text field.
    Text(TextField<E>),
    /// A one-character field (a quote, an escape, a comment marker).
    Char(TextField<E>),
    /// A bounded number.
    Num {
        value: u32,
        min: u32,
        max: u32,
        make: Make<u32, E>,
    },
    /// A dropdown.
    Select { options: Vec<Choice<E>> },
    /// A statement, not a control — for a format that genuinely has nothing to set. An empty
    /// row would read as "still loading".
    Note(&'static str),
}

/// One labelled option group, as the canvases draw it: an uppercase label, an optional hover
/// hint, and a control.
#[derive(Clone, PartialEq, Debug)]
pub struct Group<E> {
    pub label: String,
    pub hint: Option<&'static str>,
    pub control: Control<E>,
}

/// A list of option groups as a [`Form`] of [`Row`]s.
#[derive(PartialEq)]
pub struct OptionList<E: Clone + PartialEq + 'static> {
    groups: Vec<Group<E>>,
    scope: String,
    on_edit: EventHandler<E>,
}

impl<E: Clone + PartialEq + 'static> OptionList<E> {
    /// Render `groups`, reporting each control's edit to `on_edit`.
    ///
    /// `scope` names **which list this is** — the format, in both windows that use this. It
    /// joins each row's label to make the key, and it is not optional, for two reasons that turn
    /// out to be the same one.
    ///
    /// A row is identified by its label, and two formats can label different rows the same way:
    /// CSV's `COMPRESSION` and JSON's `COMPRESSION` are not the same control — they carry
    /// different `Edit`s, writing different fields. Keyed on the label alone the differ pairs
    /// them across a format switch and *reuses the scope*, so a control's buffer would carry
    /// from one format's option to another's.
    ///
    /// And the same pairing crashed the window. Freya's differ matches siblings by key
    /// (`path_element.rs::diff`) and records a matched pair at different indices as **moved** —
    /// CSV's `COMPRESSION` is the 9th row where JSON's is the 3rd — after which `run_scope`
    /// looks the component up at its new path and unwraps the `scope_id` that a move left
    /// behind. Scoping the key means a format switch is a clean remove-and-add, which is what it
    /// actually is.
    pub fn new(
        scope: impl Into<String>,
        groups: Vec<Group<E>>,
        on_edit: impl Into<EventHandler<E>>,
    ) -> Self {
        Self {
            groups,
            scope: scope.into(),
            on_edit: on_edit.into(),
        }
    }
}

impl<E: Clone + PartialEq + 'static> Component for OptionList<E> {
    fn render(&self) -> impl IntoElement {
        // The shared form list, so the rhythm between rows is the app's and not one window's.
        let mut list = Form::new();
        for group in &self.groups {
            // Keyed by **scope and** label — see [`OptionList::new`]. The scope is what keeps a
            // row from being paired with a same-named row belonging to another format.
            let key = format!("{}·{}", self.scope, group.label);
            list = list.child(
                OptionGroup {
                    group: group.clone(),
                    on_edit: self.on_edit.clone(),
                    key: DiffKey::None,
                }
                .key(key),
            );
        }
        list
    }
}

/// One labelled group: the uppercase label (+ its hint), then the control under it.
#[derive(PartialEq)]
struct OptionGroup<E: Clone + PartialEq + 'static> {
    group: Group<E>,
    on_edit: EventHandler<E>,
    key: DiffKey,
}

impl<E: Clone + PartialEq + 'static> KeyExt for OptionGroup<E> {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl<E: Clone + PartialEq + 'static> Component for OptionGroup<E> {
    fn render(&self) -> impl IntoElement {
        let on_edit = self.on_edit.clone();
        // One component per shape — see the module doc on why this isn't a helper fn.
        let control: Element = match self.group.control.clone() {
            Control::Seg { options, custom } => SegControl {
                options,
                custom,
                on_edit,
            }
            .into(),
            Control::Toggle { on, edit, hint } => ToggleControl {
                on,
                edit,
                hint,
                on_edit,
            }
            .into(),
            Control::Text(field) => FieldControl {
                field,
                width: TEXT_WIDTH,
                height: FIELD_HEIGHT,
                align: TextAlign::Left,
                on_edit,
            }
            .into(),
            Control::Char(field) => FieldControl {
                field,
                width: CHAR_WIDTH,
                height: FIELD_HEIGHT,
                align: TextAlign::Center,
                on_edit,
            }
            .into(),
            Control::Num {
                value,
                min,
                max,
                make,
            } => NumControl {
                value,
                min,
                max,
                make,
                on_edit,
            }
            .into(),
            Control::Select { options } => SelectControl { options, on_edit }.into(),
            Control::Note(text) => Note::new(text).into(),
        };

        // The label, its hint and the gap under them are the shared form row's — a surface
        // contributes only which control goes in it.
        Row::new(self.group.label.clone())
            .map(self.group.hint, |row, hint| row.hint(hint))
            .child(control)
    }

    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }
}

/// A pill of mutually exclusive values, with the custom field beside it when one is offered.
#[derive(PartialEq)]
struct SegControl<E: Clone + PartialEq + 'static> {
    options: Vec<Choice<E>>,
    custom: Option<TextField<E>>,
    on_edit: EventHandler<E>,
}

impl<E: Clone + PartialEq + 'static> Component for SegControl<E> {
    fn render(&self) -> impl IntoElement {
        // The canvas's form control, not the compact toolbar one: roomier segments, gaps
        // instead of dividers, on the recessed surface.
        let mut pill = SegmentedToggle::new().form();
        for choice in &self.options {
            let edit = choice.edit.clone();
            let on_edit = self.on_edit.clone();
            pill = pill.child(
                ToggleSegment::text(choice.label.clone())
                    .selected(choice.selected)
                    .on_press(move |_| on_edit.call(edit.clone())),
            );
        }

        rect()
            .horizontal()
            .cross_align(Alignment::Center)
            .spacing(8.)
            .child(pill)
            // The box beside a segmented control is built to the **buttons'** height, not the
            // 30px every other field uses: they sit side by side in one row, so a box that is
            // short of its neighbours reads as a mistake whatever the canvas says. Narrow and
            // centred with them — it holds a token like `\N` or a delimiter, not a sentence.
            .maybe_child(self.custom.clone().map(|field| FieldControl {
                field,
                width: CUSTOM_WIDTH,
                height: SegmentedToggle::FORM_SEGMENT_HEIGHT,
                align: TextAlign::Center,
                on_edit: self.on_edit.clone(),
            }))
    }
}

/// A switch — Freya's own, not a hand-rolled track and knob — with the canvas's sentence beside
/// it saying what the current position *means*.
#[derive(PartialEq)]
struct ToggleControl<E: Clone + PartialEq + 'static> {
    on: bool,
    edit: E,
    hint: Option<String>,
    on_edit: EventHandler<E>,
}

impl<E: Clone + PartialEq + 'static> Component for ToggleControl<E> {
    fn render(&self) -> impl IntoElement {
        let edit = self.edit.clone();
        let on_edit = self.on_edit.clone();
        let switch = Switch::new()
            .toggled(self.on)
            .on_toggle(move |_| on_edit.call(edit.clone()));

        // Bare when there is nothing to say, so a caller's own layout lands on the switch
        // itself rather than on a wrapper that hugs it.
        match self.hint.clone() {
            None => switch.into_element(),
            Some(hint) => rect()
                .horizontal()
                .cross_align(Alignment::Center)
                .spacing(12.)
                .child(switch)
                // `Prose` at the ambient colour, which is how the export window sets the
                // sentence beside its own switch. Not the hint tone: that is the eyebrow's
                // register, pitched to recede under a control, and this is a sentence the reader
                // is meant to read — set in it, the row's own explanation is dimmer than the
                // label above it.
                .child(Prose::new(hint))
                .into_element(),
        }
    }
}

/// A free-text field (a delimiter, a quote character, a custom token).
///
/// The box itself is the shared [`ValueField`] — its height, its length cap and its mono dress
/// are the app's. What is left here is the edit buffer, and carrying what is typed out.
#[derive(PartialEq)]
struct FieldControl<E: Clone + PartialEq + 'static> {
    field: TextField<E>,
    width: f32,
    /// Normally [`FIELD_HEIGHT`]; the box beside a segmented control matches those buttons.
    height: f32,
    /// The canvas centres the one- and few-character boxes and left-aligns the wider ones.
    align: TextAlign,
    on_edit: EventHandler<E>,
}

impl<E: Clone + PartialEq + 'static> Component for FieldControl<E> {
    fn render(&self) -> impl IntoElement {
        // `Input` writes its bound state directly (there is no on-change prop), so the buffer
        // is the field's and this effect carries it out. No sync-back effect is needed: the
        // group list is keyed by label, so a format switch unmounts these controls outright and
        // the next mount re-seeds.
        let text = use_state({
            let initial = self.field.value.clone();
            move || initial
        });

        let make = self.field.make;
        let on_edit = self.on_edit.clone();
        use_side_effect(move || {
            // Reported unconditionally: a surface's edit path is idempotent, so a no-op costs
            // nothing — and comparing here against a captured value is precisely the bug this
            // shape replaced (`use_side_effect` builds its closure once, so the capture froze
            // at the first render and typing a field back to its original value wrote nothing).
            // `ValueField` has already trimmed the state to `max_len`, so this reads what the
            // box shows.
            on_edit.call(make.edit(text.read().clone()));
        });

        ValueField::new(text)
            .width(Size::px(self.width))
            .height(Size::px(self.height))
            .max_len(self.field.max_len)
            .align(self.align)
            .placeholder(self.field.placeholder)
    }
}

/// A bounded number — the shared [`NumberField`]. The parse, the clamp and the buffer are the
/// component's; the only thing here is what a new value *means*.
#[derive(PartialEq)]
struct NumControl<E: Clone + PartialEq + 'static> {
    value: u32,
    min: u32,
    max: u32,
    make: Make<u32, E>,
    on_edit: EventHandler<E>,
}

impl<E: Clone + PartialEq + 'static> Component for NumControl<E> {
    fn render(&self) -> impl IntoElement {
        let make = self.make;
        let on_edit = self.on_edit.clone();
        NumberField::new(self.value, self.min, self.max)
            .width(Size::px(NUM_WIDTH))
            .on_change(move |value: u32| on_edit.call(make.edit(value)))
    }
}

/// A dropdown — the app-standard `Select`, never a hand-rolled lookalike.
#[derive(PartialEq)]
struct SelectControl<E: Clone + PartialEq + 'static> {
    options: Vec<Choice<E>>,
    on_edit: EventHandler<E>,
}

impl<E: Clone + PartialEq + 'static> Component for SelectControl<E> {
    fn render(&self) -> impl IntoElement {
        let current = self
            .options
            .iter()
            .find(|c| c.selected)
            .map(|c| c.label.clone())
            .unwrap_or_default();

        rect()
            .width(Size::px(SELECT_WIDTH))
            .height(Size::px(SELECT_HEIGHT))
            .child(
                Select::new()
                    .selected_item(MonoValue::new(current))
                    .children(
                        self.options
                            .iter()
                            .map(|choice| {
                                let edit = choice.edit.clone();
                                let on_edit = self.on_edit.clone();
                                MenuItem::new()
                                    .selected(choice.selected)
                                    .on_press(move |_| on_edit.call(edit.clone()))
                                    .child(MonoValue::new(choice.label.clone()))
                                    .into()
                            })
                            .collect::<Vec<Element>>(),
                    ),
            )
    }
}
