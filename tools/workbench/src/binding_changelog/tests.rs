use super::*;
use std::path::PathBuf;

const COMMIT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SINCE: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

#[test]
fn gained_lines_are_the_added_lines_minus_moves_and_preamble() -> Result<(), Error> {
    let diff = "diff --git a/x/CHANGELOG.md b/x/CHANGELOG.md\n\
                    --- a/x/CHANGELOG.md\n\
                    +++ b/x/CHANGELOG.md\n\
                    @@ -1,1 +1,10 @@\n\
                    +# zl-netutils changelog\n\
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
        since: vec![SINCE.to_string()],
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
    let repo = scratch_repo("changelog-context")?;
    let changelog = repo.join(CRATE_CHANGELOG);
    let old = "## [Unreleased]\n\n### Added\n- a\n- b\n- c\n- d\n- e\n- f\n";
    crate::write(&changelog, old)?;
    crate::hermetic_git(&repo, &["add", CRATE_CHANGELOG])?;
    crate::hermetic_git(&repo, &["commit", "--quiet", "--message", "since"])?;
    let since = crate::hermetic_git(&repo, &["rev-parse", crate::BRANCH_TIP])?;
    crate::write(&changelog, format!("{old}- new\n"))?;
    crate::hermetic_git(
        &repo,
        &["commit", "--quiet", "--all", "--message", "commit"],
    )?;
    let commit = crate::hermetic_git(&repo, &["rev-parse", crate::BRANCH_TIP])?;
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
        since: vec![SINCE.to_string()],
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

fn bullets(items: &[&str]) -> String {
    items.iter().map(|item| format!("- {item}\n")).collect()
}

fn changelog_commit(
    repo: &Path,
    dir: &str,
    added: &[&str],
    fixed: &[&str],
) -> Result<String, Vec<String>> {
    let file = repo.join(dir).join(CRATE_CHANGELOG);
    crate::create_parent(&file)?;
    crate::write(
        &file,
        format!(
            "# Changelog\n\n## [Unreleased]\n\n### Added\n{}\n### Changed\n\n### Fixed\n{}",
            bullets(added),
            bullets(fixed)
        ),
    )?;
    crate::hermetic_git(repo, &["add", "--all"])?;
    binding_manifest::commit_on(repo, "changelog")
}

fn scratch_repo(name: &str) -> Result<crate::ScratchDir, Vec<String>> {
    let repo = crate::scratch_dir(name)?;
    crate::hermetic_git(&repo, &["init", "--quiet", "--initial-branch", "main"])?;
    Ok(repo)
}

fn entry(dir: &str, commit: &str, since: Option<&str>) -> binding_manifest::Entry {
    binding_manifest::Entry {
        commit: commit.to_string(),
        since: since.map(str::to_string),
        digests: Vec::new(),
        audited: vec![binding_manifest::AuditedCrate {
            name: "a".to_string(),
            dir: PathBuf::from(dir),
        }],
    }
}

#[test]
fn a_moved_changelog_gains_only_its_new_items() -> Result<(), Error> {
    let repo = scratch_repo("changelog-moved")?;
    let origin = changelog_commit(&repo, "old", &["one"], &[])?;
    crate::hermetic_git(&repo, &["mv", "old", "new"])?;
    let moved = changelog_commit(&repo, "new", &["one", "two"], &[])?;
    let file = regenerated_from(&repo, &[entry("new", &moved, Some(&origin))])?;
    assert!(file.contains("- two") && !file.contains("- one"), "{file}");
    Ok(())
}

#[test]
fn a_release_line_section_follows_the_candidate_it_fixes_and_the_merge_back_follows_both(
) -> Result<(), Error> {
    let repo = scratch_repo("changelog-release-line")?;
    let dir = "crate-a";
    let origin = changelog_commit(&repo, dir, &[], &[])?;
    let candidate = changelog_commit(&repo, dir, &["one"], &[])?;
    crate::hermetic_git(&repo, &["switch", "--quiet", "--create", "release"])?;
    let fix = changelog_commit(&repo, dir, &["one"], &["fix"])?;
    crate::hermetic_git(&repo, &["switch", "--quiet", "main"])?;
    let later = changelog_commit(&repo, dir, &["one", "two"], &[])?;
    crate::hermetic_git(
        &repo,
        &["merge", "--quiet", "--no-ff", "--no-edit", "release"],
    )?;
    let after = changelog_commit(&repo, dir, &["one", "two", "three"], &["fix"])?;

    let entries = [
        entry(dir, &candidate, Some(&origin)),
        entry(dir, &later, None),
        entry(dir, &fix, None),
        entry(dir, &after, None),
    ];
    let file = regenerated_from(&repo, &entries)?;
    let section_start = format!("\n{SECTION_MARK}");
    let sections: Vec<&str> = file.split(section_start.as_str()).skip(1).collect();
    let fix_section = sections
        .iter()
        .find(|section| section.starts_with(&fix))
        .expect("the newest section is the release line candidate's");
    assert!(
        fix_section.contains(&format!("{SINCE_PREFIX}{candidate}{SINCE_SUFFIX}")),
        "{fix_section}"
    );
    assert!(
        fix_section.contains("- fix") && !fix_section.contains("- two"),
        "{fix_section}"
    );
    let later_section = sections
        .iter()
        .find(|section| section.starts_with(&later))
        .expect("the later commit of main has its own section");
    assert!(
        later_section.contains(&format!("{SINCE_PREFIX}{candidate}{SINCE_SUFFIX}"))
            && later_section.contains("- two")
            && !later_section.contains("- fix"),
        "{later_section}"
    );
    let after_section = sections
        .iter()
        .find(|section| section.starts_with(&after))
        .expect("the first commit after the merge-back has its own section");
    assert!(
        after_section.contains(&format!(
            "{SINCE_PREFIX}{later}{SINCE_AND}{fix}{SINCE_SUFFIX}"
        )) && after_section.contains("- three")
            && !after_section.contains("- two")
            && !after_section.contains("- fix")
            && !after_section.contains("Fixed"),
        "{after_section}"
    );
    Ok(())
}

#[test]
fn the_since_line_names_every_commit_and_joins_the_last_with_and() {
    let since =
        |commits: &[&str]| since_list(&commits.iter().map(|c| c.to_string()).collect::<Vec<_>>());
    assert_eq!(since(&[]), "");
    assert_eq!(since(&["a"]), "a");
    assert_eq!(since(&["a", "b"]), "a and b");
    assert_eq!(since(&["a", "b", "c"]), "a, b and c");
}

#[test]
fn common_lines_are_those_gained_against_every_since_and_an_emptied_heading_goes() {
    let lines = |text: &str| text.lines().map(str::to_string).collect::<Vec<_>>();
    let against_dev = lines(
        "##### Added\n- three\n- three\n\n##### Changed\n###### Deep\n- deep\n\n##### Fixed\n- fix\n```sh\n# fenced\n```",
    );
    let against_line = lines("##### Added\n- two\n- three\n\n##### Fixed\n```sh\n# fenced\n```");
    assert_eq!(
        common(vec![against_dev.clone(), against_line]),
        lines("##### Added\n- three\n\n##### Fixed\n```sh\n# fenced\n```")
    );
    assert_eq!(common(vec![against_dev.clone()]), against_dev);
    assert_eq!(common(Vec::new()), Vec::<String>::new());
}

#[test]
fn the_committed_file_matches_its_published_entries() -> Result<(), Error> {
    let root = crate::repo_root()?;
    let published = binding_manifest::entries_at(&root, None)?;
    assert_eq!(
        crate::read(&root.join(FILE))?,
        regenerated_from(&root, &published)?
    );
    Ok(())
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
