//! Session + Project store hooks — window-root initialisation.

use freya::prelude::{spawn, use_hook};
use freya::radio::{use_init_radio_station, RadioStation};
use strata_core::engine::TableSpec;
use strata_core::project as project_io;

use crate::apps::project::contexts::EngineCtx;

use super::{Chan, ProjChan, ProjectState, SessionState};

/// Initialise this window's Session store (opening one blank tab) and provide it via context.
/// Call once in the window root; returns the station for the root to read / drive.
pub fn use_init_session() -> RadioStation<SessionState, Chan> {
    use_init_radio_station::<SessionState, Chan>(|| {
        let mut s = SessionState::default();
        s.open_blank();
        s
    })
}

/// Initialise this window's Project store — open the project folder (argv\[1\], default
/// the repo's `sample/`), scaffolding a fresh `.strata/` when the folder has none — and
/// kick off engine registration of its defs (tables, then views). Call once in the
/// window root, after the engine is in context.
///
/// The open itself is synchronous (one small JSON read, needed before anything can
/// render meaningfully); registration is IO-heavy (schema inference reads file footers)
/// and runs as a spawned task, landing results row by row through [`ProjChan::Tables`] /
/// [`ProjChan::Views`] so rows flip `Loading → Ready/Failed` as answers arrive.
pub fn use_init_project(engine: &EngineCtx) -> RadioStation<ProjectState, ProjChan> {
    let station = use_init_radio_station::<ProjectState, ProjChan>(open_project);
    let engine = engine.clone();
    use_hook(move || {
        spawn(register_defs(engine, station));
    });
    station
}

/// Open the launch project: argv\[1\] as the project folder, defaulting to `sample/`
/// (the committed sample project) until the launcher / open-dialog lands (P4-02/P4-13
/// UI). A folder without a project gets one scaffolded; an unreadable folder or defs
/// file logs and comes up as an empty in-memory project rather than failing the window.
fn open_project() -> ProjectState {
    let arg = std::env::args().nth(1).unwrap_or_else(|| "sample".into());
    let root = match std::fs::canonicalize(&arg) {
        Ok(root) => root,
        Err(e) => {
            tracing::error!("open project folder `{arg}`: {e}");
            return ProjectState::default();
        }
    };
    let defs = if project_io::exists_at(&root) {
        project_io::load_defs(&root)
    } else {
        project_io::scaffold(&root)
    };
    match defs {
        Ok(defs) => ProjectState::from_defs(defs, root),
        Err(e) => {
            tracing::error!("open project: {e}");
            ProjectState::default()
        }
    }
}

/// Register the opened project's defs on the engine: every table (relative sources
/// resolved against the project folder), then every view.
///
/// Views can read other views, and DataFusion requires a view's dependencies to exist
/// when its `CREATE VIEW` plans — but the defs file carries no dependency order (it's
/// sorted alphabetically). Rather than parse SQL to topo-sort, retry to a fixed point:
/// each round creates what it can, and a view whose dependency landed last round
/// succeeds this round. No progress → the remainder are genuinely broken (bad SQL or a
/// missing table) and their errors land on their rows.
async fn register_defs(
    engine: EngineCtx,
    mut station: RadioStation<ProjectState, ProjChan>,
) {
    // Snapshot the work up front (peek — a task has no reactive context): results land
    // by name, so concurrent def edits can't be clobbered by a stale row write.
    let (tables, views) = {
        let p = station.peek();
        let Some(root) = p.root.clone() else { return };
        let tables: Vec<(String, TableSpec)> = p
            .tables
            .iter()
            .map(|t| {
                (
                    t.def.name.clone(),
                    TableSpec {
                        name: t.def.name.clone(),
                        paths: t
                            .def
                            .sources
                            .iter()
                            .map(|s| project_io::resolve_source(&root, s))
                            .collect(),
                        format: t.def.format.clone(),
                        partitions: t.def.partition_cols.clone(),
                    },
                )
            })
            .collect();
        let views: Vec<(String, String)> = p
            .views
            .iter()
            .map(|v| (v.def.name.clone(), v.def.sql.clone()))
            .collect();
        (tables, views)
    };

    for (name, spec) in tables {
        match engine.register(spec).await {
            Ok(meta) => station.write_channel(ProjChan::Tables).table_registered(&name, meta),
            Err(e) => {
                tracing::error!("register table '{name}' failed: {e}");
                station.write_channel(ProjChan::Tables).table_failed(&name, e);
            }
        }
    }

    let mut pending = views;
    while !pending.is_empty() {
        let before = pending.len();
        let mut failed = Vec::new();
        for (name, sql) in pending {
            match engine.create_view(name.clone(), sql.clone()).await {
                Ok(meta) => station.write_channel(ProjChan::Views).view_registered(&name, meta),
                Err(e) => failed.push((name, sql, e)),
            }
        }
        if failed.len() == before {
            // A full round without progress — the rest are genuinely broken.
            for (name, _, e) in failed {
                tracing::error!("create view '{name}' failed: {e}");
                station.write_channel(ProjChan::Views).view_failed(&name, e);
            }
            break;
        }
        pending = failed.into_iter().map(|(n, s, _)| (n, s)).collect();
    }
}
