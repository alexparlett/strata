//! Settings ▸ Engine ▸ Properties — a free-form key/value editor (design24), modeled on
//! JetBrains' Environment Variables. The draft is an ordered [`EngineRow`] list; Apply
//! normalizes it to the persisted `Settings.engine` map. This is **bespoke** (not a
//! strata-forms typed struct): the row list + selection + inspector + name autocomplete
//! don't fit a generic field-array, so [`EngineState`] holds the rows directly.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;

use dioxus::prelude::*;

use crate::ui::components::{
    Badge, BadgeVariant, Button, ButtonVariant, Caption, Control, Icon, IconButton,
    IconButtonVariant, Input, Meta, MonoValue, Popup, Rect, RectAlign, Spacer, Strong, Tooltip,
};
use crate::ui::icons::{IconName, IconSize};

/// A draft key/value row with a stable id (for keying + selection).
#[derive(Clone, PartialEq)]
pub(super) struct EngineRow {
    pub id: usize,
    pub name: String,
    pub value: String,
}

/// The Properties editor's reactive state — provided to the page + footer via context.
#[derive(Clone, Copy)]
pub(super) struct EngineState {
    rows: Signal<Vec<EngineRow>>,
    sel: Signal<Option<usize>>,
    /// Reveal *all* rows' errors — set by Apply (the whole-form gate).
    show_errors: Signal<bool>,
    /// Rows the user has blurred at least once — their errors show inline while editing,
    /// without the mid-type nagging of revealing every row up front.
    touched: Signal<HashSet<usize>>,
    name_menu: Signal<Option<usize>>,
    anchor: Signal<Rect>,
    refs: Signal<HashMap<usize, Rc<MountedData>>>,
    applied: Signal<BTreeMap<String, String>>,
    seq: Signal<usize>,
}

/// Applied map → ordered rows (sorted by key for a tidy first render), ids `0..n`.
fn rows_from_map(map: &BTreeMap<String, String>) -> Vec<EngineRow> {
    map.iter()
       .enumerate()
       .map(|(i, (k, v))| EngineRow {
           id: i,
           name: k.clone(),
           value: v.clone(),
       })
       .collect()
}

/// Create the editor state, seeded from the applied overrides.
pub(super) fn use_engine_state(applied: BTreeMap<String, String>) -> EngineState {
    let initial = rows_from_map(&applied);
    let start_seq = initial.len();
    EngineState {
        rows: use_signal(move || initial),
        sel: use_signal(|| None),
        show_errors: use_signal(|| false),
        touched: use_signal(HashSet::new),
        name_menu: use_signal(|| None),
        anchor: use_signal(|| Rect::point(0.0, 0.0)),
        refs: use_signal(HashMap::new),
        applied: use_signal(move || applied),
        seq: use_signal(move || start_seq),
    }
}

impl EngineState {
    fn next_id(self) -> usize {
        let mut seq = self.seq;
        let id = seq();
        seq.set(id + 1);
        id
    }

    fn add(self) {
        let id = self.next_id();
        self.rows.clone().write().push(EngineRow {
            id,
            name: String::new(),
            value: String::new(),
        });
        self.sel.clone().set(Some(id));
    }

    fn remove(self) {
        // `let-else` (not `if let`) so the `peek()` read-guard on `sel` is released before
        // we mutate below — an `if let` scrutinee keeps its temporary alive for the whole
        // block, which would collide with the write (AlreadyBorrowed panic).
        let Some(id) = *self.sel.peek() else { return };
        let pos = self.rows.peek().iter().position(|r| r.id == id);
        self.rows.clone().write().retain(|r| r.id != id);
        self.refs.clone().write().remove(&id);
        // Keep the keyboard flowing: select + focus the row above (or the new top row when
        // the first row was removed); clear selection only when the list is now empty.
        let next = pos.and_then(|p| self.rows.peek().get(p.saturating_sub(1)).map(|r| r.id));
        match next {
            Some(nid) => {
                self.sel.clone().set(Some(nid));
                self.focus_name(nid);
            }
            None => self.sel.clone().set(None),
        }
    }

    /// Focus a row's name input via its stored mount handle — used after [`remove`] to move
    /// focus to the neighbouring row. Deferred via `spawn` so it runs after the re-render.
    fn focus_name(self, id: usize) {
        let handle = self.refs.peek().get(&id).cloned();
        if let Some(h) = handle {
            spawn(async move {
                let _ = h.set_focus(true).await;
            });
        }
    }

    fn duplicate(self) {
        let Some(id) = *self.sel.peek() else { return };
        let src = self.rows.peek().iter().find(|r| r.id == id).cloned();
        let Some(src) = src else { return };
        let new_id = self.next_id();
        let new_row = EngineRow {
            id: new_id,
            name: src.name.clone(),
            value: src.value.clone(),
        };
        let mut rows = self.rows;
        let pos = rows.peek().iter().position(|r| r.id == id);
        match pos {
            Some(p) => rows.write().insert(p + 1, new_row),
            None => rows.write().push(new_row),
        }
        self.sel.clone().set(Some(new_id));
    }

    fn paste(self, text: &str) {
        let mut added: Vec<EngineRow> = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let (k, v) = line
                .split_once('=')
                .or_else(|| line.split_once('\t'))
                .unwrap_or((line, ""));
            let id = self.next_id();
            added.push(EngineRow {
                id,
                name: k.trim().to_string(),
                value: v.trim().to_string(),
            });
        }
        if let Some(last) = added.last().map(|r| r.id) {
            self.rows.clone().write().extend(added);
            self.sel.clone().set(Some(last));
        }
    }

    fn set_name(self, id: usize, name: String) {
        let mut rows = self.rows;
        let mut lock = rows.write();
        if let Some(r) = lock.iter_mut().find(|r| r.id == id) {
            r.name = name;
        }
    }

    fn set_value(self, id: usize, value: String) {
        let mut rows = self.rows;
        let mut lock = rows.write();
        if let Some(r) = lock.iter_mut().find(|r| r.id == id) {
            r.value = value;
        }
    }

    fn select(self, id: usize) {
        self.sel.clone().set(Some(id));
    }

    fn set_ref(self, id: usize, md: Rc<MountedData>) {
        self.refs.clone().write().insert(id, md);
    }

    /// Focus a row's name field: select it, open its autocomplete menu, and measure the
    /// input so the popup can anchor to it.
    fn open_name_menu(self, id: usize) {
        self.sel.clone().set(Some(id));
        self.name_menu.clone().set(Some(id));
        let handle = self.refs.peek().get(&id).cloned();
        if let Some(h) = handle {
            let mut anchor = self.anchor;
            spawn(async move {
                if let Ok(r) = h.get_client_rect().await {
                    anchor.set(Rect {
                        x: r.origin.x,
                        y: r.origin.y,
                        w: r.size.width,
                        h: r.size.height,
                    });
                }
            });
        }
    }

    fn close_name_menu(self) {
        self.name_menu.clone().set(None);
    }

    fn pick_name(self, id: usize, name: String) {
        self.set_name(id, name);
        self.close_name_menu();
    }

    /// Mark a row blurred, so its error (if any) reveals inline from now on.
    fn touch(self, id: usize) {
        self.touched.clone().write().insert(id);
    }

    /// Discard unsaved edits: rows back to the applied baseline.
    pub(super) fn revert(self) {
        let base = self.applied.peek().clone();
        self.rows.clone().set(rows_from_map(&base));
        self.sel.clone().set(None);
        self.show_errors.clone().set(false);
        self.touched.clone().write().clear();
        self.name_menu.clone().set(None);
    }

    /// `{ rowId → message }` for every invalid row: a value with no name, a duplicate name
    /// (both offending rows flagged), or a value that fails its known key's type check.
    fn errors(&self) -> HashMap<usize, String> {
        let mut out = HashMap::new();
        let mut seen: HashMap<String, usize> = HashMap::new();
        for r in self.rows.read().iter() {
            let n = r.name.trim();
            if n.is_empty() {
                if !r.value.trim().is_empty() {
                    out.insert(r.id, "Enter a property name for this value.".to_string());
                }
                continue;
            }
            if let Some(&first) = seen.get(n) {
                out.insert(r.id, "Duplicate property name.".to_string());
                out.insert(first, "Duplicate property name.".to_string());
            } else {
                seen.insert(n.to_string(), r.id);
                // Value type check for known keys — catches e.g. a non-numeric batch_size or
                // a bad runtime.memory_limit before Apply (runtime keys are never applied
                // live, so DataFusion wouldn't otherwise reject them until restart).
                if let Some(msg) = crate::engine_config::value_error(n, &r.value) {
                    out.insert(r.id, msg);
                }
            }
        }
        out
    }

    /// Normalize rows → the applied `name → value` map (non-blank names, last wins).
    pub(super) fn to_map(&self) -> BTreeMap<String, String> {
        let mut m = BTreeMap::new();
        for r in self.rows.read().iter() {
            let n = r.name.trim();
            if !n.is_empty() {
                m.insert(n.to_string(), r.value.trim().to_string());
            }
        }
        m
    }

    /// Has the draft diverged from the applied baseline?
    pub(super) fn dirty(&self) -> bool {
        self.to_map() != *self.applied.read()
    }

    /// Validate for the footer's Apply: `true` if clean; otherwise reveal the per-row
    /// errors (so Apply can navigate here + block) and return `false`.
    pub(super) fn validate_and_show(self) -> bool {
        if self.errors().is_empty() {
            true
        } else {
            self.show_errors.clone().set(true);
            false
        }
    }
}

/// Autocomplete suggestions for the row whose name field is focused: catalog keys that
/// match the typed query and aren't already used by another row (top 7).
fn suggestions(rows: &[EngineRow], menu_id: usize) -> Vec<&'static crate::engine_config::EngineKey> {
    let Some(row) = rows.iter().find(|r| r.id == menu_id) else {
        return Vec::new();
    };
    let q = row.name.trim().to_lowercase();
    let used: HashSet<&str> = rows
        .iter()
        .filter(|r| r.id != menu_id)
        .map(|r| r.name.trim())
        .filter(|n| !n.is_empty())
        .collect();
    let list: Vec<_> = crate::engine_config::ENGINE_KEYS
        .iter()
        .filter(|e| !used.contains(e.key) && (q.is_empty() || e.key.to_lowercase().contains(&q)))
        .collect();
    if list.len() == 1 && list[0].key.to_lowercase() == q {
        return Vec::new();
    }
    list.into_iter().take(7).collect()
}

#[component]
pub(super) fn Engine() -> Element {
    let st = use_context::<super::SettingsCtx>().engine;
    let rows = st.rows.read().clone();
    let sel = *st.sel.read();
    let show_errors = *st.show_errors.read();
    let touched = st.touched.read().clone();
    let errs = st.errors();
    let dirty = st.dirty();
    let has_sel = sel.map(|id| rows.iter().any(|r| r.id == id)).unwrap_or(false);

    rsx! {
        div { class: "engine-hd",
            Spacer {}
            if dirty {
                Button {
                    variant: ButtonVariant::Ghost,
                    onclick: move |_| st.revert(),
                    Icon { name: IconName::Refresh, size: IconSize::Xs }
                    "Revert changes"
                }
            }
        }
        div { class: "engine-note",
            span { class: "engine-note-ic", Icon { name: IconName::Info, size: IconSize::Sm } }
            Caption { "DataFusion ConfigOptions applied to every new session. Enter any datafusion.* property — names autocomplete. Runtime properties (datafusion.runtime.*) take effect on engine restart." }
        }
        Strong { style: "display:block;margin-bottom:var(--sp-3);", "Configuration properties" }
        div { class: "engine-toolbar",
            IconButton {
                variant: IconButtonVariant::Toolbar, compact: true, class: "engine-add",
                icon: IconName::Plus, icon_size: IconSize::Md, title: "Add property",
                onclick: move |_| st.add(),
            }
            IconButton {
                variant: IconButtonVariant::Danger, compact: true, disabled: !has_sel,
                icon: IconName::Minus, icon_size: IconSize::Md, title: "Remove property",
                onclick: move |_| st.remove(),
            }
            IconButton {
                variant: IconButtonVariant::Toolbar, compact: true, disabled: !has_sel,
                icon: IconName::Copy, icon_size: IconSize::Sm, title: "Duplicate property",
                onclick: move |_| st.duplicate(),
            }
            IconButton {
                variant: IconButtonVariant::Toolbar, compact: true,
                icon: IconName::Clipboard, icon_size: IconSize::Sm, title: "Paste from clipboard",
                onclick: move |_| {
                    if let Ok(mut cb) = arboard::Clipboard::new() {
                        if let Ok(text) = cb.get_text() { st.paste(&text); }
                    }
                },
            }
        }
        div { class: "engine-table",
            div { class: "engine-thead",
                div { class: "engine-th name", Control { style: "color:var(--text2);", "Name" } }
                div { class: "engine-th", Control { style: "color:var(--text2);", "Value" } }
            }
            div { class: "engine-tbody ps-scroll",
                if rows.is_empty() {
                    div { class: "engine-empty",
                        Icon { name: IconName::Lines, size: IconSize::Md }
                        Caption { style: "color:var(--faint2);", "No properties — the engine uses its defaults." }
                    }
                }
                for row in rows.iter().cloned() {
                    {engine_row_view(st, row, sel, &errs, show_errors, &touched)}
                }
            }
        }
        {name_popup(st, &rows)}
        {inspector(st, &rows)}
    }
}

/// One Name/Value row + (below it) its inline error strip when revealed (on Apply, or once
/// the row has been blurred — see `touched`).
fn engine_row_view(
    st: EngineState,
    row: EngineRow,
    sel: Option<usize>,
    errs: &HashMap<usize, String>,
    show_errors: bool,
    touched: &HashSet<usize>,
) -> Element {
    let id = row.id;
    let name = row.name.trim();
    let known = !name.is_empty() && crate::engine_config::key_def(name).is_some();
    let is_custom = !name.is_empty() && !known;
    let restart = crate::engine_config::is_restart_key(name);
    let changed = !name.is_empty()
        && st
        .applied
        .read()
        .get(name)
        .map(|a| a != &row.value)
        .unwrap_or(true);
    let restart_badge = restart && changed;
    let selected = sel == Some(id);
    let err = errs.get(&id).cloned();
    // Reveal on Apply (`show_errors`) or once this row has been blurred (`touched`).
    let show_err = (show_errors || touched.contains(&id)) && err.is_some();
    let err_msg = err.unwrap_or_default();
    let row_cls = if selected { "engine-row sel" } else { "engine-row" };
    let name_color = if is_custom { "var(--warm)" } else { "var(--text)" };
    let err_border = if show_err { "var(--red2)" } else { "transparent" };
    // Keyed by the stable row id so Dioxus preserves each row's DOM node across
    // insert/remove/reorder — that keeps the stored name-input mount handles (used for
    // autocomplete anchoring + post-remove focus) pointing at the right element.
    rsx! {
        div { key: "{id}", class: "engine-rowgroup",
            div { class: row_cls, onpointerdown: move |_| st.select(id),
                div { class: "engine-cell name", style: "border-left:2px solid {err_border};",
                    Input {
                        class: "engine-name-in",
                        style: "color:{name_color};",
                        value: "{row.name}",
                        placeholder: "datafusion.…",
                        onmounted: move |e: Event<MountedData>| st.set_ref(id, e.data()),
                        onfocusin: move |_| st.open_name_menu(id),
                        oninput: move |v: String| st.set_name(id, v),
                        onfocusout: move |_| { st.close_name_menu(); st.touch(id); },
                    }
                    if restart_badge {
                        Tooltip { message: "Runtime property — applies on engine restart",
                            span { class: "engine-restart-badge",
                                Icon { name: IconName::Refresh, size: IconSize::Xs }
                            }
                        }
                    }
                }
                div { class: "engine-cell",
                    Input {
                        class: "engine-val-in",
                        value: "{row.value}",
                        placeholder: "value",
                        onfocusin: move |_| st.select(id),
                        oninput: move |v: String| st.set_value(id, v),
                        onfocusout: move |_| st.touch(id),
                    }
                }
            }
            if show_err {
                div { class: "engine-err-strip",
                    Icon { name: IconName::ErrCircle, size: IconSize::Xs }
                    Caption { style: "color:var(--red2);", "{err_msg}" }
                }
            }
        }
    }
}

/// The floating autocomplete list, anchored to the focused name input.
fn name_popup(st: EngineState, rows: &[EngineRow]) -> Element {
    let Some(id) = *st.name_menu.read() else {
        return rsx! {};
    };
    let sugg = suggestions(rows, id);
    if sugg.is_empty() {
        return rsx! {};
    }
    let anchor = *st.anchor.read();
    rsx! {
        Popup { anchor, align: RectAlign::BOTTOM_START, card_class: "ds-menu engine-ac", width: 440,
            for e in sugg {
                {
                    let key = e.key.to_string();
                    let def = if e.default.is_empty() { "(empty)".to_string() } else { e.default.to_string() };
                    rsx! {
                        div {
                            class: "engine-ac-item",
                            onmousedown: move |ev| ev.prevent_default(),
                            onclick: move |_| st.pick_name(id, key.clone()),
                            MonoValue { class: "engine-ac-key", "{e.key}" }
                            Meta { class: "engine-ac-def", "{def}" }
                        }
                    }
                }
            }
        }
    }
}

/// The inspector strip for the selected row: description + default + RESTART/CUSTOM.
fn inspector(st: EngineState, rows: &[EngineRow]) -> Element {
    let Some(id) = *st.sel.read() else {
        return rsx! {};
    };
    let Some(row) = rows.iter().find(|r| r.id == id) else {
        return rsx! {};
    };
    let name = row.name.trim();
    if name.is_empty() {
        return rsx! {};
    }
    let def = crate::engine_config::key_def(name);
    let restart = crate::engine_config::is_restart_key(name);
    let desc = def
        .map(|e| e.desc.to_string())
        .unwrap_or_else(|| "Custom property — not a recognized DataFusion option. It will be applied as-is.".to_string());
    let default_val = def.map(|e| {
        if e.default.is_empty() {
            "(empty)".to_string()
        } else {
            e.default.to_string()
        }
    });
    let name = name.to_string();
    rsx! {
        div { class: "engine-inspector",
            div { class: "engine-insp-hd",
                MonoValue { style: "color:var(--text2);", "{name}" }
                if restart {
                    Badge { color: "var(--warm)", "Restart" }
                }
                if def.is_none() {
                    Badge { variant: BadgeVariant::Draft, "Custom" }
                }
            }
            Caption { style: "display:block;margin-top:var(--sp-2);", "{desc}" }
            if let Some(d) = default_val {
                div { class: "engine-insp-def",
                    Meta { "Default:\u{00a0}" }
                    Meta { style: "color:var(--text3);", "{d}" }
                }
            }
        }
    }
}
