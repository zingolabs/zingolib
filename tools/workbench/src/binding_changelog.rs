use std::path::{Path, PathBuf};

use crate::MANIFEST;

pub const BINARY: &str = "binding-changelog";
pub const FILE: &str = "bindings/CHANGELOG.md";
const CRATE_CHANGELOG: &str = "CHANGELOG.md";
const BINDING_CRATES: [&str; 2] = ["zingo-ffi/lib", "zingo-netutils/nym-proxy-ffi"];
const SINCE_FLAG: &str = "--since";
const COMMIT_FLAG: &str = "--commit";
const CHECK_FLAG: &str = "--check";
const HEAD: &str = "HEAD";
const USAGE: &str =
    "usage: binding-changelog --since <commit> [--commit <commit>] | binding-changelog --check";
const TITLE: &str = "# Binding Layer changelog";
const SECTION_MARK: &str = "## ";
const SINCE_PREFIX: &str = "Since ";
const SINCE_SUFFIX: char = '.';
const CRATE_MARK: &str = "### ";
const HEADING_MARK: char = '#';
const DEMOTION: &str = "##";
const DIFF_HEADER_MARK: &str = "diff --git";
const HUNK_MARK: &str = "@@";
const ADDED_MARK: char = '+';
const REMOVED_MARK: char = '-';
const PATH_OPEN: &str = " (";
const PATH_CLOSE: char = ')';
const TREE_ARGS: [&str; 7] = [
    "tree", "--edges", "normal", "--depth", "1", "--prefix", "none",
];
const TREE_FORMAT: [&str; 2] = ["--format", "{p}"];
const PREAMBLE: [&str; 4] = [
    "# Changelog",
    "All notable changes to this project will be documented in this file.",
    "The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),",
    "and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Crate {
    pub name: String,
    pub dir: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    pub commit: String,
    pub since: String,
    pub entries: Vec<(String, Vec<String>)>,
}

pub fn parse_tree_line(line: &str, root: &Path) -> Option<Crate> {
    let (head, rest) = line.split_once(PATH_OPEN)?;
    let dir = Path::new(rest.strip_suffix(PATH_CLOSE)?);
    let name = head.split_whitespace().next()?;
    dir.starts_with(root).then(|| Crate {
        name: name.to_string(),
        dir: dir.to_path_buf(),
    })
}

pub fn audited_crates(tree_outputs: &[String], root: &Path) -> Vec<Crate> {
    tree_outputs
        .iter()
        .flat_map(|output| output.lines())
        .filter_map(|line| parse_tree_line(line, root))
        .fold(Vec::new(), |mut crates, found| {
            if !crates.contains(&found) {
                crates.push(found);
            }
            crates
        })
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
    added
        .into_iter()
        .filter(|text| !text.trim().is_empty() && !removed.contains(text) && !is_preamble(text))
        .collect()
}

fn is_preamble(line: &str) -> bool {
    PREAMBLE.iter().any(|known| line.trim() == *known)
}

fn demoted(line: &str) -> String {
    if line.starts_with(HEADING_MARK) {
        format!("{DEMOTION}{line}")
    } else {
        line.to_string()
    }
}

pub fn render_header(crates: &[Crate]) -> String {
    let names = crates
        .iter()
        .map(|found| format!("`{}`", found.name))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "{TITLE}\n\n\
         The workbench tool `{BINARY}` writes every section below, one per\n\
         published commit, from the lines that the audited crate changelogs gained\n\
         since the previous published commit. The audited crates are the two Binding\n\
         Layer crates and their direct dependencies in this repository, as\n\
         `cargo tree` reports them: {names}.\n\
         A change in another crate of this repository appears only where one of\n\
         those changelogs records it.\n"
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
    block(&format!(
        "{SECTION_MARK}{}\n\n{SINCE_PREFIX}{}{SINCE_SUFFIX}\n\n{entries}",
        section.commit, section.since
    ))
}

fn block(text: &str) -> String {
    let mut block = text.trim_end().to_string();
    block.push('\n');
    block
}

pub fn split_sections(file: &str) -> (String, Vec<String>) {
    let mut header = String::new();
    let mut sections: Vec<String> = Vec::new();
    for line in file.split_inclusive('\n') {
        if line.starts_with(SECTION_MARK) {
            sections.push(String::new());
        }
        match sections.last_mut() {
            Some(section) => section.push_str(line),
            None => header.push_str(line),
        }
    }
    (
        block(&header),
        sections.iter().map(|section| block(section)).collect(),
    )
}

pub fn section_commits(section: &str) -> Option<(String, String)> {
    let mut lines = section.lines();
    let commit = lines.next()?.strip_prefix(SECTION_MARK)?.trim();
    let since = lines
        .find_map(|line| line.strip_prefix(SINCE_PREFIX))?
        .strip_suffix(SINCE_SUFFIX)?;
    Some((commit.to_string(), since.to_string()))
}

pub fn assemble(header: &str, newest: &str, older: &[String]) -> String {
    let kept = older.iter().filter(|section| {
        section_commits(section).map(|(commit, _)| commit)
            != section_commits(newest).map(|(commit, _)| commit)
    });
    [header, "\n", newest]
        .into_iter()
        .chain(kept.flat_map(|section| ["\n", section.as_str()]))
        .collect()
}

fn resolve(root: &Path, revision: &str) -> Result<String, Vec<String>> {
    let commit = format!("{revision}^{{commit}}");
    crate::stdout_in(
        root,
        "git",
        &["rev-parse", "--verify", "--quiet", &commit],
        &[],
    )
    .map(|sha| sha.trim().to_string())
    .map_err(|_| vec![format!("{revision} is not a commit of this repository")])
}

fn changelog_exists(root: &Path, commit: &str, relative: &str) -> Result<bool, Vec<String>> {
    crate::stdout_in(
        root,
        "git",
        &["ls-tree", "--name-only", commit, "--", relative],
        &[],
    )
    .map(|listed| !listed.trim().is_empty())
}

fn tree_outputs(root: &Path) -> Result<Vec<String>, Vec<String>> {
    BINDING_CRATES
        .iter()
        .map(|dir| {
            let manifest = root.join(dir).join(MANIFEST);
            let args = [
                TREE_ARGS.as_slice(),
                &["--manifest-path", crate::utf8(&manifest)?],
                TREE_FORMAT.as_slice(),
            ]
            .concat();
            crate::stdout_in(root, "cargo", &args, &[])
        })
        .collect()
}

fn relative_changelog(root: &Path, found: &Crate) -> Result<String, Vec<String>> {
    let relative = found
        .dir
        .strip_prefix(root)
        .map_err(|_| {
            vec![format!(
                "{} is outside {}",
                found.dir.display(),
                root.display()
            )]
        })?
        .join(CRATE_CHANGELOG);
    Ok(crate::utf8(&relative)?.to_string())
}

fn gather(
    root: &Path,
    crates: &[Crate],
    since: &str,
    commit: &str,
) -> Result<Section, Vec<String>> {
    let entries = crates
        .iter()
        .map(|found| {
            let relative = relative_changelog(root, found)?;
            if !changelog_exists(root, commit, &relative)? {
                return Err(vec![format!(
                    "audited crate {} has no {relative} at {commit}",
                    found.name
                )]);
            }
            let diff = crate::stdout_in(
                root,
                "git",
                &[
                    "diff",
                    "--no-color",
                    "--no-renames",
                    since,
                    commit,
                    "--",
                    &relative,
                ],
                &[],
            )?;
            Ok((found.name.clone(), gained_lines(&diff)))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Section {
        commit: commit.to_string(),
        since: since.to_string(),
        entries,
    })
}

fn regenerate(root: &Path, since: &str, commit: &str) -> Result<(String, String), Vec<String>> {
    let crates = audited_crates(&tree_outputs(root)?, root);
    let section = gather(root, &crates, since, commit)?;
    Ok((render_header(&crates), render_section(&section)))
}

fn write(root: &Path, since: &str, commit: &str) -> Result<(), Vec<String>> {
    let file = root.join(FILE);
    let existing = if file.is_file() {
        crate::read(&file)?
    } else {
        String::new()
    };
    let (_, older) = split_sections(&existing);
    let (header, newest) = regenerate(root, since, commit)?;
    std::fs::write(&file, assemble(&header, &newest, &older))
        .map_err(|e| vec![format!("cannot write {}: {e}", file.display())])
}

fn check(root: &Path) -> Result<(), Vec<String>> {
    let file = root.join(FILE);
    let committed = crate::read(&file)?;
    let (_, sections) = split_sections(&committed);
    let regenerated = match sections.split_first() {
        None => render_header(&audited_crates(&tree_outputs(root)?, root)),
        Some((newest, older)) => {
            let (commit, since) = section_commits(newest)
                .ok_or_else(|| vec![format!("{FILE}: the newest section names no commit")])?;
            let (header, section) = regenerate(root, &since, &commit)?;
            assemble(&header, &section, older)
        }
    };
    if committed == regenerated {
        Ok(())
    } else {
        Err(vec![format!(
            "{FILE} differs from what {BINARY} generates; run `{BINARY} {SINCE_FLAG} <commit>` and commit the result"
        )])
    }
}

/// - Runs `cargo tree` and `git` child processes in `root`.
/// - Writes `bindings/CHANGELOG.md` unless `args` holds `--check`.
pub fn dispatch(root: &Path, args: &[String]) -> Result<(), Vec<String>> {
    if args.iter().any(|arg| arg == CHECK_FLAG) {
        return check(root);
    }
    let since = crate::flag_value(args, SINCE_FLAG)?
        .ok_or_else(|| vec![format!("missing {SINCE_FLAG}"), USAGE.to_string()])?;
    let commit = crate::flag_value(args, COMMIT_FLAG)?.unwrap_or(HEAD);
    write(root, &resolve(root, since)?, &resolve(root, commit)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: &str = "/host/zingolib";
    const COMMIT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const SINCE: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const OLDER: &str = "cccccccccccccccccccccccccccccccccccccccc";
    const EXPECTED_AUDITED: [&str; 5] = [
        "zingo",
        "pepper-sync",
        "zingolib",
        "zingo-nym-proxy-ffi",
        "zingo-netutils",
    ];

    fn found(name: &str, dir: &str) -> Crate {
        Crate {
            name: name.to_string(),
            dir: PathBuf::from(dir),
        }
    }

    #[test]
    fn a_tree_line_names_an_audited_crate_only_when_its_path_is_under_the_root() {
        let root = Path::new(ROOT);
        assert_eq!(
            parse_tree_line("zingolib v6.0.0 (/host/zingolib/zingolib)", root),
            Some(found("zingolib", "/host/zingolib/zingolib"))
        );
        assert_eq!(parse_tree_line("android_logger v0.11.3", root), None);
        assert_eq!(
            parse_tree_line("serde_derive v1.0.0 (proc-macro)", root),
            None
        );
        assert_eq!(
            parse_tree_line("other v1.0.0 (/host/elsewhere/other)", root),
            None
        );
    }

    #[test]
    fn audited_crates_keep_tree_order_and_drop_repeats() {
        let outputs = [
            "zingo v2.0.0 (/host/zingolib/zingo-ffi/lib)\npepper-sync v0.5.0 (/host/zingolib/pepper-sync)\nzingolib v6.0.0 (/host/zingolib/zingolib)\n".to_string(),
            "zingo-nym-proxy-ffi v0.1.0 (/host/zingolib/zingo-netutils/nym-proxy-ffi)\nzingo-netutils v5.0.1 (/host/zingolib/zingo-netutils)\nzingo-netutils v5.0.1 (/host/zingolib/zingo-netutils)\n".to_string(),
        ];
        let names: Vec<String> = audited_crates(&outputs, Path::new(ROOT))
            .into_iter()
            .map(|found| found.name)
            .collect();
        assert_eq!(names, EXPECTED_AUDITED);
    }

    #[test]
    fn the_repository_audits_the_five_crates_the_plan_names() {
        let root = crate::repo_root().unwrap();
        let names: Vec<String> = audited_crates(&tree_outputs(&root).unwrap(), &root)
            .into_iter()
            .map(|found| found.name)
            .collect();
        assert_eq!(names, EXPECTED_AUDITED);
    }

    #[test]
    fn gained_lines_are_the_added_lines_minus_moves_blanks_headers_and_preamble() {
        let diff = "diff --git a/x/CHANGELOG.md b/x/CHANGELOG.md\n\
                    --- a/x/CHANGELOG.md\n\
                    +++ b/x/CHANGELOG.md\n\
                    @@ -0,0 +1,8 @@\n\
                    +# Changelog\n\
                    +All notable changes to this project will be documented in this file.\n\
                    +## [Unreleased]\n\
                    -- moved entry\n\
                    +\n\
                    +- new entry\n\
                    +  continues here\n\
                    +- moved entry\n";
        assert_eq!(
            gained_lines(diff),
            ["## [Unreleased]", "- new entry", "  continues here"]
        );
    }

    #[test]
    fn a_section_demotes_gained_headings_and_omits_crates_that_gained_nothing() {
        let section = Section {
            commit: COMMIT.to_string(),
            since: SINCE.to_string(),
            entries: vec![
                (
                    "zingolib".to_string(),
                    vec!["### Added".to_string(), "- entry".to_string()],
                ),
                ("pepper-sync".to_string(), vec![]),
            ],
        };
        assert_eq!(
            render_section(&section),
            format!("## {COMMIT}\n\nSince {SINCE}.\n\n### zingolib\n\n##### Added\n- entry\n")
        );
    }

    #[test]
    fn the_newest_section_is_readable_back_and_replaces_its_own_commit() {
        let header = render_header(&[found("zingolib", "/host/zingolib/zingolib")]);
        let older = format!("## {OLDER}\n\nSince {SINCE}.\n");
        let stale = format!("## {COMMIT}\n\nSince {OLDER}.\n\n### zingolib\n\n- stale\n");
        let first = assemble(&header, &stale, std::slice::from_ref(&older));
        let (parsed_header, sections) = split_sections(&first);
        assert_eq!(parsed_header, header);
        assert_eq!(sections, [stale.clone(), older.clone()]);
        assert_eq!(
            section_commits(&sections[0]),
            Some((COMMIT.to_string(), OLDER.to_string()))
        );
        let fresh = format!("## {COMMIT}\n\nSince {OLDER}.\n\n### zingolib\n\n- fresh\n");
        let second = assemble(&header, &fresh, &sections);
        assert_eq!(split_sections(&second).1, [fresh, older]);
    }

    #[test]
    fn the_committed_file_is_the_header_the_tool_renders_with_no_sections() {
        let root = crate::repo_root().unwrap();
        let committed = crate::read(&root.join(FILE)).unwrap();
        let crates = audited_crates(&tree_outputs(&root).unwrap(), &root);
        assert_eq!(committed, render_header(&crates));
    }

    #[test]
    fn the_flags_are_parsed_and_since_is_required_outside_check() {
        assert!(dispatch(Path::new(ROOT), &[])
            .unwrap_err()
            .concat()
            .contains(SINCE_FLAG));
    }
}
