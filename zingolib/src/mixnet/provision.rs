//! Provisioning the mixnet transport: resolving where the `nym-proxy`
//! binary is, one precedence rule for every consumer (ADR 0024).
#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

/// The environment variable naming a `nym-proxy` binary, consulted between
/// an explicit override and the bundled location.
///
/// This name is a MINTED TOKEN and this constant is its sole production
/// site: consumers read the variable through [`resolve_proxy_path`] or, at
/// minimum, through this constant, and never restate the string in their
/// own code. There is no other enforcement mechanism — no test can catch a
/// restated environment-variable name in an out-of-repo consumer — so the
/// monopoly here is documentary: restating the name is how consumers
/// drifted before ADR 0024, and importing it is the whole fix.
pub const NYM_PROXY_ENV: &str = "ZINGO_NYM_PROXY";

/// The bare binary name resolved on PATH as the final fallback, with the
/// platform's executable suffix for the bundled lookup. Minted here for the
/// same reason as [`NYM_PROXY_ENV`]: the `bundle-nym-proxy` workbench tool
/// and every consumer's packaging must agree on this name, and agreement by
/// restatement is what this constant retires.
pub const NYM_PROXY_BINARY_NAME: &str = "nym-proxy";

/// Platform facts a consumer supplies to [`resolve_proxy_path`].
#[derive(Debug, Default)]
pub struct SpawnHints<'a> {
    /// An explicit path the user supplied this session (a `--nym-proxy`
    /// flag or its UI equivalent). Highest precedence; an empty string
    /// counts as absent.
    pub explicit: Option<&'a str>,
    /// The platform's bundled-binary directory, searched for
    /// [`NYM_PROXY_BINARY_NAME`] with the platform executable suffix.
    pub bundled_dir: Option<PathBuf>,
}

/// Resolves the `nym-proxy` binary path from a consumer's [`SpawnHints`]:
/// the explicit override, then [`NYM_PROXY_ENV`], then the hinted bundled
/// directory, then the bare [`NYM_PROXY_BINARY_NAME`] resolved on PATH.
/// An empty explicit or environment value counts as absent.
///
/// This is the one precedence rule of ADR 0024's provisioning decision;
/// consumers pass hints and never re-derive the order.
pub fn resolve_proxy_path(hints: &SpawnHints<'_>) -> String {
    choose_proxy_path(
        hints.explicit,
        std::env::var(NYM_PROXY_ENV).ok(),
        bundled_proxy_path(hints.bundled_dir.as_deref()),
    )
}

/// The directory holding the running executable.
pub fn executable_sibling_dir() -> Option<PathBuf> {
    Some(std::env::current_exe().ok()?.parent()?.to_path_buf())
}

/// Picks the proxy path from the three candidate sources
/// [`resolve_proxy_path`] gathers, with no I/O of its own.
fn choose_proxy_path(
    explicit: Option<&str>,
    env_value: Option<String>,
    bundled: Option<String>,
) -> String {
    if let Some(path) = explicit.filter(|path| !path.is_empty()) {
        return path.to_string();
    }
    if let Some(path) = env_value.filter(|path| !path.is_empty()) {
        return path;
    }
    if let Some(bundled) = bundled {
        return bundled;
    }
    NYM_PROXY_BINARY_NAME.to_string()
}

/// The proxy binary inside the hinted bundled directory, if present.
fn bundled_proxy_path(bundled_dir: Option<&Path>) -> Option<String> {
    let candidate = bundled_dir?.join(format!(
        "{NYM_PROXY_BINARY_NAME}{}",
        std::env::consts::EXE_SUFFIX
    ));
    candidate
        .is_file()
        .then(|| candidate.to_string_lossy().into_owned())
}

#[cfg(test)]
mod precedence {
    use super::*;

    #[test]
    fn explicit_wins_over_everything() {
        let chosen = choose_proxy_path(
            Some("/explicit"),
            Some("/from-env".to_string()),
            Some("/bundled".to_string()),
        );
        assert_eq!(chosen, "/explicit");
    }

    #[test]
    fn empty_explicit_counts_as_absent() {
        let chosen = choose_proxy_path(Some(""), Some("/from-env".to_string()), None);
        assert_eq!(chosen, "/from-env");
    }

    #[test]
    fn env_wins_over_bundled() {
        let chosen = choose_proxy_path(
            None,
            Some("/from-env".to_string()),
            Some("/bundled".to_string()),
        );
        assert_eq!(chosen, "/from-env");
    }

    #[test]
    fn empty_env_counts_as_absent() {
        let chosen = choose_proxy_path(None, Some(String::new()), Some("/bundled".to_string()));
        assert_eq!(chosen, "/bundled");
    }

    #[test]
    fn bundled_wins_over_bare_name() {
        let chosen = choose_proxy_path(None, None, Some("/bundled".to_string()));
        assert_eq!(chosen, "/bundled");
    }

    #[test]
    fn bare_name_is_the_final_fallback() {
        assert_eq!(choose_proxy_path(None, None, None), NYM_PROXY_BINARY_NAME);
    }

    #[test]
    fn bundled_lookup_requires_the_file_to_exist() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        assert_eq!(bundled_proxy_path(Some(dir.path())), None);

        let name = format!("{NYM_PROXY_BINARY_NAME}{}", std::env::consts::EXE_SUFFIX);
        std::fs::write(dir.path().join(&name), b"").expect("touch the candidate");
        let found = bundled_proxy_path(Some(dir.path())).expect("the candidate is found");
        assert!(found.ends_with(&name));
    }

    #[test]
    fn no_bundled_dir_hint_means_no_bundled_candidate() {
        assert_eq!(bundled_proxy_path(None), None);
    }
}
