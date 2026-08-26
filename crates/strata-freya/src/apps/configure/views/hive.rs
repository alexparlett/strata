//! **HIVE PARTITIONING** — the header and its enable switch, then one row per partition column
//! with the type it is read as, and the cast warning while any is left as text.
//!
//! The section appears only when a source path resolves to *many* files, because that is the
//! only shape a partition layout can have. It is **not** gated on parquet, unlike the canvas:
//! partition columns are a listing feature, not a parquet one — DataFusion reads a
//! Hive-partitioned CSV lake perfectly well, and `TableDef.partition_cols` has always been
//! format-agnostic. Gating it would hide a def's own stored columns the moment its format
//! changed.
//!
//! The columns themselves are **found, not guessed** (`strata_engine::detect_partitions`):
//! the paths are read for `key=` segments they name, and a directory is walked for the
//! `key=value` folders under it. The canvas says the section found them, so it has to have
//! looked.

use freya::prelude::*;
use freya::radio::use_radio_station;

use crate::apps::configure::model::PARTITION_TYPES;
use crate::apps::configure::ConfigureCtx;
use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::{ProjChan, ProjectState};
use crate::components::form::{Row, FIELD_HEIGHT};
use crate::components::icon::{Icon, IconName};
use crate::components::metrics::{SP_2, SP_3, SP_4};
use crate::components::tones::tones;
use crate::components::typography::{Caption, MonoValue, Prose};
use crate::components::window::window_theme;

/// The gap under the section header, between its rows, and beside a control.
const HEADER_GAP: f32 = SP_3;
const ROW_GAP: f32 = SP_3;
const CONTROL_GAP: f32 = SP_4;
/// The canvas's fixed column for a partition column's name.
const NAME_WIDTH: f32 = 110.;
const NAME_ICON: f32 = 11.;
/// The type picker's column.
const TYPE_WIDTH: f32 = 120.;
const WARNING_ICON: f32 = 12.;

#[derive(PartialEq)]
pub struct Hive;

impl Component for Hive {
    fn render(&self) -> impl IntoElement {
        let ctx = use_consume::<ConfigureCtx>();
        let project = use_radio_station::<ProjectState, ProjChan>();
        let root = project.peek().root.clone();
        let engine = use_consume::<EngineCtx>();
        let (may_partition, on, columns, warn) = {
            let draft = ctx.draft.read();
            (
                draft.may_partition(&root),
                draft.hive_on,
                draft.partitions.clone(),
                draft.partitions_are_text(),
            )
        };
        let toggle = rect()
            .horizontal()
            .cross_align(Alignment::Center)
            .spacing(CONTROL_GAP)
            .child(
                Switch::new()
                    .toggled(on)
                    .on_toggle(move |()| toggle(ctx, engine.clone(), &root)),
            )
            .child(Prose::new(match on {
                true => "Reading the folder tree as partition columns",
                false => "Ignoring the folder tree, reading the files as one flat table",
            }));

        rect()
            .width(Size::fill())
            .maybe_child(may_partition.then(|| {
                Row::new("HIVE PARTITIONING")
                    .child(toggle)
                    .maybe_child(on.then(|| {
                        rect()
                            .width(Size::fill())
                            .vertical()
                            .spacing(ROW_GAP)
                            .padding(Gaps::new(HEADER_GAP, 0., 0., 0.))
                            .children(columns.iter().enumerate().map(|(index, (name, dtype))| {
                                PartitionRow {
                                    index,
                                    name: name.clone(),
                                    dtype: dtype.clone(),
                                    key: DiffKey::None,
                                }
                                .key(name.clone())
                                .into_element()
                            }))
                            .maybe_child(warn.then_some(Warning))
                    }))
            }))
    }
}

/// Turn partitioning on or off — **detecting the columns on the way on**, and only when there
/// are none yet, so a def's own stored types are never overwritten by a fresh scan.
///
/// The toggle flips at once and the columns arrive after: detection lists the store, which is a
/// round trip per level and may be a network one. Spawned rather than awaited inline for that
/// reason, and re-checked inside the write, because the user can flip it back while it runs.
fn toggle(ctx: ConfigureCtx, engine: EngineCtx, root: &std::path::Path) {
    let (connection, paths) = {
        let draft = ctx.draft.peek();
        (draft.store().map(str::to_string), draft.nonblank_sources())
    };
    let root = root.to_path_buf();
    let detect = {
        let mut draft = ctx.draft.peek().clone();
        draft.hive_on = !draft.hive_on;
        let detect = draft.hive_on && draft.partitions.is_empty();
        ctx.edit(move |d| d.hive_on = !d.hive_on);
        detect
    };
    if !detect {
        return;
    }
    spawn(async move {
        let found = engine
            .catalog()
            .detect_partitions(connection, Some(root), paths)
            .await;
        ctx.edit(move |draft| {
            if draft.hive_on && draft.partitions.is_empty() {
                draft.partitions = found
                    .into_iter()
                    .map(|name| (name, "Utf8".to_string()))
                    .collect();
            }
        });
    });
}

/// One partition column: its name, and the pill of types it can be read as.
#[derive(PartialEq)]
struct PartitionRow {
    index: usize,
    name: String,
    dtype: String,
    key: DiffKey,
}

impl KeyExt for PartitionRow {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for PartitionRow {
    fn render(&self) -> impl IntoElement {
        let win = window_theme();
        let ctx = use_consume::<ConfigureCtx>();
        let index = self.index;

        let types: Vec<Element> = PARTITION_TYPES
            .iter()
            .map(|dtype| {
                let dtype = *dtype;
                MenuItem::new()
                    .selected(dtype == self.dtype)
                    .on_press(move |_| {
                        ctx.edit(move |draft| {
                            if let Some((_, slot)) = draft.partitions.get_mut(index) {
                                *slot = dtype.to_string();
                            }
                        });
                    })
                    .child(MonoValue::new(dtype))
                    .into()
            })
            .collect();
        let picker = rect()
            .width(Size::px(TYPE_WIDTH))
            .height(Size::px(FIELD_HEIGHT))
            .child(
                Select::new()
                    .selected_item(MonoValue::new(self.dtype.clone()))
                    .children(types),
            );

        rect()
            .width(Size::fill())
            .horizontal()
            .cross_align(Alignment::Center)
            .spacing(CONTROL_GAP)
            .child(
                rect()
                    .width(Size::px(NAME_WIDTH))
                    .horizontal()
                    .cross_align(Alignment::Center)
                    .spacing(SP_3)
                    .child(
                        Icon::new(IconName::Brackets)
                            .size(NAME_ICON)
                            .color(win.icon_color),
                    )
                    .child(MonoValue::new(self.name.clone()).color(win.icon_color)),
            )
            .child(picker)
    }
}

/// The cast warning, while any column is still read as text.
#[derive(PartialEq)]
struct Warning;

impl Component for Warning {
    fn render(&self) -> impl IntoElement {
        let warning = tones().warning;
        rect()
            .width(Size::fill())
            .horizontal()
            .cross_align(Alignment::Start)
            .spacing(SP_3)
            .padding(Gaps::new(SP_2, 0., 0., 0.))
            .child(
                Icon::new(IconName::Warning)
                    .size(WARNING_ICON)
                    .color(warning),
            )
            .child(
                Caption::new(
                    "Partition values are read as text, so WHERE year = 2024 needs a cast \
                     unless you set Int or Date.",
                )
                .color(warning)
                .width(Size::flex(1.))
                .wrap(),
            )
    }
}
