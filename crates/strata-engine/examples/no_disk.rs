//! An engine that writes nothing to disk: results and engine-owned tables both held in RAM.
//!
//! ```text
//! cargo run -p strata-engine --example no_disk
//! ```
//!
//! The two storage seams are separate settings because they hold different things for different
//! lengths of time. Swapping either one is one builder call and changes nothing else about how
//! the engine is used — which is the point of the seam. The guide is
//! `strata_engine::guide::storage`.
//!
//! **The caveat, stated where it is chosen.** A snapshot never outlives its process, so holding
//! results in RAM loses nothing. An internal table's *def* does outlive the process, so a
//! `MemTableStore` means a restart replays defs against data that is gone — which is why it is
//! for tests and ephemeral workspaces, and why the failure is an honest row naming the missing
//! data rather than a fault.

use std::collections::BTreeMap;
use std::error::Error;

use strata_arrow::config::display_subset;
use strata_engine::snapshots::MemSnapshotStore;
use strata_engine::tables::MemTableStore;
use strata_engine::{Engine, RunOutcome, RunTag, WsId};
use strata_model::PageQuery;

fn main() -> Result<(), Box<dyn Error>> {
    // A data dir is still needed: it is what a `CREATE TABLE` resolves the def's portable,
    // project-relative path against. The store below is what decides the bytes never land there.
    let dir = std::env::temp_dir().join("strata-example-no-disk");
    std::fs::create_dir_all(&dir)?;

    let engine = Engine::builder()
        .with_data_dir(&dir)
        .with_snapshot_store(MemSnapshotStore::default())
        .with_table_store(MemTableStore::default())
        .build();

    // A statement the engine intercepts: the rows are spooled into the table store and the def
    // is handed back as an effect for the caller to fold into its own catalog.
    let made = futures::executor::block_on(engine.ws(WsId(1)).run(
        RunTag(1),
        "CREATE TABLE cities AS SELECT * FROM (VALUES ('Lisbon', 40), ('Porto', 17)) t(city, visits)"
            .into(),
        100,
    ))?;
    let RunOutcome::Statement(report) = made else {
        return Err("a CREATE TABLE settles a statement report".into());
    };
    println!("{}", report.message);
    println!("effect: {:?}\n", report.effect);

    // Read it back. The provider re-lists per scan, so an append is visible without
    // re-registering the table.
    let outcome = futures::executor::block_on(engine.ws(WsId(1)).run(
        RunTag(2),
        "SELECT city, visits FROM cities ORDER BY visits DESC".into(),
        100,
    ))?;
    let RunOutcome::Rows(run) = outcome else {
        return Err("a SELECT settles rows".into());
    };
    let snapshot = run.output.snapshot.ok_or("the query produced no rows")?;

    let page = futures::executor::block_on(engine.snapshot(snapshot).page(
        PageQuery {
            page: 1,
            page_size: 10,
            sort: None,
        },
        display_subset(&BTreeMap::new()),
    ))?;
    for row in &page.rows {
        let cells: Vec<&str> = row.iter().map(|cell| cell.text.as_str()).collect();
        println!("  {}", cells.join("  "));
    }

    // Nothing was written under the data dir: both stores held their bytes in RAM.
    let left = std::fs::read_dir(&dir)?.count();
    println!("\nfiles under the data dir: {left}");

    std::fs::remove_dir_all(&dir).ok();
    Ok(())
}
