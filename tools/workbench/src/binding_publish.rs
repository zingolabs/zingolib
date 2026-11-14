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
pub const PUBLISH_WORKFLOW: &str = ".github/workflows/publish-bindings.yaml";
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
    match orasust::resolve(&name)? {
        orasust::Resolution::Found(seen) if seen == digest => Ok(digest),
        orasust::Resolution::Found(seen) => Err(vec![format!(
            "{name} was pushed as {digest} and resolves anonymously to {seen}"
        )]),
        orasust::Resolution::Absent => Err(vec![format!(
            "{name} was pushed as {digest} and resolves anonymously to nothing"
        )]),
        orasust::Resolution::Private => Err(vec![binding_manifest::private_package_hint(&name)]),
    }
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
    if !recorded
        && !entries.is_empty()
        && binding_manifest::recorded_ancestors(root, &entries, &commit)?.is_empty()
    {
        return Err(vec![format!(
            "no entry of {} on {default_branch} is an ancestor of {commit}, so its changelog section would have no base",
            binding_manifest::FILE
        )]);
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
mod tests;
