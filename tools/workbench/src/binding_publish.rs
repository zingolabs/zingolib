use std::path::{Path, PathBuf};

use crate::binding_changelog;
use crate::binding_layer;
use crate::binding_manifest;
use crate::orasust;

pub const BINARY: &str = "binding-publish";
pub const USERNAME_VARIABLE: &str = "REGISTRY_USERNAME";
pub const TOKEN_VARIABLE: &str = "REGISTRY_TOKEN";
pub const GZIP_MEDIA_TYPE: &str = "application/gzip";
const SHAPES: [(&str, &str); 2] = [
    (binding_manifest::ANDROID, GZIP_MEDIA_TYPE),
    (binding_manifest::IOS, GZIP_MEDIA_TYPE),
];
const TAR: &str = "tar";
const TAR_ARGS: [&str; 4] = ["--create", "--gzip", "--format=pax", "--file"];
const PACKAGE_STAGING_SUFFIX: &str = "-package";
const COMMIT_MESSAGE_PREFIX: &str = "chore(bindings): record the publication of ";
const COMMIT_FLAG: &str = "--commit";
const BUNDLES_FLAG: &str = "--bundles";
const PLAN_FLAG: &str = "--plan";
const HELD_FLAG: &str = "--held";
const PUSH_FLAG: &str = "--push";
const RECORD_FLAG: &str = "--record";
const RUN_AT_FLAG: &str = "--run-at";
const DEFAULT_BRANCH_FLAG: &str = "--default-branch";
const USAGE: &str = "usage: binding-publish --plan --commit <commit> --run-at <commit> --default-branch <ref> \
    | binding-publish --held --commit <commit> \
    | binding-publish --push --commit <commit> --bundles <directory holding each downloaded bundle artifact of the commit> \
    | binding-publish --record --commit <commit>";
const HEADS_NAMESPACE: &str = "refs/heads/";
pub const PUBLISH_WORKFLOW: &str = ".github/workflows/binding-layer-publish.yaml";
const DISPATCH_TRIGGER: &str = "workflow_dispatch:";
pub const TAG_PREFIX: &str = "publish/";
pub const RECORD_BRANCH_PREFIX: &str = "record/";
const ORIGIN: &str = "origin";
const OUTPUT_SEPARATOR: char = '=';
const COMMIT_OUTPUT: &str = "commit";
const TAG_OUTPUT: &str = "tag";
const RELAY_OUTPUT: &str = "relay";
const TAGGED_OUTPUT: &str = "tagged";
const HELD_OUTPUT: &str = "held";
const TAGS_NAMESPACE: &str = "refs/tags/";
const PEELED_SUFFIX: &str = "^{}";
const BRANCH_OUTPUT: &str = "branch";
const RECORDED_OUTPUT: &str = "recorded";
const COORDINATE_SEPARATOR: &str = ":";
const GROUP_ELEMENT: &str = "groupId";
const ARTIFACT_ELEMENT: &str = "artifactId";
const VERSION_ELEMENT: &str = "version";

struct Bundle {
    file: PathBuf,
    media_type: &'static str,
    coordinate: Option<String>,
}

pub fn media_type_of(platform: &str) -> Result<&'static str, Vec<String>> {
    SHAPES
        .into_iter()
        .find(|(known, _)| *known == platform)
        .map(|(_, media_type)| media_type)
        .ok_or_else(|| vec![format!("no bundle shape is known for {platform}")])
}

pub fn manifest_annotations(commit: &str, coordinate: Option<&str>) -> Vec<(String, String)> {
    let mut annotations = vec![(
        binding_manifest::REVISION_ANNOTATION.to_string(),
        commit.to_string(),
    )];
    annotations.extend(coordinate.map(|coordinate| {
        (
            binding_manifest::COORDINATE_ANNOTATION.to_string(),
            coordinate.to_string(),
        )
    }));
    annotations
}

fn elements<'a>(xml: &'a str, name: &str) -> Vec<&'a str> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    xml.match_indices(&open)
        .filter_map(|(start, _)| {
            let rest = &xml[start + open.len()..];
            rest.find(&close).map(|end| rest[..end].trim())
        })
        .collect()
}

pub fn maven_coordinate(metadata: &str) -> Result<String, Vec<String>> {
    let one = |name: &str| match elements(metadata, name).as_slice() {
        [value] if !value.is_empty() => Ok(value.to_string()),
        found => Err(vec![format!(
            "{} holds {} {name} elements, and the coordinate needs one",
            binding_layer::MAVEN_METADATA_FILE,
            found.len()
        )]),
    };
    Ok([
        one(GROUP_ELEMENT)?,
        one(ARTIFACT_ELEMENT)?,
        one(VERSION_ELEMENT)?,
    ]
    .join(COORDINATE_SEPARATOR))
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
    credentials: &orasust::Credentials,
) -> Result<String, Vec<String>> {
    let name = binding_manifest::reference(platform, commit);
    let digest = crate::orasust::push(
        &name,
        &bundle.file,
        bundle.media_type,
        &manifest_annotations(commit, bundle.coordinate.as_deref()),
        credentials,
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

/// - Reads `REGISTRY_USERNAME` and `REGISTRY_TOKEN` from the environment.
fn credentials_if_set() -> Option<orasust::Credentials> {
    Some(orasust::Credentials {
        username: std::env::var(USERNAME_VARIABLE).ok()?,
        token: std::env::var(TOKEN_VARIABLE).ok()?,
    })
}

/// - Reads the environment through [`credentials_if_set`].
fn credentials() -> Result<orasust::Credentials, Vec<String>> {
    credentials_if_set().ok_or_else(|| {
        vec![format!(
            "{USERNAME_VARIABLE} and {TOKEN_VARIABLE} are not both set"
        )]
    })
}

/// - For iOS, moves `dir` under a sibling staging directory, writes the commit's `Package.swift`
///   beside it from `git show`, and runs `tar` as a child process to archive the package.
fn bundle_of(root: &Path, platform: &str, dir: &Path, commit: &str) -> Result<Bundle, Vec<String>> {
    let media_type = media_type_of(platform)?;
    let (file, coordinate) = match platform {
        binding_manifest::ANDROID => {
            let metadata = maven_metadata(dir)?;
            let coordinate = maven_coordinate(&crate::read(&metadata)?)?;
            (maven_repository_archive(&metadata)?, Some(coordinate))
        }
        _ => (swift_package_archive(root, dir, commit)?, None),
    };
    Ok(Bundle {
        file,
        media_type,
        coordinate,
    })
}

/// - Runs `tar` as a child process in `parent`, writing `<name>.tar.gz` beside `<name>`.
fn archive(parent: &Path, name: &str) -> Result<PathBuf, Vec<String>> {
    let archive_name = format!("{name}{}", binding_layer::ARCHIVE_SUFFIX);
    let args = [TAR_ARGS.as_slice(), &[archive_name.as_str(), name]].concat();
    crate::stdout_in(parent, TAR, &args, &[])?;
    Ok(parent.join(archive_name))
}

fn maven_metadata(dir: &Path) -> Result<PathBuf, Vec<String>> {
    find_file(dir, binding_layer::MAVEN_METADATA_FILE, |name| {
        name == binding_layer::MAVEN_METADATA_FILE
    })
}

/// - Runs `tar` as a child process to archive the Maven repository directory above `metadata`.
fn maven_repository_archive(metadata: &Path) -> Result<PathBuf, Vec<String>> {
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

pub fn record_branch(commit: &str) -> String {
    format!("{RECORD_BRANCH_PREFIX}{commit}")
}

pub fn publish_tag(commit: &str) -> String {
    format!("{TAG_PREFIX}{commit}")
}

/// - Runs `git ls-remote --heads origin` in `root`.
fn branch_exists(root: &Path, branch: &str) -> Result<bool, Vec<String>> {
    crate::git_in(root, &["ls-remote", "--heads", ORIGIN, branch])
        .map(|listed| !listed.trim().is_empty())
}

/// - Runs `git ls-remote --heads origin` in `root`.
fn remote_record_branches(root: &Path) -> Result<Vec<String>, Vec<String>> {
    let pattern = format!("{RECORD_BRANCH_PREFIX}*");
    let listed = crate::git_in(root, &["ls-remote", "--heads", ORIGIN, &pattern])?;
    Ok(listed
        .lines()
        .filter_map(|line| line.split_whitespace().nth(1))
        .filter_map(|name| name.strip_prefix(HEADS_NAMESPACE))
        .map(str::to_string)
        .collect())
}

/// - Runs `git ls-remote` against `origin` through [`remote_record_branches`].
pub fn pending_records(
    root: &Path,
    recorded: &[binding_manifest::Entry],
) -> Result<Vec<String>, Vec<String>> {
    Ok(remote_record_branches(root)?
        .into_iter()
        .filter(|branch| {
            branch
                .strip_prefix(RECORD_BRANCH_PREFIX)
                .is_none_or(|commit| !recorded.iter().any(|entry| entry.commit == commit))
        })
        .collect())
}

/// - Runs `git switch --create`, `git add`, `git commit` and `git push --set-upstream` in `root`.
fn commit_and_push(root: &Path, commit: &str) -> Result<String, Vec<String>> {
    let branch = record_branch(commit);
    let message = format!("{COMMIT_MESSAGE_PREFIX}{commit}");
    crate::git_in(root, &["switch", "--quiet", "--create", &branch])?;
    crate::git_in(
        root,
        &["add", binding_manifest::FILE, binding_changelog::FILE],
    )?;
    crate::git_in(root, &["commit", "--quiet", "--message", &message])?;
    crate::git_in(
        root,
        &["push", "--quiet", "--set-upstream", ORIGIN, &branch],
    )?;
    Ok(branch)
}

/// - Runs `git ls-remote --tags origin` in `root`.
fn remote_tag_commit(root: &Path, tag: &str) -> Result<Option<String>, Vec<String>> {
    let plain = format!("{TAGS_NAMESPACE}{tag}");
    let peeled = format!("{plain}{PEELED_SUFFIX}");
    let listed = crate::git_in(root, &["ls-remote", "--tags", ORIGIN, &plain, &peeled])?;
    Ok(listed
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .next_back()
        .map(str::to_string))
}

/// - Runs `git ls-tree` and `git show` in `root`.
fn dispatchable(root: &Path, commit: &str) -> Result<bool, Vec<String>> {
    if !crate::listed_at(root, commit, PUBLISH_WORKFLOW)? {
        return Ok(false);
    }
    let workflow = crate::git_in(root, &["show", &format!("{commit}:{PUBLISH_WORKFLOW}")])?;
    Ok(workflow.lines().any(|line| line.trim() == DISPATCH_TRIGGER))
}

pub struct Plan {
    pub commit: String,
    pub tag: String,
    pub relay: bool,
    pub recorded: bool,
    pub entries: Vec<binding_manifest::Entry>,
}

/// - Runs `git rev-parse`, `git merge-base --is-ancestor`, `git ls-tree` and `git show` in `root`,
///   and reads `bindings/published.toml` at `default_branch` through `git show`.
pub fn planned(
    root: &Path,
    commit: &str,
    run_at: &str,
    default_branch: &str,
) -> Result<Plan, Vec<String>> {
    let commit = crate::commit_of(root, commit)?;
    let run_at = crate::commit_of(root, run_at)?;
    if !crate::is_ancestor(root, &commit, default_branch)? {
        return Err(vec![format!(
            "{commit} is not a commit of {default_branch}, and the Binding Layer is published from that branch alone"
        )]);
    }
    let entries = binding_manifest::entries_at(root, Some(default_branch))?;
    let recorded = entries.iter().any(|entry| entry.commit == commit);
    if let Some(last) = entries.last().filter(|_| !recorded) {
        if !crate::is_ancestor(root, &last.commit, &commit)? {
            return Err(vec![format!(
                "{commit} is older than {}, the last entry of {} on {default_branch}, and the record is oldest first",
                last.commit,
                binding_manifest::FILE
            )]);
        }
    }
    let relay = commit != run_at;
    if relay && !recorded && !dispatchable(root, &commit)? {
        return Err(vec![format!(
            "{commit} holds no `{DISPATCH_TRIGGER}` trigger in {PUBLISH_WORKFLOW}, so it predates publication by dispatch and cannot be published"
        )]);
    }
    Ok(Plan {
        tag: publish_tag(&commit),
        commit,
        relay,
        recorded,
        entries,
    })
}

/// - Runs `orasust` child processes through [`binding_manifest::held_digest`], once per platform.
fn held_digests(
    commit: &str,
    credentials: Option<&orasust::Credentials>,
) -> Result<Vec<(&'static str, String)>, Vec<String>> {
    let mut held = Vec::new();
    for platform in binding_manifest::PLATFORMS {
        if let Some(digest) = binding_manifest::held_digest(platform, commit, credentials)? {
            held.push((platform, digest));
        }
    }
    Ok(held)
}

/// - Runs `git` child processes in `root` through [`planned`].
/// - Runs `git ls-remote` against `origin` through [`pending_records`] and, when the run is not
///   at the commit, through [`remote_tag_commit`].
/// - Writes the plan to stdout, one `<name>=<value>` line per output.
fn plan(root: &Path, args: &[String]) -> Result<(), Vec<String>> {
    let required = |flag: &str| crate::required_flag(args, flag, USAGE);
    let plan = planned(
        root,
        required(COMMIT_FLAG)?,
        required(RUN_AT_FLAG)?,
        required(DEFAULT_BRANCH_FLAG)?,
    )?;
    if !plan.recorded {
        let own = record_branch(&plan.commit);
        let pending = pending_records(root, &plan.entries)?;
        let others: Vec<&str> = pending
            .iter()
            .map(String::as_str)
            .filter(|branch| *branch != own)
            .collect();
        if !others.is_empty() {
            return Err(vec![format!(
                "record branches are pending for {}; merge their pull requests, or delete the branches of the ones closed unmerged, before another publication, so the record stays oldest first",
                others.join(crate::LIST_SEPARATOR)
            )]);
        }
        if pending.contains(&own) {
            eprintln!(
                "{ORIGIN} holds {own}, so the record phase reuses it and opens its pull request"
            );
        }
    }
    let tagged = if plan.relay && !plan.recorded {
        match remote_tag_commit(root, &plan.tag)? {
            None => false,
            Some(at) if at == plan.commit => true,
            Some(at) => {
                return Err(vec![format!(
                    "{} points at {at}, not {}, so a relay there would build another commit",
                    plan.tag, plan.commit
                )]);
            }
        }
    } else {
        false
    };
    let outputs = [
        (COMMIT_OUTPUT, plan.commit),
        (TAG_OUTPUT, plan.tag),
        (RELAY_OUTPUT, plan.relay.to_string()),
        (TAGGED_OUTPUT, tagged.to_string()),
        (RECORDED_OUTPUT, plan.recorded.to_string()),
    ];
    for (name, value) in outputs {
        println!("{name}{OUTPUT_SEPARATOR}{value}");
    }
    Ok(())
}

/// - Reads the registry credentials in the environment when they are set.
/// - Runs `orasust version`, then `orasust` child processes through [`held_digests`].
/// - Writes `held=<bool>` to stdout.
fn held(root: &Path, args: &[String]) -> Result<(), Vec<String>> {
    let commit = crate::commit_of(root, crate::required_flag(args, COMMIT_FLAG, USAGE)?)?;
    crate::orasust::pinned()?;
    let digests = held_digests(&commit, credentials_if_set().as_ref())?;
    println!(
        "{HELD_OUTPUT}{OUTPUT_SEPARATOR}{}",
        binding_manifest::missing_platforms(&digests).is_empty()
    );
    Ok(())
}

/// - Reads the registry credentials in the environment.
/// - Runs `orasust version`, then `orasust` child processes through [`held_digests`].
/// - Reads each missing platform's bundle under `--bundles` and archives the iOS package through
///   `tar`, then pushes each such bundle to the registry.
fn push_bundles(root: &Path, args: &[String]) -> Result<(), Vec<String>> {
    let required = |flag: &str| crate::required_flag(args, flag, USAGE);
    let commit = crate::commit_of(root, required(COMMIT_FLAG)?)?;
    let bundles = Path::new(required(BUNDLES_FLAG)?);
    let credentials = credentials()?;
    crate::orasust::pinned()?;
    let held = held_digests(&commit, Some(&credentials))?;
    let built = built_platforms(bundles, &commit);
    let mut to_push = Vec::new();
    for platform in binding_manifest::missing_platforms(&held) {
        let dir = built
            .iter()
            .find(|(built, _)| *built == platform)
            .map(|(_, dir)| dir)
            .ok_or_else(|| {
                vec![format!(
                    "{} requires {platform} and {} holds no bundle for it, so that build did not succeed",
                    binding_manifest::FILE,
                    bundles.display()
                )]
            })?;
        to_push.push((platform, bundle_of(root, platform, dir, &commit)?));
    }
    for (platform, bundle) in &to_push {
        let digest = push(platform, &commit, bundle, &credentials)?;
        eprintln!(
            "{} is {digest}",
            binding_manifest::reference(platform, &commit)
        );
    }
    Ok(())
}

/// - Reads `bindings/published.toml` and runs `git ls-remote` against `origin`.
/// - Reads the registry credentials in the environment when they are set, and runs
///   `orasust version`, then `orasust` child processes through [`held_digests`].
/// - Creates and removes a detached worktree of the commit and runs `cargo metadata` in it.
/// - Writes `bindings/published.toml` and `bindings/CHANGELOG.md`, then commits them on the
///   record branch and pushes it to `origin`.
/// - Writes `recorded=true` or `branch=<record branch>` to stdout.
fn record(root: &Path, args: &[String]) -> Result<(), Vec<String>> {
    let commit = crate::commit_of(root, crate::required_flag(args, COMMIT_FLAG, USAGE)?)?;
    if binding_manifest::entries_at(root, None)?
        .iter()
        .any(|entry| entry.commit == commit)
    {
        eprintln!("{} records {commit}", binding_manifest::FILE);
        println!("{RECORDED_OUTPUT}{OUTPUT_SEPARATOR}true");
        return Ok(());
    }
    let branch = record_branch(&commit);
    if branch_exists(root, &branch)? {
        eprintln!("{ORIGIN} holds {branch}, whose pull request records {commit}");
        println!("{BRANCH_OUTPUT}{OUTPUT_SEPARATOR}{branch}");
        return Ok(());
    }
    crate::orasust::pinned()?;
    let mut digests = held_digests(&commit, credentials_if_set().as_ref())?;
    let missing = binding_manifest::missing_platforms(&digests);
    if !missing.is_empty() {
        return Err(vec![format!(
            "the registry holds no {} bundle of {commit}, and the record follows the push",
            missing.join(crate::LIST_SEPARATOR)
        )]);
    }
    digests.sort_by_key(|(platform, _)| *platform);
    let entry = binding_manifest::Entry {
        commit: commit.clone(),
        since: None,
        digests: digests
            .into_iter()
            .map(|(platform, digest)| (platform.to_string(), digest))
            .collect(),
        audited: binding_manifest::with_checkout(root, &commit, binding_manifest::audited_at)?,
    };
    let file = root.join(binding_manifest::FILE);
    let text = binding_manifest::appended(&crate::read(&file)?, &entry)?;
    crate::write(&file, text)?;
    binding_changelog::regenerate(root)?;
    let branch = commit_and_push(root, &commit)?;
    println!("{BRANCH_OUTPUT}{OUTPUT_SEPARATOR}{branch}");
    Ok(())
}

/// - Runs one of [`plan`], [`held`], [`push_bundles`] and [`record`], named by the first argument.
pub fn dispatch(root: &Path, args: &[String]) -> Result<(), Vec<String>> {
    match args.first().map(String::as_str) {
        Some(PLAN_FLAG) => plan(root, args),
        Some(HELD_FLAG) => held(root, args),
        Some(PUSH_FLAG) => push_bundles(root, args),
        Some(RECORD_FLAG) => record(root, args),
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
        let two =
            crate::refused(maven_coordinate(&metadata(&["6.0.0-aaaa", "6.0.0-bbbb"])))?.concat();
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
    fn the_publish_workflow_is_dispatched_and_grants_what_its_steps_need() -> Result<(), Vec<String>>
    {
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
    fn the_plan_relays_to_the_commit_and_refuses_one_off_the_default_branch(
    ) -> Result<(), Vec<String>> {
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

    const ZERO_DIGEST: &str =
        "sha256:0000000000000000000000000000000000000000000000000000000000000000";

    #[test]
    fn a_commit_older_than_the_last_entry_is_refused_and_a_recorded_one_is_reported(
    ) -> Result<(), Vec<String>> {
        let repo = crate::scratch_dir("publish-plan-order")?;
        crate::hermetic_git(&repo, &["init", "--quiet", "--initial-branch", BRANCH])?;
        let first = binding_manifest::commit_on(&repo, "first")?;
        let published = binding_manifest::commit_on(&repo, "published")?;
        let record = format!(
            "[[entry]]\ncommit = \"{published}\"\nsince = \"{first}\"\naudited = [\"zingo (zingo-ffi/lib)\"]\nandroid = \"{ZERO_DIGEST}\"\nios = \"{ZERO_DIGEST}\"\n"
        );
        crate::create_parent(&repo.join(binding_manifest::FILE))?;
        crate::write(&repo.join(binding_manifest::FILE), record)?;
        crate::hermetic_git(&repo, &["add", binding_manifest::FILE])?;
        let tip = binding_manifest::commit_on(&repo, "tip")?;

        let older = crate::refused(planned(&repo, &first, &tip, BRANCH))?.concat();
        assert!(
            older.contains("older") && older.contains(&published),
            "{older}"
        );
        assert!(planned(&repo, &published, &tip, BRANCH)?.recorded);
        let next = planned(&repo, &tip, &tip, BRANCH)?;
        assert!(!next.recorded && !next.relay);
        Ok(())
    }

    #[test]
    fn a_commit_before_publication_by_dispatch_is_refused_when_relayed() -> Result<(), Vec<String>>
    {
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
}
