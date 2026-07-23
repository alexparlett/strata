//! Headless preview harness: renders this surface — real component, real `midnight` theme —
//! to `target/plan-preview.png` for eyeballing against the design canvas. Ignored by default
//! (it writes a file, asserts nothing):
//! `cargo test -p strata-freya plan_preview -- --ignored`.

use freya_testing::TestingRunner;
use strata_core::engine::plan::{fmt_ms, self_time_ms, Metric, MetricKind, PlanKind, PlanNode};

use super::*;

fn m(name: &str, value: u64, kind: MetricKind) -> Metric {
    Metric {
        name: name.to_string(),
        value,
        kind,
        label: kind.format(value),
        zero: value == 0,
    }
}

fn node(
    name: &str,
    detail: &str,
    kind: PlanKind,
    depth: usize,
    rows: Option<u64>,
    metrics: Vec<Metric>,
) -> PlanNode {
    let self_ms = self_time_ms(kind, &metrics);
    PlanNode {
        name: name.to_string(),
        detail: detail.to_string(),
        kind,
        depth,
        rows,
        self_ms,
        self_label: self_ms.map(fmt_ms).unwrap_or_default(),
        metrics,
    }
}

/// A compact cut of the spec §3 reference ANALYZE plan: sort → agg → exchange → join →
/// two sibling scans, exercising rails, hotspots, insights, zeros, and the long detail.
fn fixture() -> QueryPlan {
    use MetricKind::*;
    QueryPlan {
        physical: vec![
            node("SortExec", "TopK(fetch=20), expr=[cnt@2 DESC]", PlanKind::Sort, 0,
                Some(4),
                vec![m("output_rows", 4, Count), m("elapsed_compute", 156_000, Time),
                    m("row_replacements", 4, Count)]),
            node("AggregateExec", "mode=FinalPartitioned, gby=[country@0, action@1], aggr=[count(1)]",
                PlanKind::Agg, 1, Some(4),
                vec![m("output_rows", 4, Count), m("elapsed_compute", 4_790_000, Time),
                    m("peak_mem_used", 3481, Memory), m("spill_count", 0, Count)]),
            node("RepartitionExec", "partitioning=Hash([user_id@0], 10), input_partitions=10",
                PlanKind::Exchange, 2, None,
                vec![m("repartition_time", 4_300_000, Time), m("send_time", 19_000, Time),
                    m("fetch_time", 256_000_000, Time)]),
            node("HashJoinExec", "mode=Partitioned, join_type=Inner, on=[(user_id@0, user_id@0)]",
                PlanKind::Join, 3, Some(4),
                vec![m("output_rows", 4, Count), m("build_time", 216_000, Time),
                    m("join_time", 146_000, Time), m("build_mem_used", 2148, Memory),
                    m("build_input_rows", 4, Count), m("input_rows", 5, Count),
                    m("output_batches", 4, Count)]),
            node("ParquetExec",
                "file_groups={1 group: [[…/events/year=2024/month=01/data.parquet, …/events/year=2024/month=02/data.parquet]]}, projection=[user_id, action, amount], predicate=amount@3 IS NOT NULL",
                PlanKind::Source, 4, Some(7),
                vec![m("output_rows", 7, Count), m("time_elapsed_processing", 15_594_334, Time),
                    m("time_elapsed_scanning_total", 17_147_249, Time),
                    m("metadata_load_time", 22_353_002, Time), m("bytes_scanned", 605, Bytes),
                    m("row_groups_matched_statistics", 2, Count),
                    m("row_groups_pruned_statistics", 0, Count),
                    m("pushdown_rows_matched", 0, Count), m("pushdown_rows_pruned", 0, Count),
                    m("file_open_errors", 0, Count), m("file_scan_errors", 0, Count)]),
            node("ParquetExec", "file_groups={1 group: [[…/users/users.parquet]]}, projection=[user_id, country]",
                PlanKind::Source, 4, Some(5),
                vec![m("output_rows", 5, Count), m("time_elapsed_processing", 578_000, Time),
                    m("metadata_load_time", 3_200_000, Time), m("bytes_scanned", 210, Bytes),
                    m("file_open_errors", 0, Count)]),
        ],
        logical: vec![
            node("Sort", "cnt DESC NULLS FIRST, fetch=20", PlanKind::Sort, 0, None, vec![]),
            node("Aggregate", "groupBy=[[country, action]], aggr=[[count(1)]]", PlanKind::Agg, 1, None, vec![]),
            node("TableScan", "events projection=[user_id, action, amount]", PlanKind::Source, 2, None, vec![]),
        ],
        physical_text: "SortExec: TopK(fetch=20)\n  AggregateExec: mode=FinalPartitioned\n    RepartitionExec: partitioning=Hash([user_id@0], 10)\n      HashJoinExec: mode=Partitioned, join_type=Inner\n        ParquetExec: file_groups={1 group}\n        ParquetExec: file_groups={1 group}".into(),
        logical_text: "Sort: cnt DESC NULLS FIRST, fetch=20\n  Aggregate: groupBy=[[country, action]]\n    TableScan: events".into(),
        analyze: true,
    }
}

fn app() -> impl IntoElement {
    use_init_theme(|| crate::theme::strata_theme(&strata_core::theme::load("midnight")));
    let tab = use_state(PlanTab::default);
    ExplainPlan::new(fixture(), tab)
}

#[test]
#[ignore = "writes target/plan-preview.png for eyeballing; run explicitly"]
fn plan_preview() {
    let (mut runner, _) = TestingRunner::new(app, (960., 900.).into(), |_| {}, 1.);
    runner.sync_and_update();
    runner.render_to_file(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../target/plan-preview.png"
    ));
    // Expanded states: SortExec's Metrics box open + AggregateExec's Detail expanded
    // (the second coordinate accounts for the first box's ~160px reflow).
    runner.click_cursor((63., 169.));
    runner.click_cursor((70., 424.));
    runner.sync_and_update();
    runner.render_to_file(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../target/plan-preview-open.png"
    ));
    // The Logical tab.
    runner.click_cursor((114., 18.));
    runner.sync_and_update();
    runner.render_to_file(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../target/plan-preview-logical.png"
    ));
    // The Raw text view (back on Physical).
    runner.click_cursor((44., 18.));
    runner.click_cursor((937., 18.));
    runner.sync_and_update();
    runner.render_to_file(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../target/plan-preview-raw.png"
    ));
}
