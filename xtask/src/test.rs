use std::path::Path;

use crate::ci_plan;
use crate::container::{
    CARGO_GIT_VOLUME, CARGO_REGISTRY_VOLUME, Runtime, TARGET_VOLUME, TEST_BINARIES_DIR,
};
use std::process::Command;

use crate::{CARGO, command_in, exec_in, image};

pub const LIVE_PACKAGES: [&str; 2] = ["libtonode-tests", "zingo-cli"];

pub const BUILD_EXCLUDABLE: [&str; 0] = [];

pub const PACKAGES_WORD: &str = "packages";
pub const LIVE_WORD: &str = "live";

const PROFILE_ENV: &str = "ZINGOLIB_NEXTEST_PROFILE";
const DEFAULT_PROFILE: &str = "ci";

const RETRIES_ENV: &str = "ZINGOLIB_NEXTEST_RETRIES";
const DEFAULT_RETRIES: &str = "0";

const LIBTEST_JSON_ENV: (&str, &str) = ("NEXTEST_EXPERIMENTAL_LIBTEST_JSON", "1");

const FORWARDED_ENV: [&str; 2] = ["ZINGO_REGENERATE_CHAIN_CACHE", "RUST_LOG"];

const WORKSPACE_MOUNT: &str = "/workspace";
const CARGO_GIT_MOUNT: &str = "/root/.cargo/git";
const CARGO_REGISTRY_MOUNT: &str = "/root/.cargo/registry";

const TEST_BINARIES_ENV: &str = "TEST_BINARIES_DIR";

const CONTAINER_PRELUDE: &str = "mkdir -p test_binaries/bins && ln -sf /usr/bin/zainod /usr/bin/zebrad test_binaries/bins/ && exec \"$@\"";

const PACKAGE_SELECTION_FLAGS: [&str; 6] = [
    "-p",
    "--package",
    "--workspace",
    "--all",
    "--exclude",
    "--manifest-path",
];

const PACKAGE_SELECTION_PREFIXES: [&str; 3] = ["--package=", "--exclude=", "--manifest-path="];

const WORKSPACE_FLAG: &str = "--workspace";

const EXTRA_CREDIT_ARGS: [&str; 4] = [
    "--package",
    "libtonode-tests",
    "--features",
    "extra-credit-tests",
];

const RERUN_ARGS: [&str; 2] = ["--rerun", "latest"];

const RUN_IGNORED_ARGS: [&str; 2] = ["--run-ignored", "all"];

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    Test,
    Rerun,
    ExtraCredit,
    LocalRun,
    RunIgnored,
}

impl Variant {
    fn fixed_args(self) -> &'static [&'static str] {
        match self {
            Self::Test | Self::LocalRun => &[],
            Self::Rerun => &RERUN_ARGS,
            Self::ExtraCredit => &EXTRA_CREDIT_ARGS,
            Self::RunIgnored => &RUN_IGNORED_ARGS,
        }
    }

    fn on_host(self) -> bool {
        matches!(self, Self::LocalRun | Self::RunIgnored)
    }

    /// - Runs the nextest run through [`host`] or [`container`].
    pub fn run(self, root: &Path, args: &[String]) -> Result<(), Vec<String>> {
        let args = if self == Self::Test {
            front_door_args(args)
        } else {
            args.to_vec()
        };
        let all = prepend(self.fixed_args(), &args);
        if self.on_host() {
            host(root, &all)
        } else {
            container(root, &all)
        }
    }
}

pub fn front_door_args(args: &[String]) -> Vec<String> {
    let (set_args, rest) = reserved_word_args(args);
    let mut all = set_args;
    all.extend(rest.iter().cloned());
    all
}

fn prepend(fixed: &[&str], args: &[String]) -> Vec<String> {
    fixed
        .iter()
        .map(ToString::to_string)
        .chain(args.iter().cloned())
        .collect()
}

pub fn live_filter() -> String {
    LIVE_PACKAGES
        .iter()
        .map(|package| format!("package({package})"))
        .collect::<Vec<_>>()
        .join("|")
}

fn reserved_word_args(args: &[String]) -> (Vec<String>, &[String]) {
    let Some((first, rest)) = args.split_first() else {
        return (Vec::new(), args);
    };
    let set_args = match first.as_str() {
        PACKAGES_WORD => {
            let mut set_args = vec![
                WORKSPACE_FLAG.to_string(),
                "-E".to_string(),
                format!("!({})", live_filter()),
            ];
            for package in BUILD_EXCLUDABLE {
                set_args.push("--exclude".to_string());
                set_args.push(package.to_string());
            }
            set_args
        }
        LIVE_WORD => vec![WORKSPACE_FLAG.to_string(), "-E".to_string(), live_filter()],
        _ => return (Vec::new(), args),
    };
    if has_package_selection(rest) {
        (Vec::new(), rest)
    } else {
        (set_args, rest)
    }
}

fn has_package_selection(args: &[String]) -> bool {
    args.iter().any(|arg| is_package_selection(arg))
}

pub fn is_package_selection(arg: &str) -> bool {
    PACKAGE_SELECTION_FLAGS.contains(&arg)
        || PACKAGE_SELECTION_PREFIXES
            .iter()
            .any(|prefix| arg.starts_with(prefix))
}

/// - Reads ZINGOLIB_NEXTEST_PROFILE and ZINGOLIB_NEXTEST_RETRIES from the environment.
fn nextest_args(args: &[String]) -> Vec<String> {
    let mut all = vec![
        "--profile".to_string(),
        env_or(PROFILE_ENV, DEFAULT_PROFILE),
        "--retries".to_string(),
        env_or(RETRIES_ENV, DEFAULT_RETRIES),
    ];
    all.extend(args.iter().cloned());
    if !has_package_selection(args) {
        all.push(WORKSPACE_FLAG.to_string());
    }
    all
}

/// - Reads `name` from the environment.
fn env_or(name: &str, default: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| default.to_string())
}

/// - Creates `test_binaries/bins` under `root` on disk.
/// - Replaces this process with `cargo nextest run` in `root`.
fn host(root: &Path, args: &[String]) -> Result<(), Vec<String>> {
    let directory = root.join(TEST_BINARIES_DIR);
    std::fs::create_dir_all(&directory)
        .map_err(|e| vec![format!("cannot create {}: {e}", directory.display())])?;
    let mut command = vec!["nextest".to_string(), "run".to_string()];
    command.extend(nextest_args(args));
    exec_in(
        root,
        CARGO,
        &command.iter().map(String::as_str).collect::<Vec<_>>(),
        &[LIBTEST_JSON_ENV],
    )
}

/// - Builds the test image when the runtime lacks it, through [`container_command`].
/// - Replaces this process with `<runtime> run` of the test image.
fn container(root: &Path, args: &[String]) -> Result<(), Vec<String>> {
    let command = container_command(root, args)?;
    let args: Vec<&str> = command
        .get_args()
        .map(|arg| arg.to_str().unwrap_or_default())
        .collect();
    exec_in(
        root,
        command.get_program().to_str().unwrap_or_default(),
        &args,
        &[],
    )
}

/// - Builds the test image when the runtime lacks it, through [`image::ensure`].
/// - Reads ZINGO_REGENERATE_CHAIN_CACHE and RUST_LOG from the environment.
pub fn container_command(root: &Path, args: &[String]) -> Result<Command, Vec<String>> {
    image::ensure(root)?;
    let runtime = Runtime::detect()?;
    let suffix = runtime.mount_suffix();
    let image = ci_plan::image(root)?;
    let workspace = root
        .to_str()
        .ok_or_else(|| vec![format!("{} is not valid UTF-8", root.display())])?;

    let mut command = vec![
        "run".to_string(),
        "--rm".to_string(),
        "--init".to_string(),
        "--pids-limit".to_string(),
        "-1".to_string(),
        "-v".to_string(),
        format!("{workspace}:{WORKSPACE_MOUNT}"),
        "-v".to_string(),
        format!("{TARGET_VOLUME}:{WORKSPACE_MOUNT}/target{suffix}"),
        "-v".to_string(),
        format!("{CARGO_GIT_VOLUME}:{CARGO_GIT_MOUNT}{suffix}"),
        "-v".to_string(),
        format!("{CARGO_REGISTRY_VOLUME}:{CARGO_REGISTRY_MOUNT}{suffix}"),
        "-e".to_string(),
        format!("{}={}", LIBTEST_JSON_ENV.0, LIBTEST_JSON_ENV.1),
        "-e".to_string(),
        format!("{TEST_BINARIES_ENV}={WORKSPACE_MOUNT}/{TEST_BINARIES_DIR}"),
    ];
    for name in FORWARDED_ENV {
        if std::env::var(name).is_ok_and(|value| !value.is_empty()) {
            command.push("-e".to_string());
            command.push(name.to_string());
        }
    }
    command.extend([
        "-w".to_string(),
        WORKSPACE_MOUNT.to_string(),
        image,
        "bash".to_string(),
        "-lc".to_string(),
        CONTAINER_PRELUDE.to_string(),
        "bash".to_string(),
        CARGO.to_string(),
        "nextest".to_string(),
        "run".to_string(),
    ]);
    command.extend(nextest_args(args));
    Ok(command_in(
        root,
        runtime.program(),
        &command.iter().map(String::as_str).collect::<Vec<_>>(),
        &[],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn the_live_filter_names_every_live_package() {
        assert_eq!(live_filter(), "package(libtonode-tests)|package(zingo-cli)");
    }

    #[test]
    fn packages_negates_the_live_filter_and_keeps_the_workspace_scope() {
        let args = strings(&["packages", "--no-fail-fast"]);
        let (set_args, rest) = reserved_word_args(&args);
        assert_eq!(
            set_args,
            strings(&[
                "--workspace",
                "-E",
                "!(package(libtonode-tests)|package(zingo-cli))"
            ])
        );
        assert_eq!(rest, strings(&["--no-fail-fast"]));
    }

    #[test]
    fn live_selects_the_live_filter() {
        let args = strings(&["live"]);
        let (set_args, rest) = reserved_word_args(&args);
        assert_eq!(
            set_args,
            strings(&["--workspace", "-E", live_filter().as_str()])
        );
        assert!(rest.is_empty());
    }

    #[test]
    fn an_explicit_package_selection_replaces_the_reserved_word_scope() {
        let args = strings(&["packages", "-p", "zingo-memo"]);
        let (set_args, rest) = reserved_word_args(&args);
        assert!(set_args.is_empty());
        assert_eq!(rest, strings(&["-p", "zingo-memo"]));
    }

    #[test]
    fn a_plain_invocation_has_no_reserved_word() {
        let args = strings(&["some_test_name"]);
        let (set_args, rest) = reserved_word_args(&args);
        assert!(set_args.is_empty());
        assert_eq!(rest, args);
    }

    #[test]
    fn every_selection_flag_form_is_a_package_selection() {
        for arg in [
            "-p",
            "--package",
            "--package=zingolib",
            "--workspace",
            "--all",
            "--exclude",
            "--exclude=zingo-cli",
            "--manifest-path",
            "--manifest-path=Cargo.toml",
        ] {
            assert!(is_package_selection(arg), "should reject {arg}");
        }
    }

    #[test]
    fn ordinary_nextest_args_are_not_package_selections() {
        for arg in [
            "--no-fail-fast",
            "--no-capture",
            "-E",
            "test(slow)",
            "some_test_name",
            "--run-ignored",
            "--test",
        ] {
            assert!(!is_package_selection(arg), "should pass {arg}");
        }
    }

    #[test]
    fn nextest_args_append_the_workspace_unless_a_selection_is_present() {
        let plain = nextest_args(&strings(&["-E", "test(x)"]));
        assert_eq!(plain.last().map(String::as_str), Some("--workspace"));
        assert_eq!(&plain[..2], &strings(&["--profile", "ci"])[..]);
        let selected = nextest_args(&strings(&["--package=zingo-memo"]));
        assert!(!selected.contains(&"--workspace".to_string()));
        let joined = nextest_args(&strings(&["--manifest-path", "x/Cargo.toml"]));
        assert!(!joined.contains(&"--workspace".to_string()));
    }
}
