//! File export (`COPY … TO` via the engine) + the native save-dialog flow. Split out of query.

use dioxus::prelude::*;

/// `Action::RunExport` — pick a destination (native save dialog, or a folder for a partitioned
/// export) and export the snapshot to a file via the engine's `COPY … TO`. Clipboard copy is the
/// grid's Copy (page-bounded), so export is file-only and streams to disk — safe at any size.
pub fn run_export(ex: crate::model::ExportForm) {
    let ws_id = crate::session::active_id();
    let (page, page_size) = crate::runs::RUNS
        .resolve()
        .get(ws_id)
        .map(|e| {
            let run = e.peek();
            (run.page, run.page_size)
        })
        .unwrap_or((1, 100));

    let ext = match ex.format.as_str() {
        "json" => "json",
        "parquet" => "parquet",
        "arrow" => "arrow",
        _ => "csv",
    };
    let partitioned = !ex.partition_cols.is_empty();
    let default_name = format!("{}.{ext}", ex.name);

    spawn(async move {
        // Partitioned export writes a *directory* of `col=value/` parts → pick a
        // folder (a subfolder named after the export holds the tree); otherwise
        // pick a file and force the extension so DataFusion writes one file.
        let dest = if partitioned {
            rfd::AsyncFileDialog::new()
                .pick_folder()
                .await
                .map(|h| h.path().join(&ex.name).to_string_lossy().into_owned())
        } else {
            rfd::AsyncFileDialog::new()
                .set_file_name(&default_name)
                .save_file()
                .await
                .map(|h| {
                    let mut p = h.path().to_string_lossy().into_owned();
                    let want = format!(".{ext}");
                    if !p.to_lowercase().ends_with(&want) {
                        p.push_str(&want);
                    }
                    p
                })
        };
        if let Some(path) = dest {
            crate::command!(Export {
                ws_id,
                path,
                format: ex.format,
                all: ex.scope != "page",
                page,
                page_size,
                csv_delimiter: delim_char(&ex.csv_delim),
                csv_header: ex.csv_header,
                csv_null: ex.csv_null,
                pq_compression: ex.pq_compression,
                pq_level: ex.pq_level,
                partition_cols: ex.partition_cols,
                keep_partition: ex.keep_partition,
            });
        }
        crate::overlays::close_export();
    });
}

fn delim_char(d: &str) -> char {
    match d {
        "tab" => '\t',
        "semicolon" => ';',
        "pipe" => '|',
        _ => ',',
    }
}
