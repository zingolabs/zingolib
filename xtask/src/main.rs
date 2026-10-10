#![forbid(unsafe_code)]

use std::path::Path;

use xtask::ci_plan;

type Task = fn(&Path, &[String]) -> Result<(), Vec<String>>;

const TASKS: &[(&str, Task, &str)] = &[
    (
        "ci-plan",
        ci_plan::dispatch,
        "print the CI plan as JSON; `image` or `image-tag` prints that value alone",
    ),
    (
        "rust-version",
        rust_version,
        "print the rustc version rust-toolchain.toml pins",
    ),
];

const PROGRAM: &str = "xtask";

/// - Writes the pinned rustc version to stdout.
fn rust_version(root: &Path, _args: &[String]) -> Result<(), Vec<String>> {
    println!("{}", xtask::toolchain_channel(root)?);
    Ok(())
}

fn usage() -> Vec<String> {
    let mut lines = vec!["usage: cargo xtask <task> [args]".to_string()];
    lines.extend(
        TASKS
            .iter()
            .map(|(name, _, description)| format!("  {name:<14} {description}")),
    );
    lines
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    xtask::run(
        PROGRAM,
        || {
            let (name, rest) = args.split_first().ok_or_else(usage)?;
            let (_, task, _) = TASKS
                .iter()
                .find(|(task_name, _, _)| task_name == name)
                .ok_or_else(usage)?;
            task(&xtask::repo_root()?, rest)
        },
        |()| (),
    )
}
