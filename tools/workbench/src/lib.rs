#![forbid(unsafe_code)]

pub mod binding_changelog;
pub mod binding_layer;
pub mod binding_manifest;
pub mod dupes_gate;
pub mod session;

pub const WORKBENCH_RELATIVE_DIR: &str = "tools/workbench";

pub use xtask::{
    cargo_subcommand_version, commit_of, commit_spec, create_parent, dispatch_from_root,
    finished_in, flag_value, fresh_dir, git, git_in, listed_at, parse_dest, read, repo_root, run,
    run_streaming_in, stdout_in, stdout_of, stdout_of_owned, toolchain_channel, utf8, verdict,
    Finished, CARGO, MANIFEST, TOOLCHAIN_FILE,
};
