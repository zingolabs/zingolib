use std::path::Path;

use crate::stdout_in;

pub const RUNTIME_ENV: &str = "CONTAINER_RUNTIME";

const PODMAN: &str = "podman";
const DOCKER: &str = "docker";

const PODMAN_MOUNT_SUFFIX: &str = ":U";

pub const TARGET_VOLUME: &str = "zingolib-container-target";
pub const CARGO_GIT_VOLUME: &str = "zingolib-cargo-git";
pub const CARGO_REGISTRY_VOLUME: &str = "zingolib-cargo-registry";

const VOLUMES: [&str; 3] = [TARGET_VOLUME, CARGO_GIT_VOLUME, CARGO_REGISTRY_VOLUME];

pub const TEST_BINARIES_DIR: &str = "test_binaries/bins";

const TARGET_DIR: &str = "target";

pub struct Runtime {
    program: String,
}

impl Runtime {
    /// - Reads CONTAINER_RUNTIME and PATH from the environment.
    /// - Reads the directories on PATH from disk.
    pub fn detect() -> Result<Self, Vec<String>> {
        if let Ok(program) = std::env::var(RUNTIME_ENV) {
            return Ok(Self { program });
        }
        [PODMAN, DOCKER]
            .into_iter()
            .find(|program| on_path(program))
            .map(|program| Self {
                program: program.to_string(),
            })
            .ok_or_else(|| {
                vec![format!(
                    "Neither {PODMAN} nor {DOCKER} was found. Install one or set {RUNTIME_ENV}."
                )]
            })
    }

    pub fn program(&self) -> &str {
        &self.program
    }

    pub fn mount_suffix(&self) -> &'static str {
        if self.program == PODMAN {
            PODMAN_MOUNT_SUFFIX
        } else {
            ""
        }
    }

    /// - Runs `<runtime> volume inspect` and `<runtime> volume create` as child processes.
    /// - Creates `target` and `test_binaries/bins` under `root` on disk.
    pub fn ensure_volumes(&self, root: &Path) -> Result<(), Vec<String>> {
        for volume in VOLUMES {
            if !self.succeeds(root, &["volume", "inspect", volume])? {
                stdout_in(root, &self.program, &["volume", "create", volume], &[])?;
            }
        }
        for directory in [TARGET_DIR, TEST_BINARIES_DIR] {
            let path = root.join(directory);
            std::fs::create_dir_all(&path)
                .map_err(|e| vec![format!("cannot create {}: {e}", path.display())])?;
        }
        Ok(())
    }

    /// - Runs `<runtime> image inspect` as a child process.
    pub fn has_image(&self, root: &Path, image: &str) -> Result<bool, Vec<String>> {
        self.succeeds(root, &["image", "inspect", image])
    }

    /// - Runs `<runtime> <args>` as a child process in `root`, with its output discarded.
    fn succeeds(&self, root: &Path, args: &[&str]) -> Result<bool, Vec<String>> {
        let status = std::process::Command::new(&self.program)
            .args(args)
            .current_dir(root)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(|e| vec![format!("failed to run {}: {e}", self.program)])?;
        Ok(status.success())
    }
}

/// - Reads PATH from the environment.
/// - Reads each directory on PATH from disk.
fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).any(|directory| directory.join(program).is_file()))
        .unwrap_or(false)
}
