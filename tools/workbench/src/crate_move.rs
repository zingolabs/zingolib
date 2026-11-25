use crate::{read, repo_root, required, verdict};

const OLD_NAMES: [&str; 12] = [
    "zingolib-memo",
    "zingolib-net-diag",
    "zingolib-status",
    "zingolib-netutils",
    "zingolib-price",
    "zingo-memo",
    "zingo-net-diag",
    "zingo-status",
    "zingo-netutils",
    "zingo-price",
    "zingo-cli",
    "zingo-ffi",
];
const ANY_LINE: &str = "";
const GUIDANCE_MARK: &str = "try:";
const CLI_SOURCE: &str = "bin/cli/src/lib.rs";
const RELEASE_SOP: &str = "docs/sop/SOP001-release.md";
const EXCLUSION_AUDIT: &str = "tools/workbench/src/bin/exclusion-audit.rs";
const EDITORCONFIG: &str = ".editorconfig";
const SECTION_OPEN: char = '[';
const DIRECTORY_SECTION_CLOSE: &str = "/*]";
const UNRELEASED: &str = "## [Unreleased]";
const NEXT_RELEASE: &str = "\n## [";
const SOP_LINE_WITH_AN_INTERMEDIATE_NAME: &str = "cargo publish --package zingolib-netutils";
const CONSUMER_VISIBLE_RENAMES: [(&str, &str); 1] =
    [("crates/pepper-sync/CHANGELOG.md", "`zingo-netutils`")];

fn text(relative: &str) -> Result<String, Vec<String>> {
    read(&repo_root()?.join(relative))
}

fn old_names_in(relative: &str, text: &str, mark: &str) -> Vec<String> {
    text.lines()
        .filter(|line| line.contains(mark) && OLD_NAMES.iter().any(|old| line.contains(old)))
        .map(|line| format!("{relative}: {}", line.trim()))
        .collect()
}

fn old_name_lines(relative: &str, mark: &str) -> Result<Vec<String>, Vec<String>> {
    Ok(old_names_in(relative, &text(relative)?, mark))
}

fn unreleased(relative: &str) -> Result<String, Vec<String>> {
    let changelog = text(relative)?;
    let rest = required(changelog.split_once(UNRELEASED), UNRELEASED)?.1;
    Ok(rest
        .split_once(NEXT_RELEASE)
        .map_or(rest, |(section, _)| section)
        .to_string())
}

#[test]
fn the_misplaced_option_guidance_names_the_binary() -> Result<(), Vec<String>> {
    verdict(old_name_lines(CLI_SOURCE, GUIDANCE_MARK)?)
}

#[test]
fn the_release_sop_names_no_package_by_its_old_name() -> Result<(), Vec<String>> {
    verdict(old_name_lines(RELEASE_SOP, ANY_LINE)?)
}

#[test]
fn the_exclusion_audit_fixture_lists_the_moved_layout() -> Result<(), Vec<String>> {
    verdict(old_name_lines(EXCLUSION_AUDIT, ANY_LINE)?)
}

#[test]
fn every_editorconfig_directory_section_names_a_directory() -> Result<(), Vec<String>> {
    let root = repo_root()?;
    verdict(
        text(EDITORCONFIG)?
            .lines()
            .filter_map(|line| {
                line.strip_prefix(SECTION_OPEN)?
                    .strip_suffix(DIRECTORY_SECTION_CLOSE)
            })
            .filter(|directory| !root.join(directory).is_dir())
            .map(str::to_string)
            .collect(),
    )
}

#[test]
fn each_unreleased_section_records_the_rename_its_consumers_see() -> Result<(), Vec<String>> {
    let mut silent = Vec::new();
    for (changelog, subject) in CONSUMER_VISIBLE_RENAMES {
        if !unreleased(changelog)?.contains(subject) {
            silent.push(format!("{changelog} says nothing of `{subject}`"));
        }
    }
    verdict(silent)
}

#[test]
fn a_line_that_names_an_intermediate_package_name_is_reported() {
    assert_eq!(
        old_names_in(RELEASE_SOP, SOP_LINE_WITH_AN_INTERMEDIATE_NAME, ANY_LINE),
        [format!(
            "{RELEASE_SOP}: {SOP_LINE_WITH_AN_INTERMEDIATE_NAME}"
        )]
    );
}

#[test]
fn every_pinned_subject_names_a_retired_name() -> Result<(), Vec<String>> {
    verdict(
        CONSUMER_VISIBLE_RENAMES
            .iter()
            .filter(|(_, subject)| !OLD_NAMES.iter().any(|old| subject.contains(old)))
            .map(|(changelog, subject)| {
                format!("{changelog} is pinned by `{subject}`, which names no retired package")
            })
            .collect(),
    )
}
