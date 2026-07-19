# Strata (Freya) — theme spec

A **native theme format for the Freya frontend**, authored directly against what the app
renders — no lossy mapping from the old CSS-variable tokens. Two blocks:

- **`sheet`** → copied 1:1 into Freya's `ColorsSheet`. This is the **complete 27-field
  `ColorsSheet`** (verified against the struct — Part A covers every field, no more, no less).
  Every built-in Freya component (Button, Input, Select, Switch, Tooltip, Menu, tabs, …)
  resolves its colours from here, so filling every slot is what stops a widget rendering in
  Freya's default blue.
- **`tokens`** → Strata's own colours for our **hand-rolled** components (SQL editor syntax,
  results-grid cells, data-type badges). These carry over from today's theme essentially
  unchanged.
- plus **`fonts`**.

Values below are the **current Midnight/Daylight palette** as a starting point. Cells marked
_proposed_ have no direct source in today's theme (Freya needs the slot but Strata never had
one) — a sensible derived value is given; please confirm/tune. Verification: we render
Freya's component `gallery()` under the theme and adjust until every widget looks right.

Colours are `#rrggbb`, or `#rrggbbaa` / `rgba(r,g,b,a)` where alpha is needed. Field names are
`snake_case`.

---

## A — `sheet` (→ Freya `ColorsSheet`)

> **This sheet does ~all the work.** Verified against Freya's component themes: nearly every
> built-in sets its colours as `Preference::Reference("<sheet field>")`, so filling these 27
> slots themes almost every widget automatically — no per-component colour overrides. That
> also means the fields below marked _proposed_ are **not optional**: `secondary`/`tertiary`
> drive filled-control states, `surface_inverse*` drive scrollbar thumbs + switch/radio,
> `disabled` drives disabled segments, `shadow` drives menu/card shadows. Set them all
> deliberately. (Component *sizing/radius/type* is separate — Freya's defaults, which we
> override per-component to match Strata's spacing scale; not the designer's colour job.)

### Brand
`secondary`/`tertiary` are **accent tints, not separate hues** — Freya uses them for the
*states* of filled controls (`tertiary` = hover of filled buttons/inputs/cards/chips;
`secondary` = filled-control focus borders + switch track + slider thumb). Set them as a
lighter and darker `primary`, not a different colour.

| field | controls / used by | Midnight | Daylight |
|---|---|---|---|
| `primary` | main brand accent — filled buttons, toggled switch thumb, radio/checkbox selected, progress, links | `#4cc6ff` | `#2b7fd0` |
| `secondary` | lighter accent tint — filled-control focus borders, switch track, slider thumb, checkbox tick — _tune_ | `#a9e2ff` | `#7fbce8` |
| `tertiary` | darker accent tint — hover of filled buttons/inputs/cards/chips — _tune_ | `#2ea6e0` | `#1f6bb0` |

### Status
| field | controls / used by | Midnight | Daylight |
|---|---|---|---|
| `success` | success text / badges / valid state | `#9fe6b4` | `#1a7f4b` |
| `warning` | warning text / badges | `#ffa657` | `#bc4c00` |
| `error` | error text / invalid input / destructive | `#ff8a8a` | `#c0332e` |
| `info` | informational accent — _proposed_ | `#4cc6ff` | `#2b7fd0` |

### Surfaces (elevation ramp)
| field | controls / used by | Midnight | Daylight |
|---|---|---|---|
| `background` | app base / window body | `#15181e` | `#eceef1` |
| `surface_primary` | panels, sidebars, cards | `#191d24` | `#f6f7f9` |
| `surface_secondary` | raised surface (inputs, rows) | `#1e232b` | `#ffffff` |
| `surface_tertiary` | popovers, menus, dropdowns | `#2a313c` | `#eef0f4` |
| `surface_inverse` | inverted surface (e.g. a light chip/tooltip on dark UI) — _proposed_ | `#edf0f5` | `#1a1c22` |
| `surface_inverse_secondary` | _proposed_ | `#cfd6e0` | `#33373f` |
| `surface_inverse_tertiary` | _proposed_ | `#b6bfcb` | `#474c56` |

### Borders
| field | controls / used by | Midnight | Daylight |
|---|---|---|---|
| `border` | default dividers / outlines | `#23272f` | `#edeef1` |
| `border_focus` | focused input / focus ring | `#4cc6ff` | `#2b7fd0` |
| `border_disabled` | disabled control outline | `#2c333d` | `#e3e5e9` |

### Text
| field | controls / used by | Midnight | Daylight |
|---|---|---|---|
| `text_primary` | primary body text | `#edf0f5` | `#1a1c22` |
| `text_secondary` | secondary / labels | `#cfd6e0` | `#33373f` |
| `text_placeholder` | placeholders / hints | `#6f7988` | `#7c828e` |
| `text_inverse` | text on an accent/inverse fill | `#08111a` | `#ffffff` |
| `text_highlight` | links / highlighted text — _proposed_ | `#4cc6ff` | `#2b7fd0` |

### States / Interaction
(`ColorsSheet` has **no `hover` slot** — components derive hover from a surface + opacity, or
their per-component theme. Our grid's row-hover is a `tokens.grid` value, below.)

| field | controls / used by | Midnight | Daylight |
|---|---|---|---|
| `focus` | focus background fill — _proposed_ (soft accent) | `rgba(76,198,255,.14)` | `rgba(43,127,208,.12)` |
| `active` | pressed / active background — _proposed_ | `#2a313c` | `#eef0f4` |
| `disabled` | disabled fill / text | `#4d5765` | `#a3a9b3` |

### Utility
| field | controls / used by | Midnight | Daylight |
|---|---|---|---|
| `overlay` | modal scrim / backdrop — _proposed_ | `rgba(0,0,0,.5)` | `rgba(15,23,42,.4)` |
| `shadow` | drop-shadow colour — _proposed_ | `rgba(0,0,0,.45)` | `rgba(15,23,42,.18)` |

---

## B — `tokens` (Strata's hand-rolled components)

### `syntax` — SQL editor
| field | Midnight | Daylight |
|---|---|---|
| `syn_keyword` | `#ff7b9c` | `#cf222e` |
| `syn_function` | `#d2a8ff` | `#8250df` |
| `syn_string` | `#a5d6ff` | `#0a3069` |
| `syn_number` | `#79c0ff` | `#0550ae` |
| `syn_comment` | `#6f7988` | `#7c828e` |
| `syn_identifier` | `#edf0f5` | `#1a1c22` |
| `syn_punct` | `#909aa9` | `#5f6771` |

### `data_type` — schema / type badges
| field | Midnight | Daylight |
|---|---|---|
| `t_str` | `#7ee787` | `#0a7d33` |
| `t_num` | `#79c0ff` | `#0550ae` |
| `t_bool` | `#d2a8ff` | `#8250df` |
| `t_ts` | `#ffa657` | `#bc4c00` |
| `t_struct` | `#f0a5c0` | `#bf3989` |
| `t_list` | `#8ad4ff` | `#0969da` |
| `t_map` | `#ffcf6b` | `#9a6700` |

### `grid` — results grid
| field | controls | Midnight | Daylight |
|---|---|---|---|
| `cell` | default cell text | `#cfd6e0` | `#33373f` |
| `cell_num` | numeric cell text | `#9fc6ff` | `#0550ae` |
| `cell_ts` | timestamp cell text | `#e2b98c` | `#9a6700` |
| `grid_line` | grid rules | `#23272f` | `#e3e5e9` |
| `row_hover` | hovered-row wash | `#2a323e` | `#eaf0f8` |
| `zebra` | alternating-row wash (alpha) | `rgba(255,255,255,.025)` | `rgba(15,23,42,.035)` |

(Type / status accents reuse `sheet.*` where they overlap — e.g. a numeric cell can read
`sheet.info`; badges read `sheet.success`/`error`/`warning`.)

### `accents` — tints
| field | controls | Midnight | Daylight |
|---|---|---|---|
| `accent_soft` | accent tint fills / selected-tab wash (alpha) | `rgba(76,198,255,.14)` | `rgba(43,127,208,.12)` |

---

## C — `fonts`
| field | value (both themes) |
|---|---|
| `ui` | `IBM Plex Sans` (fallback: system-ui, sans-serif) |
| `mono` | `JetBrains Mono` (fallback: ui-monospace, monospace) |

Freya wants a resolvable family name, not a CSS stack — we'll bundle/register the fonts app
side; the designer just names the family.

---

## E — Component theming map (how the sheet lands on each widget)

Extracted from Freya's `themes.rs` (v0.4). **Colour fields only** — each component's default
preference resolves these from the sheet, so this is the reverse index: it shows what every
widget pulls, so you can predict the blast radius of changing a slot. (Sizing / radius / type
are Freya defaults we override separately, not colours.) "tint" = transparent by default.

**Buttons** (variant picked per use)
| variant | background | hover | border | focus border | text |
|---|---|---|---|---|---|
| Button (default) | `surface_tertiary` | `surface_secondary` | `border` | `border_focus` | `text_primary` |
| Filled | `primary` | `tertiary` | – | `secondary` | `text_inverse` |
| Outline | `surface_tertiary` | `surface_secondary` | `border` | `secondary` | `primary` |
| Flat | – | `surface_tertiary` | – | `border` | `text_primary` |

**Inputs**
| variant | background | focus bg | text | placeholder | border | focus border |
|---|---|---|---|---|---|---|
| Input | `surface_tertiary` | `background` | `text_primary` | `text_secondary` | `border` | `border_focus` |
| Filled | `primary` | `tertiary` | `text_inverse` | `text_inverse` | – | `secondary` |
| Flat | – | `surface_tertiary` | `text_primary` | `text_secondary` | – | `border` |

**Selection / overlays**
| widget | slots |
|---|---|
| Select | menu `background`, button `surface_tertiary`, hover `surface_secondary`, text/arrow `text_primary`, border `border`, focus `border_focus` |
| Menu item | hover/selected `surface_secondary`, selected-border `border_focus`, text `text_primary` |
| Menu container | `background`, border `surface_primary`, shadow `shadow` |
| Popup | `background`, text `text_primary` |
| Tooltip | `surface_tertiary`, text `text_primary`, border `surface_primary` |

**Toggles**
| widget | slots |
|---|---|
| Switch | track `surface_secondary`, thumb `surface_inverse`, toggled-track `secondary`, toggled-thumb `primary`, focus `border_focus` |
| Checkbox | off `surface_inverse_tertiary`, on `primary`, tick `secondary`, border `surface_primary` |
| Radio | off `surface_inverse_tertiary`, on `primary`, border `surface_primary` |

**Tabs / segmented**
| widget | slots |
|---|---|
| Floating tab | hover `surface_secondary`, text `text_primary` (bg tint) |
| Segmented button | track `surface_tertiary`, border `border` |
| Segment | `surface_tertiary`, hover/selected/focus `surface_secondary`, disabled `disabled`, text `text_primary`, selected-icon `primary` |

**Sidebar / chips / accordion**
| widget | slots |
|---|---|
| Sidebar item | `surface_tertiary`, active/hover `surface_secondary`, text `text_primary`, focus `border_focus` |
| Chip | `background`, hover `tertiary`, selected `primary`, border `border`, focus `secondary`, text `text_primary`, hover/selected text `text_inverse` |
| Accordion | `surface_tertiary`, border `border`, text `text_primary` |

**Feedback / structure**
| widget | slots |
|---|---|
| Scrollbar | track `surface_primary`, thumb `surface_inverse`, hover/active `surface_inverse_secondary` / `_tertiary` |
| Progress bar | track `surface_primary`, fill `primary`, text `text_inverse` |
| Circular loader | `surface_primary` |
| Skeleton | `surface_primary` + **hardcoded** translucent-white shimmer (the one colour that won't cascade) |
| Resizable handle | `surface_secondary`, hover `surface_primary` |
| Table | `background`, hover-row `surface_secondary`, divider `surface_primary`, text/arrow `text_primary` |
| Card | filled: `primary`/hover `tertiary`/text `text_inverse`; outline: `surface_tertiary`/hover `surface_secondary`/border `border`; both shadow `shadow` |
| Titlebar button | hover `surface_secondary` (bg tint) |
| Link | `text_highlight` |

Takeaways for authoring the sheet: `surface_tertiary` is the workhorse (most default control
backgrounds); `surface_secondary` is the universal hover; `primary`+`tertiary`+`secondary`
drive every *filled/selected/toggled* state; `surface_inverse*` are thumbs + unchecked marks.

## D — file shape

```json
{
  "id": "midnight",
  "name": "Midnight",
  "mode": "dark",
  "sheet": {
    "primary": "#4cc6ff",
    "background": "#15181e",
    "surface_primary": "#191d24",
    "text_primary": "#edf0f5",
    "border": "#23272f",
    "hover": "#2a323e"
  },
  "tokens": {
    "syntax":    { "syn_keyword": "#ff7b9c" },
    "data_type": { "t_str": "#7ee787" },
    "grid":      { "cell": "#cfd6e0", "zebra": "rgba(255,255,255,.025)" },
    "accents":   { "accent_soft": "rgba(76,198,255,.14)" }
  },
  "fonts": { "ui": "IBM Plex Sans", "mono": "JetBrains Mono" }
}
```

The exact `sheet` field set is finalised when we wire the loader (the compiler pins it to
Freya's real `ColorsSheet`); if Freya adds/renames a slot we adjust the spec then.
