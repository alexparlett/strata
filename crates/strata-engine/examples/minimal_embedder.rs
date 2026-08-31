//! The whole round trip, in one file: build an engine, give it a catalog, run a statement, read
//! the result.
//!
//! ```text
//! cargo run -p strata-engine --example minimal_embedder
//! ```
//!
//! Nothing here is Strata-specific. Point `CatalogSpec` at your own files and the rest is the
//! same four calls. The guide is `strata_engine::guide::embedding`.

use std::collections::BTreeMap;
use std::error::Error;
use std::fs;

use strata_arrow::config::display_subset;
use strata_engine::register::CatalogSpec;
use strata_engine::{Engine, RunOutcome, RunTag, TableSpec, WsId};
use strata_model::{PageQuery, SourceFormat};

fn main() -> Result<(), Box<dyn Error>> {
    let dir = std::env::temp_dir().join("strata-example-minimal-embedder");
    fs::create_dir_all(&dir)?;
    fs::write(
        dir.join("events.csv"),
        "id,city,visits\n1,Lisbon,40\n2,Porto,17\n3,Lisbon,25\n",
    )?;

    let engine = Engine::builder().build();

    // The catalog the engine should hold. `sync` reconciles against it, so this is the whole
    // catalog and never a work list: a name it does not carry is deregistered.
    let spec = CatalogSpec {
        tables: vec![TableSpec {
            name: "events".into(),
            paths: vec![dir.join("events.csv").display().to_string()],
            format: SourceFormat::from_name("csv"),
            partitions: vec![],
            source: None,
            internal: false,
        }],
        ..Default::default()
    };
    // The closure is called per def as the engine answers for it, so a host with rows on screen
    // flips one at a time rather than waiting for the pass.
    let report = futures::executor::block_on(engine.catalog().sync(spec, |outcome| {
        println!("registered: {outcome:?}");
    }));
    println!("catalog generation {:?}\n", report.generation);

    // Run a statement. `run` classifies first, so this one entry point serves queries, the
    // statements the engine intercepts, and the refusals for anything the caller may not do.
    let outcome = futures::executor::block_on(engine.ws(WsId(1)).run(
        RunTag(1),
        "SELECT city, sum(visits) AS visits FROM events GROUP BY city ORDER BY visits DESC".into(),
        100,
    ))?;

    let RunOutcome::Rows(run) = outcome else {
        return Err("a SELECT settles rows".into());
    };
    let snapshot = run.output.snapshot.ok_or("the query produced no rows")?;
    println!(
        "{} rows in {} ms, snapshot {snapshot}",
        run.output.total, run.output.elapsed_ms
    );

    // The result is immutable, so read any page of it as often as you like — nothing is
    // recomputed, and memory holds one page however large the result is.
    let display = display_subset(&BTreeMap::new());
    let page = futures::executor::block_on(engine.snapshot(snapshot).page(
        PageQuery {
            page: 1,
            page_size: 10,
            sort: None,
        },
        display,
    ))?;
    for row in &page.rows {
        let cells: Vec<&str> = row.iter().map(|cell| cell.text.as_str()).collect();
        println!("  {}", cells.join("  "));
    }

    fs::remove_dir_all(&dir).ok();
    Ok(())
}
