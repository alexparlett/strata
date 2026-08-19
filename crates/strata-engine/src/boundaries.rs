//! The two import directions nothing in the workspace may take, checked by reading the source.
//!
//! Both are boundaries a type system cannot state. The **module** one replaces a crate split the
//! re-architecture deliberately did not make: `sources` and `sql` are peers inside this crate, and
//! peers that import each other are one module wearing two names — but `pub(crate)` says nothing
//! about which sibling may reach which. The **crate** one is what makes `strata-arrow` worth
//! having: a re-export added back here would let a surface that formats a cell go on compiling a
//! query planner to do it, and every such re-export was removed to stop exactly that.
//!
//! A missing directory is not a pass: the half whose directory does not exist reads no files and
//! goes live the moment one does, with no edit here.

use std::fs;
use std::path::{Path, PathBuf};

/// Every `.rs` file under `dir`, recursively. Empty when the directory does not exist — the
/// callers that need one to exist assert it themselves.
fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(rust_files(&path));
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            found.push(path);
        }
    }
    found.sort();
    found
}

/// Every `use` declaration in `source`, each flattened to one line from the `use` keyword to its
/// terminating `;` — so a brace list spanning lines is one declaration, and a visibility in front
/// of it is dropped as the thing no direction rule asks about.
///
/// Declarations only: a `use` inside a doc comment or a string sits after prose on its line, and
/// nothing but a visibility may.
fn use_declarations(source: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = source;
    while let Some(at) = rest.find("use ") {
        let before = rest[..at].rsplit('\n').next().unwrap_or_default().trim();
        rest = &rest[at..];
        let Some(end) = rest.find(';') else { break };
        let declaration = &rest[..end];
        rest = &rest[end..];
        if before.is_empty() || before.starts_with("pub") {
            found.push(declaration.split_whitespace().collect::<Vec<_>>().join(" "));
        }
    }
    found
}

/// The module `file` is, as segments under `src/` — `sources/postgres/mod.rs` is
/// `["sources", "postgres"]` and `sources/sql.rs` is `["sources", "sql"]`.
///
/// What `super` means depends on this, which is why it is computed rather than assumed: from
/// `sources/mod.rs` it is the crate root, and from `sources/sql.rs` it is `sources`.
fn module_of(file: &Path, src: &Path) -> Vec<String> {
    let relative = file.strip_prefix(src).unwrap_or(file);
    let mut segments: Vec<String> = relative
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();
    match segments.pop() {
        Some(file) if file != "mod.rs" && file != "lib.rs" => {
            segments.push(file.trim_end_matches(".rs").to_string());
        }
        _ => {}
    }
    segments
}

/// The **top-level** module this declaration reaches into, resolved against the module it is
/// written in — `None` for a path naming another crate.
///
/// Resolved rather than pattern-matched, because `super::sql` is `crate::sql` from
/// `sources/mod.rs` and `crate::sources::sql` from `sources/providers.rs`: one spelling, two
/// reaches, and only the first is one this rule is about.
fn reaches(declaration: &str, module: &[String]) -> Option<String> {
    let mut segments = declaration
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|segment| !segment.is_empty());
    if segments.next() != Some("use") {
        return None;
    }
    let mut path: Vec<String> = match segments.next()? {
        "crate" => Vec::new(),
        "super" => module
            .iter()
            .take(module.len().saturating_sub(1))
            .cloned()
            .collect(),
        _ => return None,
    };
    path.extend(segments.map(str::to_string));
    path.first().cloned()
}

fn crate_dir(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/")
        .join(name)
}

/// `sources` and `sql` are peers: neither may import the other.
#[test]
fn the_sources_and_language_layers_do_not_import_each_other() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offences = Vec::new();
    for (importer, forbidden) in [("sources", "sql"), ("sql", "sources")] {
        for file in rust_files(&src.join(importer)) {
            let source = fs::read_to_string(&file).expect("readable");
            let module = module_of(&file, &src);
            for declaration in use_declarations(&source) {
                if reaches(&declaration, &module).as_deref() == Some(forbidden) {
                    offences.push(format!("{}: {declaration}", file.display()));
                }
            }
        }
    }
    assert!(
        offences.is_empty(),
        "`sources` and `sql` are peers inside this crate and neither may import the other:\n{}",
        offences.join("\n")
    );
}

/// The items `strata-arrow` owns, by the name a `use` would write. The engine re-exported every
/// one of them once; naming any of them through `strata_engine` again means a re-export came
/// back.
const ARROW_VOCABULARY: &[&str] = &[
    "chart_role",
    "check_client_config",
    "client_key",
    "ClientKey",
    "CLIENT_KEYS",
    "column",
    "column_info",
    "config",
    "MAX_BINS",
    "plan",
    "Profiled",
    "RecordBatch",
    "Schema",
    "serialize",
    "stats_footnote",
    "value_tree",
];

/// Does this declaration name [`ARROW_VOCABULARY`] through `strata_engine`?
///
/// Segment-exact, so `strata_engine::db::SchemaVisibility` is not `Schema`, and a name reached
/// through any other crate — `strata_arrow::config`, which is the point — is not this rule's
/// business.
fn names_arrow_vocabulary(declaration: &str) -> bool {
    let mut segments = declaration
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|segment| !segment.is_empty());
    segments.next() == Some("use")
        && segments.next() == Some("strata_engine")
        && segments.any(|segment| ARROW_VOCABULARY.contains(&segment))
}

/// No frontend or agent file names the engine for something `strata-arrow` exports.
#[test]
fn the_frontend_names_strata_arrow_for_the_arrow_vocabulary() {
    let mut offences = Vec::new();
    for crate_name in ["strata-freya", "strata-agent"] {
        let src = crate_dir(crate_name).join("src");
        let files = rust_files(&src);
        assert!(
            !files.is_empty(),
            "{} has no source to check — this test is not reading what it claims to",
            src.display()
        );
        for file in files {
            let source = fs::read_to_string(&file).expect("readable");
            for declaration in use_declarations(&source) {
                if names_arrow_vocabulary(&declaration) {
                    offences.push(format!("{}: {declaration}", file.display()));
                }
            }
        }
    }
    assert!(
        offences.is_empty(),
        "these name `strata_engine` for an item `strata_arrow` exports — import it from \
         `strata_arrow`, and if the engine re-exports it again, drop the re-export:\n{}",
        offences.join("\n")
    );
}

#[test]
fn a_use_declaration_is_read_to_its_semicolon_and_prose_is_not_one() {
    let source = "//! A doc comment that says use strata_engine::plan; in prose.\n\
                  use strata_engine::{\n    RunTag,\n    WsId,\n};\n\
                  pub use crate::sql::validate;\n";
    assert_eq!(
        use_declarations(source),
        vec![
            "use strata_engine::{ RunTag, WsId, }".to_string(),
            "use crate::sql::validate".to_string(),
        ]
    );
}

/// **`super` is resolved, not matched.** The same declaration reaches the language layer from one
/// file and a sibling of this layer's own from another, and the rule is only about the first —
/// `sources/sql.rs` is a module *inside* `sources`, not the `sql` this rule fences off.
#[test]
fn a_reach_is_resolved_against_the_module_it_is_written_in() {
    let shell = vec!["sources".to_string()];
    let inner = vec!["sources".to_string(), "providers".to_string()];
    assert_eq!(
        reaches("use super::sql::validate", &shell).as_deref(),
        Some("sql"),
        "super from sources/mod.rs is the crate root"
    );
    assert_eq!(
        reaches("use super::sql::federated", &inner).as_deref(),
        Some("sources"),
        "super from a file inside sources is sources"
    );
    assert_eq!(
        reaches("use crate::sources::sql::federated", &inner).as_deref(),
        Some("sources")
    );
    assert_eq!(
        reaches("use crate::sql::qualified", &inner).as_deref(),
        Some("sql")
    );
    assert_eq!(reaches("use strata_engine::sql::validate", &inner), None);
}

#[test]
fn a_files_module_is_its_path_under_src() {
    let src = Path::new("/w/crates/strata-engine/src");
    assert_eq!(module_of(&src.join("sources/mod.rs"), src), vec!["sources"]);
    assert_eq!(
        module_of(&src.join("sources/sql.rs"), src),
        vec!["sources", "sql"]
    );
    assert_eq!(
        module_of(&src.join("sources/postgres/mod.rs"), src),
        vec!["sources", "postgres"]
    );
}

#[test]
fn the_arrow_vocabulary_is_matched_by_whole_segment_and_only_through_the_engine() {
    assert!(names_arrow_vocabulary(
        "use strata_engine::config::ENGINE_KEYS"
    ));
    assert!(names_arrow_vocabulary(
        "use strata_engine::{ column_info, TableMeta }"
    ));
    assert!(!names_arrow_vocabulary(
        "use strata_engine::db::SchemaVisibility"
    ));
    assert!(!names_arrow_vocabulary("use strata_arrow::config::key_def"));
}
