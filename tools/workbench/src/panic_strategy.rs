use std::path::Path;

use crate::{at_path, MANIFEST};

pub const MOBILE_PROFILE: &str = "mobile";

pub const PROXY_PROFILE: &str = "proxy";

pub const WALLET_ARCHIVE: &str = "wallet";

pub const PROXY_ARCHIVE: &str = "proxy";

pub const BUILT_PROFILES: [(&str, &str); 2] = [
    (WALLET_ARCHIVE, MOBILE_PROFILE),
    (PROXY_ARCHIVE, PROXY_PROFILE),
];

const PROFILE_KEY: &str = "profile";
const PANIC_KEY: &str = "panic";
const INHERITS_KEY: &str = "inherits";
const UNWIND: &str = "unwind";
const ABORT: &str = "abort";
const BUILT_IN_PROFILES: [&str; 4] = ["dev", "release", "test", "bench"];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PanicStrategy {
    Unwind,
    Abort,
}

impl std::fmt::Display for PanicStrategy {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            PanicStrategy::Unwind => UNWIND,
            PanicStrategy::Abort => ABORT,
        })
    }
}

impl PanicStrategy {
    pub fn rustc_flag(self) -> String {
        format!("-Cpanic={self}")
    }

    fn parse(manifest_name: &str, profile: &str, value: &str) -> Result<Self, Vec<String>> {
        match value {
            UNWIND => Ok(PanicStrategy::Unwind),
            ABORT => Ok(PanicStrategy::Abort),
            other => Err(vec![format!(
                "{manifest_name} sets profile.{profile}.{PANIC_KEY} to {other:?}, not {UNWIND:?} or {ABORT:?}"
            )]),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ProfileStrategy {
    pub archive: &'static str,
    pub profile: &'static str,
    pub strategy: PanicStrategy,
}

/// ```
/// use workbench::panic_strategy::{profile_panic_strategy, PanicStrategy};
///
/// let explicit = "[profile.mobile]\ninherits = \"release\"\npanic = \"abort\"\n";
/// assert_eq!(
///     profile_panic_strategy("Cargo.toml", explicit, "mobile"),
///     Ok(PanicStrategy::Abort)
/// );
///
/// let inherited = "[profile.mobile]\ninherits = \"release\"\n";
/// assert_eq!(
///     profile_panic_strategy("Cargo.toml", inherited, "mobile"),
///     Ok(PanicStrategy::Unwind)
/// );
/// ```
pub fn profile_panic_strategy(
    manifest_name: &str,
    manifest: &str,
    profile: &str,
) -> Result<PanicStrategy, Vec<String>> {
    let document: toml_edit::DocumentMut = manifest
        .parse()
        .map_err(|error| vec![format!("{manifest_name} does not parse: {error}")])?;
    let profiles = document.get(PROFILE_KEY);
    let mut visited: Vec<&str> = Vec::new();
    let mut current = profile;
    loop {
        if visited.contains(&current) {
            return Err(vec![format!(
                "{manifest_name} profile.{current} inherits itself through {}",
                visited.join(" -> ")
            )]);
        }
        visited.push(current);
        let table = profiles.and_then(|item| item.get(current));
        if let Some(value) = table.and_then(|item| item.get(PANIC_KEY)) {
            let value = value.as_str().ok_or_else(|| {
                vec![format!(
                    "{manifest_name} profile.{current}.{PANIC_KEY} is not a string"
                )]
            })?;
            return PanicStrategy::parse(manifest_name, current, value);
        }
        match table.and_then(|item| item.get(INHERITS_KEY)) {
            Some(parent) => {
                current = parent.as_str().ok_or_else(|| {
                    vec![format!(
                        "{manifest_name} profile.{current}.{INHERITS_KEY} is not a string"
                    )]
                })?;
            }
            None if BUILT_IN_PROFILES.contains(&current) => return Ok(PanicStrategy::Unwind),
            None => {
                return Err(vec![format!(
                    "{manifest_name} defines no profile.{current}, and it is not a built-in profile"
                )]);
            }
        }
    }
}

/// - Reads the root workspace manifest under `root`.
pub fn built_strategies(root: &Path) -> Result<Vec<ProfileStrategy>, Vec<String>> {
    let text = at_path(&root.join(MANIFEST), "read", |path| {
        std::fs::read_to_string(path)
    })?;
    BUILT_PROFILES
        .into_iter()
        .map(|(archive, profile)| {
            let strategy = profile_panic_strategy(MANIFEST, &text, profile)?;
            Ok(ProfileStrategy {
                archive,
                profile,
                strategy,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::process::Command;

    use super::{built_strategies, profile_panic_strategy, PanicStrategy, ProfileStrategy};
    use crate::{refused, repo_root, scratch_dir, utf8};

    const MANIFEST_NAME: &str = "Cargo.toml";
    const RUSTC: &str = "rustc";
    const C_COMPILER: &str = "cc";
    const EDITION: &str = "2021";
    const STATICLIB: &str = "staticlib";
    const LINK_LIBRARIES: [&str; 3] = ["-lpthread", "-ldl", "-lm"];
    const WALLET_ARCHIVE_FILE: &str = "libwallet.a";
    const PROXY_ARCHIVE_FILE: &str = "libproxy.a";
    const CLOSED_INPUT_PANIC: &str = "BatchRealMessageReceiver has stopped receiving!";

    const WALLET_SOURCE: &str = r#"
#[no_mangle]
pub extern "C" fn wallet_entry(value: i32) -> i32 {
    if value == 7 {
        panic!("a wallet invariant")
    }
    0
}
"#;

    const PROXY_SOURCE: &str = r#"
#[no_mangle]
pub extern "C" fn proxy_entry() -> i32 {
    match std::panic::catch_unwind(|| panic!("BatchRealMessageReceiver has stopped receiving!")) {
        Err(_contained) => 0,
        Ok(()) => 1,
    }
}
"#;

    const HOST_SOURCE: &str = r#"
int wallet_entry(int);
int proxy_entry(void);
int main(void) { return wallet_entry(1) + proxy_entry(); }
"#;

    fn succeeds(command: &mut Command, what: &str) -> Result<(), Vec<String>> {
        let status = command
            .status()
            .map_err(|error| vec![format!("cannot run {what}: {error}")])?;
        if status.success() {
            Ok(())
        } else {
            Err(vec![format!("{what} failed ({status})")])
        }
    }

    fn write(file: &Path, text: &str) -> Result<(), Vec<String>> {
        std::fs::write(file, text)
            .map_err(|error| vec![format!("cannot write {}: {error}", file.display())])
    }

    fn strategy_of<'a>(
        strategies: &'a [ProfileStrategy],
        archive: &str,
    ) -> Result<&'a ProfileStrategy, Vec<String>> {
        strategies
            .iter()
            .find(|found| found.archive == archive)
            .ok_or_else(|| vec![format!("no {archive} archive among the built profiles")])
    }

    /// - Runs `rustc` in the zingolib root, so rustup applies the pinned toolchain.
    fn build_archive(
        root: &Path,
        source: &Path,
        archive: &Path,
        strategy: PanicStrategy,
    ) -> Result<(), Vec<String>> {
        succeeds(
            Command::new(RUSTC).current_dir(root).args([
                "--crate-type",
                STATICLIB,
                "--edition",
                EDITION,
                &strategy.rustc_flag(),
                "-o",
                utf8(archive)?,
                utf8(source)?,
            ]),
            &format!("rustc on {}", source.display()),
        )
    }

    /// - Runs the C compiler as the linker over the host source and the two archives.
    fn link(
        host_source: &Path,
        archives: [&Path; 2],
        executable: &Path,
    ) -> Result<(), Vec<String>> {
        succeeds(
            Command::new(C_COMPILER)
                .arg(host_source)
                .args(archives)
                .args(LINK_LIBRARIES)
                .arg("-o")
                .arg(executable),
            &format!("the link of {}", executable.display()),
        )
    }

    #[test]
    fn a_profile_that_inherits_itself_is_refused() -> Result<(), Vec<String>> {
        let cyclic =
            "[profile.mobile]\ninherits = \"slim\"\n[profile.slim]\ninherits = \"mobile\"\n";
        let diagnostic = refused(profile_panic_strategy(MANIFEST_NAME, cyclic, "mobile"))?.concat();
        assert!(diagnostic.contains("inherits itself"), "{diagnostic}");
        Ok(())
    }

    #[test]
    fn a_custom_profile_without_a_parent_is_refused() -> Result<(), Vec<String>> {
        let diagnostic = refused(profile_panic_strategy(MANIFEST_NAME, "", "mobile"))?.concat();
        assert!(
            diagnostic.contains("defines no profile.mobile"),
            "{diagnostic}"
        );
        Ok(())
    }

    #[test]
    fn an_unknown_panic_value_is_refused() -> Result<(), Vec<String>> {
        let odd = "[profile.mobile]\ninherits = \"release\"\npanic = \"ignore\"\n";
        let diagnostic = refused(profile_panic_strategy(MANIFEST_NAME, odd, "mobile"))?.concat();
        assert!(diagnostic.contains("\"ignore\""), "{diagnostic}");
        Ok(())
    }

    #[test]
    fn a_panic_set_on_the_parent_profile_is_inherited() -> Result<(), Vec<String>> {
        let through_release =
            "[profile.release]\npanic = \"abort\"\n[profile.mobile]\ninherits = \"release\"\n";
        assert_eq!(
            profile_panic_strategy(MANIFEST_NAME, through_release, "mobile")?,
            PanicStrategy::Abort
        );
        Ok(())
    }

    #[test]
    fn both_built_profiles_ship_the_unwind_strategy() -> Result<(), Vec<String>> {
        let strategies = built_strategies(&repo_root()?)?;
        let aborting: Vec<String> = strategies
            .iter()
            .filter(|found| found.strategy != PanicStrategy::Unwind)
            .map(|found| format!("{} ({})", found.profile, found.strategy))
            .collect();
        assert!(
            aborting.is_empty(),
            "the profile aborts on panic for {}; the iOS archive carries both crates under the mobile profile and the Android proxy library ships under the proxy profile, and a panic in a tokio task of the proxy must unwind under both, or it aborts the app (issue #2926)",
            aborting.join(", ")
        );
        Ok(())
    }

    #[test]
    fn the_two_archives_build_under_distinct_profiles() -> Result<(), Vec<String>> {
        let strategies = built_strategies(&repo_root()?)?;
        let wallet = strategy_of(&strategies, super::WALLET_ARCHIVE)?;
        let proxy = strategy_of(&strategies, super::PROXY_ARCHIVE)?;
        assert_ne!(
            wallet.profile, proxy.profile,
            "the guard compares one profile against itself and can never detect divergence"
        );
        Ok(())
    }

    /// - Writes three source files to a scratch directory under the system temp dir.
    /// - Runs `rustc` twice and the C compiler twice, then runs the two linked executables.
    #[test]
    fn a_proxy_panic_is_contained_whichever_archive_links_first() -> Result<(), Vec<String>> {
        let root = repo_root()?;
        let strategies = built_strategies(&root)?;
        let wallet = strategy_of(&strategies, super::WALLET_ARCHIVE)?.strategy;
        let proxy = strategy_of(&strategies, super::PROXY_ARCHIVE)?.strategy;

        let scratch = scratch_dir("panic-strategy")?;
        let wallet_source = scratch.join("wallet.rs");
        let proxy_source = scratch.join("proxy.rs");
        let host_source = scratch.join("host.c");
        write(&wallet_source, WALLET_SOURCE)?;
        write(&proxy_source, PROXY_SOURCE)?;
        write(&host_source, HOST_SOURCE)?;
        assert!(PROXY_SOURCE.contains(CLOSED_INPUT_PANIC));

        let wallet_archive = scratch.join(WALLET_ARCHIVE_FILE);
        let proxy_archive = scratch.join(PROXY_ARCHIVE_FILE);
        build_archive(&root, &wallet_source, &wallet_archive, wallet)?;
        build_archive(&root, &proxy_source, &proxy_archive, proxy)?;

        let orders = [
            (
                "wallet_first",
                [wallet_archive.as_path(), proxy_archive.as_path()],
            ),
            (
                "proxy_first",
                [proxy_archive.as_path(), wallet_archive.as_path()],
            ),
        ];
        for (name, archives) in orders {
            let executable = scratch.join(name);
            link(&host_source, archives, &executable)?;
            let status = Command::new(&executable)
                .status()
                .map_err(|error| vec![format!("cannot run {}: {error}", executable.display())])?;
            assert!(
                status.success(),
                "with the {name} link order the proxy's panic ended the process ({status}); the wallet archive carries {} and the proxy archive carries {}",
                wallet,
                proxy
            );
        }
        Ok(())
    }
}
