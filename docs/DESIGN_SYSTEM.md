# Strata — Design System

The canonical component + token spec, distilled from the **`Canvas.dc.html`** design
canvas (v13 handoff). This is the reference for **S28** (input/control components) and
**S29** (overlay/menu components): build the reusable pieces to these values, then sweep
the app onto them.

Two conventions to keep in mind:

- **Tokens.** The canvas names colours `--c-*`; **our app** (`assets/main.css` `:root`)
  uses `--*`. Build components with **our** tokens — the map below translates. Most are a
  clean rename; a few shades diverge (flagged ⚠) and are worth reconciling.
- **Two type families.** `--ui` = *IBM Plex Sans* (labels, buttons, prose); `--mono` =
  *JetBrains Mono* (code, data values, field/section labels, figures).

---

## 1. Token map (design `--c-*` → our `--*`)

| Design token | Value | Our token | Role |
| --- | --- | --- | --- |
| `--c-bg` | `#090c11` | `--main` | app base background |
| `--c-panel` | `#0b0e13` | `--bg` | panel background |
| `--c-surface` | `#0e121a` | `--panel` | cards / raised surfaces |
| `--c-elev` | `#12161f` | `--elev` | **menu / popover bg** |
| `--c-surface2` | `#161b25` | `--elev2` | active-rail / raised fill |
| `--c-sel` | `#1b212c` | `--elev3` | **hover / selected fill** |
| `--c-sunken` | `#071019` | `--accent-ink` *(same hex; consider a `--sunken` alias)* | disabled / inset bg |
| `--c-pop` | `#0f141d` | `--surface` | popup surface |
| `--c-line` | `#12161c` | `--grid-line` | hairline |
| `--c-border` | `#1c222c` | `--line` | subtle divider |
| `--c-border2` | `#262e39` | `--line2` | **default control border** |
| `--c-border3` | `#2a3340` | `--line3` | **menu / card border** |
| `--c-border4` | `#37424f` | `--line-hi` | **hover border** |
| `--c-text` | `#e7ebf1` | `--text` | primary text |
| `--c-text2` | `#c3cbd6` | `--text2` | secondary text |
| `--c-text3` | `#aeb6c2` | `--text3` | **control label (default)** |
| `--c-muted` | `#8b95a3` | `--dim` | muted / ghost text |
| `--c-muted2` | `#6a7482` | `--dim2` | fainter icon/label |
| `--c-label` | `#5a6472` | `--dim3` | section labels |
| `--c-faint` | `#4a5462` | `--faint` | **disabled text** |
| `--c-faint2` | `#3f4854` | `--faint2` | faintest |
| `--accent` | `#4cc6ff` | `--accent` | brand / primary |
| `--accentSoft` | `accent 15%` | `--accent-soft` | ⚠ ours is **12%** (`rgba(76,198,255,.12)`) — the canvas segmented/toggle tint is 12% too, so ours is fine there; badges use 15% |
| `--c-onaccent` | `#08111a` | `--accent-ink` (`#071019`) | text/icon on an accent fill |
| `--c-ok` | `#9fe6b4` | `--green` (`#4ade80`) | ⚠ softer shade — consider a `--ok` |
| `--c-warm` | `#e2b98c` | `--cell-ts` (`#e2b98c`, exact) / `--orange` | ⚠ "warm" badges/cached |
| `--c-err` | `#ff8a8a` | `--red2` (`#f87171`) | ⚠ shade differs slightly |
| `--c-err2` | `#f8a5a5` | `--red` (`#ff9aa2`) | ⚠ error body text |
| `--c-cellhover` | `#1b2330` | `--row-hover` (`#141a24`) | ⚠ grid cell hover |
| `--c-zebra` | `rgba(255,255,255,.018)` | `--zebra` (`.055`) | ⚠ ours is heavier |

**Reconcile candidates (⚠):** `--ok`, warm, `err`/`err2` shades, cell-hover, and the
zebra opacity all differ from the canvas. Either add matching tokens or accept the
current app values — decide once, up front, when S28 lands so components are consistent.

---

## 2. Typography

| Use | Font / weight / size |
| --- | --- |
| Hero | `--ui` 600 · 26px |
| Section heading | `--ui` 600 · 17px |
| Body (interactive) | `--ui` 500 · 13px |
| Body (prose) | `--ui` 400 · 12.5px / 1.55 |
| Buttons · inputs · menus | `--ui` 500–600 · 12.5px |
| Data value (metric) | `--mono` 700 · 22px |
| Field / section label | `--mono` 500 · 12px |
| Column header / status badge | `--mono` 600 · 10–10.5px (uppercase, +0.6px tracking) |

---

## 3. Buttons

All text buttons: **34px** tall, **radius 8px**, `--ui` 600 · 12.5px. Disabled (universal):
`opacity: .6; cursor: not-allowed;` text `--faint`, bg `--accent-ink`(sunken), border `--line2`.

| Variant | Default | Hover |
| --- | --- | --- |
| **Primary** | bg `--accent`, text `--accent-ink`, no border, pad `0 16px` | `filter: brightness(1.12)` |
| **Secondary** | bg `--elev`, text `--text`, border 1px `--line3`, pad `0 16px` | bg `--elev3`, text `--accent` |
| **Ghost** | transparent, text `--dim`, pad `0 14px` | bg `--elev3`, text `--accent` |
| **Accent/state** | transparent, text `--accent`, pad `0 14px` | `brightness(1.12)` |
| **Danger** | border 1px `color-mix(--red2 45%, transparent)`, bg `color-mix(--red2 12%, transparent)`, text `--red2` | border 65% / bg 22% |
| **Soft** (menu row) | transparent, text `--text3` | `box-shadow: inset 0 0 0 999px color-mix(--text 7%, transparent)` |
| **Compact text** | pad `4px 8px`, radius 5px, `--ui` 500 · 11px, text `--dim2` | bg `--elev3`, text `--accent` |

### Icon buttons

| Variant | Size / radius / svg | Default | Hover | On (pressed) |
| --- | --- | --- | --- | --- |
| **Toolbar** (neutral) | 32px · r8 · 16px | border 1px `--line2`, bg `--panel`, text `--text2` | bg `--elev3`, text `--accent` | — |
| **Ghost** (dismiss/close) | 28px · r7 · 15px | borderless, text `--dim` | bg `--elev3`, text `--accent` | — |
| **Pager** (nav arrows) | 28×26 · r6 · 15px | borderless, text `--dim2` | bg `--elev3`, text `--accent` | — |
| **Toggle** ⭐ | 28px · r6 · 13px | borderless, text `--text3` | inset 7% overlay | **bg `color-mix(--accent 12%, transparent)` (≈ `--accent-soft`), text `--accent`** |

> ⭐ **Icon toggle** is the stateful pattern (e.g. the plan **Raw/Tree** button — already
> shipped as `.icon-btn.plain` + `.icon-btn.plain.on`). Pairs with a segmented control;
> distinct from the neutral toolbar/ghost icon buttons. The activity rail keeps its own
> denser idiom — intentionally *not* this.

### Split button ⭐ (S30 — Run)

Primary face + an attached **caret** that opens a `DropdownMenu` (Run / Explain plan /
Explain analyze). Picking a mode rewrites the buffer prefix and runs; a check marks the
current mode. Caret hidden while running (collapses to a plain Cancel button).

---

## 4. Inputs (§04)

Text field: **34px** · pad `0 12px` · **radius 8px** · `--ui` 500 · 13px · `outline:none`.

- **Default:** bg `--panel`, border 1px `--line2`, text `--text`.
- **Hover:** border 1px `--line-hi`.
- **Focus:** border 1px `--accent` + `box-shadow: inset 0 0 0 1px var(--accent), 0 0 0 3px color-mix(in srgb, var(--accent) 20%, transparent);` (the shared focus ring).
- **Disabled:** bg sunken, text `--faint`, `opacity:.6; cursor:not-allowed`.
- **Search field:** icon-slot wrapper (icon 14px `--dim2`, gap 9px) + flex input; icon → `--accent` on focus.
- **Number stepper:** 120px; input (flex) + 22px column of up/down chevrons (11px, stroke 2.6) divided by a 1px `--line2` border; `--mono` value.

Focus ring is one shared recipe — factor it into a mixin/const so every control matches.

---

## 5. Dropdowns & menus (§05 / §07) → **S29**

**Trigger (combobox):** 34px · pad `0 11px` · border 1px `--line2` · bg `--panel` ·
`--ui` 500 · 12.5px · r8 · chevron-down 12px `--dim2`; hover border `--line-hi`.

**Popup card (shared by dropdown / context menu / tooltip):** bg `--elev` · border 1px
`--line3` · **radius 9px** · padding 4px · `box-shadow: 0 12px 30px rgba(0,0,0,.4)` ·
1px row gap.

**Menu item:** min-height 30px · pad `0 10px` · r6 · `--ui` 500 · 12.5px.
- Unselected: transparent, text `--text`; hover = inset 7% overlay.
- **Selected:** bg `color-mix(in srgb, var(--accent) 10%, transparent)`, text `--accent`.
- Optional icon (15px) left + `kbd` hint (`--mono` 400 · 11px `--faint`) right.
- Divider: 1px `--grid-line`, margin `4px 6px`.

**Tooltip:** 220px · pad `10px 12px` · `--ui` 400 · 11.5px/1.5 · text `--text2` · r9 · a
6px arrow (`--line3`). *(We render the lint tooltip via `Popup{backdrop:false}` — S27.)*

---

## 6. Selection controls (§06) → **S28**

### Segmented control
- **Container:** `inline-flex; gap:3px; padding:3px;` bg `--panel`, border 1px `--line2`, **radius 9px**.
- **Segment:** pad `7px 16px`, r6, borderless, `--ui` 500 · 12.5px.
  - Unselected: transparent, text `--text3`.
  - **Selected:** bg `color-mix(in srgb, var(--accent) 12%, transparent)` (≈ `--accent-soft`), text `--accent` — **not solid accent**.
- Disabled: `opacity:.55`. *(Shipped as `.seg-row`/`.seg-toggle` — results Table/Chart + plan Physical/Logical.)*

### Toggle / switch
- **Track:** 34×19, radius 10px. Off bg `--line3`; on bg `--accent`.
- **Knob:** 15px, `#fff`, `box-shadow: 0 1px 3px rgba(0,0,0,.4)`; `translateX(2px)` off → `translateX(17px)` on; transition 0.15s.
- Label `--ui` 500 · 13px `--text3`. *(Today hand-rolled `.toggle`+`.knob` in settings — componentise.)*

### Checkbox — *(shipped: `components/checkbox.rs`)*
- 16px box, r4. Unchecked: border 1px `--line3`, transparent. **Checked:** border + bg `--accent`, white check (11px, stroke 3.2). Label `--ui` 500 · 12.5px `--text3`.

### Radio
- 16px circle. Unchecked: border 1px `--line3`. **Checked:** border `--accent`, inner 8px dot `--accent`. Label as checkbox.

---

## 7. Status (§08) → results status bar / badges

- **Status dot:** 8px. `run` = `--accent` + pulse (`scale 1→2.4, opacity .55→0`); `ok` = `--green`; `idle` = `--dim2`; `err` = `--red2`. *(Shipped in R1 as `.res-dot`.)*
- **Status badge (pill):** 22px · pad `0 10px` · r6 · `--mono` 600 · 10.5px uppercase.
  `CONNECTED`/accent = `--accent-soft` + `--accent`; `READY` = `color-mix(--green 15%)` + `--green`; `CACHED` = warm 15% + warm; `ERROR` = `color-mix(--red2 15%)` + `--red2`; `DRAFT` = `--elev3` + `--dim`.
- **Callout card (left-bar):** pad `11px 14px` · r8 · border 1px (low-sat semantic) · `border-left: 3px solid` semantic + 15px icon. Info=accent, warning=warm, error=red2. *(The results error banner is a variant of this.)*

---

## 8. Already-built surfaces (reference only)

Data grid / tabs (§09), activity rail + badges (§10), and the icon library (§11, 24px
viewBox · 20px · 1.7 stroke · round caps) are largely in place — align to the canvas
during their own tasks (R-series / S23 / `icons.rs`) rather than as part of S28/S29.

---

*Source: `Canvas.dc.html` (v13). Keep this doc in sync when the canvas changes; it is the
build target for S28/S29 and the tint/spec reference for any new control.*
