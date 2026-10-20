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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProxySource {
    Explicit,
    Environment,
    Bundled,
    PathLookup { bundled_dir: Option<PathBuf> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyResolution {
    pub path: String,
    pub source: ProxySource,
}

impl std::fmt::Display for ProxyResolution {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "'{}'", self.path)?;
        match &self.source {
            ProxySource::Explicit => write!(f, " (the explicit path for this session)"),
            ProxySource::Environment => write!(f, " (from ${NYM_PROXY_ENV})"),
            ProxySource::Bundled => write!(f, " (bundled beside the executable)"),
            ProxySource::PathLookup { bundled_dir } => {
                write!(
                    f,
                    " (left to the PATH lookup: no explicit path was given, \
                     ${NYM_PROXY_ENV} is unset, and "
                )?;
                match bundled_dir {
                    Some(dir) => write!(
                        f,
                        "no {NYM_PROXY_BINARY_NAME} is bundled in '{}'",
                        dir.display()
                    )?,
                    None => write!(f, "the executable's directory is unknown")?,
                }
                write!(f, ")")
            }
        }
    }
}

/// Resolves the `nym-proxy` binary path from a consumer's [`SpawnHints`]:
/// the explicit override, then [`NYM_PROXY_ENV`], then the hinted bundled
/// directory, then the bare [`NYM_PROXY_BINARY_NAME`] resolved on PATH.
/// An empty explicit or environment value counts as absent.
///
/// This is the one precedence rule of ADR 0024's provisioning decision;
/// consumers pass hints and never re-derive the order.
pub fn resolve_proxy_path(hints: &SpawnHints<'_>) -> ProxyResolution {
    choose_proxy_path(
        hints.explicit,
        std::env::var(NYM_PROXY_ENV).ok(),
        bundled_proxy_path(hints.bundled_dir.as_deref()),
        hints.bundled_dir.clone(),
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
    bundled_dir: Option<PathBuf>,
) -> ProxyResolution {
    if let Some(path) = explicit.filter(|path| !path.is_empty()) {
        return ProxyResolution {
            path: path.to_string(),
            source: ProxySource::Explicit,
        };
    }
    if let Some(path) = env_value.filter(|path| !path.is_empty()) {
        return ProxyResolution {
            path,
            source: ProxySource::Environment,
        };
    }
    if let Some(path) = bundled {
        return ProxyResolution {
            path,
            source: ProxySource::Bundled,
        };
    }
    ProxyResolution {
        path: NYM_PROXY_BINARY_NAME.to_string(),
        source: ProxySource::PathLookup { bundled_dir },
    }
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

    fn resolved(path: &str, source: ProxySource) -> ProxyResolution {
        ProxyResolution {
            path: path.to_string(),
            source,
        }
    }

    #[test]
    fn explicit_wins_over_everything() {
        let chosen = choose_proxy_path(
            Some("/explicit"),
            Some("/from-env".to_string()),
            Some("/bundled".to_string()),
            None,
        );
        assert_eq!(chosen, resolved("/explicit", ProxySource::Explicit));
    }

    #[test]
    fn empty_explicit_counts_as_absent() {
        let chosen = choose_proxy_path(Some(""), Some("/from-env".to_string()), None, None);
        assert_eq!(chosen, resolved("/from-env", ProxySource::Environment));
    }

    #[test]
    fn env_wins_over_bundled() {
        let chosen = choose_proxy_path(
            None,
            Some("/from-env".to_string()),
            Some("/bundled".to_string()),
            None,
        );
        assert_eq!(chosen, resolved("/from-env", ProxySource::Environment));
    }

    #[test]
    fn empty_env_counts_as_absent() {
        let chosen = choose_proxy_path(
            None,
            Some(String::new()),
            Some("/bundled".to_string()),
            None,
        );
        assert_eq!(chosen, resolved("/bundled", ProxySource::Bundled));
    }

    #[test]
    fn bundled_wins_over_bare_name() {
        let chosen = choose_proxy_path(None, None, Some("/bundled".to_string()), None);
        assert_eq!(chosen, resolved("/bundled", ProxySource::Bundled));
    }

    #[test]
    fn bare_name_is_the_final_fallback() {
        let searched = PathBuf::from("/opt/wallet");
        let chosen = choose_proxy_path(None, None, None, Some(searched.clone()));
        assert_eq!(
            chosen,
            resolved(
                NYM_PROXY_BINARY_NAME,
                ProxySource::PathLookup {
                    bundled_dir: Some(searched),
                },
            )
        );
    }

    #[test]
    fn the_fallback_trail_names_every_source_that_was_empty() {
        let chosen = choose_proxy_path(None, None, None, Some(PathBuf::from("/opt/wallet")));
        let trail = chosen.to_string();
        assert!(
            trail.starts_with("'nym-proxy' (left to the PATH lookup"),
            "{trail}"
        );
        assert!(trail.contains("no explicit path was given"), "{trail}");
        assert!(trail.contains("$ZINGO_NYM_PROXY is unset"), "{trail}");
        assert!(
            trail.contains("no nym-proxy is bundled in '/opt/wallet'"),
            "{trail}"
        );
    }

    #[test]
    fn a_found_source_renders_the_path_and_its_origin() {
        let chosen = choose_proxy_path(None, Some("/from-env".to_string()), None, None);
        assert_eq!(chosen.to_string(), "'/from-env' (from $ZINGO_NYM_PROXY)");
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
