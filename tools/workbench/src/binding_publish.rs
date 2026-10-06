use std::path::Path;

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
const PLATFORM_FLAG: &str = "--platform";
const BUNDLE_FLAG: &str = "--bundle";
const COMMIT_FLAG: &str = "--commit";
const DESCRIPTOR_FILE_FLAG: &str = "--descriptor-file";
const USAGE: &str = "usage: binding-publish --platform <platform> --bundle <file> \
    --commit <commit> --descriptor-file <file>";

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

#[cfg(feature = "registry")]
fn push(
    platform: &str,
    commit: &str,
    descriptor: &str,
    file_name: &str,
    data: Vec<u8>,
    auth: oci_client::secrets::RegistryAuth,
) -> Result<String, Vec<String>> {
    use oci_client::client::{Config, ImageLayer};
    use oci_client::manifest::OciImageManifest;
    let name = binding_manifest::reference(platform, commit);
    let image =
        oci_client::Reference::try_from(name.as_str()).map_err(|e| vec![format!("{name}: {e}")])?;
    let layers = [ImageLayer::new(
        data,
        media_type_of(file_name).to_string(),
        Some(layer_annotations(file_name).into_iter().collect()),
    )];
    let config = Config::new(EMPTY_CONFIG, EMPTY_CONFIG_MEDIA_TYPE.to_string(), None);
    let manifest = OciImageManifest::build(
        &layers,
        &config,
        Some(
            manifest_annotations(commit, descriptor)
                .into_iter()
                .collect(),
        ),
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| vec![format!("cannot start the runtime: {e}")])?;
    let client = oci_client::Client::new(oci_client::client::ClientConfig::default());
    runtime.block_on(async {
        client
            .push(&image, &layers, config, &auth, Some(manifest))
            .await
            .map_err(|e| vec![format!("cannot push {name}: {e}")])?;
        client
            .fetch_manifest_digest(&image, &auth)
            .await
            .map_err(|e| vec![format!("cannot read back the digest of {name}: {e}")])
    })
}

#[cfg(not(feature = "registry"))]
fn push(
    _platform: &str,
    _commit: &str,
    _descriptor: &str,
    _file_name: &str,
    _data: Vec<u8>,
    _auth: (String, String),
) -> Result<String, Vec<String>> {
    Err(vec![format!(
        "{BINARY} was built without the registry feature and cannot push"
    )])
}

fn credentials() -> Result<(String, String), Vec<String>> {
    let read = |variable: &str| {
        std::env::var(variable).map_err(|_| vec![format!("{variable} is not set")])
    };
    Ok((read(USERNAME_VARIABLE)?, read(TOKEN_VARIABLE)?))
}

/// - Reads the bundle and the descriptor file.
/// - Pushes the bundle to the registry with the credentials in the environment.
/// - Prints the digest the registry serves for the pushed tag.
pub fn dispatch(root: &Path, args: &[String]) -> Result<(), Vec<String>> {
    let required = |flag: &str| crate::required_flag(args, flag, USAGE);
    let platform = required(PLATFORM_FLAG)?;
    if !binding_manifest::PLATFORMS.contains(&platform) {
        return Err(vec![
            format!("unknown platform {platform}"),
            USAGE.to_string(),
        ]);
    }
    let commit = crate::commit_of(root, required(COMMIT_FLAG)?)?;
    let bundle = Path::new(required(BUNDLE_FLAG)?);
    let descriptor = crate::read(Path::new(required(DESCRIPTOR_FILE_FLAG)?))?
        .trim()
        .to_string();
    let file_name = bundle
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| vec![format!("{} has no file name", bundle.display())])?;
    let data = std::fs::read(bundle)
        .map_err(|e| vec![format!("cannot read {}: {e}", bundle.display())])?;
    let (username, token) = credentials()?;
    #[cfg(feature = "registry")]
    let auth = oci_client::secrets::RegistryAuth::Basic(username, token);
    #[cfg(not(feature = "registry"))]
    let auth = (username, token);
    let digest = push(platform, &commit, &descriptor, file_name, data, auth)?;
    println!("{digest}");
    Ok(())
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
    fn a_missing_flag_is_refused_with_the_usage() {
        let diagnostic = dispatch(Path::new("/host/zingolib"), &[])
            .unwrap_err()
            .concat();
        assert!(diagnostic.contains(PLATFORM_FLAG));
    }
}
