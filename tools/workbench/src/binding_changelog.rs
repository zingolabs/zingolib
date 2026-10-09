use std::path::Path;

use crate::binding_manifest;

pub const BINARY: &str = "binding-changelog";
pub const FILE: &str = "bindings/CHANGELOG.md";
const CRATE_CHANGELOG: &str = "CHANGELOG.md";
const CHECK_FLAG: &str = "--check";
const USAGE: &str = "usage: binding-changelog [--check]";
const TITLE: &str = "# Binding Layer changelog";
const SECTION_MARK: &str = "## ";
const SINCE_PREFIX: &str = "Since ";
const SINCE_SUFFIX: char = '.';
const AUDITED_PREFIX: &str = "Audited ";
const CRATE_MARK: &str = "### ";
const HEADING_MARK: char = '#';
const DEMOTION: &str = "##";
const DIFF_HEADER_MARK: &str = "diff --git";
const HUNK_MARK: &str = "@@";
const ADDED_MARK: char = '+';
const REMOVED_MARK: char = '-';
const DIFF_ARGS: [&str; 4] = ["diff", "--no-color", "--no-ext-diff", "--no-renames"];
const PREAMBLE_PREFIXES: [&str; 4] = [
    "# Changelog",
    "All notable changes to this project",
    "The format is based on [Keep a Changelog]",
    "and this project adheres to [Semantic Versioning]",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    pub commit: String,
    pub since: String,
    pub entries: Vec<(String, Vec<String>)>,
}

pub fn gained_lines(diff: &str) -> Vec<String> {
    let mut in_header = false;
    let mut added = Vec::new();
    let mut removed = Vec::new();
    for line in diff.lines() {
        if line.starts_with(DIFF_HEADER_MARK) {
            in_header = true;
        } else if line.starts_with(HUNK_MARK) {
            in_header = false;
        } else if !in_header {
            if let Some(text) = line.strip_prefix(ADDED_MARK) {
                added.push(text.to_string());
            } else if let Some(text) = line.strip_prefix(REMOVED_MARK) {
                removed.push(text.to_string());
            }
        }
    }
    let gained = added.into_iter().filter(|text| {
        if is_blank(text) {
            return true;
        }
        if is_preamble(text) {
            return false;
        }
        match removed.iter().position(|gone| gone == text) {
            Some(index) => {
                removed.swap_remove(index);
                false
            }
            None => true,
        }
    });
    tidy(gained)
}

fn is_blank(line: &str) -> bool {
    line.trim().is_empty()
}

fn is_preamble(line: &str) -> bool {
    PREAMBLE_PREFIXES
        .iter()
        .any(|prefix| line.trim().starts_with(prefix))
}

fn tidy(lines: impl Iterator<Item = String>) -> Vec<String> {
    let mut tidy: Vec<String> = Vec::new();
    for line in lines {
        let blank = is_blank(&line);
        let after_blank = tidy.last().is_none_or(|last| is_blank(last));
        if !(blank && after_blank) {
            tidy.push(if blank { String::new() } else { line });
        }
    }
    if tidy.last().is_some_and(|last| is_blank(last)) {
        tidy.pop();
    }
    tidy
}

fn demoted(line: &str) -> String {
    if line.starts_with(HEADING_MARK) {
        format!("{DEMOTION}{line}")
    } else {
        line.to_string()
    }
}

pub fn render_header() -> String {
    format!(
        "{TITLE}\n\n\
         The workbench tool `{BINARY}` writes every section below, one per entry\n\
         of `{}`, from the lines that the entry's audited crate changelogs\n\
         gained after the previous entry's commit, named as `Since`, up to the\n\
         entry's commit, which the section heading names. The check in CI\n\
         regenerates every section and fails when the committed file differs. The\n\
         audited crates of an entry are the two Binding Layer crates and their\n\
         direct dependencies in this repository at the entry's commit, as the\n\
         publish workflow recorded them from `cargo tree`; each section names them\n\
         in its `{}` line. A change in another crate of this repository appears\n\
         only where one of those changelogs records it.\n",
        binding_manifest::FILE,
        AUDITED_PREFIX.trim_end()
    )
}

pub fn render_section(section: &Section) -> String {
    let entries = section
        .entries
        .iter()
        .filter(|(_, lines)| !lines.is_empty())
        .map(|(name, lines)| {
            let body = lines.iter().map(|line| demoted(line)).collect::<Vec<_>>();
            format!("{CRATE_MARK}{name}\n\n{}\n", body.join("\n"))
        })
        .collect::<Vec<_>>()
        .join("\n");
    let audited = section
        .entries
        .iter()
        .map(|(name, _)| format!("`{name}`"))
        .collect::<Vec<_>>()
        .join(crate::LIST_SEPARATOR);
    block(&format!(
        "{SECTION_MARK}{}\n\n{SINCE_PREFIX}{}{SINCE_SUFFIX}\n\n{AUDITED_PREFIX}{audited}{SINCE_SUFFIX}\n\n{entries}",
        section.commit, section.since
    ))
}

fn block(text: &str) -> String {
    let mut block = text.trim_end().to_string();
    block.push('\n');
    block
}

pub fn render_file(header: &str, sections_newest_first: &[String]) -> String {
    std::iter::once(header)
        .chain(
            sections_newest_first
                .iter()
                .flat_map(|section| ["\n", section.as_str()]),
        )
        .collect()
}

fn relative_changelog(found: &binding_manifest::AuditedCrate) -> Result<String, Vec<String>> {
    Ok(crate::utf8(&found.dir.join(CRATE_CHANGELOG))?.to_string())
}

fn gather(
    root: &Path,
    crates: &[binding_manifest::AuditedCrate],
    since: &str,
    commit: &str,
) -> Result<Section, Vec<String>> {
    let entries = crates
        .iter()
        .map(|found| {
            let relative = relative_changelog(found)?;
            if !crate::listed_at(root, commit, &relative)? {
                return Err(vec![format!(
                    "audited crate {} has no {relative} at {commit}",
                    found.name
                )]);
            }
            let args = [DIFF_ARGS.as_slice(), &[since, commit, "--", &relative]].concat();
            let diff = crate::git_in(root, &args)?;
            Ok((found.name.clone(), gained_lines(&diff)))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Section {
        commit: commit.to_string(),
        since: since.to_string(),
        entries,
    })
}

fn regenerated(root: &Path) -> Result<String, Vec<String>> {
    let entries = binding_manifest::entries_at(root, None)?;
    let sections = binding_manifest::publication_commits(&entries)
        .iter()
        .zip(&entries)
        .rev()
        .map(|((commit, since), entry)| {
            Ok(render_section(&gather(
                root,
                &entry.audited,
                since,
                commit,
            )?))
        })
        .collect::<Result<Vec<_>, Vec<String>>>()?;
    Ok(render_file(&render_header(), &sections))
}

/// - Runs `git` child processes in `root`.
/// - Writes `bindings/CHANGELOG.md`.
pub fn regenerate(root: &Path) -> Result<(), Vec<String>> {
    let file = root.join(FILE);
    std::fs::write(&file, regenerated(root)?)
        .map_err(|e| vec![format!("cannot write {}: {e}", file.display())])
}

/// - Runs `git` child processes in `root`.
/// - Writes `bindings/CHANGELOG.md` unless `args` holds `--check`.
pub fn dispatch(root: &Path, args: &[String]) -> Result<(), Vec<String>> {
    match args {
        [] => regenerate(root),
        [flag] if flag == CHECK_FLAG => {
            if crate::read(&root.join(FILE))? == regenerated(root)? {
                Ok(())
            } else {
                Err(vec![format!(
                    "{FILE} differs from what {BINARY} generates from {}; run `{BINARY}` and commit the result",
                    binding_manifest::FILE
                )])
            }
        }
        _ => Err(vec![USAGE.to_string()]),
    }
}

/// - Reads the process arguments.
/// - Exits the process through [`crate::run`].
pub fn main() -> ! {
    crate::dispatch_from_root(BINARY, dispatch)
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMMIT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const SINCE: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    #[test]
    fn gained_lines_are_the_added_lines_minus_moves_headers_and_preamble() {
        let diff = "diff --git a/x/CHANGELOG.md b/x/CHANGELOG.md\n\
                    --- a/x/CHANGELOG.md\n\
                    +++ b/x/CHANGELOG.md\n\
                    @@ -0,0 +1,8 @@\n\
                    +# Changelog\n\
                    +All notable changes to this project will be documented in this file.\n\
                    +The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),\n\
                    +\n\
                    +## [Unreleased]\n\
                    -- moved entry\n\
                    +\n\
                    +\n\
                    +- new entry\n\
                    +  continues here\n\
                    +- moved entry\n\
                    +\n";
        assert_eq!(
            gained_lines(diff),
            ["## [Unreleased]", "", "- new entry", "  continues here"]
        );
    }

    #[test]
    fn a_removed_line_cancels_one_added_line_and_not_every_repeat() {
        let diff = "@@ -1,2 +1,4 @@\n\
                    -### Changed\n\
                    +### Changed\n\
                    +- new bullet\n\
                    +### Changed\n";
        assert_eq!(gained_lines(diff), ["- new bullet", "### Changed"]);
    }

    #[test]
    fn a_section_demotes_gained_headings_keeps_paragraph_breaks_and_omits_silent_crates() {
        let section = Section {
            commit: COMMIT.to_string(),
            since: SINCE.to_string(),
            entries: vec![
                (
                    "zingolib".to_string(),
                    vec![
                        "### Added".to_string(),
                        "- entry".to_string(),
                        String::new(),
                        "Consumers must re-run codegen.".to_string(),
                    ],
                ),
                ("pepper-sync".to_string(), vec![]),
            ],
        };
        assert_eq!(
            render_section(&section),
            format!(
                "## {COMMIT}\n\nSince {SINCE}.\n\nAudited `zingolib`, `pepper-sync`.\n\n### zingolib\n\n##### Added\n- entry\n\nConsumers must re-run codegen.\n"
            )
        );
    }

    #[test]
    fn the_file_is_the_header_then_the_sections_newest_first_with_one_blank_line_between() {
        let header = render_header();
        let newest = format!("## {COMMIT}\n\nSince {SINCE}.\n");
        let older = format!("## {SINCE}\n\nSince {SINCE}.\n");
        assert_eq!(render_file(&header, &[]), header);
        assert_eq!(
            render_file(&header, &[newest.clone(), older.clone()]),
            format!("{header}\n{newest}\n{older}")
        );
    }

    #[test]
    fn the_committed_file_passes_the_check_at_the_checked_out_commit() {
        let root = crate::repo_root().unwrap();
        dispatch(&root, &[CHECK_FLAG.to_string()]).unwrap();
    }

    #[test]
    fn a_flag_the_tool_does_not_know_is_refused_with_the_usage() {
        let diagnostic = dispatch(&crate::repo_root().unwrap(), &["--since".to_string()])
            .unwrap_err()
            .concat();
        assert_eq!(diagnostic, USAGE);
    }
}
