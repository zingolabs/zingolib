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
const BINARY_MEDIA_TYPE: &str = "application/octet-stream";
const ZIP_SUFFIXES: [&str; 2] = [".aar", ".zip"];
const GZIP_SUFFIXES: [&str; 2] = [".tar.gz", ".tgz"];
const AAR_SUFFIX: &str = "-release.aar";
const ARCHIVE_SUFFIX: &str = ".tar.gz";
const TAR: &str = "tar";
const TAR_ARGS: [&str; 3] = ["--create", "--gzip", "--file"];
const PACKAGE_STAGING_SUFFIX: &str = "-package";
const COMMITTER_NAME: &str = "github-actions[bot]";
const COMMITTER_EMAIL: &str = "41898282+github-actions[bot]@users.noreply.github.com";
const COMMIT_MESSAGE_PREFIX: &str = "chore(bindings): record the publication of ";
const COMMIT_FLAG: &str = "--commit";
const BUNDLES_FLAG: &str = "--bundles";
const USAGE: &str = "usage: binding-publish --commit <commit> \
    --bundles <directory holding one subdirectory per built platform>";

pub struct Credentials {
    pub username: String,
    pub token: String,
}

struct Bundle {
    file: PathBuf,
    descriptor: String,
}

pub fn media_type_of(file_name: &str) -> &'static str {
    if ZIP_SUFFIXES
        .iter()
        .any(|suffix| file_name.ends_with(suffix))
    {
        ZIP_MEDIA_TYPE
    } else if GZIP_SUFFIXES
        .iter()
        .any(|suffix| file_name.ends_with(suffix))
    {
        GZIP_MEDIA_TYPE
    } else {
        BINARY_MEDIA_TYPE
    }
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

pub fn built_platforms(bundles: &Path) -> Vec<&'static str> {
    binding_manifest::PLATFORMS
        .into_iter()
        .filter(|platform| bundles.join(platform).is_dir())
        .collect()
}

pub fn missing_required(built: &[&str]) -> Vec<&'static str> {
    binding_manifest::REQUIRED_PLATFORMS
        .into_iter()
        .filter(|platform| !built.contains(platform))
        .collect()
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
        media_type_of(file_name).to_string(),
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
        "{BINARY} was built without the registry feature and cannot push {} ({}) as {}",
        bundle.file.display(),
        bundle.descriptor,
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
    let file = match platform {
        binding_manifest::ANDROID => find_file(dir, |name| name.ends_with(AAR_SUFFIX))?,
        binding_manifest::IOS => swift_package_archive(root, dir, commit)?,
        other => return Err(vec![format!("no bundle shape is known for {other}")]),
    };
    Ok(Bundle { file, descriptor })
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

/// - Runs `git config`, `git add`, `git commit`, and `git push` as child processes in `root`.
fn commit_and_push(root: &Path, commit: &str) -> Result<(), Vec<String>> {
    let message = format!("{COMMIT_MESSAGE_PREFIX}{commit}");
    let commands: [&[&str]; 5] = [
        &["config", "user.name", COMMITTER_NAME],
        &["config", "user.email", COMMITTER_EMAIL],
        &["add", binding_manifest::FILE, binding_changelog::FILE],
        &["commit", "--message", &message],
        &["push"],
    ];
    for args in commands {
        crate::git_in(root, args)?;
    }
    Ok(())
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
    let built = built_platforms(bundles);
    let missing = missing_required(&built);
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
    for platform in built {
        let bundle = bundle_of(root, platform, &bundles.join(platform), &commit)?;
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
    fn the_media_type_follows_the_bundle_suffix() {
        assert_eq!(
            media_type_of("zingo-binding-layer-release.aar"),
            ZIP_MEDIA_TYPE
        );
        assert_eq!(media_type_of("ZingoBindings.tar.gz"), GZIP_MEDIA_TYPE);
        assert_eq!(media_type_of("bundle.bin"), BINARY_MEDIA_TYPE);
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
    fn the_required_platforms_come_from_the_manifest_module_alone() {
        assert_eq!(missing_required(&[]), binding_manifest::REQUIRED_PLATFORMS);
        assert_eq!(
            missing_required(&binding_manifest::PLATFORMS),
            Vec::<&str>::new()
        );
        assert_eq!(
            missing_required(&[binding_manifest::IOS]),
            [binding_manifest::ANDROID]
        );
    }

    #[test]
    fn the_built_platforms_are_the_subdirectories_named_after_a_platform() {
        let root = crate::repo_root().unwrap();
        assert_eq!(built_platforms(&root), Vec::<&str>::new());
    }

    #[test]
    fn a_missing_flag_is_refused_with_the_usage() {
        let diagnostic = dispatch(Path::new("/host/zingolib"), &[])
            .unwrap_err()
            .concat();
        assert!(diagnostic.contains(COMMIT_FLAG));
    }
}
