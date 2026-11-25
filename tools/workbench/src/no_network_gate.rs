use std::collections::BTreeSet;
use std::fmt;
use std::path::Path;
use std::process::Command;

use crate::{
    metadata_of, read, verdict, CARGO, LOCKED_FLAG, LOCKFILE, MANIFEST, NAME_KEY, PACKAGE_KEY,
};

pub const BINARY: &str = "no-network-gate";

const TREE: &str = "tree";
const PACKAGE_FLAG: &str = "--package";
const EDGES_FLAG: &str = "--edges";
const NORMAL_EDGES: &str = "normal";
const TARGET_FLAG: &str = "--target";
const ALL_TARGETS: &str = "all";
const PREFIX_FLAG: &str = "--prefix";
const NO_PREFIX: &str = "none";
const FORMAT_FLAG: &str = "--format";
const PACKAGE_FORMAT: &str = "{p}";
const FEATURES_FLAG: &str = "--features";
const ALL_FEATURES_FLAG: &str = "--all-features";
const LEGACY_FEATURE: &str = "legacy";
const DEFAULT_FEATURES_LABEL: &str = "default features";

const UNKNOWN_PACKAGE_MARK: &str = "did not match any packages";
const STALE_LOCKFILE_MARK: &str = "needs to be updated but --locked was passed";

const COMMON: &str = "zl-common";
const FILE_FORMAT: &str = "zl-file-format";

const ZINGOLIB: &str = "zingolib";
const PEPPER_SYNC: &str = "pepper-sync";
const NETUTILS: &str = "zl-netutils";

const TOKIO: &str = "tokio";
const TONIC: &str = "tonic";
const HYPER: &str = "hyper";
const REQWEST: &str = "reqwest";
const LIGHTWALLET_PROTOCOL: &str = "lightwallet-protocol";
const FUTURES: &str = "futures";

const ZCASH_PRIMITIVES: &str = "zcash_primitives";
const ZCASH_PROTOCOL: &str = "zcash_protocol";
const ZCASH_KEYS: &str = "zcash_keys";
const ORCHARD: &str = "orchard";
const SAPLING_CRYPTO: &str = "sapling-crypto";
const ZCASH_ADDRESS: &str = "zcash_address";
const ZCASH_ENCODING: &str = "zcash_encoding";
const ZIP32: &str = "zip32";
const INCREMENTALMERKLETREE: &str = "incrementalmerkletree";
const SHARDTREE: &str = "shardtree";

const NETWORK_MEMBERS: [&str; 3] = [ZINGOLIB, PEPPER_SYNC, NETUTILS];
const WALLET_MEMBERS: [&str; 4] = [COMMON, FILE_FORMAT, ZINGOLIB, PEPPER_SYNC];
const FILE_FORMAT_REGISTRY: [&str; 5] = [REQWEST, TONIC, TOKIO, HYPER, LIGHTWALLET_PROTOCOL];
const WALLET_STACK_REGISTRY: [&str; 10] = [
    ZCASH_PRIMITIVES,
    ZCASH_PROTOCOL,
    ZCASH_KEYS,
    ORCHARD,
    SAPLING_CRYPTO,
    ZCASH_ADDRESS,
    ZCASH_ENCODING,
    ZIP32,
    INCREMENTALMERKLETREE,
    SHARDTREE,
];

const RULES: [Rule; 4] = [
    Rule {
        package: NETUTILS,
        features: FeatureSet::All,
        members: &WALLET_MEMBERS,
        registry: &WALLET_STACK_REGISTRY,
    },
    Rule {
        package: COMMON,
        features: FeatureSet::Default,
        members: &NETWORK_MEMBERS,
        registry: &[TOKIO, TONIC, HYPER, REQWEST, LIGHTWALLET_PROTOCOL, FUTURES],
    },
    Rule {
        package: FILE_FORMAT,
        features: FeatureSet::Default,
        members: &NETWORK_MEMBERS,
        registry: &FILE_FORMAT_REGISTRY,
    },
    Rule {
        package: FILE_FORMAT,
        features: FeatureSet::Named(LEGACY_FEATURE),
        members: &NETWORK_MEMBERS,
        registry: &FILE_FORMAT_REGISTRY,
    },
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeatureSet {
    Default,
    Named(&'static str),
    All,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rule {
    pub package: &'static str,
    pub features: FeatureSet,
    pub members: &'static [&'static str],
    pub registry: &'static [&'static str],
}

impl Rule {
    fn forbids(&self, name: &str) -> bool {
        self.members.contains(&name) || self.registry.contains(&name)
    }
}

impl fmt::Display for Rule {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.features {
            FeatureSet::Default => {
                write!(formatter, "{} ({DEFAULT_FEATURES_LABEL})", self.package)
            }
            FeatureSet::Named(feature) => {
                write!(formatter, "{} ({FEATURES_FLAG} {feature})", self.package)
            }
            FeatureSet::All => {
                write!(formatter, "{} ({ALL_FEATURES_FLAG})", self.package)
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Failure {
    Forbidden {
        rule: Rule,
        dependency: String,
    },
    MemberUnknown {
        rule: Rule,
        member: &'static str,
    },
    RegistryUnknown {
        rule: Rule,
        dependency: &'static str,
    },
    PackageUnknown {
        rule: Rule,
    },
    LockfileStale {
        rule: Rule,
    },
    CargoDidNotStart {
        rule: Rule,
        error: String,
    },
    CargoKilled {
        rule: Rule,
        stderr: String,
    },
    CargoFailed {
        rule: Rule,
        code: i32,
        stderr: String,
    },
    OutputNotUtf8 {
        rule: Rule,
    },
    RootMissing {
        rule: Rule,
        first_line: Option<String>,
    },
}

impl fmt::Display for Failure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Failure::Forbidden { rule, dependency } => {
                write!(formatter, "{rule} depends on {dependency}")
            }
            Failure::MemberUnknown { rule, member } => write!(
                formatter,
                "{rule} forbids {member}, which is no workspace member, so the rule checks nothing"
            ),
            Failure::RegistryUnknown { rule, dependency } => write!(
                formatter,
                "{rule} forbids {dependency}, which is no package of the lockfile, so the rule checks nothing"
            ),
            Failure::PackageUnknown { rule } => write!(
                formatter,
                "{rule} names a package that the workspace does not hold"
            ),
            Failure::LockfileStale { rule } => write!(
                formatter,
                "{rule} cannot be checked: the lockfile is stale under {LOCKED_FLAG}"
            ),
            Failure::CargoDidNotStart { rule, error } => {
                write!(
                    formatter,
                    "{rule} cannot be checked: {CARGO} did not start: {error}"
                )
            }
            Failure::CargoKilled { rule, stderr } => write!(
                formatter,
                "{rule} cannot be checked: a signal ended `{CARGO} {TREE}`: {stderr}"
            ),
            Failure::CargoFailed { rule, code, stderr } => write!(
                formatter,
                "{rule} cannot be checked: `{CARGO} {TREE}` exited {code}: {stderr}"
            ),
            Failure::OutputNotUtf8 { rule } => write!(
                formatter,
                "{rule} cannot be checked: `{CARGO} {TREE}` wrote output that is not UTF-8"
            ),
            Failure::RootMissing { rule, first_line } => write!(
                formatter,
                "{rule} cannot be checked: the tree starts with {first_line:?}, not the package"
            ),
        }
    }
}

/// ```
/// use workbench::no_network_gate::{cargo_failure, Failure, FeatureSet, Rule};
///
/// let rule = Rule {
///     package: "zl-common",
///     features: FeatureSet::Default,
///     members: &[],
///     registry: &["digest"],
/// };
/// let ambiguous = "error: specification `digest` is ambiguous";
/// assert_eq!(
///     cargo_failure(rule, Some(101), ambiguous),
///     Failure::CargoFailed {
///         rule,
///         code: 101,
///         stderr: ambiguous.to_string(),
///     }
/// );
/// ```
pub fn cargo_failure(rule: Rule, code: Option<i32>, stderr: &str) -> Failure {
    match code {
        None => Failure::CargoKilled {
            rule,
            stderr: stderr.trim().to_string(),
        },
        Some(_) if stderr.contains(UNKNOWN_PACKAGE_MARK) => Failure::PackageUnknown { rule },
        Some(_) if stderr.contains(STALE_LOCKFILE_MARK) => Failure::LockfileStale { rule },
        Some(code) => Failure::CargoFailed {
            rule,
            code,
            stderr: stderr.trim().to_string(),
        },
    }
}

fn name_of(line: &str) -> Option<&str> {
    line.split_whitespace().next()
}

fn name_and_version(line: &str) -> String {
    line.split_whitespace()
        .take(2)
        .collect::<Vec<_>>()
        .join(" ")
}

/// ```
/// use workbench::no_network_gate::{forbidden_in, Failure, FeatureSet, Rule};
///
/// let rule = Rule {
///     package: "zl-common",
///     features: FeatureSet::Default,
///     members: &[],
///     registry: &["hyper"],
/// };
/// let tree = "zl-common v0.1.0 (/repo/crates/common)\nhyper v0.14.32\nhyper v1.11.0 (*)\n";
/// let dependencies: Vec<String> = forbidden_in(rule, tree)
///     .into_iter()
///     .map(|failure| failure.to_string())
///     .collect();
/// assert_eq!(
///     dependencies,
///     [
///         "zl-common (default features) depends on hyper v0.14.32",
///         "zl-common (default features) depends on hyper v1.11.0",
///     ]
/// );
/// ```
pub fn forbidden_in(rule: Rule, tree: &str) -> Vec<Failure> {
    let mut lines = tree.lines();
    let first_line = lines.next();
    if first_line.and_then(name_of) != Some(rule.package) {
        return vec![Failure::RootMissing {
            rule,
            first_line: first_line.map(str::to_string),
        }];
    }
    lines
        .filter(|line| name_of(line).is_some_and(|name| rule.forbids(name)))
        .map(name_and_version)
        .collect::<BTreeSet<String>>()
        .into_iter()
        .map(|dependency| Failure::Forbidden { rule, dependency })
        .collect()
}

/// ```
/// use std::collections::BTreeSet;
/// use workbench::no_network_gate::locked_packages_of;
///
/// let lockfile = concat!(
///     "[[package]]\nname = \"zl-common\"\nversion = \"0.1.0\"\n\n",
///     "[[package]]\nname = \"hyper\"\nversion = \"1.11.0\"\n",
///     "source = \"registry+https://github.com/rust-lang/crates.io-index\"\n",
/// );
/// assert_eq!(
///     locked_packages_of(lockfile),
///     Ok(BTreeSet::from(["hyper".to_string(), "zl-common".to_string()]))
/// );
/// assert!(locked_packages_of("version = 4\n").is_err());
/// ```
pub fn locked_packages_of(lockfile: &str) -> Result<BTreeSet<String>, String> {
    let document: toml_edit::DocumentMut = lockfile
        .parse()
        .map_err(|error| format!("the lockfile does not parse: {error}"))?;
    let packages = document
        .get(PACKAGE_KEY)
        .and_then(|item| item.as_array_of_tables())
        .ok_or_else(|| "the lockfile holds no [[package]] tables".to_string())?;
    Ok(packages
        .iter()
        .filter_map(|package| package.get(NAME_KEY).and_then(|name| name.as_str()))
        .map(str::to_string)
        .collect())
}

fn unknown<'names>(
    names: &'names [&'static str],
    known: &'names BTreeSet<String>,
) -> impl Iterator<Item = &'static str> + 'names {
    names.iter().copied().filter(|name| !known.contains(*name))
}

fn unknown_names(
    rule: Rule,
    workspace_members: &BTreeSet<String>,
    locked_packages: &BTreeSet<String>,
) -> Vec<Failure> {
    unknown(rule.members, workspace_members)
        .map(|member| Failure::MemberUnknown { rule, member })
        .chain(
            unknown(rule.registry, locked_packages)
                .map(|dependency| Failure::RegistryUnknown { rule, dependency }),
        )
        .collect()
}

fn tree_arguments(rule: Rule) -> Vec<&'static str> {
    let mut arguments = vec![
        TREE,
        LOCKED_FLAG,
        PACKAGE_FLAG,
        rule.package,
        EDGES_FLAG,
        NORMAL_EDGES,
        TARGET_FLAG,
        ALL_TARGETS,
        PREFIX_FLAG,
        NO_PREFIX,
        FORMAT_FLAG,
        PACKAGE_FORMAT,
    ];
    match rule.features {
        FeatureSet::Default => {}
        FeatureSet::Named(feature) => arguments.extend([FEATURES_FLAG, feature]),
        FeatureSet::All => arguments.push(ALL_FEATURES_FLAG),
    }
    arguments
}

/// - Runs `cargo tree` as a child process in `root` and waits for it.
fn tree_of(root: &Path, rule: Rule) -> Result<String, Failure> {
    let output = Command::new(CARGO)
        .args(tree_arguments(rule))
        .current_dir(root)
        .output()
        .map_err(|error| Failure::CargoDidNotStart {
            rule,
            error: error.to_string(),
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(cargo_failure(rule, output.status.code(), &stderr));
    }
    String::from_utf8(output.stdout).map_err(|_| Failure::OutputNotUtf8 { rule })
}

/// - Runs `cargo tree` as a child process in `root` and waits for it.
fn failures_of(
    root: &Path,
    rule: Rule,
    workspace_members: &BTreeSet<String>,
    locked_packages: &BTreeSet<String>,
) -> Vec<Failure> {
    let mut failures = unknown_names(rule, workspace_members, locked_packages);
    match tree_of(root, rule) {
        Ok(tree) => failures.extend(forbidden_in(rule, &tree)),
        Err(failure) => failures.push(failure),
    }
    failures
}

/// - Runs `cargo metadata` as a child process.
/// - Reads `Cargo.lock` in `root`.
/// - Runs `cargo tree` as a child process in `root`, once for each rule.
fn dispatch(root: &Path, args: &[String]) -> Result<(), Vec<String>> {
    if let Some(argument) = args.first() {
        return Err(vec![format!(
            "{BINARY} takes no argument, got {argument:?}"
        )]);
    }
    let workspace_members: BTreeSet<String> = metadata_of(&root.join(MANIFEST), false)?
        .packages
        .into_iter()
        .map(|package| package.name.to_string())
        .collect();
    let lockfile = root.join(LOCKFILE);
    let locked_packages = locked_packages_of(&read(&lockfile)?)
        .map_err(|error| vec![format!("{}: {error}", lockfile.display())])?;
    verdict(
        RULES
            .iter()
            .flat_map(|rule| failures_of(root, *rule, &workspace_members, &locked_packages))
            .map(|failure| failure.to_string())
            .collect(),
    )
}

/// - Reads the process arguments.
/// - Reads `Cargo.lock` at the root of the repository.
/// - Runs `cargo metadata` and `cargo tree` as child processes.
/// - Exits the process.
pub fn main() -> ! {
    crate::dispatch_from_root(BINARY, dispatch)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RULE: Rule = Rule {
        package: COMMON,
        features: FeatureSet::Named(LEGACY_FEATURE),
        members: &[NETUTILS],
        registry: &[HYPER],
    };
    const CARGO_ERROR_EXIT: i32 = 101;
    const UNKNOWN_PACKAGE_STDERR: &str =
        "error: package ID specification `zl-no-such-crate` did not match any packages\n";
    const STALE_LOCKFILE_STDERR: &str = "error: the lock file /repo/Cargo.lock needs to be updated but --locked was passed to prevent this\n";
    const UNLISTED_STDERR: &str = "error: failed to parse manifest at `/repo/Cargo.toml`\n";
    const ROOT_LINE: &str = "zl-common v0.1.0 (/repo/crates/common)";

    #[test]
    fn an_unknown_package_is_named_and_is_not_a_pass() {
        assert_eq!(
            cargo_failure(RULE, Some(CARGO_ERROR_EXIT), UNKNOWN_PACKAGE_STDERR),
            Failure::PackageUnknown { rule: RULE }
        );
    }

    #[test]
    fn a_stale_lockfile_is_named_and_is_not_a_pass() {
        assert_eq!(
            cargo_failure(RULE, Some(CARGO_ERROR_EXIT), STALE_LOCKFILE_STDERR),
            Failure::LockfileStale { rule: RULE }
        );
    }

    #[test]
    fn an_unlisted_cargo_error_keeps_its_exit_code_and_its_message() {
        assert_eq!(
            cargo_failure(RULE, Some(CARGO_ERROR_EXIT), UNLISTED_STDERR),
            Failure::CargoFailed {
                rule: RULE,
                code: CARGO_ERROR_EXIT,
                stderr: UNLISTED_STDERR.trim().to_string(),
            }
        );
    }

    #[test]
    fn a_cargo_without_an_exit_code_was_killed() {
        assert_eq!(
            cargo_failure(RULE, None, ""),
            Failure::CargoKilled {
                rule: RULE,
                stderr: String::new(),
            }
        );
    }

    #[test]
    fn a_tree_without_a_forbidden_package_is_clean() {
        let tree = format!("{ROOT_LINE}\nbip0039 v0.14.0\nhyperx v1.0.0\n");
        assert_eq!(forbidden_in(RULE, &tree), []);
    }

    #[test]
    fn a_forbidden_member_in_the_tree_fails() {
        let tree = format!("{ROOT_LINE}\nzl-netutils v7.0.0 (/repo/crates/netutils)\n");
        assert_eq!(
            forbidden_in(RULE, &tree),
            [Failure::Forbidden {
                rule: RULE,
                dependency: "zl-netutils v7.0.0".to_string(),
            }]
        );
    }

    #[test]
    fn an_empty_tree_is_a_failure_and_is_not_a_pass() {
        assert_eq!(
            forbidden_in(RULE, ""),
            [Failure::RootMissing {
                rule: RULE,
                first_line: None,
            }]
        );
    }

    #[test]
    fn a_tree_of_another_package_is_a_failure() {
        let tree = "zingolib v8.0.0 (/repo/crates/zingolib)\n";
        assert_eq!(
            forbidden_in(RULE, tree),
            [Failure::RootMissing {
                rule: RULE,
                first_line: Some(tree.trim().to_string()),
            }]
        );
    }

    #[test]
    fn a_forbidden_member_that_the_workspace_lacks_fails_the_rule() {
        let workspace_members = BTreeSet::from([COMMON.to_string()]);
        let locked_packages = BTreeSet::from([HYPER.to_string()]);
        assert_eq!(
            unknown_names(RULE, &workspace_members, &locked_packages),
            [Failure::MemberUnknown {
                rule: RULE,
                member: NETUTILS,
            }]
        );
    }

    #[test]
    fn a_forbidden_registry_package_that_the_lockfile_lacks_fails_the_rule() {
        let workspace_members = BTreeSet::from([NETUTILS.to_string()]);
        let locked_packages = BTreeSet::from([COMMON.to_string()]);
        assert_eq!(
            unknown_names(RULE, &workspace_members, &locked_packages),
            [Failure::RegistryUnknown {
                rule: RULE,
                dependency: HYPER,
            }]
        );
    }

    #[test]
    fn a_named_feature_rule_passes_the_feature_to_cargo() {
        assert!(tree_arguments(RULE).ends_with(&[FEATURES_FLAG, LEGACY_FEATURE]));
    }

    #[test]
    fn an_all_features_rule_passes_the_flag_to_cargo() {
        let rule = Rule {
            features: FeatureSet::All,
            ..RULE
        };
        assert!(tree_arguments(rule).ends_with(&[ALL_FEATURES_FLAG]));
    }

    #[test]
    fn a_rule_asks_cargo_for_the_graph_of_every_target() {
        let target = [TARGET_FLAG, ALL_TARGETS];
        assert!(tree_arguments(RULE)
            .windows(target.len())
            .any(|arguments| arguments == target));
    }
}
