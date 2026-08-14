# UP-03 · Surfaces: launcher affordance, dialog, setting, menubar item

**Workstream:** Updater · **Status:** ✅ (2026-08-13) · **Depends on:** UP-02 (`UpdateStatus` +
actions — all landed 2026-08-12)

## What UP-02 left for this task
- `state::updates`: `Update` (the status enum), `UpdateStatus` on `AppCtx` as `app.updates`,
  and the three actions `check` / `download` / `install`. `download` and `install` carry an
  `#[allow(dead_code)]` naming this task as the presser — **remove both allows** when the
  surfaces land, or they hide a real regression later.
- `state::updates::CURRENT` is the running version. The launcher rail's
  `Meta::new(env!("CARGO_PKG_VERSION"))` should read that const instead, so the number the check
  compares against and the number the rail prints are the same one.
- `state::updates::install_site()` answers install eligibility (`Unbundled` / `ReadOnly` /
  `Writable`), cached once per process. That is what step 2's degraded wording keys on; the
  affordance shows nothing at all when it is `Unbundled`.
- `Update::Downloading` carries `got` / `total` for step 2's progress text, and `Update::Ready`
  carries `version` and `page_url` for step 3's dialog body and its link-out.
- A `Ready` status is deliberately **not** re-checked, so a manual check is a no-op there
  as well as while `Checking` / `Downloading`.

## Goal
The mechanism becomes visible: a setting that gates the automatic check, an affordance where the
version already shows, one confirm-shaped dialog in front of the restart, and one item for
checking on demand. Deliberately quiet — no toast system, no badge on every window.

## Current state (verified 2026-08-12)
- The launcher rail prints the version — `Meta::new(env!("CARGO_PKG_VERSION"))` at
  `apps/launcher/views/rail.rs:68`. This is the one place the app already talks about its
  version, which makes it the natural update affordance.
- The results status bar is **per results pane** (`workbench/results/status_bar.rs`,
  constructed per `ResultsState`) — it is not an app status bar and is the wrong home for an
  app-scoped indicator.
- Settings toggles are five mechanical edits, four compiler-checked: field on `Settings`
  (`strata-core/src/config.rs:144`, `#[serde(default …)]`), `Default` (`config.rs:365-390`),
  the `settings_merge!` list (`config.rs:307-323` — omission is a build error), a
  `settings_index!` entry (`apps/settings/search.rs:95-192`), and the pane row. The boolean-row
  template is `ConfirmClose` (`apps/settings/views/system.rs:97-107`): `Anchor::X.row()` +
  `Switch`, both handlers through `SettingsCtx::edit`. A **new** field with a serde default is
  safe against existing config files; only a **changed** default needs migration
  (`config.rs:490-499`).
- Confirm dialogs are the slot pattern: a `State<Option<T>>` provided at the window root, the
  dialog mounted unconditionally and watching it, askers `slot.set(Some(..))`. Precedence is
  mount order (`apps/project/app.rs:756-796`). Two documented traps: read the slot into a value
  **before** `set(None)` (generational borrow across a match panics —
  `dialogs/close_confirm.rs:79-83`), and dismiss before any action that unmounts the subtree,
  via `spawn_forever` (`close_confirm.rs:91-99`).
- The palette is registered in `apps/project/commands.rs` (`#[command_router]` at `:139`);
  `close_project` (`:210-219`) is the template for a command acting through `PaletteCtx`
  handles. **The palette exists only in project windows** (`app.rs:792`) — it cannot be the
  launcher's path to the updater.

## Build

1. **The setting — the field already exists.** UP-02 added `Settings::check_updates` (default
   **true**, `#[serde(default)]`, in `Default`, in the `settings_merge!` list) because its startup
   check is gated on it. What is left here is the **row** and the **`settings_index!` entry** —
   two of the five edits above, neither compiler-checked. The row lands in
   Settings ▸ System beside `ConfirmClose`, wording in the app's IDE register — label
   `Check for updates`, hint one plain sentence (e.g.
   `Ask GitHub for a newer release when Strata starts.`). The toggle gates only the
   *automatic* check; manual checks always work.
2. **The launcher affordance.** The rail's version line becomes state-aware: on
   `Available`/`Ready` it gains an accent action under the version —
   `Update to 0.4.0` (press starts the download) / `Restart to update` (press opens the confirm
   below); `Downloading` shows quiet progress text. `Idle`/`UpToDate`/`Failed` change nothing —
   the rail never nags, and a failed check is a log line, not launcher chrome. When install
   eligibility is degraded (UP-02: no writable bundle), the action opens the release page
   instead and says so in its own wording.
3. **The dialog.** `UpdateConfirm` over its own `State<Option<UpdateAsk>>`, standard 420
   confirm (`components/dialog.rs`; Enter-confirm belongs to the card — the modal base owns
   only Esc). Body: the version it will restart into, and a plain link-out to the release page
   for what changed. Confirm = the UP-02 install action (quit-shaped; every close confirm still
   gets its say **after** it — this dialog asks "restart now?", it does not re-ask "lose the
   running query?", which stays the close confirm's question). Dismiss = status stays `Ready`,
   quietly. Mount it in **both** window kinds that can offer it (launcher root; project root
   after the existing confirms, `app.rs:763-796`), one component, two mounts — the slot is
   per-window, the status behind it is the one app-global.
4. **The palette command.** `Check for updates` in `commands.rs` — no `key` (no keymap change),
   keywords `update upgrade version release`. Body: run the manual check via the `UpdateStatus`
   handle on `PaletteCtx` (new field, gathered in `use_palette_ctx`); if the answer is
   `Available`, surface the project window's affordance path (open the dialog once downloaded,
   or start the download — same presses as the rail, through the same actions; the command adds
   no second implementation). While `Checking`/`Downloading`, the command is a no-op re-press.
5. **Project-window visibility (kept minimal, on purpose).** No persistent indicator in v1: the
   launcher rail and the menubar item are the surfaces. If an in-project indicator is wanted
   later it is a phase-5 design question (where does app chrome live in a window that is all
   panes?) — note it there rather than inventing chrome here.

## Acceptance
- [x] Toggle in Settings ▸ System, searchable, applied through the draft/apply funnel; off means
      no startup check, and the manual command still works.
- [x] Launcher rail offers the update through its existing version line; no state change for
      up-to-date/failed.
- [x] One `UpdateConfirm`, mounted at launcher and project roots, slot pattern, borrow-trap
      safe; confirm quits through the normal path (close confirms still fire), dismiss keeps
      `Ready`.
- [x] ~~Palette command~~ — **cut**, see below. Its job is App ▸ *Check for Updates…*, which acts
      through the same actions as the rail and carries no keybinding either.
- [x] All user-facing text in the IDE register (AGENTS.md §3): terse, single-quoted
      identifiers, no glyphs.

## What was built, and the four things the plan did not have

Everything above landed as written **except step 4**, which was built and then cut, and the plan
gained a **menubar item** (App ▸ *Check for Updates…*) — asked for while the task was in flight,
and the thing that shaped the rest.

**0. The palette row is gone.** It shipped, and was removed on the same review pass that added the
menubar item: the launcher rail's version line and App ▸ *Check for Updates…* are already the two
places the app talks about an update, and a third surface to keep in step with them buys a gesture
nobody reaches for by name. `commands.rs`'s "what is deliberately absent" note records it beside
the Export row, because the cost of putting it back is one method — `updater::press` is still the
funnel — and the reason not to is a judgement rather than an obstacle.

**1. One pure decision, `updater::Affordance`, rather than four surfaces each restating the
rules.** The plan describes the same conditions three times over (steps 2, 3 and 4) — show
nothing on `Idle`/`UpToDate`/`Failed`, degrade to the release page when the site is not writable,
open the confirm on `Ready`. With a fourth surface arriving that is exactly the shape that goes
wrong in one place and right in three, so `Affordance::of(&Update, &Site)` is the one answer and
`press` is a thin match over it, each arm a call into `state::updates`. Both are in
**`src/updater.rs`**, a new top-level module: `apps/` is one folder per OS window and this is read
by two of them. The rules are unit-tested without a window; `press` deliberately is not, because
it asks `install_site()` and a test binary is never a bundle — handing the site in to make it
testable is AGENTS.md §1's refused shape.

**2. The rail draws a note *and* an action, not one line.** The plan's labels name the version
(`Update to 0.4.0`), which does not fit a 200px rail once the degraded wording has to say it
opens a page. So the version moved to a quiet `Meta` line above a short accent action — and that
line does the download's progress too, so exactly one vocabulary covers "what is on offer" and
"how it is getting on". `Restart` says *downloaded* where `Get` says *available*, because the two
differ in what the press below costs.

**3. The confirm slot is the project *window's*, not the loaded subtree's** (the plan said
`app.rs:763-796`, inside `ProjectLoaded`). Two reasons, both from the menubar: `use_register_window`
is called on the window layer, so that is where the `MenuScope` can carry the slot; and an update
is a fact about the app, so a re-root must not drop a question the user has been asked. It mounts
beside `OpenPrompt`, the window layer's existing modal.

The menubar item itself needed two more pieces. It carries **no chord**, so unlike every other
custom item it cannot reach its window through the keyboard pipeline — and it cannot act directly
either: `handle_menu_event` runs on the renderer thread, outside Freya's current context, where
the `spawn_forever` inside `check`/`download` panics (and a release build catches that and exits
the process). **A first version did exactly that and was caught in review.** So the press sets
`AppCtx::update_request`, a plain `bool` a context-free `State::set` can write, and the focused
window drains it from `use_file_menu`'s effect where there *is* a scope to spawn in — AGENTS.md
§3's rule for a press with no scope of its own. Open Recent sits on the same edge and is the
precedent: it hand-rolls its open rather than calling `OpenCtx::apply`, whose `NewWindow` arm is a
`spawn_forever`. The item is disabled outside a bundle as well as over a panel, since the updater
is inert in a `cargo run` build and an enabled item there is the failure `Gate` exists to prevent.

Two smaller things review also caught, both fixed: `Update::Failed` was written five times and
read nowhere, so every failure — the signature refusals included — was silent in a design whose
own comment called it "a log line rather than chrome"; `state::updates::failed` is now the one
constructor and it logs. And the dialog promised that a window with a running query still asks
before it closes, which is false with `confirm_close_running` off; it now says a window that
*would* ask before quitting still asks.

Step 5 stands: no persistent in-project indicator.

## The follow-up: the menubar item had no answer (2026-08-14)

Shipped as above, App ▸ *Check for Updates…* over an up-to-date app **did nothing visible**. The
quiet rule was right about the rail and over-applied to the item: `Idle`/`Checking`/`UpToDate`/
`Failed` draw nothing anywhere, so a question the user asked by name went unanswered — the same
"looks live, does nothing" failure `Gate` exists to prevent, reached from the other side.

What landed:

- **`updater::raise`**, the menubar's own thin match over the one `Affordance`: raise
  `UpdateAsk::Report` on the pressing window's slot, *then* check, so the answer lands in the card
  the press opened — and it checks over an offer it already has, because the item says *check* and
  a startup `Available` can be a release stale. A staged update still diverts to the restart
  question (`press`'s), a download in flight only reports, and a dev build still offers nothing.
  The rail's `press` is untouched — there, pressing *is* the offer. **Review caught one bug in
  it**: the affordance was resolved in the `match` scrutinee, so the `peek` guard outlived the arm
  and `check`'s `status.set` panicked on the ordinary press — the generational-borrow trap the
  confirm dialogs already record. Bound in a `let` first, exactly as `press` does.
- **`UpdateAsk` is two questions.** `Restart` keeps carrying its version; `Report` carries
  nothing, because it is a view of the app-global status rather than a question about one release.
- **`Report::of`** — one pure match over the status for glyph, tone, title, subject and body, so
  the card cannot pair a tick with a failure. The subject is `Affordance::note` (one vocabulary
  with the rail) and the single accent action is `Affordance::action` through `press`, so the card
  offers nothing the rail would not and a download started in it reports progress in place.
- **The changelog**, asked for in the same pass: `update::Offer::notes` carries GitHub's release
  body (already read by the check, normalized line endings, `null` parsed as `Option`) through all
  three offer states — and on through `Affordance::Restart` to the **restart** card, which is the
  first sight of it when the download was started from the rail — and `Changelog` renders it with
  the chat pane's `MarkdownViewer` in a fixed scrolling well at the type scale's small sizes. That
  needed **`MarkdownViewer::theme` in the fork** — it was the one themed component with no
  per-instance setter (AGENTS.md §6: fix the fork, don't grow an app-side token). Fork commit must
  be pushed with this change.
- **A local releases server** (`strata-core/examples/fake_releases.rs`, `STRATA_UPDATE_ORIGIN`),
  because none of the above can be *driven* otherwise: the mechanism is inert outside a bundle, so
  a `cargo run` has no site, no offer and a disabled menu item — and even bundled there is no
  newer release unless you cut one. A first version **faked the statuses** app-side and was
  replaced: it was a second state machine beside the real one, and everything it proved was about
  itself. Pointing the *check* at `127.0.0.1` instead keeps the whole ladder real — same request,
  same JSON, a real archive downloaded with real progress and unpacked by the same `ditto` — and
  costs three debug-only seams: the origin, the signature check (a locally-made bundle carries no
  Apple signature and never could, so it is skipped **keyed on the origin**, never on a flag of
  its own), and the site + install refusal app-side. All `cfg`'d out of a release build, which is
  what makes an environment variable an acceptable way to ask. Scenarios are typed at the
  server's prompt and picked up by App ▸ Check for Updates…, which re-checks over a known offer.

## References
- `apps/launcher/views/rail.rs:68` — the version line.
- `apps/settings/views/system.rs:97-107` — the toggle-row template; `search.rs:95` — the index.
- `components/dialog.rs`, `dialogs/close_confirm.rs` — the confirm shape and its two traps.
- `apps/project/commands.rs:139,210-227` — the router and the command templates.
