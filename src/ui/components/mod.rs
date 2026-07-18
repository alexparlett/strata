//! Reusable UI component library — the shared building blocks the app is composed
//! from, independent of any one feature or store. Each component owns its own
//! look and behaviour; callers hand in content (`children`) and callbacks.
//!
//! The **overlay** family (A3, egui-style — see `docs/OVERLAY_ARCHITECTURE.md`). The base
//! is [`Popup`]: a dumb fixed-position card. Everything composes it:
//! - **menu / dropdown** = [`Backdrop`] `{ Popup { … } }` (backdrop owns dismiss + Esc + focus);
//! - **hover tooltip** = [`Tooltip`]: wraps a trigger and, on hover, shows a `Popup` with the
//!   pointer-transparent `ds-float` class (a point-pinned tooltip card is a raw `Popup`).
//!
//! [`Dialog`] (centred, scrimmed) + [`Window`] (non-modal floating panel) are the other
//! containers; [`MenuItem`] / [`MenuSep`] are the shared menu rows.
//!
//! On the base (S29, design system — see `docs/DESIGN_SYSTEM.md`): [`Select`] (single-
//! select dropdown, trigger + `.ds-menu` card) and [`ContextMenu`] (right-click menu),
//! both `Backdrop { Popup }` internally. The S27 lint hover is a point-pinned `Popup`
//! (neutral `.ds-tooltip` chrome + `ds-float` + a red icon).
//!
//! The **form-control** family (S28, design system §03/§04/§06): [`Button`] /
//! [`IconButton`], [`TextInput`] / [`NumberStepper`], [`Segment`] (single-select
//! multi-button, with [`SegmentOption`]), [`Toggle`], [`RadioGroup`] (with [`Radio`] /
//! [`RadioOption`]), and the pre-existing [`Checkbox`]. Plus the non-input design-system
//! pieces: [`Text`] + the per-role text components ([`Title`], [`Body`], [`Strong`],
//! [`Control`], [`Caption`], [`MonoValue`], [`Readout`], [`Eyebrow`], [`Meta`], [`Path`],
//! [`Micro`], [`Hero`], [`Metric`], [`Code`] — §02 ramp), [`Badge`] + [`StatusDot`]
//! (§07 pills + state dot), and the icon library (`crate::ui::icons`, with
//! `icons::catalog()` enumerating every glyph). All are controlled + stateless and render
//! additive `.ds-*` classes (`.ds-btn`, `.ds-field`, `.ds-seg`, `.ds-txt-*`, `.ds-badge`,
//! …) that sit alongside the app's legacy ad-hoc classes; call sites migrate later.

mod badge;
mod button;
mod callout;
mod checkbox;
mod context_menu;
mod dialog;
mod dropdown_menu;
mod form;
mod icon;
mod menu;
mod pager;
mod popup;
mod radio;
mod search_dialog;
mod segmented;
mod select;
mod spacer;
mod split_button;
mod text_input;
mod toggle;
mod tooltip;
mod typography;
mod window;
pub mod code_editor;

pub use badge::{Badge, BadgeVariant, Dot, DotStatus, StatusDot};
pub use button::{Button, ButtonVariant, IconButton, IconButtonVariant};
pub use callout::{Callout, CalloutVariant};
pub use checkbox::Checkbox;
pub use context_menu::ContextMenu;
pub use dialog::Dialog;
pub use dropdown_menu::DropdownMenu;
pub use form::Form;
pub use icon::Icon;
pub use menu::{MenuItem, MenuSep};
pub use pager::Pager;
pub use popup::{Backdrop, Point, Popup, Rect, RectAlign};
pub use radio::{Radio, RadioGroup, RadioOption};
pub use search_dialog::SearchDialog;
pub use segmented::{Segment, SegmentOption};
pub use select::{Select, SelectOption};
pub use spacer::Spacer;
pub use split_button::SplitButton;
pub use text_input::{Input, NumberStepper, SearchBar, TextInput};
pub use toggle::Toggle;
pub use tooltip::Tooltip;
pub use typography::{
    Body, Caption, Code, Control, Eyebrow, Hero, Meta, Metric, Micro, MonoValue, Path, Prose,
    Readout, Strong, Text, Title,
};
pub use window::{WinGeom, Window};
