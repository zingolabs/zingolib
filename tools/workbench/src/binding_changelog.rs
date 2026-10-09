use std::path::{Path, PathBuf};

use crate::{binding_manifest, MANIFEST};

pub const BINARY: &str = "binding-changelog";
pub const FILE: &str = "bindings/CHANGELOG.md";
const CRATE_CHANGELOG: &str = "CHANGELOG.md";
const BINDING_CRATES: [&str; 2] = ["zingo-ffi/lib", "zingo-netutils/nym-proxy-ffi"];
const CHECK_FLAG: &str = "--check";
const USAGE: &str = "usage: binding-changelog [--check]";
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
const TREE_ARGS: [&str; 8] = [
    "tree", "--locked", "--edges", "normal", "--depth", "1", "--prefix", "none",
];
const TREE_FORMAT: [&str; 2] = ["--format", "{p}"];
const DIFF_ARGS: [&str; 4] = ["diff", "--no-color", "--no-ext-diff", "--no-renames"];
const PREAMBLE_PREFIXES: [&str; 4] = [
    "# Changelog",
    "All notable changes to this project",
    "The format is based on [Keep a Changelog]",
    "and this project adheres to [Semantic Versioning]",
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

pub fn audited_crates(tree_outputs: &[String], root: &Path) -> Result<Vec<Crate>, Vec<String>> {
    let mut crates: Vec<Crate> = Vec::new();
    for output in tree_outputs {
        let found: Vec<Crate> = output
            .lines()
            .filter_map(|line| parse_tree_line(line, root))
            .collect();
        if found.is_empty() {
            return Err(vec![format!(
                "no crate of this `cargo tree` output lies under {}: {}",
                root.display(),
                output.lines().next().unwrap_or_default()
            )]);
        }
        for each in found {
            if !crates.contains(&each) {
                crates.push(each);
            }
        }
    }
    Ok(crates)
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

pub fn render_header(crates: &[Crate]) -> String {
    let names = crates
        .iter()
        .map(|found| format!("`{}`", found.name))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "{TITLE}\n\n\
         The workbench tool `{BINARY}` writes every section below, one per entry\n\
         of `{}`, from the lines that the audited crate changelogs\n\
         gained after the previous entry's commit, named as `Since`, up to the\n\
         entry's commit, which the section heading names. The check in CI\n\
         regenerates every section and fails when the committed file differs. The\n\
         audited crates are the two Binding Layer crates and their direct\n\
         dependencies in this repository, as `cargo tree` reports them: {names}.\n\
         A change in another crate of this repository appears only where one of\n\
         those changelogs records it.\n",
        binding_manifest::FILE
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

pub fn render_file(header: &str, sections_newest_first: &[String]) -> String {
    std::iter::once(header)
        .chain(
            sections_newest_first
                .iter()
                .flat_map(|section| ["\n", section.as_str()]),
        )
        .collect()
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
            crate::stdout_in(root, crate::CARGO, &args, &[])
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
    let crates = audited_crates(&tree_outputs(root)?, root)?;
    let entries = binding_manifest::entries_at(root, None)?;
    let sections = binding_manifest::publication_commits(&entries)
        .iter()
        .rev()
        .map(|(commit, since)| Ok(render_section(&gather(root, &crates, since, commit)?)))
        .collect::<Result<Vec<_>, Vec<String>>>()?;
    Ok(render_file(&render_header(&crates), &sections))
}

/// - Runs `cargo tree` and `git` child processes in `root`.
/// - Writes `bindings/CHANGELOG.md` unless `args` holds `--check`.
pub fn dispatch(root: &Path, args: &[String]) -> Result<(), Vec<String>> {
    let file = root.join(FILE);
    match args {
        [] => std::fs::write(&file, regenerated(root)?)
            .map_err(|e| vec![format!("cannot write {}: {e}", file.display())]),
        [flag] if flag == CHECK_FLAG => {
            if crate::read(&file)? == regenerated(root)? {
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

    const ROOT: &str = "/host/zingolib";
    const COMMIT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const SINCE: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
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

    fn names(crates: Vec<Crate>) -> Vec<String> {
        crates.into_iter().map(|found| found.name).collect()
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
        assert_eq!(
            names(audited_crates(&outputs, Path::new(ROOT)).unwrap()),
            EXPECTED_AUDITED
        );
    }

    #[test]
    fn a_tree_with_no_crate_under_the_root_is_refused_by_its_first_line() {
        let outputs = ["zingo v2.0.0 (/host/elsewhere/zingo-ffi/lib)\n".to_string()];
        let diagnostic = audited_crates(&outputs, Path::new(ROOT))
            .unwrap_err()
            .concat();
        assert!(diagnostic.contains("zingo v2.0.0 (/host/elsewhere/zingo-ffi/lib)"));
        assert!(diagnostic.contains(ROOT));
    }

    #[test]
    fn the_repository_audits_the_five_crates_the_plan_names() {
        let root = crate::repo_root().unwrap();
        let crates = audited_crates(&tree_outputs(&root).unwrap(), &root).unwrap();
        assert_eq!(names(crates), EXPECTED_AUDITED);
    }

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
                "## {COMMIT}\n\nSince {SINCE}.\n\n### zingolib\n\n##### Added\n- entry\n\nConsumers must re-run codegen.\n"
            )
        );
    }

    #[test]
    fn the_file_is_the_header_then_the_sections_newest_first_with_one_blank_line_between() {
        let header = render_header(&[found("zingolib", "/host/zingolib/zingolib")]);
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
