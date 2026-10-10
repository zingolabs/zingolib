#![forbid(unsafe_code)]

use std::path::Path;

use xtask::{
    binding_changelog, binding_manifest, binding_publish, birth_trial, build_binding_layer,
    bundle_nym_proxy, ci_plan, dupes_gate, exclusion_audit, exit_census, feature_census,
    feature_sweep, image, run_cli, sweep_teardown, sync_ab, sync_bench, test, test_summary,
};

type Entry = fn(&Path, &[String]) -> Result<(), Vec<String>>;

enum Task {
    Run(Entry),
    With(Entry, &'static [&'static str]),
    Test(test::Variant),
}

impl Task {
    fn run(&self, root: &Path, args: &[String]) -> Result<(), Vec<String>> {
        match self {
            Self::Run(task) => task(root, args),
            Self::With(task, fixed) => {
                let all: Vec<String> = fixed
                    .iter()
                    .map(ToString::to_string)
                    .chain(args.iter().cloned())
                    .collect();
                task(root, &all)
            }
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
        "hierarchy-test",
        Task::Run(test_summary::dispatch),
        "the three gated test phases (packages, zingo-cli, libtonode) with a combined summary",
    ),
    (
        "lite-hierarchy",
        Task::With(test_summary::dispatch, &[test_summary::LITE_FLAG]),
        "the same three phases with the libtonode one narrowed to send_shield_cycle",
    ),
    (
        "exclusion-audit",
        Task::Run(exclusion_audit::dispatch),
        "check that each member of BUILD_EXCLUDABLE, or a named candidate, is free to exclude",
    ),
    (
        "feature-sweep",
        Task::Run(feature_sweep::dispatch),
        "check the crates this branch touches in every feature combination",
    ),
    (
        "feature-census",
        Task::Run(feature_census::dispatch),
        "report declared dependency features nothing needs; `--all`, `--base <ref>`, `--bless`",
    ),
    (
        "build-binding-layer",
        Task::Run(build_binding_layer::dispatch),
        "build the Binding Layer for `android` or `ios`; `artifact` names the artifacts, `image` builds the Android image",
    ),
    (
        binding_changelog::BINARY,
        Task::Run(binding_changelog::dispatch),
        "generate or `--check` bindings/CHANGELOG.md from the audited crates' changelogs",
    ),
    (
        binding_manifest::BINARY,
        Task::Run(binding_manifest::dispatch),
        "`--check` bindings/published.toml, or print `--newest` or `--orasust-version`",
    ),
    (
        binding_publish::BINARY,
        Task::Run(binding_publish::dispatch),
        "publish the Binding Layer bundles and record them in the manifest",
    ),
    (
        "bundle-nym-proxy",
        Task::Run(bundle_nym_proxy::dispatch),
        "build nym-proxy from the zingo-netutils workspace and place it beside the wallet binaries; `--release`, `--dest <dir>`",
    ),
    (
        "exit-census",
        Task::Run(exit_census::dispatch),
        "count the Nym exits the proxy discovers, grouped by gateway",
    ),
    (
        "run-cli",
        Task::Run(run_cli::dispatch),
        "build and launch zingo-cli with the mixnet transport and nym-proxy bundled",
    ),
    (
        "sweep-teardown",
        Task::Run(sweep_teardown::dispatch),
        "probe the sweep's indexers through a standalone proxy with grpcurl; `--rounds N`, `--proxy <path>`",
    ),
    (
        "sync-bench",
        Task::Run(sync_bench::dispatch),
        "time sync inside a real run-cli --online session",
    ),
    (
        "sync-ab",
        Task::Run(sync_ab::dispatch),
        "compare two commits' sync rate in interleaved run-cli --online sessions",
    ),
    (
        "birth-trial",
        Task::Run(birth_trial::dispatch),
        "measure proxy births and their Sentinel round trips; `--births N`",
    ),
    (
        dupes_gate::BINARY,
        Task::Run(dupes_gate::dispatch),
        "run the duplicated-code gate; `--install` installs the pinned cargo-dupes; other arguments go to cargo-dupes",
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
