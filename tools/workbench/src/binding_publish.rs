use std::path::{Path, PathBuf};

use crate::binding_changelog;
use crate::binding_layer;
use crate::binding_manifest;

pub const BINARY: &str = "binding-publish";
pub const USERNAME_VARIABLE: &str = "REGISTRY_USERNAME";
pub const TOKEN_VARIABLE: &str = "REGISTRY_TOKEN";
pub const EMPTY_CONFIG_MEDIA_TYPE: &str = "application/vnd.oci.empty.v1+json";
pub const EMPTY_CONFIG: &str = "{}";
const ZIP_MEDIA_TYPE: &str = "application/zip";
const GZIP_MEDIA_TYPE: &str = "application/gzip";
const ARCHIVE_SUFFIX: &str = ".tar.gz";
const SHAPES: [(&str, &str); 2] = [
    (binding_manifest::ANDROID, ZIP_MEDIA_TYPE),
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

pub fn layer_annotations(file_name: &str) -> Vec<(String, String)> {
    vec![(
        binding_manifest::TITLE_ANNOTATION.to_string(),
        file_name.to_string(),
    )]
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

fn file_name_of(file: &Path) -> Result<&str, Vec<String>> {
    file.file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| vec![format!("{} has no file name", file.display())])
}

fn find_file(dir: &Path, wanted: impl Fn(&str) -> bool) -> Result<PathBuf, Vec<String>> {
    let mut pending = vec![dir.to_path_buf()];
    while let Some(current) = pending.pop() {
        let entries = std::fs::read_dir(&current)
            .map_err(|e| vec![format!("cannot read {}: {e}", current.display())])?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(&wanted)
            {
                return Ok(path);
            }
        }
    }
    Err(vec![format!(
        "no file the bundle needs lies under {}",
        dir.display()
    )])
}

#[cfg(feature = "registry")]
fn push(
    platform: &str,
    commit: &str,
    bundle: &Bundle,
    credentials: &Credentials,
) -> Result<String, Vec<String>> {
    use oci_client::client;
    use oci_client::manifest;
    use oci_client::secrets;
    let file_name = file_name_of(&bundle.file)?;
    let data = crate::read_bytes(&bundle.file)?;
    let name = binding_manifest::reference(platform, commit);
    let image =
        oci_client::Reference::try_from(name.as_str()).map_err(|e| vec![format!("{name}: {e}")])?;
    let layers = [client::ImageLayer::new(
        data,
        bundle.media_type.to_string(),
        Some(layer_annotations(file_name).into_iter().collect()),
    )];
    let config = client::Config::new(EMPTY_CONFIG, EMPTY_CONFIG_MEDIA_TYPE.to_string(), None);
    let image_manifest = manifest::OciImageManifest::build(
        &layers,
        &config,
        Some(
            manifest_annotations(commit, &bundle.descriptor)
                .into_iter()
                .collect(),
        ),
    );
    let auth =
        secrets::RegistryAuth::Basic(credentials.username.clone(), credentials.token.clone());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| vec![format!("cannot start the runtime: {e}")])?;
    let registry = client::Client::new(client::ClientConfig::default());
    runtime.block_on(async {
        registry
            .push(&image, &layers, config, &auth, Some(image_manifest))
            .await
            .map_err(|e| vec![format!("cannot push {name}: {e}")])?;
        registry
            .fetch_manifest_digest(&image, &auth)
            .await
            .map_err(|e| vec![format!("cannot read back the digest of {name}: {e}")])
    })
}

#[cfg(not(feature = "registry"))]
fn push(
    platform: &str,
    commit: &str,
    bundle: &Bundle,
    _credentials: &Credentials,
) -> Result<String, Vec<String>> {
    Err(vec![format!(
        "{BINARY} was built without the registry feature and cannot push {} ({}, {}) as {}",
        bundle.file.display(),
        bundle.descriptor,
        bundle.media_type,
        binding_manifest::reference(platform, commit)
    )])
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
    let descriptor = crate::read(&find_file(dir, |name| {
        name == binding_layer::DESCRIPTOR_FILE
    })?)?
    .trim()
    .to_string();
    let media_type = media_type_of(platform)?;
    let file = match platform {
        binding_manifest::ANDROID => {
            find_file(dir, |name| name.ends_with(binding_layer::AAR_SUFFIX))?
        }
        _ => swift_package_archive(root, dir, commit)?,
    };
    Ok(Bundle {
        file,
        descriptor,
        media_type,
    })
}

fn swift_package_archive(root: &Path, dir: &Path, commit: &str) -> Result<PathBuf, Vec<String>> {
    let staging = dir.with_file_name(format!("{}{PACKAGE_STAGING_SUFFIX}", file_name_of(dir)?));
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
    let manifest_file = package_dir.join(file_name_of(manifest_path)?);
    std::fs::write(&manifest_file, manifest_text)
        .map_err(|e| vec![format!("cannot write {}: {e}", manifest_file.display())])?;
    let archive_name = format!("{}{ARCHIVE_SUFFIX}", binding_layer::SWIFT_PACKAGE);
    let args = [
        TAR_ARGS.as_slice(),
        &[archive_name.as_str(), binding_layer::SWIFT_PACKAGE],
    ]
    .concat();
    crate::stdout_in(&staging, TAR, &args, &[])?;
    Ok(staging.join(archive_name))
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
/// - Creates and removes a detached worktree of the commit and runs `cargo tree` in it.
/// - Reads each built platform's bundle under `--bundles`, archives the iOS package through `tar`,
///   and pushes each bundle to the registry.
/// - Writes `bindings/published.toml` and `bindings/CHANGELOG.md`, then commits and pushes them.
pub fn dispatch(root: &Path, args: &[String]) -> Result<(), Vec<String>> {
    let required = |flag: &str| crate::required_flag(args, flag, USAGE);
    let commit = crate::commit_of(root, required(COMMIT_FLAG)?)?;
    let bundles = Path::new(required(BUNDLES_FLAG)?);
    let newest = binding_manifest::newest_awaiting(root)?;
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
    let file = root.join(binding_manifest::FILE);
    let crates = binding_manifest::with_checkout(root, &commit, binding_manifest::audited_at)?;
    let mut text = binding_manifest::recorded_audited(&crate::read(&file)?, &commit, &crates)?;
    for (platform, dir) in built {
        let bundle = bundle_of(root, platform, &dir, &commit)?;
        let digest = push(platform, &commit, &bundle, &credentials)?;
        text = binding_manifest::recorded(&text, &commit, platform, &digest)?;
    }
    std::fs::write(&file, text)
        .map_err(|e| vec![format!("cannot write {}: {e}", file.display())])?;
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
    fn every_platform_has_one_bundle_shape_and_nothing_else_does() {
        assert_eq!(media_type_of(binding_manifest::ANDROID), Ok(ZIP_MEDIA_TYPE));
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
        assert_eq!(
            layer_annotations("x.aar"),
            [(
                "org.opencontainers.image.title".to_string(),
                "x.aar".to_string()
            )]
        );
    }

    #[test]
    fn the_built_platforms_are_the_bundle_artifacts_of_the_commit_and_nothing_else() {
        let commit = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let bundles =
            std::env::temp_dir().join(format!("workbench-built-platforms-{}", std::process::id()));
        crate::fresh_dir(&bundles).unwrap();
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
            std::fs::create_dir(bundles.join(name)).unwrap();
        }
        assert_eq!(
            built_platforms(&bundles, commit),
            [
                (binding_manifest::ANDROID, bundles.join(&android)),
                (binding_manifest::IOS, bundles.join(&ios)),
            ]
        );
        assert_eq!(built_platforms(&bundles.join("absent"), commit), Vec::new());
        std::fs::remove_dir_all(&bundles).unwrap();
    }

    #[test]
    fn a_missing_flag_is_refused_with_the_usage() {
        let diagnostic = dispatch(Path::new("/host/zingolib"), &[])
            .unwrap_err()
            .concat();
        assert!(diagnostic.contains(COMMIT_FLAG));
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
    const MANIFEST_BIN: &str = "name = \"binding-manifest\"";
    const REQUIRED_FEATURES: &str = "required-features";
    const BRANCH: &str = "main";
    const PUBLISHED: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn repo_file(relative: &str) -> String {
        crate::read(&crate::repo_root().unwrap().join(relative)).unwrap()
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

    fn required_inputs(workflow: &str) -> Vec<String> {
        let mut required = Vec::new();
        for block in blocks_under(workflow, INPUTS_KEY) {
            let shallowest = block.iter().map(|line| indent_of(line)).min().unwrap();
            let mut name = "";
            for line in block {
                if indent_of(line) == shallowest {
                    name = line.trim().trim_end_matches(':');
                } else if line.trim() == REQUIRED_INPUT {
                    required.push(name.to_string());
                }
            }
        }
        required
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
                    .map(|line| line.trim().split(':').next().unwrap().to_string())
                    .collect()
            })
            .collect()
    }

    #[test]
    fn the_publish_workflow_holds_the_permission_its_reactions_need() {
        let workflow = repo_file(PUBLISH_WORKFLOW);
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
    }

    #[test]
    fn reading_the_newest_entry_needs_no_registry() {
        let manifest = repo_file(WORKBENCH_MANIFEST);
        let bin = manifest.find(MANIFEST_BIN).unwrap();
        let stanza = manifest[bin..].split("\n\n").next().unwrap();
        assert!(
            !stanza.contains(REQUIRED_FEATURES),
            "the manifest binary is gated on a feature that only --check needs:\n{stanza}"
        );
        let workflow = repo_file(PUBLISH_WORKFLOW);
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
    }

    #[test]
    fn every_caller_passes_every_required_input_of_the_build_workflows() {
        for workflow in BUILD_WORKFLOWS {
            let required = required_inputs(&repo_file(workflow));
            assert!(!required.is_empty(), "{workflow} requires no input");
            for caller in CALLERS {
                for passed in passed_inputs(&repo_file(caller), workflow) {
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
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        crate::git_in(dir, args).unwrap()
    }

    fn clone(scratch: &Path, remote: &Path, name: &str) -> PathBuf {
        git(
            scratch,
            &["clone", "--quiet", remote.to_str().unwrap(), name],
        );
        let dir = scratch.join(name);
        git(&dir, &["config", "user.name", name]);
        git(
            &dir,
            &["config", "user.email", &format!("{name}@example.invalid")],
        );
        dir
    }

    fn append(dir: &Path, relative: &str, line: &str) {
        let file = dir.join(relative);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        let mut text = std::fs::read_to_string(&file).unwrap_or_default();
        text.push_str(line);
        text.push('\n');
        std::fs::write(file, text).unwrap();
    }

    #[test]
    fn the_record_push_survives_a_push_to_the_branch_during_the_builds() {
        let scratch =
            std::env::temp_dir().join(format!("workbench-record-push-{}", std::process::id()));
        crate::fresh_dir(&scratch).unwrap();
        let remote = scratch.join("remote.git");
        git(
            &scratch,
            &[
                "init",
                "--quiet",
                "--bare",
                "--initial-branch",
                BRANCH,
                "remote.git",
            ],
        );
        let runner = clone(&scratch, &remote, "runner");
        append(&runner, binding_manifest::FILE, "[[entry]]");
        append(
            &runner,
            binding_changelog::FILE,
            "# Binding Layer changelog",
        );
        git(
            &runner,
            &["add", binding_manifest::FILE, binding_changelog::FILE],
        );
        git(&runner, &["commit", "--quiet", "--message", "seed"]);
        git(
            &runner,
            &["push", "--quiet", "--set-upstream", "origin", BRANCH],
        );

        let rival = clone(&scratch, &remote, "rival");
        append(&rival, "README.md", "a fix pushed while the builds run");
        git(&rival, &["add", "README.md"]);
        git(&rival, &["commit", "--quiet", "--message", "concurrent"]);
        git(&rival, &["push", "--quiet"]);

        append(&runner, binding_manifest::FILE, "digest = \"sha256:feed\"");
        append(&runner, binding_changelog::FILE, "## section");
        assert_eq!(commit_and_push(&runner, PUBLISHED), Ok(()));

        git(&rival, &["pull", "--quiet", "--rebase"]);
        let subjects = git(&rival, &["log", "--format=%s", "-3"]);
        assert_eq!(
            subjects.lines().collect::<Vec<_>>(),
            [
                format!("{COMMIT_MESSAGE_PREFIX}{PUBLISHED}").as_str(),
                "concurrent",
                "seed",
            ]
        );
        std::fs::remove_dir_all(&scratch).unwrap();
    }
}
