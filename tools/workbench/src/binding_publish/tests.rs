use super::*;

#[test]
fn a_missing_file_is_named_in_the_diagnostic() -> Result<(), Vec<String>> {
    let empty = crate::scratch_dir("find-file")?;
    let diagnostic = find_file(&empty, binding_layer::DESCRIPTOR_FILE, |name| {
        name == binding_layer::DESCRIPTOR_FILE
    })
    .err()
    .ok_or_else(|| vec!["an empty directory yielded a file".to_string()])?
    .concat();
    assert!(
        diagnostic.contains(binding_layer::DESCRIPTOR_FILE),
        "{diagnostic}"
    );
    Ok(())
}

#[test]
fn every_platform_has_one_bundle_shape_and_nothing_else_does() {
    assert_eq!(
        media_type_of(binding_manifest::ANDROID),
        Ok(GZIP_MEDIA_TYPE)
    );
    assert_eq!(media_type_of(binding_manifest::IOS), Ok(GZIP_MEDIA_TYPE));
    assert!(media_type_of("linux").is_err());
    assert_eq!(
        SHAPES.map(|(platform, _)| platform),
        binding_manifest::PLATFORMS
    );
}

#[test]
fn the_annotations_carry_the_commit_and_the_coordinate_of_a_maven_bundle() {
    let commit = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let revision = (
        "org.opencontainers.image.revision".to_string(),
        commit.to_string(),
    );
    assert_eq!(
        manifest_annotations(commit, None),
        std::slice::from_ref(&revision)
    );
    assert_eq!(
        manifest_annotations(
            commit,
            Some("org.zingolabs:zingo-binding-layer:6.0.0-aaaaaaaaaaaaaaaa")
        ),
        [
            revision,
            (
                "org.zingolabs.zingolib.coordinate".to_string(),
                "org.zingolabs:zingo-binding-layer:6.0.0-aaaaaaaaaaaaaaaa".to_string()
            ),
        ]
    );
}

fn metadata(versions: &[&str]) -> String {
    let listed: String = versions
        .iter()
        .map(|version| format!("      <version>{version}</version>\n"))
        .collect();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<metadata>\n  <groupId>org.zingolabs</groupId>\n  \
             <artifactId>zingo-binding-layer</artifactId>\n  <versioning>\n    <latest>{}</latest>\n    \
             <versions>\n{listed}    </versions>\n  </versioning>\n</metadata>\n",
        versions.first().unwrap_or(&"")
    )
}

#[test]
fn the_maven_coordinate_is_the_one_group_artifact_and_version_of_the_metadata() {
    assert_eq!(
        maven_coordinate(&metadata(&["6.0.0-aaaaaaaaaaaaaaaa"])),
        Ok("org.zingolabs:zingo-binding-layer:6.0.0-aaaaaaaaaaaaaaaa".to_string())
    );
}

#[test]
fn a_metadata_with_two_versions_or_no_group_yields_no_coordinate() -> Result<(), Vec<String>> {
    let two = crate::refused(maven_coordinate(&metadata(&["6.0.0-aaaa", "6.0.0-bbbb"])))?.concat();
    assert!(two.contains("2 version"), "{two}");
    let none = crate::refused(maven_coordinate("<metadata></metadata>"))?.concat();
    assert!(none.contains("0 groupId"), "{none}");
    Ok(())
}

#[test]
fn the_built_platforms_are_the_bundle_artifacts_of_the_commit_and_nothing_else(
) -> Result<(), Vec<String>> {
    let commit = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let bundles = crate::scratch_dir("built-platforms")?;
    let android = binding_layer::artifact_name(binding_layer::BUNDLE_ARTIFACT, "android", commit);
    let ios = binding_layer::artifact_name(binding_layer::BUNDLE_ARTIFACT, "ios", commit);
    for name in [
        ios.as_str(),
        android.as_str(),
        &binding_layer::artifact_name(binding_layer::ABI_ARTIFACT, "x86_64", commit),
        &binding_layer::artifact_name(binding_layer::BUNDLE_ARTIFACT, "linux", commit),
        &binding_layer::artifact_name(binding_layer::BUNDLE_ARTIFACT, "android", "bbbb"),
        "stray",
    ] {
        crate::fresh_dir(&bundles.join(name))?;
    }
    assert_eq!(
        built_platforms(&bundles, commit),
        [
            (binding_manifest::ANDROID, bundles.join(&android)),
            (binding_manifest::IOS, bundles.join(&ios)),
        ]
    );
    assert_eq!(built_platforms(&bundles.join("absent"), commit), Vec::new());
    Ok(())
}

#[test]
fn a_missing_flag_is_refused_with_the_usage() -> Result<(), Vec<String>> {
    let diagnostic = crate::refused(dispatch(Path::new("/host/zingolib"), &[]))?.concat();
    assert!(diagnostic.contains(COMMIT_FLAG));
    Ok(())
}

const BUILD_WORKFLOWS: [&str; 2] = [
    ".github/workflows/binding-layer-android.yaml",
    ".github/workflows/binding-layer-ios.yaml",
];
const CALLERS: [&str; 3] = [
    ".github/workflows/ci-pr.yaml",
    ".github/workflows/ci-nightly.yaml",
    PUBLISH_WORKFLOW,
];
const WORKBENCH_MANIFEST: &str = "tools/workbench/Cargo.toml";
const PERMISSIONS_KEY: &str = "permissions:";
const INPUTS_KEY: &str = "inputs:";
const WITH_KEY: &str = "with:";
const USES_KEY: &str = "uses: ./";
const REQUIRED_INPUT: &str = "required: true";
const COMMENT_TRIGGER: &str = "issue_comment:";
const REQUIRED_GRANTS: [&str; 3] = ["contents: write", "actions: write", "packages: write"];
const PLAN_COMMAND: &str = "--bin binding-publish -- --plan";
const MANIFEST_WORKFLOW: &str = ".github/workflows/binding-layer-manifest.yaml";
const INSTALL_ACTION: &str = "uses: ./.github/actions/install-orasust";
const INLINE_INSTALL: &str = "cargo install --locked \"orasust@";
const FEATURES_FLAG: &str = "--features";
const REQUIRED_FEATURES: &str = "required-features";
const FEATURES_TABLE: &str = "[features]";
const BRANCH: &str = "main";
const PUBLISHED: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn repo_file(relative: &str) -> Result<String, Vec<String>> {
    crate::read(&crate::repo_root()?.join(relative))
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

fn blocks_under<'a>(text: &'a str, key: &str) -> Vec<Vec<&'a str>> {
    let lines: Vec<&str> = text.lines().collect();
    let mut blocks = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if line.trim() != key {
            continue;
        }
        let indent = indent_of(line);
        let block = lines[index + 1..]
            .iter()
            .take_while(|next| next.trim().is_empty() || indent_of(next) > indent)
            .filter(|next| !next.trim().is_empty())
            .copied()
            .collect();
        blocks.push(block);
    }
    blocks
}

fn required_inputs(workflow: &str) -> Result<Vec<String>, Vec<String>> {
    let mut required = Vec::new();
    for block in blocks_under(workflow, INPUTS_KEY) {
        let shallowest = crate::required(
            block.iter().map(|line| indent_of(line)).min(),
            "line under the inputs key",
        )?;
        let mut name = "";
        for line in block {
            if indent_of(line) == shallowest {
                name = line.trim().trim_end_matches(':');
            } else if line.trim() == REQUIRED_INPUT {
                required.push(name.to_string());
            }
        }
    }
    Ok(required)
}

fn passed_inputs(caller: &str, workflow: &str) -> Vec<Vec<String>> {
    let uses = format!("{USES_KEY}{workflow}");
    let lines: Vec<&str> = caller.lines().collect();
    lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.trim() == uses)
        .map(|(index, _)| {
            let rest = lines[index + 1..].join("\n");
            blocks_under(&rest, WITH_KEY)
                .into_iter()
                .next()
                .unwrap_or_default()
                .into_iter()
                .map(|line| {
                    let trimmed = line.trim();
                    trimmed
                        .split_once(':')
                        .map_or(trimmed, |(name, _)| name)
                        .to_string()
                })
                .collect()
        })
        .collect()
}

const IOS_WORKFLOW: &str = ".github/workflows/binding-layer-ios.yaml";
const STEP_MARK: &str = "- name:";
const CACHE_RESTORE: &str = "uses: actions/cache/restore@";
const CACHE_SAVE: &str = "uses: actions/cache/save@";
const PATHS_KEY: &str = "path: |";
const KEY_KEY: &str = "key:";
const SAVE_GUARD: &str = "if: github.ref == 'refs/heads/dev'";
const RUST_SETUP: &str = "uses: ./.github/actions/rust-setup";
const RUST_CACHE_OFF: &str = "cache: \"false\"";
const MATRIX_TARGET: &str = "matrix.target";
const BUNDLE_INPUT: &str = "${{ inputs.bundle }}";
const MATRIX_TARGET_EXPRESSION: &str = "${{ matrix.target }}";
const ARTIFACT_NAME_KEY: &str = "name: ";
const ARTIFACT_PATTERN_KEY: &str = "pattern: ";
const FAIL_FAST_OFF: &str = "fail-fast: false";
const FETCH_TAGS: &str = "fetch-tags: true";
const CARGO_BIN: &str = ".cargo/bin";

struct CacheStep {
    paths: Vec<String>,
    key: String,
    guarded: bool,
}

fn steps(text: &str) -> Vec<Vec<&str>> {
    let mut steps: Vec<Vec<&str>> = Vec::new();
    for line in text.lines() {
        if line.trim().starts_with(STEP_MARK) {
            steps.push(vec![line]);
        } else if let Some(step) = steps.last_mut() {
            step.push(line);
        }
    }
    steps
}

fn cache_steps(text: &str, uses: &str) -> Result<Vec<CacheStep>, Vec<String>> {
    steps(text)
        .into_iter()
        .filter(|step| step.iter().any(|line| line.trim().starts_with(uses)))
        .map(|step| {
            let joined = step.join("\n");
            let paths = blocks_under(&joined, PATHS_KEY)
                .into_iter()
                .flatten()
                .map(|line| line.trim().to_string())
                .collect();
            let key = crate::required(
                step.iter()
                    .map(|line| line.trim())
                    .find_map(|line| line.strip_prefix(KEY_KEY)),
                "cache key",
            )?
            .trim()
            .to_string();
            let guarded = step.iter().any(|line| line.trim() == SAVE_GUARD);
            Ok(CacheStep {
                paths,
                key,
                guarded,
            })
        })
        .collect()
}

#[test]
fn the_ios_workflow_caches_cargo_explicitly_and_saves_only_from_dev() -> Result<(), Vec<String>> {
    let workflow = repo_file(IOS_WORKFLOW)?;
    let restores = cache_steps(&workflow, CACHE_RESTORE)?;
    let saves = cache_steps(&workflow, CACHE_SAVE)?;
    assert!(restores.len() >= 2, "one restore per job");
    assert_eq!(restores.len(), saves.len());
    for (restore, save) in restores.iter().zip(&saves) {
        assert!(!restore.paths.is_empty());
        assert_eq!(restore.paths, save.paths);
        assert_eq!(restore.key, save.key);
        assert!(!restore.guarded);
        assert!(save.guarded, "{} saves off dev", save.key);
    }
    assert!(saves.iter().any(|save| save.key.contains(MATRIX_TARGET)));
    let setups = workflow
        .lines()
        .filter(|line| line.trim() == RUST_SETUP)
        .count();
    let disabled = workflow
        .lines()
        .filter(|line| line.trim() == RUST_CACHE_OFF)
        .count();
    assert_eq!(
        setups, disabled,
        "every rust-setup use turns rust-cache off"
    );
    Ok(())
}

#[test]
fn the_ios_slice_jobs_follow_the_slice_naming_rule_and_run_to_the_end() -> Result<(), Vec<String>> {
    let workflow = repo_file(IOS_WORKFLOW)?;
    let lines: Vec<&str> = workflow.lines().map(str::trim).collect();
    let upload = format!(
        "{ARTIFACT_NAME_KEY}{}",
        binding_layer::ios_slice_artifact(BUNDLE_INPUT, MATRIX_TARGET_EXPRESSION)
    );
    let download = format!(
        "{ARTIFACT_PATTERN_KEY}{}",
        binding_layer::ios_slice_pattern(BUNDLE_INPUT)
    );
    assert!(lines.contains(&upload.as_str()), "{upload}");
    assert!(lines.contains(&download.as_str()), "{download}");
    assert!(
        lines.contains(&FAIL_FAST_OFF),
        "one failing slice must not cancel the others"
    );
    assert_eq!(
        lines.iter().filter(|line| **line == FETCH_TAGS).count(),
        0,
        "the descriptor names the commit, and nothing reads the tags"
    );
    let slice_save = crate::required(
        cache_steps(&workflow, CACHE_SAVE)?
            .into_iter()
            .find(|save| save.key.contains(MATRIX_TARGET)),
        "the slice save",
    )?;
    assert!(
        slice_save.paths.iter().any(|path| path == CARGO_BIN),
        "the cache holds bindgen-cli, so cargo install skips the build on a hit"
    );
    Ok(())
}

#[test]
fn the_publish_workflow_is_dispatched_and_grants_what_its_steps_need() -> Result<(), Vec<String>> {
    let workflow = repo_file(PUBLISH_WORKFLOW)?;
    let lines: Vec<&str> = workflow.lines().map(str::trim).collect();
    assert!(
        lines.contains(&DISPATCH_TRIGGER),
        "the publish workflow has no `{DISPATCH_TRIGGER}` trigger, and the plan relays to one"
    );
    assert!(
        !lines.contains(&COMMENT_TRIGGER),
        "the comment front door is gone; a publication starts with `gh workflow run`"
    );
    let grants: Vec<&str> = blocks_under(&workflow, PERMISSIONS_KEY)
        .into_iter()
        .flatten()
        .map(str::trim)
        .collect();
    for grant in REQUIRED_GRANTS {
        assert!(
            grants.contains(&grant),
            "the tag push, the relay and the registry push need `{grant}`, and the workflow grants {grants:?}"
        );
    }
    Ok(())
}

#[test]
fn the_registry_client_is_installed_through_one_action() -> Result<(), Vec<String>> {
    for workflow in [PUBLISH_WORKFLOW, MANIFEST_WORKFLOW] {
        let text = repo_file(workflow)?;
        assert!(
            text.lines().any(|line| line.trim() == INSTALL_ACTION),
            "{workflow} installs orasust without the action"
        );
        assert!(
            !text.contains(INLINE_INSTALL),
            "{workflow} installs orasust inline beside the action"
        );
    }
    Ok(())
}

#[test]
fn planning_the_publication_needs_no_registry_feature() -> Result<(), Vec<String>> {
    let manifest = repo_file(WORKBENCH_MANIFEST)?;
    assert!(
        !manifest.contains(REQUIRED_FEATURES) && !manifest.contains(FEATURES_TABLE),
        "a workbench binary is gated on a feature, and the registry client is a separate binary:\n{manifest}"
    );
    let workflow = repo_file(PUBLISH_WORKFLOW)?;
    let planning: Vec<&str> = workflow
        .lines()
        .filter(|line| line.contains(PLAN_COMMAND))
        .map(str::trim)
        .collect();
    assert!(
        !planning.is_empty(),
        "the publish workflow never plans the publication"
    );
    assert!(
        planning.iter().all(|line| !line.contains(FEATURES_FLAG)),
        "planning the publication compiles the registry client: {planning:?}"
    );
    Ok(())
}

#[test]
fn the_plan_relays_to_the_commit_and_refuses_one_off_the_default_branch() -> Result<(), Vec<String>>
{
    let repo = crate::scratch_dir("publish-plan")?;
    crate::hermetic_git(&repo, &["init", "--quiet", "--initial-branch", BRANCH])?;
    append(&repo, PUBLISH_WORKFLOW, "on:")?;
    append(&repo, PUBLISH_WORKFLOW, &format!("  {DISPATCH_TRIGGER}"))?;
    crate::hermetic_git(&repo, &["add", PUBLISH_WORKFLOW])?;
    let first = binding_manifest::commit_on(&repo, "first")?;
    let tip = binding_manifest::commit_on(&repo, "tip")?;
    crate::hermetic_git(&repo, &["switch", "--quiet", "--create", "side"])?;
    let side = binding_manifest::commit_on(&repo, "side")?;
    crate::hermetic_git(&repo, &["switch", "--quiet", BRANCH])?;

    let relayed = planned(&repo, &first, &tip, BRANCH)?;
    assert!(relayed.relay && !relayed.recorded);
    assert_eq!(relayed.commit, first);
    assert_eq!(relayed.tag, format!("{TAG_PREFIX}{first}"));
    let direct = planned(&repo, &tip, &tip, BRANCH)?;
    assert!(!direct.relay && !direct.recorded);
    let off_branch = crate::refused(planned(&repo, &side, &tip, BRANCH))?.concat();
    assert!(
        off_branch.contains(&side) && off_branch.contains(BRANCH),
        "{off_branch}"
    );
    Ok(())
}

const ZERO_DIGEST: &str = "sha256:0000000000000000000000000000000000000000000000000000000000000000";

#[test]
fn a_commit_descending_from_no_entry_is_refused_and_a_recorded_one_is_reported(
) -> Result<(), Vec<String>> {
    let repo = crate::scratch_dir("publish-plan-order")?;
    crate::hermetic_git(&repo, &["init", "--quiet", "--initial-branch", BRANCH])?;
    let first = binding_manifest::commit_on(&repo, "first")?;
    let published = binding_manifest::commit_on(&repo, "published")?;
    let record = format!(
        "[[entry]]\ncommit = \"{published}\"\nsince = \"{first}\"\naudited = [\"zingolib-ffi (ffi/lib)\"]\nandroid = \"{ZERO_DIGEST}\"\nios = \"{ZERO_DIGEST}\"\n"
    );
    crate::create_parent(&repo.join(binding_manifest::FILE))?;
    crate::write(&repo.join(binding_manifest::FILE), record)?;
    crate::hermetic_git(&repo, &["add", binding_manifest::FILE])?;
    let tip = binding_manifest::commit_on(&repo, "tip")?;

    let rootless = crate::refused(planned(&repo, &first, &tip, BRANCH))?.concat();
    assert!(
        rootless.contains("no entry") && rootless.contains(&first),
        "{rootless}"
    );
    assert!(planned(&repo, &published, &tip, BRANCH)?.recorded);
    let next = planned(&repo, &tip, &tip, BRANCH)?;
    assert!(!next.recorded && !next.relay);
    Ok(())
}

#[test]
fn a_release_line_candidate_descending_from_an_earlier_entry_is_accepted() -> Result<(), Vec<String>>
{
    let repo = crate::scratch_dir("publish-plan-release-line")?;
    crate::hermetic_git(&repo, &["init", "--quiet", "--initial-branch", BRANCH])?;
    append(&repo, PUBLISH_WORKFLOW, "on:")?;
    append(&repo, PUBLISH_WORKFLOW, &format!("  {DISPATCH_TRIGGER}"))?;
    crate::hermetic_git(&repo, &["add", PUBLISH_WORKFLOW])?;
    let first = binding_manifest::commit_on(&repo, "first")?;
    let candidate = binding_manifest::commit_on(&repo, "candidate")?;
    crate::hermetic_git(&repo, &["switch", "--quiet", "--create", "release"])?;
    let fix = binding_manifest::commit_on(&repo, "fix")?;
    crate::hermetic_git(&repo, &["switch", "--quiet", BRANCH])?;
    let later = binding_manifest::commit_on(&repo, "later")?;
    let record = format!(
        "[[entry]]\ncommit = \"{candidate}\"\nsince = \"{first}\"\naudited = [\"zingolib-ffi (ffi/lib)\"]\nandroid = \"{ZERO_DIGEST}\"\nios = \"{ZERO_DIGEST}\"\n\
             [[entry]]\ncommit = \"{later}\"\naudited = [\"zingolib-ffi (ffi/lib)\"]\nandroid = \"{ZERO_DIGEST}\"\nios = \"{ZERO_DIGEST}\"\n"
    );
    crate::create_parent(&repo.join(binding_manifest::FILE))?;
    crate::write(&repo.join(binding_manifest::FILE), record)?;
    crate::hermetic_git(&repo, &["add", binding_manifest::FILE])?;
    binding_manifest::commit_on(&repo, "record")?;
    crate::hermetic_git(
        &repo,
        &["merge", "--quiet", "--no-ff", "--no-edit", "release"],
    )?;
    let tip = crate::commit_of(&repo, crate::BRANCH_TIP)?;

    let plan = planned(&repo, &fix, &tip, BRANCH)?;
    assert!(plan.relay && !plan.recorded);
    assert_eq!(plan.commit, fix);
    let rootless = crate::refused(planned(&repo, &first, &tip, BRANCH))?.concat();
    assert!(rootless.contains("no entry"), "{rootless}");
    Ok(())
}

#[test]
fn a_commit_before_publication_by_dispatch_is_refused_when_relayed() -> Result<(), Vec<String>> {
    let repo = crate::scratch_dir("publish-plan-old")?;
    crate::hermetic_git(&repo, &["init", "--quiet", "--initial-branch", BRANCH])?;
    append(&repo, PUBLISH_WORKFLOW, "on:")?;
    append(&repo, PUBLISH_WORKFLOW, &format!("  {COMMENT_TRIGGER}"))?;
    crate::hermetic_git(&repo, &["add", PUBLISH_WORKFLOW])?;
    let old = binding_manifest::commit_on(&repo, "old")?;
    crate::write(
        &repo.join(PUBLISH_WORKFLOW),
        format!("on:\n  {DISPATCH_TRIGGER}\n"),
    )?;
    crate::hermetic_git(&repo, &["add", PUBLISH_WORKFLOW])?;
    let tip = binding_manifest::commit_on(&repo, "tip")?;

    assert_eq!(dispatchable(&repo, &old), Ok(false));
    assert_eq!(dispatchable(&repo, &tip), Ok(true));
    let refused = crate::refused(planned(&repo, &old, &tip, BRANCH))?.concat();
    assert!(refused.contains("predates"), "{refused}");
    assert!(!planned(&repo, &tip, &tip, BRANCH)?.relay);
    Ok(())
}

#[test]
fn every_caller_passes_every_required_input_of_the_build_workflows() -> Result<(), Vec<String>> {
    for workflow in BUILD_WORKFLOWS {
        let required = required_inputs(&repo_file(workflow)?)?;
        assert!(!required.is_empty(), "{workflow} requires no input");
        for caller in CALLERS {
            for passed in passed_inputs(&repo_file(caller)?, workflow) {
                let missing: Vec<&String> = required
                    .iter()
                    .filter(|input| !passed.contains(input))
                    .collect();
                assert!(
                    missing.is_empty(),
                    "{caller} calls {workflow} without {missing:?}"
                );
            }
        }
    }
    Ok(())
}

fn clone(scratch: &Path, remote: &Path, name: &str) -> Result<PathBuf, Vec<String>> {
    crate::hermetic_git(scratch, &["clone", "--quiet", crate::utf8(remote)?, name])?;
    let dir = scratch.join(name);
    crate::configure_hermetic_git(&dir)?;
    Ok(dir)
}

fn append(dir: &Path, relative: &str, line: &str) -> Result<(), Vec<String>> {
    let file = dir.join(relative);
    crate::create_parent(&file)?;
    let mut text = if file.exists() {
        crate::read(&file)?
    } else {
        String::new()
    };
    text.push_str(line);
    text.push('\n');
    crate::write(&file, text)
}

#[test]
fn the_record_lands_on_its_own_branch_of_the_remote() -> Result<(), Vec<String>> {
    let scratch = crate::scratch_dir("record-branch")?;
    let remote = scratch.join("remote.git");
    crate::hermetic_git(
        &scratch,
        &[
            "init",
            "--quiet",
            "--bare",
            "--initial-branch",
            BRANCH,
            "remote.git",
        ],
    )?;
    let runner = clone(&scratch, &remote, "runner")?;
    append(&runner, binding_manifest::FILE, "[[entry]]")?;
    append(
        &runner,
        binding_changelog::FILE,
        "# Binding Layer changelog",
    )?;
    crate::hermetic_git(
        &runner,
        &["add", binding_manifest::FILE, binding_changelog::FILE],
    )?;
    crate::hermetic_git(&runner, &["commit", "--quiet", "--message", "seed"])?;
    crate::hermetic_git(
        &runner,
        &["push", "--quiet", "--set-upstream", "origin", BRANCH],
    )?;

    let branch = record_branch(PUBLISHED);
    assert_eq!(branch_exists(&runner, &branch), Ok(false));
    assert_eq!(pending_records(&runner, &[]), Ok(Vec::new()));
    append(&runner, binding_manifest::FILE, "digest = \"sha256:feed\"")?;
    append(&runner, binding_changelog::FILE, "## section")?;
    assert_eq!(commit_and_push(&runner, PUBLISHED), Ok(branch.clone()));
    assert_eq!(branch_exists(&runner, &branch), Ok(true));
    assert_eq!(
        pending_records(&runner, &[]),
        Ok(vec![branch.clone()]),
        "an unrecorded record branch is pending"
    );
    let recorded = binding_manifest::Entry {
        commit: PUBLISHED.to_string(),
        ..binding_manifest::Entry::default()
    };
    assert_eq!(
        pending_records(&runner, std::slice::from_ref(&recorded)),
        Ok(Vec::new()),
        "a merged record's branch is not pending"
    );

    let recorded = crate::hermetic_git(
        &runner,
        &["log", "--format=%s", "-1", &format!("{ORIGIN}/{branch}")],
    )?;
    assert_eq!(
        recorded.trim(),
        format!("{COMMIT_MESSAGE_PREFIX}{PUBLISHED}")
    );
    let untouched = crate::hermetic_git(
        &runner,
        &["log", "--format=%s", "-1", &format!("{ORIGIN}/{BRANCH}")],
    )?;
    assert_eq!(untouched.trim(), "seed");
    Ok(())
}
