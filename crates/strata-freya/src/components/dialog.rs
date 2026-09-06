//! Confirmation card with shared header, body, actions, and modal keyboard behaviour.

use freya::components::Checkbox;
use freya::prelude::*;

use crate::components::divider::Divider;
use crate::components::icon::{Icon, IconName};
use crate::components::metrics::ACTION_HEIGHT;
use crate::components::metrics::{R_2, R_4, SP_2, SP_3, SP_4, SP_5, SP_6};
use crate::components::modal::Modal;
use crate::components::typography::Prose;
use crate::theme::{use_roles, Role};

/// The comps' card width — 420 for every confirm in the design.
const DEFAULT_WIDTH: f32 = 420.;

/// The header chip's box and its glyph. One size for every dialog — see the module doc.
const CHIP: f32 = 28.;
const CHIP_RADIUS: f32 = R_2;
const CHIP_ICON: f32 = 16.;
/// Alpha of the chip's fill, tinted from the dialog's tone (≈13%, the comps' figure).
const CHIP_TINT: u8 = 33;

/// A dialog's header: a `tone`-tinted chip carrying `icon`, beside the title run.
///
/// The tone is the dialog's *character* and the only thing that varies — `warning` for a question
/// about work in flight, `error` for a destructive one — and it colours the glyph and its fill
/// together, so a dialog can't end up with a red icon in an amber chip.
#[derive(PartialEq)]
pub struct DialogHeader {
    icon: IconName,
    tone: Color,
    child: Element,
}

impl DialogHeader {
    /// `child` is the title run: a single `Title`, a stacked title + subtitle, or a `paragraph()`
    /// of mixed spans — whatever the comp calls for. It is given the width beside the chip.
    pub fn new(icon: IconName, tone: Color, child: impl IntoElement) -> Self {
        Self {
            icon,
            tone,
            child: child.into_element(),
        }
    }
}

impl Component for DialogHeader {
    fn render(&self) -> impl IntoElement {
        rect()
            .width(Size::fill())
            .horizontal()
            .content(Content::Flex)
            .cross_align(Alignment::Center)
            .spacing(SP_4)
            .child(
                rect()
                    .width(Size::px(CHIP))
                    .height(Size::px(CHIP))
                    .corner_radius(CHIP_RADIUS)
                    .background(self.tone.with_a(CHIP_TINT))
                    .main_align(Alignment::Center)
                    .cross_align(Alignment::Center)
                    .child(Icon::new(self.icon).color(self.tone).size(CHIP_ICON)),
            )
            .child(
                rect()
                    .width(Size::flex(1.))
                    .vertical()
                    .child(self.child.clone()),
            )
    }
}

/// A labelled checkbox with one keyboard activation target.
#[derive(PartialEq)]
pub struct CheckboxRow {
    label: String,
    selected: bool,
    on_toggle: Option<EventHandler<Event<PressEventData>>>,
    trailing: Option<Element>,
}

impl CheckboxRow {
    pub fn new(label: impl Into<String>, selected: bool) -> Self {
        Self {
            label: label.into(),
            selected,
            on_toggle: None,
            trailing: None,
        }
    }

    pub fn on_toggle(mut self, on_toggle: impl Into<EventHandler<Event<PressEventData>>>) -> Self {
        self.on_toggle = Some(on_toggle.into());
        self
    }

    /// A mark at the row's trailing edge — the schemas picker's "not in the data source" warning.
    pub fn trailing(mut self, trailing: impl IntoElement) -> Self {
        self.trailing = Some(trailing.into_element());
        self
    }
}

impl Component for CheckboxRow {
    fn render(&self) -> impl IntoElement {
        let roles = use_roles();
        rect()
            .width(Size::fill())
            .horizontal()
            .content(Content::Flex)
            .cross_align(Alignment::Center)
            .spacing(SP_3)
            .padding((SP_2, SP_3))
            .corner_radius(R_2)
            .map(self.on_toggle.clone(), |el, handler| {
                el.on_press(move |e: Event<PressEventData>| {
                    e.prevent_default();
                    e.stop_propagation();
                    handler.call(e);
                })
            })
            .child(Checkbox::new().selected(self.selected).size(CHECKBOX))
            .child(
                Prose::new(self.label.clone())
                    .color(roles.get(Role::TextMuted))
                    .width(Size::flex(1.))
                    .text_overflow(TextOverflow::Ellipsis),
            )
            .maybe_child(self.trailing.clone())
    }
}

/// The box in a [`CheckboxRow`].
const CHECKBOX: f32 = 16.;

#[derive(PartialEq)]
pub struct Dialog {
    header: Option<Element>,
    body: Element,
    actions: Vec<Button>,
    on_dismiss: Option<EventHandler<()>>,
    on_confirm: Option<EventHandler<()>>,
    modal: bool,
}

impl Default for Dialog {
    fn default() -> Self {
        Self::new()
    }
}

impl Dialog {
    pub fn new() -> Self {
        Self {
            header: None,
            body: rect().into_element(),
            actions: Vec::new(),
            on_dismiss: None,
            on_confirm: None,
            modal: true,
        }
    }

    /// Whether the dialog's key barrier consumes **every** key (the default, and right for
    /// a confirm raised over live features — see the module doc). Pass `false` for a dialog
    /// that *is* the window's whole content, with nothing behind it to protect: Esc and
    /// Enter keep their dialog meaning, and every other chord stays the window's — which is
    /// what keeps ⌘O and ⌘, alive on the project-load fault, whose menubar items arrive as
    /// synthesized key presses a barrier would swallow.
    pub fn modal(mut self, modal: bool) -> Self {
        self.modal = modal;
        self
    }

    /// The chip-and-title row above the body — normally a [`DialogHeader`].
    pub fn header(mut self, header: impl IntoElement) -> Self {
        self.header = Some(header.into_element());
        self
    }

    /// The card body — full width under the header, already inset by the card's padding. Pass a
    /// `rect()` with whatever direction and spacing the comp calls for.
    ///
    /// Named `body`, not `child`: it **replaces** rather than appends, so borrowing the builder
    /// spelling would make `Dialog::new().child(a).child(b)` silently drop `a`.
    pub fn body(mut self, body: impl IntoElement) -> Self {
        self.body = body.into_element();
        self
    }

    /// Append a button to the action strip, left to right (so the confirming action goes last —
    /// it ends up nearest the corner).
    ///
    /// Takes the `Button` itself, not an `Element`, so the strip can size it: pass the variant
    /// (`.flat()` / `.outline()` / `.filled()`), its colours and its handler, and leave the box to
    /// the dialog.
    pub fn action(mut self, action: Button) -> Self {
        self.actions.push(action);
        self
    }

    /// Esc, or a press on the backdrop.
    pub fn on_dismiss(mut self, on_dismiss: impl Into<EventHandler<()>>) -> Self {
        self.on_dismiss = Some(on_dismiss.into());
        self
    }

    /// Enter. Omit it and Enter is merely swallowed by the barrier, which is right for a dialog
    /// with no single obvious action.
    pub fn on_confirm(mut self, on_confirm: impl Into<EventHandler<()>>) -> Self {
        self.on_confirm = Some(on_confirm.into());
        self
    }
}

impl Component for Dialog {
    fn render(&self) -> impl IntoElement {
        let roles = use_roles();

        let dismiss = self.on_dismiss.clone();
        let confirm = self.on_confirm.clone();

        let strip = (!self.actions.is_empty()).then(|| {
            rect()
                .width(Size::fill())
                .horizontal()
                .main_align(Alignment::End)
                .cross_align(Alignment::Center)
                .spacing(SP_3)
                .padding((SP_4, SP_6))
                .background(roles.get(Role::SurfaceRaised))
                .children(self.actions.iter().map(|action| {
                    let layout = action.get_theme_layout().cloned().unwrap_or_default();
                    let layout = match layout.height {
                        Some(_) => layout,
                        None => layout.height(Size::px(ACTION_HEIGHT)),
                    };
                    action.clone().theme_layout(layout).into_element()
                }))
        });

        let card = rect()
            .width(Size::px(DEFAULT_WIDTH))
            .max_width(Size::window_percent(92.))
            .corner_radius(R_4)
            .background(roles.get(Role::ElevatedSurface))
            .border(Border::new().width(1.).fill(roles.get(Role::Border)))
            .shadow(
                Shadow::new()
                    .y(30.)
                    .blur(80.)
                    .color(roles.get(Role::Shadow)),
            )
            .overflow(Overflow::Clip)
            .a11y_role(AccessibilityRole::Dialog)
            .vertical()
            .child(
                rect()
                    .width(Size::fill())
                    .vertical()
                    .spacing(SP_4)
                    .padding((SP_6, SP_6, SP_5, SP_6))
                    .maybe_child(self.header.clone())
                    .child(self.body.clone()),
            )
            .maybe_child(strip.as_ref().map(|_| {
                Divider::horizontal()
                    .color(roles.get(Role::Border))
                    .into_element()
            }))
            .maybe_child(strip)
            .child(
                rect().on_global_key_down(move |e: Event<KeyboardEventData>| {
                    if matches!(&e.key, Key::Named(NamedKey::Enter)) {
                        if let Some(confirm) = &confirm {
                            confirm.call(());
                        }
                        e.prevent_default();
                    }
                }),
            );

        let mut modal = Modal::new(card).barrier(self.modal);
        if let Some(dismiss) = dismiss {
            modal = modal.on_close_request(dismiss);
        }
        modal
    }
}

#[cfg(test)]
mod focus_tests {
    use super::*;
    use freya_testing::TestingRunner;
    use std::time::Duration;

    type Handles = (
        State<bool>,
        State<String>,
        State<String>,
        State<bool>,
        State<usize>,
    );

    fn app() -> impl IntoElement {
        use_init_theme(|| crate::theme::strata_theme(&strata_core::theme::load("midnight")));
        let (mut open, background, inside, mut checked, mut confirmed) = use_consume::<Handles>();
        rect()
            .expanded()
            .vertical()
            .maybe_child(open().then(|| {
                Dialog::new()
                    .body(
                        rect().vertical().child(Input::new(inside)).child(
                            CheckboxRow::new("Remember choice", checked())
                                .on_toggle(move |_| checked.toggle()),
                        ),
                    )
                    .on_dismiss(move |()| open.set(false))
                    .on_confirm(move |()| confirmed.set(confirmed() + 1))
                    .action(
                        Button::new()
                            .child("Cancel")
                            .on_press(move |_| open.set(false)),
                    )
            }))
            .child(Input::new(background).auto_focus(true))
    }

    fn runner() -> (TestingRunner, Handles) {
        let (mut runner, handles) = TestingRunner::new(
            app,
            (800., 600.).into(),
            |r| {
                r.provide_root_context(|| {
                    (
                        State::create(false),
                        State::create(String::new()),
                        State::create(String::new()),
                        State::create(false),
                        State::create(0usize),
                    )
                })
            },
            1.,
        );
        settle(&mut runner);
        (runner, handles)
    }

    fn settle(runner: &mut TestingRunner) {
        runner.poll_n(Duration::from_millis(10), 5);
    }

    #[test]
    fn dialog_moves_focus_and_restores_it_on_dismissal() {
        let (mut runner, (mut open, background, _, _, _)) = runner();
        runner.write_text("before");
        assert_eq!(&*background.peek(), "before");
        open.set(true);
        settle(&mut runner);
        runner.write_text("X");
        assert_eq!(&*background.peek(), "before");
        runner.press_key(Key::Named(NamedKey::Escape));
        settle(&mut runner);
        assert!(!open());
        runner.write_text("after");
        assert_eq!(&*background.peek(), "beforeafter");
    }

    #[test]
    fn tab_and_shift_tab_stay_inside_the_dialog() {
        let (mut runner, (mut open, background, inside, _, _)) = runner();
        open.set(true);
        settle(&mut runner);
        for modifiers in [Modifiers::empty(), Modifiers::SHIFT] {
            for _ in 0..8 {
                runner.press_key_with_modifiers(Key::Named(NamedKey::Tab), modifiers);
                settle(&mut runner);
                runner.write_text("x");
            }
        }
        assert!(background.peek().is_empty());
        assert!(!inside.peek().is_empty(), "Tab must reach the dialog input");
    }

    #[test]
    fn enter_on_checkbox_toggles_without_confirming() {
        let (mut runner, (mut open, _, _, checked, confirmed)) = runner();
        open.set(true);
        settle(&mut runner);
        runner.press_key(Key::Named(NamedKey::Tab));
        runner.press_key(Key::Named(NamedKey::Tab));
        settle(&mut runner);
        runner.press_key(Key::Named(NamedKey::Enter));
        settle(&mut runner);
        assert!(checked());
        assert_eq!(confirmed(), 0);
    }
}
