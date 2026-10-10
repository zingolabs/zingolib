use std::path::Path;

use crate::{CARGO, exec_in};

const MANIFEST_PATH: &str = "tools/workbench/Cargo.toml";

#[derive(Clone, Copy)]
pub struct Binary(pub &'static str);

impl Binary {
    /// - Replaces this process with `cargo run` of the workbench binary in `root`.
    pub fn run(self, root: &Path, args: &[String]) -> Result<(), Vec<String>> {
        let mut command = vec![
            "run",
            "--quiet",
            "--manifest-path",
            MANIFEST_PATH,
            "--bin",
            self.0,
            "--",
        ];
        command.extend(args.iter().map(String::as_str));
        exec_in(root, CARGO, &command, &[])
    }
}
