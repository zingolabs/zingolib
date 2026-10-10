#![forbid(unsafe_code)]

pub mod binding_changelog;
pub mod binding_layer;
pub mod binding_manifest;
pub mod binding_publish;
pub mod orasust;

pub use xtask::{
    cargo_subcommand_version, commit_of, commit_spec, create_parent, declares_a_package,
    dispatch_from_root, display_relative, finished_in, flag_value, fresh_dir, git, git_in,
    listed_at, manifest_above, merge_base, package_location, parse_dest, read, read_bytes,
    repo_root, required_flag, run, run_streaming_in, stdout_in, stdout_of, stdout_of_owned,
    toolchain_channel, touched_manifests, utf8, verdict, workspace_manifests, workspace_members,
    Finished, CARGO, DEFAULT_BASE, FALLBACK_BASE, LIST_SEPARATOR, MANIFEST, MANIFEST_PATH_FLAG,
    PACKAGE_ID_FORMAT, TOOLCHAIN_FILE,
};
