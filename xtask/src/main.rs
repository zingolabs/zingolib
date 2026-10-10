#![forbid(unsafe_code)]

use std::path::Path;

use xtask::{ci_plan, image, test, workbench};

enum Task {
    Run(fn(&Path, &[String]) -> Result<(), Vec<String>>),
    Test(test::Variant),
    Workbench(workbench::Binary),
}

impl Task {
    fn run(&self, root: &Path, args: &[String]) -> Result<(), Vec<String>> {
        match self {
            Self::Run(task) => task(root, args),
            Self::Test(variant) => variant.run(root, args),
            Self::Workbench(binary) => binary.run(root, args),
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
        "hierarchy-test",
        Task::Workbench(workbench::Binary::named(workbench::TEST_SUMMARY)),
        "the three gated test phases (packages, zingo-cli, libtonode) with a combined summary",
    ),
    (
        "lite-hierarchy",
        Task::Workbench(workbench::Binary {
            name: workbench::TEST_SUMMARY,
            fixed_args: &[workbench::LITE_FLAG],
        }),
        "the same three phases with the libtonode one narrowed to send_shield_cycle",
    ),
    (
        "exclusion-audit",
        Task::Workbench(workbench::Binary::named("exclusion-audit")),
        "check that each member of BUILD_EXCLUDABLE, or a named candidate, is free to exclude",
    ),
    (
        "feature-sweep",
        Task::Workbench(workbench::Binary::named("feature-sweep")),
        "check the crates this branch touches in every feature combination",
    ),
    (
        "bundle-nym-proxy",
        Task::Workbench(workbench::Binary::named("bundle-nym-proxy")),
        "build nym-proxy from the zingo-netutils workspace and place it beside the wallet binaries",
    ),
    (
        "run-cli",
        Task::Workbench(workbench::Binary::named("run-cli")),
        "build and launch zingo-cli with the mixnet transport and nym-proxy bundled",
    ),
    (
        "sync-bench",
        Task::Workbench(workbench::Binary::named("sync-bench")),
        "time sync inside a real run-cli --online session",
    ),
    (
        "sync-ab",
        Task::Workbench(workbench::Binary::named("sync-ab")),
        "compare two commits' sync rate in interleaved run-cli --online sessions",
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
