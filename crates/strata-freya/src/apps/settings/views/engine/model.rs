//! The properties editor's **row list** — what the Engine pane edits, and the one thing in the
//! window that is not a field of [`Settings`](strata_core::config::Settings).
//!
//! The setting itself is a `BTreeMap<String, String>` of non-default overrides, which is the
//! right shape to *apply* and the wrong shape to *edit*: a map cannot hold the row you just
//! added and have not named, cannot hold two rows with the same name long enough for you to fix
//! one, and reorders itself under the cursor. So the editor keeps an ordered list of identified
//! rows and projects it back into the map ([`to_map`](PropRows::to_map)) on every edit — the
//! blank row and the duplicate simply do not survive the projection, which is exactly the
//! behaviour their error messages promise.
//!
//! Row identity is a plain counter, minted per row. It is what selection, the error map and the
//! autocomplete all address, and it has to survive a rename — the name cannot be the key of the
//! thing whose whole purpose is to let you retype a name.
//!
//! Validation is [`strata_core::engine::config`]'s, not this module's: `value_error` already
//! knows every catalogued key's shape, and the two problems it cannot see (an unnamed value, a
//! duplicated name) are properties of the *list* rather than of a key, which is why they live
//! here and nowhere else.

use std::collections::{BTreeMap, BTreeSet};

use strata_core::engine::config::{is_owned_key, key_def, value_error, EngineKey, ENGINE_KEYS};

/// How many catalogue matches the name field offers at once (canvas: 7).
const MAX_SUGGESTIONS: usize = 7;

/// What the catalogue makes of a row's name.
///
/// **One lookup, so the surfaces that dress a row cannot disagree.** Each of them used to ask
/// `key_def` for itself and read `None` as "custom", which silently folded in the case neither
/// names: a **reserved** key is recognised *and* refused, so the inspector called it a custom
/// property the engine "may decline" while the row's own error strip said it was reserved. Two
/// answers to one question. This is that question, asked once.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KeyStatus {
    /// No name typed yet.
    Blank,
    /// A catalogued key, with its entry.
    Known(&'static EngineKey),
    /// A key the **app** owns and config may never set — the catalog and schema our tables live
    /// in, and the planner spans the editor's diagnostics need
    /// ([`is_owned_key`](strata_core::engine::config::is_owned_key)). Absent from the catalogue,
    /// so it can only ever be typed by hand or arrive as a stale saved override.
    Reserved,
    /// Not in the catalogue. Not an error: it may be a key from a DataFusion newer than this
    /// build, which the engine will simply decline.
    Custom,
}

impl KeyStatus {
    /// What the catalogue makes of `key` (already trimmed). The whole rule, in one place —
    /// [`PropRow::status`] is this, and so is every surface that dresses a name box.
    ///
    /// Reserved is checked **before** the catalogue: an owned key is absent from `ENGINE_KEYS` by
    /// design, so a lookup alone cannot tell it from one nobody has heard of.
    pub fn of(key: &str) -> KeyStatus {
        if key.is_empty() {
            return KeyStatus::Blank;
        }
        if is_owned_key(key) {
            return KeyStatus::Reserved;
        }
        match key_def(key) {
            Some(def) => KeyStatus::Known(def),
            None => KeyStatus::Custom,
        }
    }

    /// The catalogue entry behind this status — the *only* thing that carries a default and a
    /// description, which is why `Reserved` and `Custom` have neither to show.
    pub fn def(self) -> Option<&'static EngineKey> {
        match self {
            KeyStatus::Known(def) => Some(def),
            _ => None,
        }
    }
}

/// One row of the editor: a property name and its value, under an id that outlives both.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PropRow {
    pub id: u64,
    pub name: String,
    pub value: String,
}

impl PropRow {
    /// The name with its surrounding space removed — what every question about this row (its
    /// catalogue entry, whether it duplicates another, what it applies as) is asked with.
    pub fn key(&self) -> &str {
        self.name.trim()
    }

    /// What this row's name is, as far as the catalogue is concerned.
    pub fn status(&self) -> KeyStatus {
        KeyStatus::of(self.key())
    }
}

/// The editor's rows, their order, and which one is selected.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct PropRows {
    rows: Vec<PropRow>,
    /// The next id to mint. Monotonic for the list's life, so a removed row's id is never
    /// reused and a stale selection can only fail to resolve, never resolve to the wrong row.
    next_id: u64,
    /// The row the toolbar acts on and the inspector describes.
    pub selected: Option<u64>,
}

impl PropRows {
    /// Seed the list from a saved override map — one row per entry, in the map's own (sorted)
    /// order, nothing selected.
    pub fn from_map(overrides: &BTreeMap<String, String>) -> Self {
        let mut list = Self::default();
        list.reseed(overrides);
        list
    }

    /// Replace every row from an override map, minting ids from this list's own counter.
    fn reseed(&mut self, overrides: &BTreeMap<String, String>) {
        self.rows.clear();
        for (name, value) in overrides {
            self.push(name.clone(), value.clone());
        }
        // `push` selects what it adds, which is right for a row the user asked for and wrong
        // for a whole list arriving at once.
        self.selected = None;
    }

    /// The rows, in display order.
    pub fn rows(&self) -> &[PropRow] {
        &self.rows
    }

    /// Project the rows back into the override map the engine is configured from: names
    /// trimmed, unnamed rows dropped, and a duplicated name resolved the way the editor shows
    /// it — the last value typed wins.
    ///
    /// Total by design. A list carrying errors still projects; what stops it reaching the
    /// engine is [`errors`](Self::errors) blocking Apply, not this returning something partial.
    pub fn to_map(&self) -> BTreeMap<String, String> {
        self.rows
            .iter()
            .filter(|row| !row.key().is_empty())
            .map(|row| (row.key().to_string(), row.value.trim().to_string()))
            .collect()
    }

    /// Append a row and select it. Returns its id.
    fn push(&mut self, name: String, value: String) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.rows.push(PropRow { id, name, value });
        self.selected = Some(id);
        id
    }

    /// Add a blank row at the end, selected and ready to be named.
    pub fn add(&mut self) -> u64 {
        self.push(String::new(), String::new())
    }

    /// Select the row for `key` if the property is overridden, and report whether there was one.
    ///
    /// What the Settings search does with a `datafusion.*` hit (P4-09) once it has routed to this
    /// pane. It deliberately **does not add** a row for a property nobody has set: following a
    /// search result is navigation, and this list is an edit. A named row with no value still
    /// projects into the draft (`to_map` only drops the *unnamed* ones), so an auto-added row would
    /// leave Apply live for an override the user never asked for — and the grid, whose whole claim
    /// is that it lists the overrides in force, showing one that isn't.
    pub fn reveal(&mut self, key: &str) -> Option<u64> {
        let id = self
            .rows
            .iter()
            .find(|row| row.key() == key)
            .map(|r| r.id)?;
        self.selected = Some(id);
        Some(id)
    }

    /// Remove the selected row, selecting the one that takes its place (or the new last row,
    /// when the removed one was last). A no-op with nothing selected.
    pub fn remove_selected(&mut self) {
        let Some(at) = self.selected_index() else {
            return;
        };
        self.rows.remove(at);
        self.selected = self
            .rows
            .get(at)
            .or_else(|| self.rows.last())
            .map(|row| row.id);
    }

    /// Copy the selected row in below itself, and select the copy — the fast way to write a
    /// second key in the same namespace.
    pub fn duplicate_selected(&mut self) {
        let Some(at) = self.selected_index() else {
            return;
        };
        let id = self.next_id;
        self.next_id += 1;
        let copy = PropRow {
            id,
            ..self.rows[at].clone()
        };
        self.rows.insert(at + 1, copy);
        self.selected = Some(id);
    }

    /// Append the rows in `text` — one per non-blank line, split on the first `=` or tab, with
    /// a line carrying neither taken as a bare name. Selects the last row added.
    ///
    /// Lenient on purpose: the text comes from a clipboard, so it is as likely to be a block
    /// copied out of `datafusion.conf` as it is to be one key someone read in an issue. What it
    /// cannot parse it still adds, as a name with no value, where the editor's own validation
    /// can say what is wrong with it.
    pub fn paste(&mut self, text: &str) {
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            match line.split_once(['=', '\t']) {
                Some((name, value)) => {
                    self.push(name.trim().to_string(), value.trim().to_string());
                }
                None => {
                    self.push(line.to_string(), String::new());
                }
            }
        }
    }

    /// Replace every row from a saved override map — the Revert action. Nothing stays selected:
    /// the row the inspector was describing may not exist any more.
    ///
    /// Reseeded rather than rebuilt from `default`, so the id counter carries across: a row from
    /// before the revert can never share an id with one from after it, and a selection or an
    /// in-flight edit still addressed by id can only fail to resolve, never resolve to a
    /// different property.
    pub fn revert(&mut self, overrides: &BTreeMap<String, String>) {
        self.reseed(overrides);
    }

    pub fn set_name(&mut self, id: u64, name: String) {
        if let Some(row) = self.row_mut(id) {
            row.name = name;
        }
    }

    pub fn set_value(&mut self, id: u64, value: String) {
        if let Some(row) = self.row_mut(id) {
            row.value = value;
        }
    }

    /// Row `id`'s name / value as the list currently holds them — what a text box compares
    /// against before pushing a change, so a keystroke that changes nothing writes nothing.
    pub fn name_of(&self, id: u64) -> Option<String> {
        self.rows
            .iter()
            .find(|row| row.id == id)
            .map(|row| row.name.clone())
    }

    pub fn value_of(&self, id: u64) -> Option<String> {
        self.rows
            .iter()
            .find(|row| row.id == id)
            .map(|row| row.value.clone())
    }

    /// The selected row, if it still exists.
    pub fn selected_row(&self) -> Option<&PropRow> {
        let id = self.selected?;
        self.rows.iter().find(|row| row.id == id)
    }

    /// Whether the list holds anything to revert or clear.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Why each faulty row cannot be applied, by row id — the whole of what blocks Apply.
    ///
    /// Three kinds, in the order a row hits them: a value with no name to apply it to, a name
    /// that another row already claims (both rows are marked, because either is the one to fix),
    /// and a value that does not fit the key's shape.
    pub fn errors(&self) -> BTreeMap<u64, String> {
        let mut errors = BTreeMap::new();
        let mut claimed: BTreeMap<&str, u64> = BTreeMap::new();
        for row in &self.rows {
            let key = row.key();
            if key.is_empty() {
                if !row.value.trim().is_empty() {
                    errors.insert(row.id, "Enter a property name for this value.".to_string());
                }
                continue;
            }
            if let Some(first) = claimed.get(key) {
                let message = "Duplicate property name.".to_string();
                errors.insert(*first, message.clone());
                errors.insert(row.id, message);
                continue;
            }
            claimed.insert(key, row.id);
            if let Some(message) = value_error(key, &row.value) {
                errors.insert(row.id, message);
            }
        }
        errors
    }

    /// Catalogue names to offer for row `id`: every known key containing what has been typed,
    /// minus the ones other rows already claim, capped at [`MAX_SUGGESTIONS`].
    ///
    /// Empty once the typed name matches its only remaining candidate exactly — at that point
    /// the list can only offer back what is already in the box.
    pub fn suggestions(&self, id: u64) -> Vec<&'static EngineKey> {
        let Some(row) = self.rows.iter().find(|row| row.id == id) else {
            return Vec::new();
        };
        let typed = row.key().to_lowercase();
        let claimed: BTreeSet<&str> = self
            .rows
            .iter()
            .filter(|other| other.id != id)
            .map(PropRow::key)
            .filter(|key| !key.is_empty())
            .collect();
        let matches: Vec<&'static EngineKey> = ENGINE_KEYS
            .iter()
            .filter(|entry| !claimed.contains(entry.key))
            .filter(|entry| typed.is_empty() || entry.key.to_lowercase().contains(&typed))
            .collect();
        match matches.as_slice() {
            [only] if only.key.to_lowercase() == typed => Vec::new(),
            _ => matches.into_iter().take(MAX_SUGGESTIONS).collect(),
        }
    }

    fn selected_index(&self) -> Option<usize> {
        let id = self.selected?;
        self.rows.iter().position(|row| row.id == id)
    }

    fn row_mut(&mut self, id: u64) -> Option<&mut PropRow> {
        self.rows.iter_mut().find(|row| row.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// A known key, so the tests exercise the catalogue rather than only the list.
    const BATCH: &str = "datafusion.execution.batch_size";
    const RUNTIME: &str = "datafusion.runtime.memory_limit";

    #[test]
    fn a_seeded_list_round_trips_through_the_map() {
        let overrides = map(&[(BATCH, "4096"), (RUNTIME, "2G")]);
        let rows = PropRows::from_map(&overrides);
        assert_eq!(rows.rows().len(), 2);
        assert_eq!(rows.to_map(), overrides);
        assert!(rows.errors().is_empty());
        assert_eq!(rows.selected, None, "seeding selects nothing");
    }

    #[test]
    fn an_unnamed_row_is_dropped_from_the_map_and_only_complains_once_it_has_a_value() {
        let mut rows = PropRows::from_map(&map(&[(BATCH, "4096")]));
        let blank = rows.add();
        assert_eq!(rows.to_map(), map(&[(BATCH, "4096")]));
        assert!(
            rows.errors().is_empty(),
            "a row you just added is not a fault"
        );

        rows.set_value(blank, "8".into());
        assert_eq!(
            rows.errors().get(&blank).map(String::as_str),
            Some("Enter a property name for this value.")
        );
        assert_eq!(rows.to_map(), map(&[(BATCH, "4096")]), "still not applied");
    }

    #[test]
    fn a_duplicate_name_marks_both_rows_and_the_last_value_wins() {
        let mut rows = PropRows::default();
        let first = rows.push(BATCH.into(), "1024".into());
        let second = rows.push(BATCH.into(), "4096".into());

        let errors = rows.errors();
        assert_eq!(errors.len(), 2, "either row is the one to fix");
        assert_eq!(errors[&first], "Duplicate property name.");
        assert_eq!(errors[&second], "Duplicate property name.");
        assert_eq!(rows.to_map(), map(&[(BATCH, "4096")]));
    }

    #[test]
    fn a_value_that_does_not_fit_the_key_is_an_error_and_a_custom_key_is_not() {
        let mut rows = PropRows::default();
        let bad = rows.push(BATCH.into(), "not a number".into());
        let custom = rows.push("datafusion.made.up".into(), "anything".into());

        let errors = rows.errors();
        assert!(errors.contains_key(&bad), "batch_size is an Int");
        assert!(
            !errors.contains_key(&custom),
            "an uncatalogued key has no shape to check against"
        );
    }

    /// Revealing a property (the Settings search's engine hits, P4-09) selects the row it has, and
    /// adds **nothing** when it has none: following a search result is navigation, and a row here is
    /// an override.
    #[test]
    fn revealing_a_property_selects_its_row_and_never_adds_one() {
        let mut rows = PropRows::from_map(&map(&[(BATCH, "4096")]));
        let before = rows.to_map();

        let existing = rows.reveal(BATCH);
        assert_eq!(existing, Some(rows.rows()[0].id));
        assert_eq!(rows.selected, existing);

        assert_eq!(rows.reveal(RUNTIME), None, "not overridden, so no row");
        assert_eq!(rows.rows().len(), 1, "and none was made for it");
        assert_eq!(rows.to_map(), before, "the draft is untouched");

        // A name typed with space around it is the same property, so the reveal finds it.
        let padded = rows.push(
            format!("  {}  ", "datafusion.explain.format"),
            String::new(),
        );
        assert_eq!(rows.reveal("datafusion.explain.format"), Some(padded));
    }

    #[test]
    fn removing_selects_the_row_that_takes_its_place() {
        let mut rows = PropRows::default();
        let first = rows.push("a".into(), String::new());
        let second = rows.push("b".into(), String::new());
        let third = rows.push("c".into(), String::new());

        rows.selected = Some(second);
        rows.remove_selected();
        assert_eq!(rows.selected, Some(third), "the row now at that index");

        rows.remove_selected();
        assert_eq!(rows.selected, Some(first), "removing the last falls back");

        rows.remove_selected();
        assert_eq!(rows.selected, None, "nothing left to select");
        rows.remove_selected();
        assert!(rows.is_empty(), "and removing nothing is a no-op");
    }

    #[test]
    fn duplicating_inserts_below_and_selects_the_copy() {
        let mut rows = PropRows::default();
        let first = rows.push(BATCH.into(), "1024".into());
        rows.push("z".into(), String::new());
        rows.selected = Some(first);

        rows.duplicate_selected();
        let ids: Vec<u64> = rows.rows().iter().map(|row| row.id).collect();
        assert_eq!(ids[0], first);
        assert_eq!(rows.selected, Some(ids[1]), "the copy is selected");
        assert_ne!(ids[1], first, "and it is a different row");
        assert_eq!(rows.rows()[1].name, BATCH);
        assert_eq!(rows.rows()[1].value, "1024");
    }

    #[test]
    fn paste_takes_equals_tabs_and_bare_names_and_skips_blank_lines() {
        let mut rows = PropRows::default();
        rows.paste(&format!(
            "{BATCH} = 4096\n\n{RUNTIME}\t2G\ndatafusion.explain.format\n   \n"
        ));

        assert_eq!(rows.rows().len(), 3);
        assert_eq!(rows.to_map(), {
            let mut expected = map(&[(BATCH, "4096"), (RUNTIME, "2G")]);
            expected.insert("datafusion.explain.format".into(), String::new());
            expected
        });
        assert_eq!(
            rows.selected,
            Some(rows.rows()[2].id),
            "the last row pasted"
        );
    }

    #[test]
    fn revert_restores_the_saved_map_and_never_reuses_a_row_id() {
        let saved = map(&[(BATCH, "4096")]);
        let mut rows = PropRows::from_map(&saved);
        let before: Vec<u64> = rows.rows().iter().map(|row| row.id).collect();
        rows.add();
        rows.paste("datafusion.explain.format = tree");

        rows.revert(&saved);
        assert_eq!(rows.to_map(), saved);
        assert_eq!(rows.selected, None);
        let after: Vec<u64> = rows.rows().iter().map(|row| row.id).collect();
        assert!(
            after.iter().all(|id| !before.contains(id)),
            "a stale selection must not resolve to a different property"
        );
    }

    #[test]
    fn suggestions_match_anywhere_hide_claimed_keys_and_stop_at_an_exact_hit() {
        let mut rows = PropRows::default();
        let editing = rows.push("batch".into(), String::new());
        assert!(
            rows.suggestions(editing).iter().any(|e| e.key == BATCH),
            "a substring matches, not only a prefix"
        );

        rows.push(BATCH.into(), "4096".into());
        assert!(
            !rows.suggestions(editing).iter().any(|e| e.key == BATCH),
            "another row already claims it"
        );

        let mut rows = PropRows::default();
        let exact = rows.push(BATCH.into(), String::new());
        assert!(
            rows.suggestions(exact).is_empty(),
            "offering back what is already typed is not a suggestion"
        );
    }

    #[test]
    fn suggestions_are_capped_and_a_blank_name_offers_the_catalogue() {
        let mut rows = PropRows::default();
        let blank = rows.add();
        assert_eq!(rows.suggestions(blank).len(), MAX_SUGGESTIONS);
    }

    #[test]
    fn a_reserved_key_is_refused_by_the_catalogue_rather_than_silently_ignored() {
        let mut rows = PropRows::default();
        let owned = rows.push("datafusion.catalog.default_schema".into(), "public".into());
        assert!(
            rows.errors().contains_key(&owned),
            "Strata names its own catalog and schema"
        );
    }

    /// The autocomplete can only offer what the catalogue holds, so this is what keeps a reserved
    /// key from ever being suggested. A comment saying they are "deliberately absent" is not a
    /// guard: adding one to `ENGINE_KEYS` would put it in the dropdown, where picking it produces
    /// a row that is invalid the instant it is created.
    #[test]
    fn the_catalogue_holds_no_key_the_app_reserves() {
        for entry in ENGINE_KEYS {
            assert!(
                !is_owned_key(entry.key),
                "{} is reserved and must not be offered",
                entry.key
            );
        }
    }

    #[test]
    fn a_reserved_name_is_its_own_status_not_a_custom_one() {
        let mut rows = PropRows::default();
        let reserved = rows.push("datafusion.catalog.default_catalog".into(), "x".into());
        let custom = rows.push("datafusion.made.up".into(), "x".into());
        let known = rows.push(BATCH.into(), "4096".into());
        let blank = rows.add();

        let status = |id: u64| {
            rows.rows()
                .iter()
                .find(|row| row.id == id)
                .expect("row")
                .status()
        };
        // The conflation this type exists to prevent: reserved is recognised *and* refused, so a
        // surface reading it as "custom" tells the user the engine "may decline" a key it is
        // certain to.
        assert_eq!(status(reserved), KeyStatus::Reserved);
        assert_eq!(status(custom), KeyStatus::Custom);
        assert!(matches!(status(known), KeyStatus::Known(def) if def.key == BATCH));
        assert_eq!(status(blank), KeyStatus::Blank);
    }
}
