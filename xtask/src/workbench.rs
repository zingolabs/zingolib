use std::path::Path;

use crate::{CARGO, exec_in};

const MANIFEST_PATH: &str = "tools/workbench/Cargo.toml";

pub const TEST_SUMMARY: &str = "test-summary";

pub const LITE_FLAG: &str = "--lite";

#[derive(Clone, Copy)]
pub struct Binary {
    pub name: &'static str,
    pub fixed_args: &'static [&'static str],
}

impl Binary {
    pub const fn named(name: &'static str) -> Self {
        Self {
            name,
            fixed_args: &[],
        }
    }

    /// - Replaces this process with `cargo run` of the workbench binary in `root`.
    pub fn run(self, root: &Path, args: &[String]) -> Result<(), Vec<String>> {
        let mut command = vec![
            "run",
            "--quiet",
            "--manifest-path",
            MANIFEST_PATH,
            "--bin",
            self.name,
            "--",
        ];
        command.extend(self.fixed_args);
        command.extend(args.iter().map(String::as_str));
        exec_in(root, CARGO, &command, &[])
    }
}
