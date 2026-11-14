use super::*;

const FIRST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SECOND: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const ORIGIN: &str = "cccccccccccccccccccccccccccccccccccccccc";
const DIGEST: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const ROOT: &str = "/host/zingolib";
const EXPECTED_AUDITED: [&str; 5] = [
    "zingolib-ffi (ffi/lib)",
    "pepper-sync (crates/pepper-sync)",
    "zingolib (crates/zingolib)",
    "zingolib-nym-proxy-ffi (crates/netutils/nym-proxy-ffi)",
    "zl-netutils (crates/netutils)",
];

fn audited(items: &[&str]) -> Result<Vec<AuditedCrate>, Vec<String>> {
    items
        .iter()
        .map(|item| {
            crate::required(
                AuditedCrate::parse(item),
                &format!("audited crate in `{item}`"),
            )
        })
        .collect()
}

fn entry(commit: &str, since: Option<&str>, platforms: &[&str]) -> Result<Entry, Vec<String>> {
    Ok(Entry {
        commit: commit.to_string(),
        since: since.map(str::to_string),
        digests: platforms
            .iter()
            .map(|platform| (platform.to_string(), DIGEST.to_string()))
            .collect(),
        audited: audited(&EXPECTED_AUDITED[..2])?,
    })
}

fn rendered(crates: &[AuditedCrate]) -> Vec<String> {
    crates.iter().map(ToString::to_string).collect()
}

fn manifest(entries: &[Entry]) -> String {
    entries
        .iter()
        .map(|entry| {
            let since = entry
                .since
                .as_ref()
                .map(|since| format!("since = \"{since}\"\n"))
                .unwrap_or_default();
            let digests: String = entry
                .digests
                .iter()
                .map(|(platform, digest)| format!("{platform} = \"{digest}\"\n"))
                .collect();
            let audited = if entry.audited.is_empty() {
                String::new()
            } else {
                format!("audited = {}\n", audited_array(&entry.audited))
            };
            format!(
                "# a comment\n\n[[entry]]\ncommit = \"{}\"\n{since}{digests}{audited}",
                entry.commit
            )
        })
        .collect()
}

#[test]
fn the_manifest_round_trips_through_its_text_form() -> Result<(), Vec<String>> {
    let entries = vec![
        entry(FIRST, Some(ORIGIN), &[ANDROID, IOS])?,
        entry(SECOND, None, &PLATFORMS)?,
    ];
    let parsed = parse(&manifest(&entries))?;
    assert_eq!(parsed, entries);
    assert_eq!(validate(&parsed), Ok(()));
    Ok(())
}

#[test]
fn the_grammar_rejects_what_it_does_not_name() -> Result<(), Vec<String>> {
    let rejected = [
        "[[entry]]\ncommit = \"short\"\n",
        "[[entry]]\nandroid = \"sha256:short\"\n",
        "[[entry]]\ncommit = \"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"\ncolour = \"red\"\n",
        "commit = \"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"\n",
        "[[entry]]\ncommit = aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n",
        "[[entry]]\n[entry.digests]\n",
        "[[entry]]\naudited = \"zingolib-ffi (ffi/lib)\"\n",
        "[[entry]]\naudited = []\n",
        "[[entry]]\naudited = [\"zingolib-ffi\"]\n",
        "[[entry]]\naudited = [\"zingolib-ffi (ffi/lib)\", \"pepper-sync pepper-sync\"]\n",
        "[[entry]]\nandroid = [\"sha256:0\"]\n",
    ];
    for text in rejected {
        let diagnostic = crate::refused(parse(text))?.concat();
        assert!(diagnostic.starts_with(FILE), "{text}: {diagnostic}");
    }
    assert!(parse("[[entry]]\nandroid = \"x\"\nandroid = \"y\"\n").is_err());
    assert!(parse(
        "[[entry]]\naudited = [\"zingolib-ffi (ffi/lib)\"]\naudited = [\"zingolib-ffi (ffi/lib)\"]\n"
    )
    .is_err());
    Ok(())
}

#[test]
fn a_platform_outside_the_set_is_refused_by_name() -> Result<(), Vec<String>> {
    assert_eq!(platform(ANDROID), Ok(ANDROID));
    assert_eq!(platform(IOS), Ok(IOS));
    let diagnostic = crate::refused(platform("linux"))?.concat();
    assert!(diagnostic.contains("linux") && diagnostic.contains(ANDROID));
    Ok(())
}

#[test]
fn an_audited_crate_round_trips_through_its_text_form_and_a_tree_line_is_made_relative(
) -> Result<(), Vec<String>> {
    let found = crate::required(
        AuditedCrate::parse("zingolib-ffi (ffi/lib)"),
        "audited crate in the text form",
    )?;
    assert_eq!(found.to_string(), "zingolib-ffi (ffi/lib)");
    assert_eq!(found.dir, PathBuf::from("ffi/lib"));
    assert_eq!(AuditedCrate::parse("zingolib-ffi"), None);
    assert_eq!(AuditedCrate::parse("zingolib-ffi v2.0.0 (ffi/lib)"), None);
    Ok(())
}

#[test]
fn a_package_under_the_root_is_an_audited_crate_and_one_outside_is_not() -> Result<(), Vec<String>>
{
    let root = crate::repo_root()?;
    let metadata = crate::metadata_of(&root.join(crate::MANIFEST), true)?;
    let package = |name: &str| {
        crate::required(
            metadata
                .packages
                .iter()
                .find(|package| package.name.as_str() == name),
            &format!("package {name} in the metadata"),
        )
    };
    assert_eq!(
        AuditedCrate::from_package(package("zingolib-ffi")?, &root),
        AuditedCrate::parse("zingolib-ffi (ffi/lib)")
    );
    assert_eq!(AuditedCrate::from_package(package("serde")?, &root), None);
    assert_eq!(
        AuditedCrate::from_package(package("zingolib-ffi")?, Path::new(ROOT)),
        None
    );
    Ok(())
}

#[test]
fn the_checked_out_commit_audits_the_five_crates_the_plan_names() -> Result<(), Vec<String>> {
    let root = crate::repo_root()?;
    assert_eq!(rendered(&audited_at(&root)?), EXPECTED_AUDITED);
    Ok(())
}

#[test]
fn the_recorded_audited_crates_are_checked_against_cargo_tree_at_the_commit(
) -> Result<(), Vec<String>> {
    let root = crate::repo_root()?;
    let head = crate::commit_of(&root, crate::BRANCH_TIP)?;
    let mut recorded = entry(&head, Some(ORIGIN), &[ANDROID])?;
    recorded.audited = audited(&EXPECTED_AUDITED)?;
    assert_eq!(
        audited_diagnostics(&root, std::slice::from_ref(&recorded))?,
        Vec::<String>::new()
    );
    recorded.audited.pop();
    let diagnostic = audited_diagnostics(&root, &[recorded])?.concat();
    assert!(diagnostic.contains(&head), "{diagnostic}");
    assert!(!root.join(CHECKOUTS_DIR).join(&head).exists());
    Ok(())
}

#[test]
fn a_push_re_derives_only_the_appended_audited_sets_and_may_verify_every_digest(
) -> Result<(), Vec<String>> {
    let head = vec![
        entry(FIRST, Some(ORIGIN), &["android"])?,
        entry(SECOND, None, &["android"])?,
    ];
    let (appended, digests) = verification_scopes(&head, 1, true);
    assert_eq!(appended, &head[1..]);
    assert_eq!(digests, &head[..]);
    let (appended, digests) = verification_scopes(&head, 1, false);
    assert_eq!(appended, &head[1..]);
    assert_eq!(digests, &head[1..]);
    let (appended, _) = verification_scopes(&head, 5, false);
    assert!(appended.is_empty());
    Ok(())
}

#[test]
fn the_required_set_difference_has_one_home() {
    assert_eq!(REQUIRED_PLATFORMS, PLATFORMS);
    assert_eq!(missing_required(&[]), REQUIRED_PLATFORMS);
    assert_eq!(missing_required(&PLATFORMS), Vec::<&str>::new());
    assert_eq!(missing_required(&[IOS]), [ANDROID]);
    assert_eq!(missing_required(&[ANDROID]), [IOS]);
    assert_eq!(
        repository(ANDROID),
        "zingolabs/zingolib/binding-layer-android"
    );
}

#[test]
fn an_incomplete_entry_is_refused_by_each_part_it_lacks() -> Result<(), Vec<String>> {
    let mut incomplete = entry(FIRST, Some(ORIGIN), &[])?;
    incomplete.audited.clear();
    let parts = REQUIRED_PLATFORMS.len() + 1;
    assert_eq!(missing(&incomplete).len(), parts);
    let diagnostics = crate::refused(validate(&[incomplete]))?;
    assert_eq!(diagnostics.len(), parts, "{diagnostics:?}");
    assert_eq!(
        diagnostics[0],
        format!("entry 1 ({FIRST}) names no {ANDROID} digest")
    );
    Ok(())
}

#[test]
fn an_appended_entry_follows_the_last_one_after_one_blank_line() -> Result<(), Vec<String>> {
    let first = entry(FIRST, Some(ORIGIN), &PLATFORMS)?;
    let text = manifest(std::slice::from_ref(&first));
    let mut second = entry(SECOND, None, &PLATFORMS)?;
    second.audited = audited(&EXPECTED_AUDITED)?;
    let grown = appended(&text, &second)?;
    assert_eq!(parse(&grown)?, [first, second.clone()]);
    assert!(grown.starts_with(&text), "{grown}");
    let tail = format!(
        "\n\n[[entry]]\ncommit = \"{SECOND}\"\naudited = {}\nandroid = \"{DIGEST}\"\nios = \"{DIGEST}\"\n",
        audited_array(&second.audited)
    );
    assert!(grown.ends_with(&tail), "{grown}");
    let mut incomplete = second;
    incomplete.digests.pop();
    assert!(appended(&text, &incomplete).is_err());
    assert!(appended(&text, &entry(FIRST, None, &PLATFORMS)?).is_err());
    Ok(())
}

#[test]
fn validation_pins_the_since_rule_the_digest_rule_and_uniqueness() -> Result<(), Vec<String>> {
    let diagnostics = crate::refused(validate(&[
        entry(FIRST, None, &[])?,
        entry(SECOND, Some(ORIGIN), &[IOS])?,
        entry(SECOND, None, &[ANDROID])?,
    ]))?
    .concat();
    assert!(diagnostics.contains("first and names no since"));
    assert!(diagnostics.contains("names no android digest"));
    assert!(diagnostics.contains("names no ios digest"));
    assert_eq!(validate(&[entry(FIRST, Some(ORIGIN), &PLATFORMS)?]), Ok(()));
    assert!(validate(&[entry(FIRST, Some(ORIGIN), &[ANDROID])?]).is_err());
    assert!(validate(&[entry(FIRST, Some(ORIGIN), &[IOS])?]).is_err());
    assert!(diagnostics.contains("only the first entry may"));
    assert!(diagnostics.contains("repeats an earlier entry"));
    Ok(())
}

#[test]
fn an_unreadable_package_is_explained_by_its_visibility() {
    let hint = private_package_hint("ghcr.io/zingolabs/zingolib/binding-layer-android:abc");
    assert!(hint.contains("binding-layer-android:abc"));
    assert!(hint.contains("public"));
}

#[test]
fn a_merged_entry_never_changes_and_the_head_may_only_append() -> Result<(), Vec<String>> {
    let base = vec![entry(FIRST, Some(ORIGIN), &[ANDROID])?];
    let appended = vec![base[0].clone(), entry(SECOND, None, &[ANDROID])?];
    assert_eq!(unchanged_since_base(&base, &appended), Ok(()));
    let edited = vec![entry(FIRST, Some(ORIGIN), &[ANDROID, IOS])?];
    assert!(unchanged_since_base(&base, &edited).is_err());
    assert!(unchanged_since_base(&base, &[]).is_err());
    let reordered = vec![entry(SECOND, Some(ORIGIN), &[ANDROID])?, base[0].clone()];
    assert!(unchanged_since_base(&base, &reordered).is_err());
    Ok(())
}

#[test]
fn each_publication_follows_its_maximal_recorded_ancestors_and_the_first_its_since(
) -> Result<(), Vec<String>> {
    let repo = crate::scratch_dir("manifest-publication-commits")?;
    crate::hermetic_git(&repo, &["init", "--quiet", "--initial-branch", "main"])?;
    let origin = commit_on(&repo, "origin")?;
    let first = commit_on(&repo, "first")?;
    let between = commit_on(&repo, "between")?;
    crate::hermetic_git(&repo, &["switch", "--quiet", "--create", "side", &first])?;
    let fix = commit_on(&repo, "fix")?;
    crate::hermetic_git(&repo, &["switch", "--quiet", "main"])?;
    let second = commit_on(&repo, "second")?;
    crate::hermetic_git(&repo, &["merge", "--quiet", "--no-ff", "--no-edit", "side"])?;
    let after = commit_on(&repo, "after")?;

    // `between` is published after `second`, which descends from it, so it is nearest by
    // publication and not by ancestry. `after` descends from `second` and from `fix`, and
    // neither descends from the other.
    let entries = vec![
        entry(&first, Some(&origin), &[ANDROID])?,
        entry(&second, None, &[ANDROID])?,
        entry(&fix, None, &[ANDROID])?,
        entry(&between, None, &[ANDROID])?,
        entry(&after, None, &[ANDROID])?,
    ];
    assert_eq!(
        publication_commits(&repo, &entries)?,
        [
            (first.clone(), vec![origin.clone()]),
            (second.clone(), vec![first.clone()]),
            (fix.clone(), vec![first.clone()]),
            (between.clone(), vec![first.clone()]),
            (after.clone(), vec![second.clone(), fix.clone()]),
        ]
    );
    let rootless = vec![
        entry(&second, Some(&origin), &[ANDROID])?,
        entry(&first, None, &[ANDROID])?,
    ];
    let refused = crate::refused(publication_commits(&repo, &rootless))?.concat();
    assert!(
        refused.contains("entry 2")
            && refused.contains(&first)
            && refused.contains("no earlier entry is an ancestor"),
        "{refused}"
    );
    Ok(())
}

#[test]
fn a_reference_names_the_platform_repository_and_the_commit_tag() {
    assert_eq!(
        reference(ANDROID, FIRST),
        format!("ghcr.io/zingolabs/zingolib/binding-layer-android:{FIRST}")
    );
}

#[test]
fn commits_are_resolved_in_one_pass_and_a_missing_one_is_named() -> Result<(), Vec<String>> {
    let root = crate::repo_root()?;
    let head = crate::commit_of(&root, crate::BRANCH_TIP)?;
    assert_eq!(
        commits_exist(&root, &[entry(&head, None, &[ANDROID])?]),
        Ok(())
    );
    let diagnostic = crate::refused(commits_exist(
        &root,
        &[entry(&head, Some(FIRST), &[ANDROID])?],
    ))?
    .concat();
    assert!(diagnostic.contains(FIRST), "{diagnostic}");
    Ok(())
}

#[test]
fn a_manifest_commit_must_lie_on_the_branch_and_after_its_since() -> Result<(), Vec<String>> {
    let repo = crate::scratch_dir("commits-reachable")?;
    crate::hermetic_git(&repo, &["init", "--quiet", "--initial-branch", "main"])?;
    let root_commit = commit_on(&repo, "root")?;
    crate::hermetic_git(&repo, &["switch", "--quiet", "--create", "side"])?;
    let side = commit_on(&repo, "side")?;
    crate::hermetic_git(&repo, &["switch", "--quiet", "main"])?;
    let tip = commit_on(&repo, "tip")?;

    assert_eq!(
        commits_reachable(&repo, &[entry(&tip, Some(&root_commit), &[ANDROID])?]),
        Ok(())
    );
    let off_branch = crate::refused(commits_reachable(
        &repo,
        &[entry(&side, Some(&root_commit), &[ANDROID])?],
    ))?
    .concat();
    assert!(
        off_branch.contains(&side) && off_branch.contains(crate::BRANCH_TIP),
        "{off_branch}"
    );
    let later_since = crate::refused(commits_reachable(
        &repo,
        &[entry(&tip, Some(&side), &[ANDROID])?],
    ))?
    .concat();
    assert!(later_since.contains(&side), "{later_since}");
    Ok(())
}

#[test]
fn an_absent_base_checks_every_entry_and_a_present_one_is_kept() -> Result<(), Vec<String>> {
    let root = crate::repo_root()?;
    let no_commit_before = "0".repeat(COMMIT_LENGTH);
    assert_eq!(merged_base(&root, None), Ok(None));
    assert_eq!(merged_base(&root, Some("")), Ok(None));
    assert_eq!(merged_base(&root, Some(&no_commit_before)), Ok(None));
    assert_eq!(merged_base(&root, Some("no-such-ref")), Ok(None));
    assert_eq!(
        merged_base(&root, Some(crate::BRANCH_TIP)),
        Ok(Some(crate::BRANCH_TIP))
    );
    Ok(())
}

#[test]
fn a_checkout_body_failure_survives_the_worktree_removal() -> Result<(), Vec<String>> {
    let repo = crate::scratch_dir("checkout-failure")?;
    crate::hermetic_git(&repo, &["init", "--quiet", "--initial-branch", "main"])?;
    let tip = commit_on(&repo, "tip")?;
    let failure = vec!["the body failed".to_string()];
    let result: Result<(), Vec<String>> = with_checkout(&repo, &tip, |_| Err(failure.clone()));
    assert_eq!(result, Err(failure));
    assert!(!repo.join(CHECKOUTS_DIR).join(&tip).exists());
    Ok(())
}

#[test]
fn the_committed_manifest_parses_and_its_published_entries_are_complete() -> Result<(), Vec<String>>
{
    let root = crate::repo_root()?;
    let published = entries_at(&root, None)?;
    assert!(!published.is_empty());
    assert_eq!(validate(&published), Ok(()));
    Ok(())
}
