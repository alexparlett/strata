# Editor feasibility spike — completion (S7) + validation squiggles (S25)

**Question.** Before designing autocomplete (S7) and the static SQL validator (S25)
together, what does our editor (`dioxus-code-editor` 0.1.2, on `dioxus-code` 0.1.x)
actually let us do about (a) caret offset + selection, (b) caret pixel coordinates
for a completion dropdown, (c) inline decorations / squiggles, and (d) reusing its
tokeniser? Both features are gated on the same answers.

**TL;DR.** Everything is feasible, but **none of it is exposed through the crate's
Rust API** — the component is a sealed controlled black box (`value` in, `oninput`
out). The good news is the edit surface is a **real `<textarea>`**, so caret,
selection, and coordinates are all physically reachable from the DOM. The cost is a
**JS-interop bridge** (this is a `wry` webview desktop app — no native `web-sys`),
plus a decision about whether to **scrape the upstream component's DOM** or **vendor
our own editor** built on `dioxus-code`'s public `advanced` building blocks.

---

## 1. What the editor actually is

Contrary to the stale S7 note ("editor is uncontrolled → likely a custom overlay"),
the editor **is controlled** and is a **textarea-over-highlight** design. From the
0.1.2 source, the rendered DOM is:

```
div.dxc-editor                       (root; theme classes + our `class`)
  div.dxc-editor-gutter               (line numbers, aria-hidden)  — rendered by the crate
    div.dxc-editor-gutter-line "N"
  div.dxc-editor-viewport
    div.dxc-editor-highlight          (aria-hidden; the *visual* syntax layer) — rendered by the crate
      div.dxc-editor-line
        span.a-<tag> … (TokenSpan)    (one <span> per highlight token)
    textarea.dxc-editor-input         (the *actual* editable layer; transparent text over the highlight)
```

The `<textarea class="dxc-editor-input">` is the real input. The highlight `div`
sits underneath it purely for colour. This is the classic Prism/CodeMirror-5-style
overlay editor.

## 2. `CodeEditorProps` — the entire public surface

`value`, `language`, `theme`, `line_numbers`, `read_only`, `spellcheck`,
`aria_label`, `placeholder`, `class`, `oninput`. That's all.

**Absent (this is the crux):** no caret/selection prop or event, no `onkeydown`,
no `onselect`, no element `ref`, no decorations/markers prop, no coordinate API.
`oninput` yields only "the full editor text after each input event" — no offset.

So the crate hands us nothing for either feature directly. We reach past it, into
the DOM, or we replace it.

## 3. Capability findings

**(a) Caret offset + selection — FEASIBLE.** A `<textarea>` exposes
`selectionStart` / `selectionEnd` directly. We can read them and listen for
`keydown` / `selectionchange` / `click`. But not from Rust natively: this is a
desktop `wry` webview, so `web-sys`/`wasm-bindgen` don't execute (they run only in
the crate's own wasm-targeted paths). Access path = **`dioxus::document::eval`**: a
small JS snippet queries `.dxc-editor-input` inside our editor wrapper, reads the
selection, attaches listeners, and posts values back to Rust over the eval channel.
Reliable, but it is an interop layer we own.

**(b) Completion dropdown anchor (caret pixel coords) — FEASIBLE.** Textareas give
no native caret rectangle. Standard **mirror-div** technique (clone the textarea's
font/padding/scroll into a hidden div, insert text up to the caret, measure a marker
span's `getBoundingClientRect`) yields x/y. Same eval bridge. Well-trodden
(`textarea-caret-position`). The text is monospace and `wrap:"off"`, which makes the
metrics especially simple (x ≈ col × ch-width − scrollLeft; y ≈ line × line-height −
scrollTop) — arguably we can skip the mirror div and compute from metrics.

**(c) Inline squiggles / decorations — FEASIBLE, most effort.** The highlight layer
is rendered by the crate from *its* tokens; we cannot inject markers through the API.
Options, cheapest first:
  1. **Problems panel only** (already shipped) + a **gutter dot** overlay — no inline
     marks. Honest MVP.
  2. **Our own absolute overlay** inside `.dxc-editor-viewport`, drawing underlines at
     computed rects (same monospace metrics as the caret). Line-based layout + fixed
     char width make this tractable, if fiddly on scroll/resize.
  3. Full decoration API — only realistic if we vendor the editor (see §5).

**(d) Reusing the tokeniser — PARTIAL.** `dioxus-code` is built on `arborium`
(tree-sitter family) and its public `advanced` module exposes `Buffer`
(live text+grammar, `edit`/`replace`/`highlighted`), `HighlightedSource`, and
`HighlightSpan` = **a byte range + a tag string** (e.g. `0..2, "k"`). So we can get a
**tagged token stream with byte offsets** — enough to find the token under the caret
and to scan for unbalanced parens / lint identifier tokens — **but there is no parse
tree and no error nodes**. Clause-context ("are we after FROM?"), alias binding, and
semantic checks (unknown table/column) are still ours to build over the token stream.
Net: we can reuse a tokeniser (or hand-roll a ~200-line SQL lexer just as easily);
we get no free AST.

## 4. The desktop constraint (important)

Because the app is `dioxus` **desktop** (native Rust + `wry` webview), the
`web-sys`/`wasm-bindgen` that `dioxus-code-editor` lists as deps do **not** give *us*
a native DOM handle. Any DOM read (selection, caret rect) or listener must go through
`dioxus::document::eval` (JS in the webview, values marshalled back). This is the one
unavoidable interop layer, and it is **100% shared** by S7 and S25.

## 5. The strategic choice this surfaces

Both features need the same bridge; the real fork is **how** we attach it:

- **Option A — scrape the upstream component's DOM.** Keep `CodeEditor` as-is;
  from a wrapper, `eval` against `.dxc-editor-input` / `.dxc-editor-highlight`.
  *Pro:* no fork, least code now. *Con:* depends on the crate's internal class names
  and DOM staying stable across versions; squiggles are an awkward external overlay;
  keyboard routing (⌘Space, ↑↓/Enter/Esc while the popup is open) fights the
  textarea's own handling.

- **Option B — vendor our own editor on `dioxus_code::advanced`.** The public
  `Buffer` / `HighlightedSource` / `TokenSpan` are exactly what the 257-line upstream
  component uses; we can reproduce it and add first-class support: a caret/selection
  channel, `onkeydown` routing, and a **decorations layer** rendered alongside the
  highlight (native squiggles, gutter marks). *Pro:* clean, version-stable, both
  features get a real API instead of DOM-scraping; squiggles become trivial. *Con:*
  more upfront work; we still need one small embedded JS snippet for `selectionStart`
  (the browser only exposes selection via the DOM — verify whether dioxus 0.7
  `FormData`/`MountedData` surfaces textarea selection first; if it does, Option B
  needs *no* custom JS at all).

**Recommendation: Option B — vendor.** Autocomplete and squiggles both live or die on
tight caret/selection/keyboard integration; scraping a third-party DOM for that is the
kind of thing that breaks on a patch bump. Vendoring is ~a day of editor work that
turns the shared foundation into a clean, owned surface both features build on. Do the
dioxus-0.7 selection-API check first — it decides whether even the JS snippet is
needed.

### Source review — vendoring is small and clean (confirmed)

Reading `code-editor/src/{lib.rs, edit_capture.rs, edit_capture/desktop.rs}`:

- **Only public APIs.** The component uses stock dioxus 0.7 (`MountedEvent`,
  `FormEvent`, `onmounted`/`oninput` attribute builders) and `dioxus_code::advanced`
  (`Buffer`, `HighlightedSource`, `TokenSpan`, `SourceEdit`) — all public/semver. No
  private coupling.
- **The web-sys complexity is wasm-only.** `edit_capture.rs` splits by
  `#[cfg(target_arch = "wasm32")]` → `web.rs` (the only `web_sys`/`wasm-bindgen`
  user, for `beforeinput` selection reads) vs `desktop.rs`. A desktop-only vendor
  **deletes `web.rs`**, dropping those deps and all of that complexity.
- **Desktop capture is trivial.** `desktop.rs::PlatformEditTracker` is a pure
  old-vs-new **string diff** producing a `SourceEdit` (byte range) to drive
  incremental highlighting. `mount()` is a **no-op today** — it receives the
  textarea's `MountedEvent` and ignores it.
- **The seam we need already exists.** `use_input_edit_attributes` attaches an
  `onmounted` to the `<textarea>` that hands the `MountedData` to the tracker. That
  ignored handle **is exactly our hook** — on vendor, we capture it and attach caret/
  selection reads, keydown routing, and coordinate math to the element we own.

**Vendor surface:** `lib.rs` (~257 lines) + `edit_capture.rs` (tracker + the two
attrs) + `desktop.rs` (string diff) + the CSS asset. Drop `web.rs`. ~350 readable
lines, then extend with: a caret/selection channel, `onkeydown` routing, and a
decorations layer in the highlight `div`.

**Residual JS.** `MountedData` (dioxus 0.7) exposes rects/scroll/focus but **not**
textarea `selectionStart` — the browser only gives selection via the DOM. So reading
the caret still needs a ~5-line `eval`, but now it lives *inside our component* keyed
to the element we mounted (not scraping a foreign DOM). Confirm the dioxus-0.7
`MountedData` selection surface before writing it; if a future version adds it, the
eval disappears.

## 6. Implications for the S7 + S25 design

The shared foundation is **two** pieces, both used by each feature:

1. **A SQL analysis layer** (`crate::sql` / `crate::lang`) — pure Rust: token stream
   (reuse `dioxus_code::advanced::Buffer` or a small lexer) → caret-context state
   machine → symbol table (catalog tables + their Arrow columns + in-statement
   aliases) → structural errors. `diagnostics::validate` and the completion provider
   are thin consumers. The per-tab debounced `use_revalidate` we already built
   generalises into "analyse on edit", cached per tab; completion reads the cache.
2. **An editor integration surface** (the caret/selection/keydown bridge + optional
   decorations layer) — the thing that makes either feature attach to *this* editor.
   Section 5 decides its shape.

A cross-cutting prerequisite for the symbol table: **column schemas must be available
synchronously on the UI thread.** They live engine-side today; the engine likely has
to push each table's Arrow schema into a UI-side catalog store on register. This
blocks the *semantic* half of both features (unknown-column completion + validation),
though not the structural/keyword half.

## 7. Open sub-questions to resolve at build time

- Does dioxus 0.7 `MountedData`/`FormData` expose textarea `selectionStart` without
  custom JS? (Decides whether Option B needs any eval at all.)
- Reuse `dioxus_code::advanced::Buffer` tokens vs. a hand lexer — coupling to their
  tag taxonomy vs. ~200 lines of our own. (Lean hand-lexer is probably cleaner.)
- Engine→UI schema push: shape + where it lands (extend the catalog/session store).
- Squiggle fidelity for the first cut: gutter-only (§3.1) vs. inline overlay (§3.2).

## 8. Verdict

Not blocked. Both features are buildable on this editor. The shared, load-bearing
work is (1) the SQL analysis layer and (2) an owned editor bridge — best delivered by
vendoring the editor (Option B), pending the dioxus-0.7 selection-API check. Design
S7 and S25 as two consumers of that one foundation.
