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
//! The columns themselves are **found, not guessed** (`strata_core::engine::detect_partitions`):
//! the paths are read for `key=` segments they name, and a directory is walked for the
//! `key=value` folders under it. The canvas says the section found them, so it has to have
//! looked.

use freya::prelude::*;

use strata_core::engine::detect_partitions;

use crate::apps::configure::model::PARTITION_TYPES;
use crate::apps::configure::ConfigureCtx;
use crate::components::icon::{Icon, IconName};
use crate::components::segmented_toggle::{SegmentedToggle, ToggleSegment};
use crate::components::typography::{Body, Caption, Eyebrow, MonoValue};

/// The gap under the section header, between its rows, and beside a control.
const HEADER_GAP: f32 = 8.;
const ROW_GAP: f32 = 8.;
const CONTROL_GAP: f32 = 12.;
/// The canvas's fixed column for a partition column's name.
const NAME_WIDTH: f32 = 110.;
const NAME_ICON: f32 = 11.;
const WARNING_ICON: f32 = 12.;

#[derive(PartialEq)]
pub struct Hive;

impl Component for Hive {
    fn render(&self) -> impl IntoElement {
        let form = crate::components::form::form_theme();
        let ctx = use_consume::<ConfigureCtx>();
        let (may_partition, on, columns, warn) = {
            let draft = ctx.draft.read();
            (
                draft.may_partition(),
                draft.hive_on,
                draft.partitions.clone(),
                draft.partitions_are_text(),
            )
        };
        if !may_partition {
            return rect();
        }

        rect()
            .width(Size::fill())
            .vertical()
            .spacing(HEADER_GAP)
            .child(
                rect()
                    .width(Size::fill())
                    .vertical()
                    .spacing(2.)
                    .child(Eyebrow::new("HIVE PARTITIONING").color(form.label_color))
                    .child(
                        Caption::new("Found key=value folders in the source paths.")
                            .color(form.label_color)
                            .width(Size::fill())
                            .wrap(),
                    ),
            )
            // The switch is a **sibling** of its sentence, never wrapped in a pressable row: a
            // built-in's `on_press` does not stop propagation, so an ancestor would take the
            // same click and toggle twice, back to where it started.
            .child(
                rect()
                    .horizontal()
                    .cross_align(Alignment::Center)
                    .spacing(CONTROL_GAP)
                    .child(Switch::new().toggled(on).on_toggle(move |_| toggle(ctx)))
                    .child(
                        Body::new(match on {
                            true => "Reading the folder tree as partition columns",
                            false => "Ignoring the folder tree — files read as one flat table",
                        })
                        .color(form.label_color),
                    ),
            )
            .maybe_child(on.then(|| {
                let mut list = rect()
                    .width(Size::fill())
                    .vertical()
                    .spacing(ROW_GAP)
                    .padding(Gaps::new(4., 0., 0., 0.))
                    .child(
                        Caption::new("Confirm the type each partition column is read as.")
                            .color(form.label_color)
                            .width(Size::fill())
                            .wrap(),
                    );
                for (index, (name, dtype)) in columns.iter().enumerate() {
                    list = list.child(
                        PartitionRow {
                            index,
                            name: name.clone(),
                            dtype: dtype.clone(),
                            key: DiffKey::None,
                        }
                        .key(name.clone()),
                    );
                }
                list.maybe_child(warn.then(|| Warning))
            }))
    }
}

/// Turn partitioning on or off — **detecting the columns on the way on**, and only when there
/// are none yet, so a def's own stored types are never overwritten by a fresh scan.
fn toggle(ctx: ConfigureCtx) {
    let paths = ctx.draft.peek().nonblank_sources();
    ctx.edit(move |draft| {
        draft.hive_on = !draft.hive_on;
        if draft.hive_on && draft.partitions.is_empty() {
            draft.partitions = detect_partitions(&paths)
                .into_iter()
                // Text is what DataFusion infers on its own, so it is what an undecided column
                // starts as — and what the warning below is about.
                .map(|name| (name, "Utf8".to_string()))
                .collect();
        }
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
        let colors = use_theme().read().colors().clone();
        let ctx = use_consume::<ConfigureCtx>();
        let index = self.index;

        let mut pill = SegmentedToggle::new().form();
        for dtype in PARTITION_TYPES {
            pill = pill.child(
                ToggleSegment::text(dtype)
                    .selected(dtype == self.dtype)
                    .on_press(move |_| {
                        ctx.edit(move |draft| {
                            if let Some((_, slot)) = draft.partitions.get_mut(index) {
                                *slot = dtype.to_string();
                            }
                        })
                    }),
            );
        }

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
                    .spacing(6.)
                    .child(
                        Icon::new(IconName::Brackets)
                            .size(NAME_ICON)
                            .color(colors.primary),
                    )
                    .child(MonoValue::new(self.name.clone()).color(colors.primary)),
            )
            .child(pill)
    }
}

/// The cast warning, while any column is still read as text.
#[derive(PartialEq)]
struct Warning;

impl Component for Warning {
    fn render(&self) -> impl IntoElement {
        // The sheet directly, not this window's theme: `warning` is one of the four semantic
        // slots and has to follow the app-wide ramp wherever it appears.
        let warning = use_theme().read().colors().warning;
        rect()
            .width(Size::fill())
            .horizontal()
            .cross_align(Alignment::Start)
            .spacing(8.)
            .padding(Gaps::new(4., 0., 0., 0.))
            .child(
                Icon::new(IconName::Warning)
                    .size(WARNING_ICON)
                    .color(warning),
            )
            .child(
                Caption::new(
                    "Partition values are read as text — WHERE year = 2024 needs a cast unless \
                     you set Int or Date.",
                )
                .color(warning)
                .width(Size::flex(1.))
                .wrap(),
            )
    }
}
