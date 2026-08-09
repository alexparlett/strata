# Strata (Freya) — theme spec

A **role-first theme format**: a theme file authors a closed vocabulary of ~100 named colour
roles, and nothing about components. Every component's dress is fixed onto roles by **one static
mapping table** in the app (`strata-freya/src/theme/components.rs`); a theme retunes the whole
app by retuning roles alone. This is the shape the newest IDE theme systems converged on (Zed's
role-only format; Visual Studio 2026's 1,806-token → 229-role consolidation), adopted here after
the per-component format's two shipped themes had measurably drifted apart — same field
`specific` in one theme and `reference` in the other, per-theme palette aliasing with no stable
answer to "is X the same token as Y", and a dead component group nothing had validated.

A theme file has four blocks:

- **`roles`** — a flat map of dotted role names → colour strings. The closed set below;
  the schema enumerates every name and rejects unknown ones.
- **`syntax`** — the editor's syntax colours, keyed by author-facing scope name
  (`keyword`, `punctuation.bracket`, `string.escape`, …). The scope list is
  `strata-code-editor`'s `SYNTAX_SCOPES` (34 scopes; Rust field ↔ scope name by
  underscore ↔ dot, with `type_` ↔ `type`). Plain colour strings; a
  `{ color, font_style, font_weight }` object form is the documented extension point if the
  editor grows styled scopes.
- **`fonts`** — `{ "ui": …, "mono": … }`, the two families the `typography` roles resolve
  through. Naming a new family here means embedding it in the bundle in the same change.
- **`typography`** — the type scale: 11 named roles (`title`, `strong_body`, `body_medium`,
  `control`, `body`, `caption`, `data_value`, `code_block`, `field_label`, `meta`,
  `mono_path`), each `{ family: ui|mono, weight, size, line_height?, letter_spacing? }`.
  The editor's type is `code_block`; the tooltip's is `body` — both resolved by the mapping
  table, so the scale is the single source of type.

Colours are `#rrggbb`, `#rrggbbaa`, or `rgba(r,g,b,a)`. Reference the schema via
`"$schema": "./theme.schema.json"` for editor autocomplete + validation.

There is deliberately **no aliasing inside the file** (no variables, no role-to-role
references): values are literal colours, because per-theme aliasing is exactly how the old
palette rotted. The don't-repeat-yourself pressure is answered by the role set being small and
semantic, not by an indirection layer.

## The role vocabulary

Every role is **required** unless marked `(opt → X)`, in which case omitting it reads role `X`
(`surface.stripe` omits to transparent). A fallback is always "read this other named role" —
never a computed tint, which no shipping theme format does either. The authoritative table is
`strata-core/src/theme.rs`'s `roles!` invocation; the schema is generated from it
(`UPDATE_SCHEMA=1 cargo test -p strata-freya schema_in_sync`).

**Surfaces** — elevation tiers, not widget names:
`background` (the app base coat) · `surface.background` (the standard panel: tab bar,
grid body, input wells) · `surface.raised` (one step up: the sidebar, drawer, inspector,
settings/launcher body) · `surface.sunken` (opt → `background`; the EXPLAIN canvas) · `surface.subtle` (the
faintest raised box) · `surface.stripe` (opt → transparent; the grid's zebra tint, authored
translucent) · `elevated_surface.background` (floating chrome: menus, popups, tooltips, palette,
modals) · `backdrop` (the modal scrim) · `shadow`.

**Location refinements** — a *place* that may leave its tier, the pressure valve against
under-granular roles: `panel.background` (opt → `surface.raised`) · `tab_bar.background`
(opt → `surface.background`) · `status_bar.background` (opt → `surface.background`) ·
`title_bar.background` (opt → `surface.raised`).

**Elements** — two interaction families with explicit states (no state is ever derived):
`element.{background,hover,active,selected,disabled}` for filled controls (buttons, select
triggers, grid headers) · `ghost_element.{background,hover,active,selected}` for flush controls
(tabs, sidebar rows, drawer rows, segments) · `elevated_element.hover` (one translucent wash for
items on elevated/filled bases: menu items, select options, completion rows, card hover) ·
`list.hover` (data rows — hue-distinct from control hover in Daylight) ·
`drop_target.background` · `track` (progress/slider channel) · `knob` (switch thumb, check
mark — its own role because `text.on_accent` is dark in Midnight).

**Borders**: `border` · `border.variant` (fainter: in-list rules, tree guides) ·
`border.control` (control outlines) · `border.strong` (emphasized: hovered cards, the keymap's
dashed slot) · `border.focused` · `border.selected` (opt → `border.focused`) ·
`border.disabled` (opt → `border.variant`) · `border.overlay` (floating-chrome edges,
checkbox/radio rest outlines, the switch track).

**Text** — icons read these too; an `icon.*` family is the named escape if an icon ever needs
to differ: `text` · `text.muted` · `text.control` · `text.dim` · `text.label` (uppercase
eyebrows) · `text.placeholder` · `text.disabled` · `text.accent` (opt → `accent`) ·
`text.on_accent`.

**Accent**: `accent` · `accent.hover` · `accent.ring` (opt → `accent`) · `accent.selection`
(the ~12% wash — selected rows, nav pills, the palette's active row) · `accent.muted` (the
~22% wash) · `accent.badge`.

**Status** — one global triad per semantic, never per-surface:
`error` + `error.background` + `error.background.hover` + `error.border` (error alone carries a
hover, for Cancel and Run-while-running) · `warning`/`success`/`info` each + `.background` +
`.border`. In app code the four *tones* are read through the shared `tones()` hook
(`components/tones.rs`), never restated per surface; the tinted `.background`/`.border`
variants reach components through mapping-table rows (the run/cancel buttons' running dress,
export's warning banner), which is the table doing its job, not a surface restating the ramp.

**Editor chrome**: `editor.background` (its own role — the built-ins genuinely put it on
different tiers) · `editor.line_number` · `editor.active_line_number` · `editor.selection` ·
`editor.cursor` (opt → `accent`). Syntax is the separate `syntax` block.

**Scrollbar**: `scrollbar.track` · `scrollbar.thumb` · `scrollbar.thumb.hover` ·
`scrollbar.thumb.active`.

**Data ramps** — Strata's own categorical vocabularies:
`data_type.{string,number,boolean,timestamp,struct,list,map}` (the seven-hue display taxonomy —
dtype labels, swatches, per-type cell text) · `chart.1` … `chart.10` (the ordered series ramp) ·
`entity.{table,view,query,column,function,keyword}` (catalog icons + completion kinds, one
agreed set) · `format.{parquet,csv,json,arrow,view}` (source-format badges — deliberately NOT
folded into the data-type ramp even where hues coincide: retinting strings must not repaint
file badges).

## Resolution model

`strata_theme()` resolves the file once per theme build: roles → a `RoleColors` array, installed
on the Freya `Theme` under `ROLES_KEY`; the mapping table registers every component theme with
`Preference::Reference(role-name)` colour fields, which the fork resolves against the palette at
**read** time — so the registrations are theme-independent and only the palette, syntax and
fonts vary per theme.

The fork stays untouched. Its 27-slot `ColorsSheet` is fed by `bridge_sheet()` (each old slot
mapped to a role by the slot's behaviour in fork defaults), so built-in component defaults the
table doesn't override keep painting correctly, and the dotted role names resolve through the
pluggable `Palette::color` seam. Seven role names coincide with core slot names (`background`,
`border`, `shadow`, and the four status tones); the bridge maps those slots to the same-named
roles, and the `a_role_reference_resolves_to_its_own_colour` test pins that either resolution
path answers the role's own colour.

**Missing/unknown is loud, never fatal.** A missing required role or syntax scope paints
**magenta**; unknown names are ignored. Both are warned per file at discovery
(`ThemeRegistry::with_dirs`). A **pre-roles** file (one with a `sheet`/`components` section)
fails to parse on the missing `roles` field and is skipped with a legacy-specific warning — the
app never breaks on an old theme, it just doesn't list it.

In app code, a surface reads colours from its **component theme** (`get_theme!`); the roles are
reached directly (`use_roles().get(Role::…)`) only where the component theme has no field for
the value, and the four semantic tones only through `tones()`. Splitting an over-shared role is
a three-site change: add it to `roles!`, retarget the mapping rows, author the value in the two
built-in files — `schema_in_sync` regenerates the schema.

## Discovery

Midnight and Daylight ship embedded (`strata-core` `include_str!`s this repo's `themes/*.json`).
User themes: drop a `*.json` of the same shape into the user themes dir
(macOS `~/Library/Application Support/Strata/themes`, else `~/.config/Strata/themes`); a file
reusing a built-in `id` replaces it in place. Discovery happens **once** at launch
(`ThemesCtx::discover()` in `main`), so a new file means a restart.
(`theme::open_user_themes_dir` exists to reveal the folder but no surface calls it yet.)

## Example (abbreviated)

```json
{
  "$schema": "./theme.schema.json",
  "id": "midnight",
  "name": "Midnight",
  "mode": "dark",
  "roles": {
    "background": "#15181e",
    "surface.background": "#191d24",
    "surface.raised": "#1e232b",
    "elevated_surface.background": "#262c35",
    "element.background": "#2a313c",
    "element.hover": "#333d4b",
    "ghost_element.hover": "#262d37",
    "border": "#2c333d",
    "text": "#edf0f5",
    "text.muted": "#cfd6e0",
    "accent": "#4cc6ff",
    "accent.selection": "rgba(76,198,255,.12)",
    "error": "#ff8a8a",
    "error.background": "rgba(229,72,77,.16)",
    "editor.background": "#191d24",
    "data_type.number": "#79c0ff",
    "chart.1": "#4cc6ff"
  },
  "syntax": {
    "keyword": "#ff7b9c",
    "string": "#a5d6ff",
    "punctuation.bracket": "#909aa9",
    "type": "#d2a8ff"
  },
  "fonts": { "ui": "IBM Plex Sans", "mono": "JetBrains Mono" },
  "typography": {
    "body": { "family": "ui", "weight": 400, "size": 12.5 },
    "code_block": { "family": "mono", "weight": 400, "size": 12, "line_height": 1.6 }
  }
}
```

The full files are `themes/midnight.json` / `themes/daylight.json` — the working reference for
every role's intended tier and weight in each mode.
