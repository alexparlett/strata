//! The **component mapping table** — every component's dress, fixed onto [`Role`]s.
//!
//! This is what replaced the theme files' `components` sections: one static table instead of
//! two divergent per-theme copies. Colour fields are `Preference::Reference`s built through
//! [`role`] — the only constructor, so the table cannot hold a typo'd name — and resolve
//! against the active palette at read time, which is what makes the registrations
//! theme-independent. Layout tokens come off the spacing and radius scale
//! ([`crate::components::metrics`]) — deliberately constants rather than theme fields, because a
//! step does not vary by theme; the editor's and tooltip's type comes off the resolved
//! [`Typography`] so the scale stays the single source (AGENTS.md §3).
//!
//! **Built-ins** are a partial retune over the fork's registered default ([`builtin`]): a
//! field the app agrees with is stated nowhere, and resolves through
//! [`bridge_sheet`](super::bridge_sheet). **Strata's own components** are whole-cloth struct
//! literals — the compiler enforces every field, so the omitted-field-paints-magenta class of
//! bug (Daylight's grid header hover) is unrepresentable.

use freya::prelude::*;
use strata_core::theme::{Role, Typography};

use crate::apps::export::ExportThemePreference;
use crate::apps::launcher::LauncherThemePreference;
use crate::apps::project::{
    CancelButtonThemePreference, CatalogThemePreference, CellViewThemePreference,
    ChartThemePreference, ChatThemePreference, CommandPaletteThemePreference,
    ConnectionsThemePreference, DataGridThemePreference, DrawerThemePreference,
    ExplainPlanThemePreference, HeaderBarThemePreference, InspectorThemePreference,
    RecordViewThemePreference, StatusBarThemePreference, TabBarThemePreference, TabThemePreference,
};
use crate::apps::settings::SettingsThemePreference;
use crate::components::avatar::AvatarThemePreference;
use crate::components::form::FormThemePreference;
use crate::components::keycap::KeyCapColorsThemePreference;
use crate::components::metrics::{R_1, R_XS, SP_1, SP_2, SP_3, SP_4};
use crate::components::run_button::RunButtonThemePreference;
use crate::components::segmented_toggle::SegmentedToggleThemePreference;
use crate::components::toggle_button::ToggleButtonThemePreference;
use crate::components::type_palette::TypePaletteThemePreference;
use crate::components::window::WindowThemePreference;
use strata_code_editor::prelude::EditorThemePreference;

/// The one way the table states a colour: a [`Role`] reference, resolved at read time.
fn role(r: Role) -> Preference<Color> {
    Preference::reference(r.name())
}

/// Structural absence — a fill or stroke that is deliberately not painted.
fn clear() -> Preference<Color> {
    Preference::Specific(Color::TRANSPARENT)
}

/// Clone the fork's registered default for `key` and retune only what Strata differs on.
/// Everything unstated resolves through the bridge sheet exactly as the fork authored it.
fn builtin<T: Clone + 'static>(th: &mut Theme, key: &'static str, retune: impl FnOnce(&mut T)) {
    let mut p = th
        .get::<T>(key)
        .cloned()
        .expect("the fork registers every built-in component theme");
    retune(&mut p);
    th.set(key, p);
}

/// Register every component theme the app dresses — Freya built-ins Strata retunes, then
/// Strata's own components. Called once per theme build; `typo` feeds the two components
/// whose type is themed (tooltip, editor).
// Long because it is the table, and the table is the point: AGENTS.md §3 fixes every component's
// dress onto roles in **one** static mapping, so splitting this into `register_buttons` /
// `register_inputs` / … would scatter the one place you go to ask what dresses a component. It has
// no control flow at all — a flat sequence of `builtin::<T>` / `custom` registrations, cognitive
// complexity zero — so length here measures the component count, not tangle.
#[allow(clippy::too_many_lines)]
pub(super) fn register_component_themes(th: &mut Theme, typo: &Typography) {
    // ---- Freya built-ins: partial retunes ---------------------------------------------------

    builtin::<ButtonColorsThemePreference>(th, "button", |p| {
        p.hover_background = role(Role::ElementHover);
        p.border_fill = role(Role::BorderControl);
        p.hover_border_fill = role(Role::Accent);
        p.focus_border_fill = role(Role::Accent);
        p.color = role(Role::TextControl);
        p.hover_color = role(Role::Accent);
    });
    // `filled_button` needs no retune at all: the fork's accent dress resolves through the
    // bridge (primary → accent, tertiary → accent.hover, secondary → accent.ring).
    builtin::<ButtonColorsThemePreference>(th, "outline_button", |p| {
        p.background = role(Role::GhostElementBackground);
        p.hover_background = role(Role::GhostElementHover);
        p.border_fill = role(Role::BorderControl);
        p.focus_border_fill = role(Role::Accent);
        p.color = role(Role::TextMuted);
    });
    builtin::<ButtonColorsThemePreference>(th, "flat_button", |p| {
        p.hover_background = role(Role::ElementHover);
        p.focus_border_fill = role(Role::Accent);
        p.color = role(Role::TextDim);
        p.hover_color = role(Role::Accent);
    });
    builtin::<CardColorsThemePreference>(th, "filled_card", |p| {
        p.background = role(Role::ElementBackground);
        p.hover_background = role(Role::ElevatedElementHover);
        p.color = role(Role::Text);
    });
    builtin::<CardColorsThemePreference>(th, "outline_card", |p| {
        p.background = role(Role::SurfaceBackground);
        p.shadow = clear();
    });
    // A field's four states, on all three variants: the box never changes fill (the canvas's
    // field keeps `--c-panel` throughout), hover brightens the outline, focus takes it to the
    // accent, and the keyboard adds the accent wash ring outside it.
    builtin::<InputColorsThemePreference>(th, "input", |p| {
        p.background = role(Role::SurfaceBackground);
        p.hover_background = role(Role::SurfaceBackground);
        p.focus_background = role(Role::SurfaceBackground);
        p.placeholder_color = role(Role::TextPlaceholder);
        p.border_fill = role(Role::BorderControl);
        p.hover_border_fill = role(Role::BorderStrong);
        p.focus_border_fill = role(Role::BorderFocused);
        p.focus_ring_fill = role(Role::AccentMuted);
    });
    builtin::<InputColorsThemePreference>(th, "filled_input", |p| {
        p.background = role(Role::SurfaceBackground);
        p.hover_background = role(Role::SurfaceBackground);
        p.focus_background = role(Role::SurfaceBackground);
        p.color = role(Role::Text);
        p.placeholder_color = role(Role::TextPlaceholder);
        p.border_fill = role(Role::BorderControl);
        p.hover_border_fill = role(Role::BorderStrong);
        p.focus_border_fill = role(Role::BorderFocused);
        p.focus_ring_fill = role(Role::AccentMuted);
    });
    // The flat field carries no outline, and every one in Strata sits inside a box that does
    // (the palette's search row, a rename slot, the composer's bar) — so the hover belongs to
    // that box and this field declines it rather than growing a fill the canvas never draws.
    builtin::<InputColorsThemePreference>(th, "flat_input", |p| {
        p.background = role(Role::SurfaceBackground);
        p.hover_background = role(Role::SurfaceBackground);
        p.focus_background = role(Role::SurfaceBackground);
        p.placeholder_color = role(Role::TextPlaceholder);
        p.hover_border_fill = clear();
        p.focus_border_fill = role(Role::BorderFocused);
        p.focus_ring_fill = role(Role::AccentMuted);
    });
    builtin::<SwitchColorsThemePreference>(th, "switch", |p| {
        p.background = role(Role::BorderOverlay);
        p.thumb_background = role(Role::Knob);
        p.toggled_background = role(Role::Accent);
        p.toggled_thumb_background = role(Role::Knob);
    });
    builtin::<SwitchLayoutThemePreference>(th, "switch_layout", |p| {
        p.width = Preference::Specific(34.);
        p.height = Preference::Specific(19.);
        p.padding = Preference::Specific(0.);
        p.thumb_size = Preference::Specific(15.);
        p.toggled_thumb_size = Preference::Specific(15.);
        p.pressed_thumb_size_offset = Preference::Specific(0.);
        p.thumb_offset = Preference::Specific(2.);
        p.toggled_thumb_offset = Preference::Specific(17.);
    });
    builtin::<CheckboxThemePreference>(th, "checkbox", |p| {
        p.unselected_border_fill = role(Role::BorderOverlay);
        p.selected_icon_fill = role(Role::Knob);
        p.hover_border_fill = role(Role::BorderStrong);
        p.focus_border_fill = role(Role::BorderFocused);
    });
    builtin::<RadioItemThemePreference>(th, "radio", |p| {
        p.unselected_fill = role(Role::ElementBackground);
        p.border_fill = role(Role::BorderOverlay);
    });
    builtin::<SelectThemePreference>(th, "select", |p| {
        p.select_background = role(Role::ElementBackground);
        p.background_button = role(Role::SurfaceBackground);
        p.hover_background = role(Role::ElevatedElementHover);
        p.border_fill = role(Role::BorderControl);
        p.arrow_fill = role(Role::TextDim);
        p.list_margin = Preference::Specific(SP_1);
    });
    builtin::<MenuContainerThemePreference>(th, "menu_container", |p| {
        p.background = role(Role::ElevatedSurface);
        p.border_fill = role(Role::BorderOverlay);
    });
    builtin::<MenuItemThemePreference>(th, "menu_item", |p| {
        p.background = role(Role::GhostElementBackground);
        p.hover_background = role(Role::ElevatedElementHover);
        p.select_background = role(Role::ElementSelected);
        p.select_border_fill = clear();
    });
    builtin::<PopupThemePreference>(th, "popup", |p| {
        p.background = role(Role::ElevatedSurface);
    });
    builtin::<TooltipThemePreference>(th, "tooltip", |p| {
        p.background = role(Role::ElevatedSurface);
        p.color = role(Role::TextMuted);
        p.border_fill = role(Role::BorderOverlay);
        // The tooltip's type is the scale's `body` role, never its own font.
        p.font_family = Preference::Specific(typo.body.family.clone());
        p.font_size = Preference::Specific(typo.body.size);
        p.font_weight = Preference::Specific(typo.body.weight);
    });
    builtin::<FloatingTabThemePreference>(th, "floating_tab", |p| {
        p.background = role(Role::GhostElementBackground);
        p.hover_background = role(Role::GhostElementHover);
        p.selected_background = role(Role::GhostElementSelected);
        p.color = role(Role::TextMuted);
        p.padding = Preference::Specific(Gaps::new(SP_2, SP_3, SP_2, SP_3));
        p.corner_radius = Preference::Specific(CornerRadius::new_all(R_XS));
    });
    builtin::<SegmentedButtonThemePreference>(th, "segmented_button", |p| {
        p.border_fill = role(Role::BorderControl);
    });
    builtin::<ButtonSegmentThemePreference>(th, "button_segment", |p| {
        p.background = role(Role::GhostElementBackground);
        p.hover_background = role(Role::GhostElementHover);
        p.disabled_background = clear();
        p.selected_background = role(Role::GhostElementSelected);
        p.focus_background = role(Role::GhostElementHover);
        p.color = role(Role::TextMuted);
    });
    builtin::<ChipThemePreference>(th, "chip", |p| {
        p.border_fill = role(Role::BorderControl);
        p.focus_border_fill = role(Role::Accent);
        p.color = role(Role::TextMuted);
        p.selected_icon_fill = role(Role::TextOnAccent);
        p.hover_icon_fill = role(Role::TextOnAccent);
    });
    builtin::<SideBarItemThemePreference>(th, "sidebar_item", |p| {
        p.color = role(Role::TextMuted);
        p.background = role(Role::GhostElementBackground);
        p.active_background = role(Role::GhostElementSelected);
        p.hover_background = role(Role::GhostElementHover);
    });
    builtin::<AccordionThemePreference>(th, "accordion", |p| {
        p.background = role(Role::SurfaceBackground);
    });
    builtin::<ScrollBarThemePreference>(th, "scrollbar", |p| {
        p.background = role(Role::ScrollbarTrack);
        p.thumb_background = role(Role::ScrollbarThumb);
        p.hover_thumb_background = role(Role::ScrollbarThumbHover);
        p.active_thumb_background = role(Role::ScrollbarThumbActive);
    });
    builtin::<ProgressBarThemePreference>(th, "progressbar", |p| {
        p.color = role(Role::Accent);
        p.background = role(Role::Track);
    });
    builtin::<CircularLoaderThemePreference>(th, "circular_loader", |p| {
        p.primary_color = role(Role::Accent);
    });
    builtin::<SkeletonThemePreference>(th, "skeleton", |p| {
        p.background = role(Role::GhostElementHover);
    });
    builtin::<ResizableHandleThemePreference>(th, "resizable_handle", |p| {
        p.background = role(Role::Border);
        p.hover_background = role(Role::Accent);
    });
    builtin::<SliderThemePreference>(th, "slider", |p| {
        p.background = role(Role::Track);
        p.thumb_background = role(Role::Accent);
        p.thumb_inner_background = role(Role::TextOnAccent);
        p.border_fill = role(Role::BorderControl);
    });
    builtin::<ColorPickerThemePreference>(th, "color_picker", |p| {
        p.background = role(Role::ElevatedSurface);
        p.border_fill = role(Role::BorderControl);
    });
    builtin::<TableThemePreference>(th, "table", |p| {
        p.background = role(Role::SurfaceBackground);
        p.arrow_fill = role(Role::TextDim);
        p.hover_row_background = role(Role::ListHover);
        p.divider_fill = role(Role::BorderVariant);
        p.border_fill = role(Role::BorderControl);
        p.color = role(Role::TextMuted);
    });
    builtin::<TreeThemePreference>(th, "tree", |p| {
        p.background = clear();
        p.arrow_fill = role(Role::TextDim);
        p.hover_item_background = clear();
        p.selected_item_background = clear();
        p.selected_item_color = role(Role::Text);
        p.guide_fill = role(Role::BorderVariant);
    });

    // ---- Strata's own components: whole-cloth ----------------------------------------------

    // The shared categorical type ramp every dtype-showing surface reads (see
    // `components::type_palette`).
    th.set(
        "type_palette",
        TypePaletteThemePreference {
            str_color: role(Role::DataTypeString),
            num_color: role(Role::DataTypeNumber),
            bool_color: role(Role::DataTypeBoolean),
            ts_color: role(Role::DataTypeTimestamp),
            struct_color: role(Role::DataTypeStruct),
            list_color: role(Role::DataTypeList),
            map_color: role(Role::DataTypeMap),
        },
    );
    // A key cap looks like a cap on both windows that draw one (see `components::keycap`).
    th.set(
        "keycap",
        KeyCapColorsThemePreference {
            background: role(Role::SurfaceBackground),
            border_fill: role(Role::BorderOverlay),
            color: role(Role::TextMuted),
        },
    );
    th.set(
        "header_bar",
        HeaderBarThemePreference {
            background: role(Role::TitleBarBackground),
            color: role(Role::Text),
            border_fill: role(Role::Border),
        },
    );
    th.set(
        "launcher",
        LauncherThemePreference {
            background: role(Role::SurfaceRaised),
            rail_background: role(Role::ElevatedSurface),
            border_fill: role(Role::Border),
            title_color: role(Role::TextControl),
            label_color: role(Role::TextLabel),
            nav_background: role(Role::AccentSelection),
            row_hover_background: role(Role::GhostElementHover),
            remove_hover_background: role(Role::ErrorBackground),
        },
    );
    th.set(
        "settings",
        SettingsThemePreference {
            background: role(Role::SurfaceRaised),
            nav_background: role(Role::ElevatedSurface),
            border_fill: role(Role::Border),
            icon_color: role(Role::Accent),
            icon_background: role(Role::AccentBadge),
            group_color: role(Role::TextMuted),
            chevron_color: role(Role::TextPlaceholder),
            item_color: role(Role::TextDim),
            item_active_background: role(Role::AccentSelection),
            item_active_color: role(Role::Text),
            hint_color: role(Role::TextPlaceholder),
            card_background: role(Role::SurfaceBackground),
            card_border_fill: role(Role::BorderControl),
            card_hover_border_fill: role(Role::BorderStrong),
            card_divider_fill: role(Role::BorderVariant),
            selected_color: role(Role::Accent),
            badge_builtin_color: role(Role::TextDim),
            badge_user_color: role(Role::EntityQuery),
            table_head_background: role(Role::SurfaceRaised),
            table_selection_background: role(Role::AccentSelection),
            slot_border_fill: role(Role::BorderStrong),
            mark_background: role(Role::ElevatedSurface),
            mark_color: role(Role::TextDisabled),
        },
    );
    th.set(
        "export",
        ExportThemePreference {
            background: role(Role::ElevatedSurface),
            panel_background: role(Role::SurfaceBackground),
            header_background: role(Role::SurfaceRaised),
            border_fill: role(Role::Border),
            divider_fill: role(Role::BorderVariant),
            control_border_fill: role(Role::BorderControl),
            icon_color: role(Role::Accent),
            icon_background: role(Role::AccentBadge),
            label_color: role(Role::TextLabel),
            hint_color: role(Role::TextPlaceholder),
            empty_color: role(Role::TextDim),
            card_color: role(Role::Text),
            card_hover_border_fill: role(Role::BorderStrong),
            card_active_background: role(Role::AccentSelection),
            card_active_border_fill: role(Role::Accent),
            badge_background: role(Role::Accent),
            badge_color: role(Role::TextOnAccent),
            warning_background: role(Role::WarningBackground),
            warning_border_fill: role(Role::WarningBorder),
        },
    );
    th.set(
        "avatar",
        AvatarThemePreference {
            background: role(Role::Success),
            color: role(Role::TextOnAccent),
            active_background: role(Role::Accent),
            active_color: role(Role::TextOnAccent),
            corner_radius: Preference::Specific(CornerRadius::new_all(R_1)),
        },
    );
    // The editor's chrome; its type is the scale's `code_block` role, resolved here so the
    // scale stays the single source. Syntax colours register separately, from the theme
    // file's own `syntax` section (see `strata_theme`).
    th.set(
        "code_editor",
        EditorThemePreference {
            background: role(Role::EditorBackground),
            gutter_selected: role(Role::EditorActiveLineNumber),
            gutter_unselected: role(Role::EditorLineNumber),
            gutter_border: role(Role::Border),
            line_selected_background: clear(),
            cursor: role(Role::EditorCursor),
            highlight: role(Role::EditorSelection),
            text: role(Role::Text),
            whitespace: role(Role::TextPlaceholder),
            diagnostic_error: role(Role::Error),
            diagnostic_warning: role(Role::Warning),
            diagnostic_info: role(Role::Info),
            panel_background: role(Role::SurfaceRaised),
            panel_border: role(Role::Border),
            completion_background: role(Role::ElevatedSurface),
            completion_border: role(Role::BorderOverlay),
            completion_selected_background: role(Role::ElevatedElementHover),
            completion_detail: role(Role::TextMuted),
            completion_kind_table: role(Role::EntityTable),
            completion_kind_view: role(Role::EntityView),
            completion_kind_column: role(Role::EntityColumn),
            completion_kind_function: role(Role::EntityFunction),
            completion_kind_keyword: role(Role::EntityKeyword),
            font_family: Preference::Specific(typo.code_block.family.clone()),
            font_size: Preference::Specific(typo.code_block.size),
            font_weight: Preference::Specific(typo.code_block.weight),
            line_height: Preference::Specific(typo.code_block.line_height.unwrap_or(1.4)),
        },
    );
    th.set(
        "run_button",
        RunButtonThemePreference {
            background: role(Role::Accent),
            hover_background: role(Role::AccentHover),
            color: role(Role::TextOnAccent),
            disabled_background: role(Role::ElementDisabled),
            disabled_hover_background: role(Role::ElementDisabled),
            disabled_color: role(Role::TextLabel),
            running_background: role(Role::ErrorBackground),
            running_hover_background: role(Role::ErrorBackgroundHover),
            running_color: role(Role::Error),
            focus_border_fill: role(Role::BorderFocused),
        },
    );
    // Tracks `run_button`'s running dress — the same cancel meaning, one set of roles.
    th.set(
        "cancel_button",
        CancelButtonThemePreference {
            background: role(Role::ErrorBackground),
            hover_background: role(Role::ErrorBackgroundHover),
            border_fill: role(Role::ErrorBorder),
            color: role(Role::Error),
        },
    );
    th.set(
        "window",
        WindowThemePreference {
            background: role(Role::ElevatedSurface),
            panel_background: role(Role::SurfaceBackground),
            border_fill: role(Role::Border),
            row_selected_background: role(Role::AccentSelection),
            icon_color: role(Role::Accent),
            icon_background: role(Role::AccentBadge),
        },
    );
    th.set(
        "form",
        FormThemePreference {
            title_color: role(Role::Text),
            label_color: role(Role::TextLabel),
            hint_color: role(Role::TextPlaceholder),
            required_color: role(Role::TextPlaceholder),
            divider_fill: role(Role::Border),
            note_background: role(Role::SurfaceBackground),
            note_border_fill: role(Role::BorderControl),
            note_color: role(Role::TextDim),
            reveal_background: role(Role::AccentMuted),
        },
    );
    th.set(
        "segmented_toggle",
        SegmentedToggleThemePreference {
            background: role(Role::SurfaceRaised),
            form_background: role(Role::SurfaceBackground),
            border_fill: role(Role::BorderControl),
            divider_fill: role(Role::BorderControl),
            item_color: role(Role::TextControl),
            // Translucent by authorship, which is what a wash painted over a toolbar pill, a
            // form pill and a raised strip alike has to be.
            item_hover_background: role(Role::ElevatedElementHover),
            item_active_background: role(Role::AccentSelection),
            item_active_color: role(Role::Accent),
            item_focus_border_fill: role(Role::BorderFocused),
        },
    );
    th.set(
        "toggle_button",
        ToggleButtonThemePreference {
            background: role(Role::GhostElementBackground),
            color: role(Role::TextDim),
            // As above: a toggle sits on the rail, a toolbar and a header, so its wash is the
            // translucent one rather than a fill authored for one tier.
            hover_background: role(Role::ElevatedElementHover),
            hover_color: role(Role::Text),
            active_background: role(Role::AccentMuted),
            active_color: role(Role::Accent),
            focus_border_fill: role(Role::BorderFocused),
        },
    );
    th.set(
        "tab_bar",
        TabBarThemePreference {
            background: role(Role::TabBarBackground),
            divider_fill: role(Role::Border),
        },
    );
    th.set(
        "tab",
        TabThemePreference {
            background: role(Role::GhostElementBackground),
            hover_background: role(Role::GhostElementHover),
            // The active tab reveals the app base coat, seating it over the editor pane.
            active_background: role(Role::Background),
            color: role(Role::TextDim),
            active_color: role(Role::Text),
            accent: role(Role::Accent),
        },
    );
    th.set(
        "status_bar",
        StatusBarThemePreference {
            background: role(Role::StatusBarBackground),
            color: role(Role::TextDim),
            border_fill: role(Role::Border),
            sub_color: role(Role::TextPlaceholder),
            control_color: role(Role::TextControl),
        },
    );
    th.set(
        "explain_plan",
        ExplainPlanThemePreference {
            background: role(Role::SurfaceSunken),
            card_background: role(Role::SurfaceRaised),
            border_fill: role(Role::BorderVariant),
            group_background: role(Role::Background),
            insight_background: role(Role::SurfaceSubtle),
            color: role(Role::TextDim),
            value_color: role(Role::Text),
            key_color: role(Role::TextDisabled),
            muted_color: role(Role::TextPlaceholder),
            raw_color: role(Role::TextMuted),
            hot_color: role(Role::Warning),
            // "Warm" borrows the timestamp hue — value-exact with the old accent_tan.
            warm_color: role(Role::DataTypeTimestamp),
        },
    );
    th.set(
        "cell_view",
        CellViewThemePreference {
            backdrop: role(Role::Backdrop),
            background: role(Role::ElevatedSurface),
            border_fill: role(Role::BorderOverlay),
            divider_fill: role(Role::Border),
            name_color: role(Role::Text),
            badge_color: role(Role::Accent),
            badge_background: role(Role::AccentBadge),
            close_color: role(Role::TextPlaceholder),
            close_hover_background: role(Role::ElementHover),
            close_hover_color: role(Role::Text),
            body_background: role(Role::SurfaceBackground),
            body_color: role(Role::TextMuted),
        },
    );
    th.set(
        "record_view",
        RecordViewThemePreference {
            backdrop: role(Role::Backdrop),
            background: role(Role::ElevatedSurface),
            border_fill: role(Role::BorderOverlay),
            divider_fill: role(Role::Border),
            row_divider_fill: role(Role::BorderVariant),
            label_color: role(Role::Text),
            name_color: role(Role::Text),
            value_color: role(Role::TextMuted),
            null_color: role(Role::TextPlaceholder),
            nested_background: role(Role::SurfaceBackground),
            nested_color: role(Role::TextMuted),
        },
    );
    th.set(
        "catalog",
        CatalogThemePreference {
            label_color: role(Role::TextLabel),
            chevron_color: role(Role::TextPlaceholder),
            name_color: role(Role::Text),
            column_color: role(Role::TextMuted),
            meta_color: role(Role::TextDisabled),
            rail_fill: role(Role::Border),
            table_color: role(Role::EntityTable),
            internal_color: role(Role::EntityTableInternal),
            view_color: role(Role::EntityView),
            query_color: role(Role::EntityQuery),
            part_color: role(Role::Accent),
            part_background: role(Role::AccentBadge),
            warn_color: role(Role::Warning),
        },
    );
    th.set(
        "chat",
        ChatThemePreference {
            background: role(Role::SurfaceBackground),
            border_fill: role(Role::Border),
            title_color: role(Role::Text),
            role_color: role(Role::TextDim),
            meta_color: role(Role::TextPlaceholder),
            figures_color: role(Role::TextDim),
            card_background: role(Role::SurfaceRaised),
            card_border_fill: role(Role::BorderVariant),
            sql_color: role(Role::TextMuted),
            chip_background: role(Role::AccentBadge),
            chip_color: role(Role::TextAccent),
            row_hover_fill: role(Role::GhostElementHover),
        },
    );
    // The fork's own markdown viewer, tuned to the transcript's scale (AGENTS.md §3 — a
    // built-in is a partial retune, never a lookalike). Its defaults reference the sheet slots
    // `bridge_sheet` already fills, so what is set here is what the chat needs differently: the
    // pane's own prose size, and a code block that reads as the app's code rather than as a
    // generic grey box.
    th.set(
        "markdown_viewer",
        MarkdownViewerThemePreference {
            color: role(Role::Text),
            color_link: role(Role::TextAccent),
            background_code: role(Role::EditorBackground),
            color_code: role(Role::TextMuted),
            background_blockquote: role(Role::SurfaceSubtle),
            border_blockquote: role(Role::BorderVariant),
            background_divider: role(Role::Border),
            heading_h1: Preference::Specific(16.0),
            heading_h2: Preference::Specific(15.0),
            heading_h3: Preference::Specific(14.0),
            heading_h4: Preference::Specific(13.0),
            heading_h5: Preference::Specific(12.5),
            heading_h6: Preference::Specific(12.5),
            paragraph_size: Preference::Specific(12.5),
            code_font_size: Preference::Specific(11.0),
            table_font_size: Preference::Specific(11.5),
        },
    );
    th.set(
        "connections",
        ConnectionsThemePreference {
            provider_color: role(Role::Accent),
            bucket_color: role(Role::Text),
            hint_color: role(Role::TextDisabled),
            empty_background: role(Role::SurfaceBackground),
            empty_border_fill: role(Role::BorderControl),
            empty_color: role(Role::TextPlaceholder),
        },
    );
    th.set(
        "inspector",
        InspectorThemePreference {
            background: role(Role::PanelBackground),
            label_color: role(Role::TextLabel),
            name_color: role(Role::Text),
            value_color: role(Role::Text),
            field_color: role(Role::TextMuted),
            meta_color: role(Role::TextPlaceholder),
            note_color: role(Role::TextDim),
            border_fill: role(Role::Border),
            divider_fill: role(Role::BorderVariant),
            box_background: role(Role::ElementBackground),
            field_background: role(Role::SurfaceBackground),
            emphasis_color: role(Role::TextAccent),
            fill_color: role(Role::Accent),
            null_color: role(Role::TextDisabled),
            tile_color: role(Role::Accent),
            format_parquet_color: role(Role::FormatParquet),
            format_csv_color: role(Role::FormatCsv),
            format_json_color: role(Role::FormatJson),
            format_arrow_color: role(Role::FormatArrow),
            format_view_color: role(Role::FormatView),
        },
    );
    th.set(
        "drawer",
        DrawerThemePreference {
            background: role(Role::PanelBackground),
            border_fill: role(Role::Border),
            label_color: role(Role::TextMuted),
            group_icon_color: role(Role::TextPlaceholder),
            group_color: role(Role::TextMuted),
            meta_color: role(Role::TextDisabled),
            value_color: role(Role::TextDim),
            message_color: role(Role::TextMuted),
            row_hover_fill: role(Role::GhostElementHover),
            divider_fill: role(Role::BorderVariant),
            empty_color: role(Role::TextDim),
        },
    );
    th.set(
        "command_palette",
        CommandPaletteThemePreference {
            background: role(Role::ElevatedSurface),
            border_fill: role(Role::BorderOverlay),
            backdrop: role(Role::Backdrop),
            label_color: role(Role::TextLabel),
            row_active_background: role(Role::AccentSelection),
            row_active_color: role(Role::Text),
            row_color: role(Role::TextMuted),
            icon_color: role(Role::TextDisabled),
            sub_color: role(Role::TextDisabled),
            esc_color: role(Role::TextDisabled),
            shadow: role(Role::Shadow),
        },
    );
    th.set(
        "chart",
        ChartThemePreference {
            background: role(Role::SurfaceRaised),
            panel_background: role(Role::SurfaceBackground),
            border_fill: role(Role::Border),
            label_color: role(Role::TextLabel),
            tile_color: role(Role::TextControl),
            tile_border_fill: role(Role::BorderControl),
            tile_hover_border_fill: role(Role::BorderStrong),
            tile_active_background: role(Role::AccentSelection),
            tile_active_border_fill: role(Role::Accent),
            tile_active_color: role(Role::Accent),
            grid_fill: role(Role::BorderControl),
            axis_fill: role(Role::Border),
            tick_color: role(Role::TextPlaceholder),
            legend_color: role(Role::TextMuted),
            note_color: role(Role::TextDim),
            // The high-cardinality banner's box, on the same two roles the Export window's
            // banner takes — one warning tone app-wide, not a second.
            warning_background: role(Role::WarningBackground),
            warning_border_fill: role(Role::WarningBorder),
            series_1: role(Role::Chart1),
            series_2: role(Role::Chart2),
            series_3: role(Role::Chart3),
            series_4: role(Role::Chart4),
            series_5: role(Role::Chart5),
            series_6: role(Role::Chart6),
            series_7: role(Role::Chart7),
            series_8: role(Role::Chart8),
            series_9: role(Role::Chart9),
            series_10: role(Role::Chart10),
            heat_low: role(Role::ChartHeatLow),
            heat_high: role(Role::ChartHeatHigh),
        },
    );
    th.set(
        "datagrid",
        DataGridThemePreference {
            background: role(Role::SurfaceBackground),
            arrow_fill: role(Role::TextDim),
            row_background: clear(),
            zebra_row_background: role(Role::SurfaceStripe),
            cell_hover_background: role(Role::ListHover),
            selection_border_fill: role(Role::Accent),
            gutter_color: role(Role::TextDisabled),
            gutter_active_background: role(Role::ElementSelected),
            gutter_active_color: role(Role::Accent),
            header_background: role(Role::ElementBackground),
            header_hover_background: role(Role::GhostElementHover),
            header_color: role(Role::Text),
            header_label_color: role(Role::TextLabel),
            header_active_background: role(Role::ElementSelected),
            header_active_color: role(Role::Accent),
            divider_fill: role(Role::BorderVariant),
            column_divider_fill: role(Role::Border),
            header_divider_fill: role(Role::BorderControl),
            cell_num_color: role(Role::DataTypeNumber),
            cell_ts_color: role(Role::DataTypeTimestamp),
            color: role(Role::TextMuted),
            comfortable_cell_padding: Preference::Specific(Gaps::new(SP_3, SP_4, SP_3, SP_4)),
            compact_cell_padding: Preference::Specific(Gaps::new(SP_2, SP_4, SP_2, SP_4)),
        },
    );
}
