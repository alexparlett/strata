//! The two import directions nothing in the workspace may take, checked by reading the source
//!.
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

/// Does this declaration reach into the crate-local module `module`?
///
/// `crate::sql::…` and, from a `mod.rs`, `super::sql::…` are the same reach spelled two ways, so
/// both count. A path that starts anywhere else names another crate and is not this rule's
/// business.
fn reaches(declaration: &str, module: &str) -> bool {
    let mut segments = declaration
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|segment| !segment.is_empty());
    segments.next() == Some("use")
        && segments
            .next()
            .is_some_and(|head| head == "crate" || head == "super")
        && segments.any(|segment| segment == module)
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
            for declaration in use_declarations(&source) {
                if reaches(&declaration, forbidden) {
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
    assert!(reaches("use crate::sql::validate", "sql"));
    assert!(reaches("use super::sql::validate", "sql"));
    assert!(!reaches("use strata_engine::sql::validate", "sql"));
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
