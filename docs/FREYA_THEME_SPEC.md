# Strata (Freya) — theme spec

A **native theme format for the Freya frontend**, authored directly against Freya's theming model — no lossy mapping.
Three blocks:

- **`sheet`** → copied 1:1 into Freya's `ColorsSheet` (the 27-slot palette). Every component's
  `Reference("<slot>")` resolves against this at render, so it does most of the work.
- **`components`** → per-component overrides, keyed by **Freya's component key**
  (`"button"`, `"menu_container"`, `"switch"`, …). Each field is a **Preference**: a *Specific*
  value or a *Reference* to a sheet slot. Overrides are **partial** — unspecified fields keep Freya's default. Our own
  components (grid, editor, …) join this map once built with
  `define_theme!`.
- **`fonts`**.

Colours are `#rrggbb`, `#rrggbbaa`, or `rgba(r,g,b,a)`. Field names are `snake_case`.

Midnight/Daylight ship built-in; custom themes load the same shape (roadmap: a plugin theme dir, like any IDE). Every
theme file is validated by **`themes/theme.schema.json`** — reference it via `"$schema": "./theme.schema.json"` for
editor autocomplete + validation.

---

## A — `sheet` (→ Freya `ColorsSheet`, all 27 fields)

Verified complete against the `ColorsSheet` struct. Fill every slot — components reference these, so a gap shows as a
leftover Freya default.

### Brand

`secondary`/`tertiary` are **accent tints**, not separate hues — Freya uses them for filled-control states (`tertiary` =
filled hover; `secondary` = filled focus / switch track / slider thumb). Set them as a lighter and darker `primary`.

| field       | controls                                                       | Midnight  | Daylight  |
|-------------|----------------------------------------------------------------|-----------|-----------|
| `primary`   | filled buttons, toggled thumb, selected marks, progress, links | `#4cc6ff` | `#2b7fd0` |
| `secondary` | lighter accent tint (filled focus, switch track, slider)       | `#a9e2ff` | `#7fbce8` |
| `tertiary`  | darker accent tint (filled hover)                              | `#2ea6e0` | `#1f6bb0` |

### Status

| field     | controls             | Midnight  | Daylight  |
|-----------|----------------------|-----------|-----------|
| `success` | success / valid      | `#9fe6b4` | `#1a7f4b` |
| `warning` | warning              | `#ffa657` | `#bc4c00` |
| `error`   | error / destructive  | `#ff8a8a` | `#c0332e` |
| `info`    | informational accent | `#4cc6ff` | `#2b7fd0` |

### Surfaces (elevation ramp)

| field                       | controls                      | Midnight  | Daylight  |
|-----------------------------|-------------------------------|-----------|-----------|
| `background`                | app base / window body        | `#15181e` | `#eceef1` |
| `surface_primary`           | panels, sidebars              | `#333b47` | `#d3d7de` |
| `surface_secondary`         | universal hover / raised rows | `#38414f` | `#e4e8ee` |
| `surface_tertiary`          | default control background    | `#2a313c` | `#ffffff` |
| `surface_inverse`           | thumbs, unchecked marks       | `#6f7988` | `#aeb4bf` |
| `surface_inverse_secondary` | thumb hover                   | `#8792a2` | `#949ba7` |
| `surface_inverse_tertiary`  | thumb active / unchecked fill | `#a1abbb` | `#7c828e` |

### Borders

| field             | controls            | Midnight  | Daylight  |
|-------------------|---------------------|-----------|-----------|
| `border`          | dividers / outlines | `#363e4a` | `#d0d4db` |
| `border_focus`    | focus ring          | `#4cc6ff` | `#2b7fd0` |
| `border_disabled` | disabled outline    | `#23272f` | `#edeef1` |

### Text
| field              | controls                  | Midnight  | Daylight  |
|--------------------|---------------------------|-----------|-----------|
| `text_primary`     | body text                 | `#edf0f5` | `#1a1c22` |
| `text_secondary`   | labels / placeholders-ish | `#cfd6e0` | `#33373f` |
| `text_placeholder` | placeholders              | `#6f7988` | `#7c828e` |
| `text_inverse`     | text on an accent fill    | `#08111a` | `#ffffff` |
| `text_highlight`   | links / highlight         | `#4cc6ff` | `#2b7fd0` |

### States / Utility

(`ColorsSheet` has no `hover` slot — hover comes from `surface_secondary`.)

| field      | controls              | Midnight               | Daylight               |
|------------|-----------------------|------------------------|------------------------|
| `focus`    | focus background fill | `rgba(76,198,255,.14)` | `rgba(43,127,208,.12)` |
| `active`   | pressed background    | `#414a57`              | `#dfe4ea`              |
| `disabled` | disabled fill/text    | `#4d5765`              | `#a3a9b3`              |
| `overlay`  | modal scrim           | `rgba(0,0,0,.5)`       | `rgba(15,23,42,.4)`    |
| `shadow`   | drop shadow           | `rgba(0,0,0,.45)`      | `rgba(15,23,42,.18)`   |

---

## B — `components` (per-component overrides)

### Preference grammar (per field value)

Each field is a **tagged `Preference`** — an object with exactly one of `specific` / `reference`
(a serde externally-tagged enum, so the discriminator is explicit — no string-vs-object sniffing):

| author writes                                             | means                                                                 |
|-----------------------------------------------------------|-----------------------------------------------------------------------|
| `{ "specific": "#2a313c" }` / `{ "specific": "rgba(…)" }` | `Preference::Specific(Color)`                                         |
| `{ "specific": 14 }`                                      | `Preference::Specific(f32)` (`font_size`, `size`)                     |
| `{ "specific": 8 }`                                       | `Preference::Specific(CornerRadius::new_all(8))` (`corner_radius`)    |
| `{ "specific": 4 }` / `{ "specific": [6,12,6,12] }`       | `Preference::Specific(Gaps)` — all-sides / `[top,right,bottom,left]`  |
| `{ "reference": "surface_tertiary" }`                     | `Preference::Reference("surface_tertiary")` — resolves from the sheet |

The `specific` value's JSON type (string / number / array) is inferred, then coerced to the field's known type (see the
table below). **References are colours-only** — Freya panics on a reference for a number/gaps/radius field. **Overrides
are partial**: only the fields you list change; the rest keep Freya's default (which references the sheet, so still
follows the palette).

### Supported component keys + field types

**Generic across all components.** A single macro in `theme.rs` drives `get → override the
listed fields → set` for *any* component in the registration, so a theme author can override any field of any registered
component (colour fields as `specific` or `reference`; layout fields as `specific`). The registration covers the
built-in Freya set — buttons (+ variants +
`button_layout`), cards, inputs (+ variants + `input_layout`), `switch`(+`switch_layout`),
`checkbox`, `radio`, `select`, `menu_container`, `menu_item`, `popup`, `tooltip`,
`floating_tab`, `segmented_button`, `button_segment`, `chip`, `sidebar_item`, `accordion`,
`scrollbar`, `progressbar`, `circular_loader`, `skeleton`, `resizable_handle`, `slider`,
`color_picker`, `table`, `typography`. Adding another Freya component is **one line** in the registration. Field names +
types come from Freya's `themes.rs`; the schema (`theme.schema.json`) validates authored files. Representative subset:

| key                | colour fields                                                                                                            | layout fields                  |
|--------------------|--------------------------------------------------------------------------------------------------------------------------|--------------------------------|
| `scrollbar`        | background, thumb_background, hover_thumb_background, active_thumb_background                                            | size (f32)                     |
| `switch`           | background, thumb_background, toggled_background, toggled_thumb_background, focus_border_fill                            | —                              |
| `checkbox`         | unselected_fill, selected_fill, selected_icon_fill, border_fill                                                          | —                              |
| `menu_container`   | background, shadow, border_fill                                                                                          | padding (Gaps), corner_radius  |
| `menu_item`        | background, hover_background, select_background, border_fill, select_border_fill, color                                  | corner_radius                  |
| `tooltip`          | background, color, border_fill                                                                                           | font_size (f32)                |
| `table`            | background, arrow_fill, row_background, hover_row_background, divider_fill, color                                        | corner_radius                  |
| `button`           | background, hover_background, border_fill, focus_border_fill, color                                                      | — (layout via `button_layout`) |
| `input`            | background, focus_background, color, placeholder_color, border_fill, focus_border_fill                                   | —                              |
| `select`           | select_background, background_button, hover_background, color, border_fill, focus_border_fill, arrow_fill                | —                              |
| `sidebar_item`     | color, background, active_background, hover_background, focus_border_fill                                                | corner_radius, padding, margin |
| `chip`             | background, hover_background, selected_background, border_fill, focus_border_fill, color, hover_color, selected_color, … | corner_radius, padding         |
| `segmented_button` | background, border_fill                                                                                                  | corner_radius                  |
| `button_segment`   | background, hover_background, disabled_background, selected_background, focus_background, color, selected_icon_fill      | padding, selected_padding      |

Full field lists + Freya's defaults live in `freya-components/src/theming/themes.rs`; the loader mirrors them per key.

### Our own components (future)

The results grid, code editor, status dots, and typography presets will define their
`*ThemePreference` with `define_theme!` (exported at `freya::components::define_theme`), register defaults in the same
pass, and read via `get_theme!`. They then appear as component keys here (`"editor"`, `"grid"`, …) authored
identically — one system for Freya's widgets and ours.

---

## C — `fonts`

| field  | value                                         |
|--------|-----------------------------------------------|
| `ui`   | `IBM Plex Sans` (bundled app-side; name only) |
| `mono` | `JetBrains Mono`                              |

---

## D — file shape

```json
{
  "id": "midnight",
  "name": "Midnight",
  "mode": "dark",
  "sheet": {
    "primary": "#4cc6ff",
    "background": "#15181e",
    "surface_tertiary": "#2a313c",
    "text_primary": "#edf0f5"
  },
  "components": {
    "menu_container": {
      "background": { "specific": "#262c35" },
      "border_fill": { "reference": "border" },
      "shadow": { "specific": "rgba(0,0,0,.45)" },
      "corner_radius": { "specific": 8 }
    },
    "switch": {
      "background": { "specific": "#333d4b" },
      "toggled_background": { "specific": "#2ea6e0" },
      "focus_border_fill": { "reference": "border_focus" }
    },
    "tooltip": {
      "background": { "specific": "#2a313c" },
      "color": { "specific": "#edf0f5" },
      "font_size": { "specific": 14 }
    }
  },
  "fonts": { "ui": "IBM Plex Sans", "mono": "JetBrains Mono" }
}
```

(`sheet` field set is pinned to Freya's `ColorsSheet`; the `components` key/field set is whatever the loader maps —
extended as we adopt more components.)
