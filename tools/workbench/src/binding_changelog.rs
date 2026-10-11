use std::fmt;
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
const DEMOTION_LEVELS: usize = 2;
const DIFF_HEADER_MARK: &str = "diff --git";
const HUNK_MARK: &str = "@@";
const RANGE_SEPARATOR: char = ',';
const FENCE_MARKS: [char; 2] = ['`', '~'];
const FENCE_LENGTH_MIN: usize = 3;
const INDENT_LIMIT: usize = 3;
const HEADING_LEVEL_LIMIT: usize = 6;
const LIST_MARKS: [&str; 3] = ["- ", "* ", "+ "];
const ORDERED_MARKS: [char; 2] = ['.', ')'];
const ORDERED_DIGITS_LIMIT: usize = 9;
const THEMATIC_MARKS: [char; 3] = ['*', '-', '_'];
const THEMATIC_LENGTH_MIN: usize = 3;
const ADDED_MARK: char = '+';
const KEPT_MARK: char = ' ';
const NO_NEWLINE_MARK: char = '\\';
const REMOVED_MARK: char = '-';
const DIFF_ARGS: [&str; 8] = [
    "-c",
    "diff.suppressBlankEmpty=false",
    "diff",
    "--no-color",
    "--no-ext-diff",
    "--no-renames",
    "--diff-algorithm=myers",
    "--indent-heuristic",
];
const CONTEXT_FLAG: &str = "--unified=";
const WHOLE_FILE_CONTEXT: i32 = i32::MAX / 2;
const TITLE_LEVEL: usize = 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    pub commit: String,
    pub since: String,
    pub entries: Vec<(String, Vec<String>)>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    HunkHeader(String),
    ItemAboveHeadings(String),
    MissingChangelog {
        name: String,
        relative: String,
        commit: String,
    },
    Differs,
    Usage,
    Diagnostics(Vec<String>),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::HunkHeader(line) => write!(f, "cannot read the hunk header `{line}`"),
            Error::ItemAboveHeadings(line) => write!(
                f,
                "`{line}` was gained above the first heading of its changelog"
            ),
            Error::MissingChangelog {
                name,
                relative,
                commit,
            } => write!(f, "audited crate {name} has no {relative} at {commit}"),
            Error::Differs => write!(
                f,
                "{FILE} differs from what {BINARY} generates from {}; run `{BINARY}` and commit the result",
                binding_manifest::FILE
            ),
            Error::Usage => f.write_str(USAGE),
            Error::Diagnostics(lines) => f.write_str(&lines.join("\n")),
        }
    }
}

impl std::error::Error for Error {}

from_diagnostics!(Error::Diagnostics);

impl From<Error> for Vec<String> {
    fn from(error: Error) -> Self {
        match error {
            Error::Diagnostics(lines) => lines,
            other => vec![other.to_string()],
        }
    }
}

fn unindented(line: &str) -> Option<&str> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    (indent <= INDENT_LIMIT).then(|| &line[indent..])
}

fn heading_level(line: &str) -> Option<usize> {
    let rest = unindented(line)?;
    let level = rest.chars().take_while(|c| *c == HEADING_MARK).count();
    let followed = rest[level..]
        .chars()
        .next()
        .is_none_or(|c| c == ' ' || c == '\t');
    ((1..=HEADING_LEVEL_LIMIT).contains(&level) && followed).then_some(level)
}

fn fence_opening(line: &str) -> Option<(char, usize)> {
    let rest = unindented(line)?;
    let mark = rest.chars().next().filter(|c| FENCE_MARKS.contains(c))?;
    let length = rest.chars().take_while(|c| *c == mark).count();
    (length >= FENCE_LENGTH_MIN).then_some((mark, length))
}

fn is_thematic_break(rest: &str) -> bool {
    let marks: String = rest.chars().filter(|c| *c != ' ' && *c != '\t').collect();
    marks.len() >= THEMATIC_LENGTH_MIN
        && THEMATIC_MARKS
            .iter()
            .any(|mark| marks.chars().all(|c| c == *mark))
}

fn is_ordered_item(rest: &str) -> bool {
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    let mut after = rest[digits..].chars();
    (1..=ORDERED_DIGITS_LIMIT).contains(&digits)
        && after.next().is_some_and(|c| ORDERED_MARKS.contains(&c))
        && after.next().is_none_or(|c| c == ' ' || c == '\t')
}

fn is_list_item(line: &str) -> bool {
    let rest = line.trim_start();
    let unordered = LIST_MARKS.iter().any(|mark| rest.starts_with(mark));
    (unordered && !is_thematic_break(rest)) || is_ordered_item(rest)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Heading(usize),
    Fenced,
    Plain,
}

fn kinds<'a>(lines: impl Iterator<Item = &'a str> + 'a) -> impl Iterator<Item = Kind> + 'a {
    let mut fence: Option<(char, usize)> = None;
    lines.map(move |line| {
        let opening = fence_opening(line);
        match (fence, opening) {
            (Some((mark, length)), Some((closing, closing_length)))
                if closing == mark && closing_length >= length =>
            {
                fence = None;
            }
            (Some(_), _) => {}
            (None, _) => fence = opening,
        }
        if fence.is_some() || opening.is_some() {
            Kind::Fenced
        } else {
            heading_level(line).map_or(Kind::Plain, Kind::Heading)
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

struct Diff<'a> {
    removed: Vec<&'a str>,
    new_file: Vec<(&'a str, bool)>,
}

fn diff_lines(diff: &str) -> Result<Diff<'_>, Error> {
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
            let start =
                hunk_new_start(header).ok_or_else(|| Error::HunkHeader(line.to_string()))?;
            overlap = next_new_line.saturating_sub(start);
            continue;
        }
        if in_header || line.starts_with(NO_NEWLINE_MARK) {
            continue;
        }
        let mark = line.chars().next().unwrap_or(KEPT_MARK);
        let text = &line[mark.len_utf8().min(line.len())..];
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
    Ok(Diff { removed, new_file })
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

pub fn gained_lines(diff: &str) -> Result<Vec<String>, Error> {
    let Diff {
        mut removed,
        new_file,
    } = diff_lines(diff)?;
    let kinds = kinds(new_file.iter().map(|(text, _)| *text));
    let mut lines: Vec<String> = Vec::new();
    let mut headings: Vec<(usize, &str)> = Vec::new();
    let mut emitted = 0;
    for (&(text, added), kind) in new_file.iter().zip(kinds) {
        if let Kind::Heading(level) = kind {
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
        if headings.is_empty() {
            if added
                && kind == Kind::Plain
                && is_list_item(text)
                && !cancels_a_move(&mut removed, text)
            {
                return Err(Error::ItemAboveHeadings(text.to_string()));
            }
            continue;
        }
        if !added || (!is_blank(text) && cancels_a_move(&mut removed, text)) {
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

fn demoted(level: usize, text: &str) -> String {
    let marks = (level + DEMOTION_LEVELS).min(HEADING_LEVEL_LIMIT);
    let body = text.trim_start().trim_start_matches(HEADING_MARK);
    format!("{}{body}", HEADING_MARK.to_string().repeat(marks))
}

fn emit_context(lines: &mut Vec<String>, headings: &[(usize, &str)], emitted: &mut usize) {
    for (level, text) in &headings[*emitted..] {
        if lines.last().is_some_and(|last| !is_blank(last)) {
            lines.push(String::new());
        }
        lines.push(demoted(*level, text));
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
         publish workflow recorded them from `cargo metadata`; each section names them\n\
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
        .map(|(name, lines)| format!("{CRATE_MARK}{name}\n\n{}\n", lines.join("\n")))
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

fn relative_changelog(found: &binding_manifest::AuditedCrate) -> Result<String, Error> {
    Ok(crate::utf8(&found.dir.join(CRATE_CHANGELOG))?.to_string())
}

fn gather(
    root: &Path,
    crates: &[binding_manifest::AuditedCrate],
    since: &str,
    commit: &str,
) -> Result<Section, Error> {
    let entries = crates
        .iter()
        .map(|found| {
            let relative = relative_changelog(found)?;
            if !crate::listed_at(root, commit, &relative)? {
                return Err(Error::MissingChangelog {
                    name: found.name.clone(),
                    relative,
                    commit: commit.to_string(),
                });
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
fn changelog_diff(root: &Path, since: &str, commit: &str, relative: &str) -> Result<String, Error> {
    let context = format!("{CONTEXT_FLAG}{WHOLE_FILE_CONTEXT}");
    let args = [
        DIFF_ARGS.as_slice(),
        &[&context, since, commit, "--", relative],
    ]
    .concat();
    Ok(crate::git_in(root, &args)?)
}

fn regenerated(root: &Path) -> Result<String, Error> {
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
        .collect::<Result<Vec<_>, Error>>()?;
    Ok(render_file(&render_header(), &sections))
}

/// - Runs `git` child processes in `root`.
/// - Writes `bindings/CHANGELOG.md`.
pub fn regenerate(root: &Path) -> Result<(), Error> {
    Ok(crate::write(&root.join(FILE), regenerated(root)?)?)
}

/// - Runs `git` child processes in `root`.
/// - Writes `bindings/CHANGELOG.md` unless `args` holds `--check`.
pub fn dispatch(root: &Path, args: &[String]) -> Result<(), Error> {
    match args {
        [] => regenerate(root),
        [flag] if flag == CHECK_FLAG => {
            if crate::read(&root.join(FILE))? == regenerated(root)? {
                Ok(())
            } else {
                Err(Error::Differs)
            }
        }
        _ => Err(Error::Usage),
    }
}

/// - Reads the process arguments.
/// - Exits the process through [`crate::run`].
pub fn main() -> ! {
    crate::dispatch_from_root(BINARY, |root, args| dispatch(root, args).map_err(Vec::from))
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMMIT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const SINCE: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    #[test]
    fn gained_lines_are_the_added_lines_minus_moves_and_preamble() -> Result<(), Error> {
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
            gained_lines(diff)?,
            ["#### [Unreleased]", "- new entry", "  continues here"]
        );
        Ok(())
    }

    #[test]
    fn a_removed_line_cancels_one_added_line_and_not_every_repeat() -> Result<(), Error> {
        let diff = "@@ -1,2 +1,4 @@\n\
                    \x20### Changed\n\
                    -- bullet\n\
                    +- bullet\n\
                    +- new bullet\n\
                    +- bullet\n";
        assert_eq!(
            gained_lines(diff)?,
            ["##### Changed", "- new bullet", "- bullet"]
        );
        Ok(())
    }

    #[test]
    fn a_gained_item_carries_the_unchanged_headings_above_it() -> Result<(), Error> {
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
            gained_lines(diff)?,
            [
                "#### [Unreleased]",
                "",
                "##### Added",
                "- new entry",
                "",
                "##### Changed",
                "- changed entry"
            ]
        );
        Ok(())
    }

    #[test]
    fn a_comment_inside_a_fenced_block_is_not_a_heading() -> Result<(), Error> {
        let diff = "@@ -1,6 +1,7 @@\n\
                    \x20## [Unreleased]\n\
                    \x20\n\
                    \x20### Added\n\
                    \x20```sh\n\
                    \x20# a comment in a shell example\n\
                    \x20```\n\
                    +- new entry\n";
        assert_eq!(
            gained_lines(diff)?,
            ["#### [Unreleased]", "", "##### Added", "- new entry"]
        );
        Ok(())
    }

    #[test]
    fn a_release_cut_whose_items_all_moved_gains_nothing() -> Result<(), Error> {
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
        assert_eq!(gained_lines(diff)?, [""; 0]);
        Ok(())
    }

    #[test]
    fn a_hunk_header_the_tool_cannot_read_is_refused() {
        assert_eq!(
            gained_lines("@@ garbage @@\n+- entry\n"),
            Err(Error::HunkHeader("@@ garbage @@".to_string()))
        );
    }

    #[test]
    fn a_kept_line_repeated_by_overlapping_hunks_is_walked_once() -> Result<(), Error> {
        let diff = "@@ -1,1 +1,2 @@\n\
                    \x20## [Unreleased]\n\
                    +- a\n\
                    @@ -1,1 +1,3 @@\n\
                    \x20## [Unreleased]\n\
                    \x20- a\n\
                    +- b\n";
        assert_eq!(gained_lines(diff)?, ["#### [Unreleased]", "- a", "- b"]);
        Ok(())
    }

    #[test]
    fn a_gained_comment_inside_a_kept_fence_is_not_demoted_in_the_section() -> Result<(), Error> {
        let diff = "@@ -1,5 +1,6 @@\n\
                    \x20## [Unreleased]\n\
                    \x20\n\
                    \x20### Added\n\
                    \x20```sh\n\
                    +# a new comment in the example\n\
                    \x20```\n";
        let section = Section {
            commit: COMMIT.to_string(),
            since: SINCE.to_string(),
            entries: vec![("zingolib".to_string(), gained_lines(diff)?)],
        };
        let rendered = render_section(&section);
        assert!(
            rendered.contains("\n# a new comment in the example\n"),
            "{rendered}"
        );
        Ok(())
    }

    #[test]
    fn a_gained_item_above_the_first_heading_is_refused() {
        let diff = "@@ -1,2 +1,3 @@\n\
                    \x20# Changelog\n\
                    +- a stray item\n\
                    \x20## [Unreleased]\n";
        assert_eq!(
            gained_lines(diff),
            Err(Error::ItemAboveHeadings("- a stray item".to_string()))
        );
    }

    #[test]
    fn an_ordered_item_above_the_first_heading_is_refused() {
        let diff = "@@ -1,2 +1,3 @@\n\
                    \x20# Changelog\n\
                    +1. a stray step\n\
                    \x20## [Unreleased]\n";
        assert_eq!(
            gained_lines(diff),
            Err(Error::ItemAboveHeadings("1. a stray step".to_string()))
        );
    }

    #[test]
    fn a_moved_fenced_or_ruled_line_above_the_first_heading_is_not_an_item() -> Result<(), Error> {
        let diff = "@@ -1,7 +1,9 @@\n\
                    \x20# Changelog\n\
                    -- a moved note\n\
                    +* * *\n\
                    +```sh\n\
                    +- not an item\n\
                    +```\n\
                    +- a moved note\n\
                    \x20## [Unreleased]\n\
                    +- new entry\n";
        assert_eq!(gained_lines(diff)?, ["#### [Unreleased]", "- new entry"]);
        Ok(())
    }

    #[test]
    fn an_empty_context_line_counts_as_a_kept_blank_line() -> Result<(), Error> {
        let diff = "@@ -1,2 +1,3 @@\n\
                    \x20## [Unreleased]\n\
                    \n\
                    +- a\n";
        assert_eq!(gained_lines(diff)?, ["#### [Unreleased]", "- a"]);
        Ok(())
    }

    #[test]
    fn demotion_stops_at_six_marks() {
        assert_eq!(demoted(2, "## [Unreleased]"), "#### [Unreleased]");
        assert_eq!(demoted(5, "##### Five"), "###### Five");
        assert_eq!(demoted(6, "   ######\tSix"), "######\tSix");
    }

    #[test]
    fn list_items_are_bullets_and_ordered_items_but_not_rules() {
        assert!(is_list_item("- a"));
        assert!(is_list_item("  + a"));
        assert!(is_list_item("1. a"));
        assert!(is_list_item("12) a"));
        assert!(is_list_item("1."));
        assert!(!is_list_item("1234567890. a"));
        assert!(!is_list_item("1.a"));
        assert!(!is_list_item("* * *"));
        assert!(!is_list_item("- - -"));
        assert!(!is_list_item("prose"));
    }

    #[test]
    fn a_heading_may_be_indented_or_followed_by_a_tab_and_has_at_most_six_marks() {
        assert_eq!(heading_level("##\tChanged"), Some(2));
        assert_eq!(heading_level("   ## Indented"), Some(2));
        assert_eq!(heading_level("    ## Code"), None);
        assert_eq!(heading_level("####### Seven"), None);
        assert_eq!(heading_level("#"), Some(1));
        assert_eq!(heading_level("#hashtag"), None);
    }

    #[test]
    fn a_tilde_fence_and_a_longer_backtick_fence_hide_their_lines() -> Result<(), Error> {
        let tilde = "@@ -1,6 +1,7 @@\n\
                     \x20## [Unreleased]\n\
                     \x20\n\
                     \x20### Added\n\
                     \x20~~~\n\
                     \x20# a comment\n\
                     \x20~~~\n\
                     +- new entry\n";
        assert_eq!(
            gained_lines(tilde)?,
            ["#### [Unreleased]", "", "##### Added", "- new entry"]
        );
        let longer = "@@ -1,7 +1,8 @@\n\
                      \x20## [Unreleased]\n\
                      \x20\n\
                      \x20### Added\n\
                      \x20````\n\
                      \x20```\n\
                      \x20# a comment\n\
                      \x20````\n\
                      +- new entry\n";
        assert_eq!(
            gained_lines(longer)?,
            ["#### [Unreleased]", "", "##### Added", "- new entry"]
        );
        Ok(())
    }

    #[test]
    fn the_diff_carries_a_heading_far_above_the_change_as_context() -> Result<(), Error> {
        let repo = crate::scratch_dir("changelog-context")?;
        crate::hermetic_git(&repo, &["init", "--quiet", "--initial-branch", "main"])?;
        let changelog = repo.join(CRATE_CHANGELOG);
        let old = "## [Unreleased]\n\n### Added\n- a\n- b\n- c\n- d\n- e\n- f\n";
        crate::write(&changelog, old)?;
        crate::hermetic_git(&repo, &["add", CRATE_CHANGELOG])?;
        crate::hermetic_git(&repo, &["commit", "--quiet", "--message", "since"])?;
        let since = crate::hermetic_git(&repo, &["rev-parse", "HEAD"])?;
        crate::write(&changelog, format!("{old}- new\n"))?;
        crate::hermetic_git(
            &repo,
            &["commit", "--quiet", "--all", "--message", "commit"],
        )?;
        let commit = crate::hermetic_git(&repo, &["rev-parse", "HEAD"])?;
        let diff = changelog_diff(&repo, &since, &commit, CRATE_CHANGELOG)?;
        assert_eq!(
            gained_lines(&diff)?,
            ["#### [Unreleased]", "", "##### Added", "- new"]
        );
        Ok(())
    }

    #[test]
    fn a_section_keeps_paragraph_breaks_and_omits_silent_crates() {
        let section = Section {
            commit: COMMIT.to_string(),
            since: SINCE.to_string(),
            entries: vec![
                (
                    "zingolib".to_string(),
                    vec![
                        "##### Added".to_string(),
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
    fn the_committed_file_passes_the_check_at_the_checked_out_commit() -> Result<(), Error> {
        dispatch(&crate::repo_root()?, &[CHECK_FLAG.to_string()])
    }

    #[test]
    fn a_flag_the_tool_does_not_know_is_refused_with_the_usage() -> Result<(), Error> {
        assert_eq!(
            dispatch(&crate::repo_root()?, &["--since".to_string()]),
            Err(Error::Usage)
        );
        assert_eq!(Error::Usage.to_string(), USAGE);
        Ok(())
    }
}
