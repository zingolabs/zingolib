use std::path::Path;

use crate::{at_path, MANIFEST};

pub const MOBILE_PROFILE: &str = "mobile";

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

pub fn custom_profiles(manifest_name: &str, manifest: &str) -> Result<Vec<String>, Vec<String>> {
    let document: toml_edit::DocumentMut = manifest
        .parse()
        .map_err(|error| vec![format!("{manifest_name} does not parse: {error}")])?;
    Ok(document
        .get(PROFILE_KEY)
        .and_then(toml_edit::Item::as_table_like)
        .map(|profiles| {
            profiles
                .iter()
                .map(|(name, _)| name.to_string())
                .filter(|name| !BUILT_IN_PROFILES.contains(&name.as_str()))
                .collect()
        })
        .unwrap_or_default())
}

/// - Reads the root workspace manifest under `root`.
pub fn built_strategy(root: &Path) -> Result<PanicStrategy, Vec<String>> {
    let text = at_path(&root.join(MANIFEST), "read", |path| {
        std::fs::read_to_string(path)
    })?;
    profile_panic_strategy(MANIFEST, &text, MOBILE_PROFILE)
}

#[cfg(test)]
mod tests;
