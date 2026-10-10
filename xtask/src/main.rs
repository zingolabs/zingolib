#![forbid(unsafe_code)]

use std::path::Path;

use xtask::{ci_plan, image, test};

enum Task {
    Run(fn(&Path, &[String]) -> Result<(), Vec<String>>),
    Test(test::Variant),
}

impl Task {
    fn run(&self, root: &Path, args: &[String]) -> Result<(), Vec<String>> {
        match self {
            Self::Run(task) => task(root, args),
            Self::Test(variant) => variant.run(root, args),
        }
    }
}

const TASKS: &[(&str, Task, &str)] = &[
    (
        "ci-plan",
        Task::Run(ci_plan::dispatch),
        "print the CI plan as JSON; `image` or `image-tag` prints that value alone",
    ),
    (
        "image",
        Task::Run(image::dispatch),
        "`build` the reproducible test image, or `ensure` the runtime holds it",
    ),
    (
        "test",
        Task::Test(test::Variant::Test),
        "containerized `cargo nextest run`; the first word `packages` or `live` selects that set",
    ),
    (
        "rerun",
        Task::Test(test::Variant::Rerun),
        "containerized rerun of the tests that failed or did not finish last time",
    ),
    (
        "test-extra-credit",
        Task::Test(test::Variant::ExtraCredit),
        "containerized run of libtonode's extra-credit suite",
    ),
    (
        "local-run",
        Task::Test(test::Variant::LocalRun),
        "`cargo nextest run` on the host with the CI defaults",
    ),
    (
        "run-ignored",
        Task::Test(test::Variant::RunIgnored),
        "`cargo nextest run` of the ignored tests on the host",
    ),
    (
        "rust-version",
        Task::Run(rust_version),
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
            .map(|(name, _, description)| format!("  {name:<18} {description}")),
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
            task.run(&xtask::repo_root()?, rest)
        },
        |()| (),
    )
}
