use std::path::{Path, PathBuf};

use crate::binding_changelog;
use crate::binding_layer;
use crate::binding_manifest;

pub const BINARY: &str = "binding-publish";
pub const USERNAME_VARIABLE: &str = "REGISTRY_USERNAME";
pub const TOKEN_VARIABLE: &str = "REGISTRY_TOKEN";
pub const GZIP_MEDIA_TYPE: &str = "application/gzip";
const SHAPES: [(&str, &str); 2] = [
    (binding_manifest::ANDROID, GZIP_MEDIA_TYPE),
    (binding_manifest::IOS, GZIP_MEDIA_TYPE),
];
const TAR: &str = "tar";
const TAR_ARGS: [&str; 3] = ["--create", "--gzip", "--file"];
const PACKAGE_STAGING_SUFFIX: &str = "-package";
const PUSH_ATTEMPTS: usize = 2;
const COMMIT_MESSAGE_PREFIX: &str = "chore(bindings): record the publication of ";
const COMMIT_FLAG: &str = "--commit";
const BUNDLES_FLAG: &str = "--bundles";
const USAGE: &str = "usage: binding-publish --commit <commit> \
    --bundles <directory holding each downloaded bundle artifact of the commit>";

pub struct Credentials {
    pub username: String,
    pub token: String,
}

struct Bundle {
    file: PathBuf,
    descriptor: String,
    media_type: &'static str,
}

pub fn media_type_of(platform: &str) -> Result<&'static str, Vec<String>> {
    SHAPES
        .into_iter()
        .find(|(known, _)| *known == platform)
        .map(|(_, media_type)| media_type)
        .ok_or_else(|| vec![format!("no bundle shape is known for {platform}")])
}

pub fn manifest_annotations(commit: &str, descriptor: &str) -> Vec<(String, String)> {
    vec![
        (
            binding_manifest::REVISION_ANNOTATION.to_string(),
            commit.to_string(),
        ),
        (
            binding_manifest::DESCRIPTOR_ANNOTATION.to_string(),
            descriptor.to_string(),
        ),
    ]
}

pub fn built_platforms(bundles: &Path, commit: &str) -> Vec<(&'static str, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(bundles) else {
        return Vec::new();
    };
    let mut built: Vec<(&'static str, PathBuf)> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .filter_map(|path| {
            let name = path.file_name()?.to_str()?;
            let segment =
                binding_layer::artifact_segment(name, binding_layer::BUNDLE_ARTIFACT, commit)?;
            let platform = binding_manifest::platform(segment).ok()?;
            Some((platform, path.clone()))
        })
        .collect();
    built.sort_by_key(|(platform, _)| *platform);
    built
}

fn find_file(
    dir: &Path,
    what: &str,
    wanted: impl Fn(&str) -> bool,
) -> Result<PathBuf, Vec<String>> {
    crate::files_under(dir, &[])?
        .into_iter()
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(&wanted)
        })
        .ok_or_else(|| vec![format!("no {what} lies under {}", dir.display())])
}

/// - Runs `orasust push` and then an anonymous `orasust resolve` as child processes.
fn push(
    platform: &str,
    commit: &str,
    bundle: &Bundle,
    credentials: &Credentials,
) -> Result<String, Vec<String>> {
    let name = binding_manifest::reference(platform, commit);
    let digest = crate::orasust::push(
        &name,
        &bundle.file,
        bundle.media_type,
        &manifest_annotations(commit, &bundle.descriptor),
        &credentials.username,
        &credentials.token,
    )?;
    let seen = crate::orasust::resolve(&name)
        .map_err(|_| vec![binding_manifest::private_package_hint(&name)])?;
    if seen != digest {
        return Err(vec![format!(
            "{name} was pushed as {digest} and resolves anonymously to {seen}"
        )]);
    }
    Ok(digest)
}

fn credentials() -> Result<Credentials, Vec<String>> {
    let read = |variable: &str| {
        std::env::var(variable).map_err(|_| vec![format!("{variable} is not set")])
    };
    Ok(Credentials {
        username: read(USERNAME_VARIABLE)?,
        token: read(TOKEN_VARIABLE)?,
    })
}

/// - Reads the descriptor file under `dir`.
/// - For iOS, moves `dir` under a sibling staging directory, writes the commit's `Package.swift`
///   beside it from `git show`, and runs `tar` as a child process to archive the package.
fn bundle_of(root: &Path, platform: &str, dir: &Path, commit: &str) -> Result<Bundle, Vec<String>> {
    let descriptor = crate::read(&find_file(dir, binding_layer::DESCRIPTOR_FILE, |name| {
        name == binding_layer::DESCRIPTOR_FILE
    })?)?
    .trim()
    .to_string();
    let media_type = media_type_of(platform)?;
    let file = match platform {
        binding_manifest::ANDROID => maven_repository_archive(dir)?,
        _ => swift_package_archive(root, dir, commit)?,
    };
    Ok(Bundle {
        file,
        descriptor,
        media_type,
    })
}

/// - Runs `tar` as a child process in `parent`, writing `<name>.tar.gz` beside `<name>`.
fn archive(parent: &Path, name: &str) -> Result<PathBuf, Vec<String>> {
    let archive_name = format!("{name}{}", binding_layer::ARCHIVE_SUFFIX);
    let args = [TAR_ARGS.as_slice(), &[archive_name.as_str(), name]].concat();
    crate::stdout_in(parent, TAR, &args, &[])?;
    Ok(parent.join(archive_name))
}

/// - Runs `tar` as a child process to archive the Maven repository directory under `dir`.
fn maven_repository_archive(dir: &Path) -> Result<PathBuf, Vec<String>> {
    let metadata = find_file(dir, binding_layer::MAVEN_METADATA_FILE, |name| {
        name == binding_layer::MAVEN_METADATA_FILE
    })?;
    let repository = crate::required(
        metadata.ancestors().find(|ancestor| {
            ancestor
                .file_name()
                .is_some_and(|name| name == binding_layer::MAVEN_REPOSITORY_DIR)
        }),
        &format!(
            "directory named {} above {}",
            binding_layer::MAVEN_REPOSITORY_DIR,
            metadata.display()
        ),
    )?;
    let parent = crate::required(repository.parent(), "parent of the Maven repository")?;
    archive(parent, binding_layer::MAVEN_REPOSITORY_DIR)
}

fn swift_package_archive(root: &Path, dir: &Path, commit: &str) -> Result<PathBuf, Vec<String>> {
    let staging = dir.with_file_name(format!(
        "{}{PACKAGE_STAGING_SUFFIX}",
        crate::file_name_of(dir)?
    ));
    let package_dir = crate::fresh_dir(&staging.join(binding_layer::SWIFT_PACKAGE))?;
    let output_dir = package_dir.join(binding_layer::SWIFT_PACKAGE_OUTPUT_DIR);
    std::fs::rename(dir, &output_dir).map_err(|e| {
        vec![format!(
            "cannot move {} to {}: {e}",
            dir.display(),
            output_dir.display()
        )]
    })?;
    let manifest_path = Path::new(binding_layer::SWIFT_PACKAGE_MANIFEST);
    let manifest_text = crate::git_in(
        root,
        &["show", &format!("{commit}:{}", manifest_path.display())],
    )?;
    let manifest_file = package_dir.join(crate::file_name_of(manifest_path)?);
    std::fs::write(&manifest_file, manifest_text)
        .map_err(|e| vec![format!("cannot write {}: {e}", manifest_file.display())])?;
    archive(&staging, binding_layer::SWIFT_PACKAGE)
}

/// - Runs `git add` and `git commit` in `root`, then `git pull --rebase` and `git push`, the
///   pair at most `PUSH_ATTEMPTS` times.
fn commit_and_push(root: &Path, commit: &str) -> Result<(), Vec<String>> {
    let message = format!("{COMMIT_MESSAGE_PREFIX}{commit}");
    crate::git_in(
        root,
        &["add", binding_manifest::FILE, binding_changelog::FILE],
    )?;
    crate::git_in(root, &["commit", "--message", &message])?;
    let mut rejected = Vec::new();
    for _ in 0..PUSH_ATTEMPTS {
        crate::git_in(root, &["pull", "--rebase"])?;
        match crate::git_in(root, &["push"]) {
            Ok(_) => return Ok(()),
            Err(diagnostics) => rejected = diagnostics,
        }
    }
    Err(rejected)
}

/// - Reads `bindings/published.toml` and the credentials in the environment.
/// - Creates and removes a detached worktree of the commit and runs `cargo metadata` in it.
/// - Reads every built platform's bundle under `--bundles` and archives the iOS package through
///   `tar`, then pushes each bundle to the registry.
/// - Writes `bindings/published.toml` and `bindings/CHANGELOG.md`, then commits and pushes them.
pub fn dispatch(root: &Path, args: &[String]) -> Result<(), Vec<String>> {
    let required = |flag: &str| crate::required_flag(args, flag, USAGE);
    let commit = crate::commit_of(root, required(COMMIT_FLAG)?)?;
    let bundles = Path::new(required(BUNDLES_FLAG)?);
    let newest = binding_manifest::newest_awaiting(root, None)?;
    if newest != commit {
        return Err(vec![format!(
            "{commit} is not the newest entry awaiting `{}` in {}, which is {newest}",
            binding_manifest::PUBLISH_COMMAND,
            binding_manifest::FILE
        )]);
    }
    let built = built_platforms(bundles, &commit);
    let names: Vec<&str> = built.iter().map(|(platform, _)| *platform).collect();
    let missing = binding_manifest::missing_required(&names);
    if !missing.is_empty() {
        return Err(vec![format!(
            "{} requires {} and {} holds no bundle for it, so that build did not succeed",
            binding_manifest::FILE,
            missing.join(", "),
            bundles.display()
        )]);
    }
    let credentials = credentials()?;
    crate::orasust::pinned()?;
    let file = root.join(binding_manifest::FILE);
    let crates = binding_manifest::with_checkout(root, &commit, binding_manifest::audited_at)?;
    let mut text = binding_manifest::recorded_audited(&crate::read(&file)?, &commit, &crates)?;
    let bundles: Vec<(&str, Bundle)> = built
        .into_iter()
        .map(|(platform, dir)| Ok((platform, bundle_of(root, platform, &dir, &commit)?)))
        .collect::<Result<_, Vec<String>>>()?;
    for (platform, bundle) in &bundles {
        let digest = push(platform, &commit, bundle, &credentials)?;
        text = binding_manifest::recorded(&text, &commit, platform, &digest)?;
    }
    crate::write(&file, text)?;
    binding_changelog::regenerate(root)?;
    commit_and_push(root, &commit)
}

/// - Reads the process arguments.
/// - Exits the process through [`crate::run`].
pub fn main() -> ! {
    crate::dispatch_from_root(BINARY, dispatch)
}

#[cfg(test)]
mod tests {
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
    fn the_annotations_carry_the_commit_the_descriptor_and_the_file_name() {
        let commit = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        assert_eq!(
            manifest_annotations(commit, "zl_6.0.0_2691e"),
            [
                (
                    "org.opencontainers.image.revision".to_string(),
                    commit.to_string()
                ),
                (
                    "org.zingolabs.zingolib.descriptor".to_string(),
                    "zl_6.0.0_2691e".to_string()
                ),
            ]
        );
    }

    #[test]
    fn the_built_platforms_are_the_bundle_artifacts_of_the_commit_and_nothing_else(
    ) -> Result<(), Vec<String>> {
        let commit = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let bundles = crate::scratch_dir("built-platforms")?;
        let android =
            binding_layer::artifact_name(binding_layer::BUNDLE_ARTIFACT, "android", commit);
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

    const PUBLISH_WORKFLOW: &str = ".github/workflows/binding-layer-publish.yaml";
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
    const REACTIONS_PATH: &str = "/reactions";
    const ISSUES_WRITE: &str = "issues: write";
    const NEWEST_COMMAND: &str = "--bin binding-manifest -- --newest";
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
    fn the_ios_workflow_caches_cargo_explicitly_and_saves_only_from_dev() -> Result<(), Vec<String>>
    {
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
    fn the_ios_slice_jobs_follow_the_slice_naming_rule_and_run_to_the_end(
    ) -> Result<(), Vec<String>> {
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
            1,
            "only the slice job writes the descriptor, which reads the tags"
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
    fn the_publish_workflow_holds_the_permission_its_reactions_need() -> Result<(), Vec<String>> {
        let workflow = repo_file(PUBLISH_WORKFLOW)?;
        let reacting = workflow
            .lines()
            .filter(|line| line.contains(REACTIONS_PATH))
            .count();
        assert!(reacting > 0, "the workflow posts no reaction");
        let grants: Vec<&str> = blocks_under(&workflow, PERMISSIONS_KEY)
            .into_iter()
            .flatten()
            .map(str::trim)
            .collect();
        assert!(
            grants.contains(&ISSUES_WRITE),
            "{reacting} step(s) post a reaction, which GitHub gates on `{ISSUES_WRITE}`, but the workflow grants only {grants:?}"
        );
        Ok(())
    }

    #[test]
    fn reading_the_newest_entry_needs_no_registry() -> Result<(), Vec<String>> {
        let manifest = repo_file(WORKBENCH_MANIFEST)?;
        assert!(
            !manifest.contains(REQUIRED_FEATURES) && !manifest.contains(FEATURES_TABLE),
            "a workbench binary is gated on a feature, and the registry client is a separate binary:\n{manifest}"
        );
        let workflow = repo_file(PUBLISH_WORKFLOW)?;
        let newest: Vec<&str> = workflow
            .lines()
            .filter(|line| line.contains(NEWEST_COMMAND))
            .map(str::trim)
            .collect();
        assert!(
            !newest.is_empty(),
            "the publish workflow never reads the newest entry"
        );
        assert!(
            newest.iter().all(|line| !line.contains(FEATURES_FLAG)),
            "reading the newest entry compiles the registry client: {newest:?}"
        );
        Ok(())
    }

    #[test]
    fn every_caller_passes_every_required_input_of_the_build_workflows() -> Result<(), Vec<String>>
    {
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
    fn the_record_push_survives_a_push_to_the_branch_during_the_builds() -> Result<(), Vec<String>>
    {
        let scratch = crate::scratch_dir("record-push")?;
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

        let rival = clone(&scratch, &remote, "rival")?;
        append(&rival, "README.md", "a fix pushed while the builds run")?;
        crate::hermetic_git(&rival, &["add", "README.md"])?;
        crate::hermetic_git(&rival, &["commit", "--quiet", "--message", "concurrent"])?;
        crate::hermetic_git(&rival, &["push", "--quiet"])?;

        append(&runner, binding_manifest::FILE, "digest = \"sha256:feed\"")?;
        append(&runner, binding_changelog::FILE, "## section")?;
        assert_eq!(commit_and_push(&runner, PUBLISHED), Ok(()));

        crate::hermetic_git(&rival, &["pull", "--quiet", "--rebase"])?;
        let subjects = crate::hermetic_git(&rival, &["log", "--format=%s", "-3"])?;
        assert_eq!(
            subjects.lines().collect::<Vec<_>>(),
            [
                format!("{COMMIT_MESSAGE_PREFIX}{PUBLISHED}").as_str(),
                "concurrent",
                "seed",
            ]
        );
        Ok(())
    }
}
