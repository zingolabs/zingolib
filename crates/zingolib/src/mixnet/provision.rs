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
mod precedence;
