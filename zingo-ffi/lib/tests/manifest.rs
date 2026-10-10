#![forbid(unsafe_code)]

use std::fs;
use std::path::Path;

const MANIFEST: &str = "Cargo.toml";

const DEPENDENCY_TABLES: [&str; 3] = [
    "[dependencies]",
    "[dev-dependencies]",
    "[build-dependencies]",
];

const WORKSPACE_MARKER: &str = "workspace = true";

const COMMENT: char = '#';

const TABLE_OPEN: char = '[';

#[test]
fn every_dependency_comes_from_the_workspace_table() {
    let manifest =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(MANIFEST)).unwrap();
    let mut in_dependencies = false;
    let mut local = Vec::new();
    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with(TABLE_OPEN) {
            in_dependencies = DEPENDENCY_TABLES.contains(&trimmed);
            continue;
        }
        let pinned_here = !trimmed.is_empty()
            && !trimmed.starts_with(COMMENT)
            && !trimmed.contains(WORKSPACE_MARKER);
        if in_dependencies && pinned_here {
            local.push(trimmed.to_string());
        }
    }
    assert!(
        local.is_empty(),
        "dependencies pinned outside [workspace.dependencies]:\n{local:#?}"
    );
}
