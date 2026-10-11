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
const RANGE_SEPARATOR: char = ',';
const FENCE_MARK: &str = "```";
const ADDED_MARK: char = '+';
const KEPT_MARK: char = ' ';
const NO_NEWLINE_MARK: char = '\\';
const REMOVED_MARK: char = '-';
const DIFF_ARGS: [&str; 4] = ["diff", "--no-color", "--no-ext-diff", "--no-renames"];
const CONTEXT_FLAG: &str = "--unified=";
const WHOLE_FILE_CONTEXT: i32 = i32::MAX / 2;
const TITLE_LEVEL: usize = 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    pub commit: String,
    pub since: String,
    pub entries: Vec<(String, Vec<String>)>,
}

fn heading_level(line: &str) -> Option<usize> {
    let level = line.chars().take_while(|c| *c == HEADING_MARK).count();
    (level > 0 && line[level..].starts_with(' ')).then_some(level)
}

fn heading_levels<'a>(
    lines: impl Iterator<Item = &'a str> + 'a,
) -> impl Iterator<Item = Option<usize>> + 'a {
    let mut in_fence = false;
    lines.map(move |line| {
        if line.trim_start().starts_with(FENCE_MARK) {
            in_fence = !in_fence;
        }
        if in_fence {
            None
        } else {
            heading_level(line)
        }
    })
}

fn hunk_new_start(header: &str) -> Option<usize> {
    header
        .split(' ')
        .find_map(|field| field.strip_prefix(ADDED_MARK))?
        .split(RANGE_SEPARATOR)
        .next()?
        .parse()
        .ok()
}

type NewFileLines<'a> = Vec<(&'a str, bool)>;

fn diff_lines(diff: &str) -> Result<(Vec<&str>, NewFileLines<'_>), Vec<String>> {
    let mut in_header = false;
    let mut next_new_line: usize = 1;
    let mut overlap = 0;
    let mut removed = Vec::new();
    let mut new_file = Vec::new();
    for line in diff.lines() {
        if line.starts_with(DIFF_HEADER_MARK) {
            in_header = true;
            continue;
        }
        if let Some(header) = line.strip_prefix(HUNK_MARK) {
            in_header = false;
            let start = hunk_new_start(header)
                .ok_or_else(|| vec![format!("cannot read the hunk header `{line}`")])?;
            overlap = next_new_line.saturating_sub(start);
            continue;
        }
        if in_header || line.starts_with(NO_NEWLINE_MARK) {
            continue;
        }
        let Some(mark) = line.chars().next() else {
            continue;
        };
        let text = &line[mark.len_utf8()..];
        if overlap > 0 {
            if mark != REMOVED_MARK {
                overlap -= 1;
            }
            continue;
        }
        match mark {
            ADDED_MARK => {
                new_file.push((text, true));
                next_new_line += 1;
            }
            KEPT_MARK => {
                new_file.push((text, false));
                next_new_line += 1;
            }
            REMOVED_MARK => removed.push(text),
            _ => {}
        }
    }
    Ok((removed, new_file))
}

fn cancels_a_move(removed: &mut Vec<&str>, text: &str) -> bool {
    match removed.iter().position(|gone| *gone == text) {
        Some(index) => {
            removed.swap_remove(index);
            true
        }
        None => false,
    }
}

pub fn gained_lines(diff: &str) -> Result<Vec<String>, Vec<String>> {
    let (mut removed, new_file) = diff_lines(diff)?;
    let levels = heading_levels(new_file.iter().map(|(text, _)| *text));
    let mut lines: Vec<String> = Vec::new();
    let mut headings: Vec<(usize, &str)> = Vec::new();
    let mut emitted = 0;
    for (&(text, added), level) in new_file.iter().zip(levels) {
        if let Some(level) = level {
            let above = headings
                .iter()
                .take_while(|(seen, _)| *seen < level)
                .count();
            headings.truncate(above);
            emitted = emitted.min(above);
            if level > TITLE_LEVEL {
                headings.push((level, text));
            }
            continue;
        }
        if headings.is_empty() || !added || (!is_blank(text) && cancels_a_move(&mut removed, text))
        {
            continue;
        }
        if is_blank(text) {
            if emitted == headings.len() {
                lines.push(String::new());
            }
            continue;
        }
        emit_context(&mut lines, &headings, &mut emitted);
        lines.push(text.to_string());
    }
    Ok(tidy(lines))
}

fn emit_context(lines: &mut Vec<String>, headings: &[(usize, &str)], emitted: &mut usize) {
    for (_, text) in &headings[*emitted..] {
        if lines.last().is_some_and(|last| !is_blank(last)) {
            lines.push(String::new());
        }
        lines.push(text.to_string());
    }
    *emitted = headings.len();
}

fn is_blank(line: &str) -> bool {
    line.trim().is_empty()
}

fn tidy(lines: Vec<String>) -> Vec<String> {
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

fn demoted(line: &str, level: Option<usize>) -> String {
    if level.is_some() {
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
         entry's commit, which the section heading names, each under the\n\
         headings it sits beneath in its changelog. The check in CI\n\
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
            let levels = heading_levels(lines.iter().map(String::as_str));
            let body = lines
                .iter()
                .zip(levels)
                .map(|(line, level)| demoted(line, level))
                .collect::<Vec<_>>();
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
            Ok((
                found.name.clone(),
                gained_lines(&changelog_diff(root, since, commit, &relative)?)?,
            ))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Section {
        commit: commit.to_string(),
        since: since.to_string(),
        entries,
    })
}

/// - Runs `git diff` in `root`.
fn changelog_diff(
    root: &Path,
    since: &str,
    commit: &str,
    relative: &str,
) -> Result<String, Vec<String>> {
    let context = format!("{CONTEXT_FLAG}{WHOLE_FILE_CONTEXT}");
    let args = [
        DIFF_ARGS.as_slice(),
        &[&context, since, commit, "--", relative],
    ]
    .concat();
    crate::git_in(root, &args)
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
    fn gained_lines_are_the_added_lines_minus_moves_and_preamble() {
        let diff = "diff --git a/x/CHANGELOG.md b/x/CHANGELOG.md\n\
                    --- a/x/CHANGELOG.md\n\
                    +++ b/x/CHANGELOG.md\n\
                    @@ -1,1 +1,10 @@\n\
                    +# zingo-netutils changelog\n\
                    +All notable changes to this project will be documented in this file.\n\
                    +The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),\n\
                    +Entries follow the house style.\n\
                    +\n\
                    +## [Unreleased]\n\
                    -- moved entry\n\
                    +\n\
                    +- new entry\n\
                    +  continues here\n\
                    +- moved entry\n";
        assert_eq!(
            gained_lines(diff).unwrap(),
            ["## [Unreleased]", "- new entry", "  continues here"]
        );
    }

    #[test]
    fn a_removed_line_cancels_one_added_line_and_not_every_repeat() {
        let diff = "@@ -1,2 +1,4 @@\n\
                    \x20### Changed\n\
                    -- bullet\n\
                    +- bullet\n\
                    +- new bullet\n\
                    +- bullet\n";
        assert_eq!(
            gained_lines(diff).unwrap(),
            ["### Changed", "- new bullet", "- bullet"]
        );
    }

    #[test]
    fn a_gained_item_carries_the_unchanged_headings_above_it() {
        let diff = "@@ -1,10 +1,14 @@\n\
                    \x20# Changelog\n\
                    \x20\n\
                    \x20## [Unreleased]\n\
                    \x20\n\
                    \x20### Added\n\
                    \x20- old entry\n\
                    +- new entry\n\
                    +\n\
                    +### Changed\n\
                    +- changed entry\n\
                    \x20\n\
                    \x20### Fixed\n\
                    \x20- old fix\n";
        assert_eq!(
            gained_lines(diff).unwrap(),
            [
                "## [Unreleased]",
                "",
                "### Added",
                "- new entry",
                "",
                "### Changed",
                "- changed entry"
            ]
        );
    }

    #[test]
    fn a_comment_inside_a_fenced_block_is_not_a_heading() {
        let diff = "@@ -1,6 +1,7 @@\n\
                    \x20## [Unreleased]\n\
                    \x20\n\
                    \x20### Added\n\
                    \x20```sh\n\
                    \x20# a comment in a shell example\n\
                    \x20```\n\
                    +- new entry\n";
        assert_eq!(
            gained_lines(diff).unwrap(),
            ["## [Unreleased]", "", "### Added", "- new entry"]
        );
    }

    #[test]
    fn a_release_cut_whose_items_all_moved_gains_nothing() {
        let diff = "@@ -1,4 +1,6 @@\n\
                    \x20## [Unreleased]\n\
                    -\n\
                    -### Added\n\
                    -- entry\n\
                    +\n\
                    +## [0.2.0] - 2026-10-11\n\
                    +\n\
                    +### Added\n\
                    +- entry\n";
        assert_eq!(gained_lines(diff).unwrap(), [""; 0]);
    }

    #[test]
    fn a_hunk_header_the_tool_cannot_read_is_refused() {
        let diagnostic = gained_lines("@@ garbage @@\n+- entry\n")
            .unwrap_err()
            .concat();
        assert_eq!(diagnostic, "cannot read the hunk header `@@ garbage @@`");
    }

    #[test]
    fn a_kept_line_repeated_by_overlapping_hunks_is_walked_once() {
        let diff = "@@ -1,1 +1,2 @@\n\
                    \x20## [Unreleased]\n\
                    +- a\n\
                    @@ -1,1 +1,3 @@\n\
                    \x20## [Unreleased]\n\
                    \x20- a\n\
                    +- b\n";
        assert_eq!(
            gained_lines(diff).unwrap(),
            ["## [Unreleased]", "- a", "- b"]
        );
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        crate::hermetic_git(dir, args)
    }

    #[test]
    fn the_diff_carries_a_heading_far_above_the_change_as_context() {
        let repo = crate::scratch_dir("changelog-context");
        git(&repo, &["init", "--quiet", "--initial-branch", "main"]);
        let changelog = repo.join(CRATE_CHANGELOG);
        let old = "## [Unreleased]\n\n### Added\n- a\n- b\n- c\n- d\n- e\n- f\n";
        std::fs::write(&changelog, old).unwrap();
        git(&repo, &["add", CRATE_CHANGELOG]);
        git(&repo, &["commit", "--quiet", "--message", "since"]);
        let since = git(&repo, &["rev-parse", "HEAD"]);
        std::fs::write(&changelog, format!("{old}- new\n")).unwrap();
        git(
            &repo,
            &["commit", "--quiet", "--all", "--message", "commit"],
        );
        let commit = git(&repo, &["rev-parse", "HEAD"]);
        let diff = changelog_diff(&repo, &since, &commit, CRATE_CHANGELOG).unwrap();
        assert_eq!(
            gained_lines(&diff).unwrap(),
            ["## [Unreleased]", "", "### Added", "- new"]
        );
        std::fs::remove_dir_all(&repo).unwrap();
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
                        "```sh".to_string(),
                        "# the codegen step".to_string(),
                        "```".to_string(),
                    ],
                ),
                ("pepper-sync".to_string(), vec![]),
            ],
        };
        assert_eq!(
            render_section(&section),
            format!(
                "## {COMMIT}\n\nSince {SINCE}.\n\nAudited `zingolib`, `pepper-sync`.\n\n### zingolib\n\n##### Added\n- entry\n\nConsumers must re-run codegen.\n```sh\n# the codegen step\n```\n"
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
