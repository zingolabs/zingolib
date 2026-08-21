#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use workbench::{git, read, repo_root, run};

/// The blessed entries, one per line, relative to the repository root.
const BLESSING_PATH: &str = "tools/workbench/feature-census-blessed.txt";

/// Separates a blessed entry's key from the reason it was blessed.
const BLESSING_SEPARATOR: &str = "  ";

/// Joins a candidate's crate, dependency, and feature into its one key.
const KEY_SEPARATOR: &str = "::";

/// The build directory the census keeps apart from an ordinary check.
const CENSUS_TARGET_DIR: &str = "target/census";

/// The branch a census compares against when the caller names no base.
const DEFAULT_BASE: &str = "origin/dev";

/// The fallback base for a checkout whose remote branch is absent.
const FALLBACK_BASE: &str = "dev";

/// The manifest filename every crate directory carries.
const MANIFEST: &str = "Cargo.toml";

/// The key a dependency's feature list is spelled under.
const FEATURES_KEY: &str = "features = [";

/// The feature set a crate is probed under where its whole set is
/// unaffordable, because probing once per feature would resolve and build
/// that set once per feature.
const PROBE_FEATURES: [(&str, &str); 1] = [(
    // `nym` resolves the nym-sdk stack in this crate's own lockfile, which is
    // minutes of build per probe; the light features cover the fetch and the
    // transmit legs, which is where this crate's own feature declarations are.
    "zingo-netutils",
    "socks5-transmit,socks5-fetch,testutils",
)];

/// One dependency feature the census can probe.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Candidate {
    /// The crate directory whose manifest declares it, or `.` for the root.
    krate: String,
    /// The dependency the feature is enabled on.
    dependency: String,
    /// The feature itself.
    feature: String,
    /// Where the enclosing feature list's body starts in the manifest text.
    body_start: usize,
    /// Where that body ends, at its closing bracket.
    body_end: usize,
}

impl Candidate {
    /// The one key a blessing names this candidate by.
    fn key(&self) -> String {
        [
            self.krate.as_str(),
            self.dependency.as_str(),
            self.feature.as_str(),
        ]
        .join(KEY_SEPARATOR)
    }
}

/// What the caller asked the census to cover.
enum Scope {
    /// Every manifest in the repository.
    Everything,
    /// One manifest per crate the branch touches.
    Touched(Vec<PathBuf>),
}

/// What one command line asks the census to do.
struct Request {
    all: bool,
    bless: bool,
    base: String,
    crates: Vec<String>,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    run("feature-census", || census(&args), |()| {})
}

/// Probes each declared dependency feature and reports the ones nothing needs.
fn census(args: &[String]) -> Result<(), Vec<String>> {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_usage();
        return Ok(());
    }

    let root = repo_root()?;
    let request = parse(args)?;
    let manifests = match scope(&root, &request)? {
        Scope::Everything => every_manifest(&root),
        Scope::Touched(paths) => paths,
    };

    if manifests.is_empty() {
        println!("feature-census: no manifest is touched; nothing to probe");
        return Ok(());
    }

    let mut unneeded = Vec::new();
    for manifest in &manifests {
        println!("feature-census: {}", display_relative(&root, manifest));
        unneeded.extend(probe_manifest(&root, manifest)?);
    }
    unneeded.sort();

    if request.bless {
        return bless(&root, &unneeded);
    }
    judge(&root, &unneeded)
}

/// Prints how to call the census and what each argument selects.
fn print_usage() {
    println!("usage: feature-census [--all] [--base <ref>] [--bless] [<crate-dir>...]");
    println!();
    println!("Removes each declared dependency feature in turn and checks the crate");
    println!("without it. A feature whose removal still compiles is reported, because");
    println!("nothing in this workspace needs the API it adds.");
    println!();
    println!("A feature that changes behaviour without changing the API compiles away");
    println!("just the same, so the report is a question, not a verdict. Answer it once");
    println!("by blessing the feature with a reason, and the census stays quiet after.");
    println!();
    println!("  --all          probe every manifest in the repository");
    println!("  --base <ref>   compare against <ref> instead of {DEFAULT_BASE}");
    println!("  --bless        rewrite the blessing file to today's report");
    println!("  <crate-dir>    probe these crates and no others");
    println!();
    println!("With no argument the census probes the crates this branch touches.");
}

/// Reads a command line, refusing an unknown flag or a `--base` without its reference.
fn parse(args: &[String]) -> Result<Request, Vec<String>> {
    let mut request = Request {
        all: false,
        bless: false,
        base: DEFAULT_BASE.to_string(),
        crates: Vec::new(),
    };
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "--" {
            continue;
        } else if arg == "--all" {
            request.all = true;
        } else if arg == "--bless" {
            request.bless = true;
        } else if let Some(reference) = arg.strip_prefix("--base=") {
            request.base = reference.to_string();
        } else if arg == "--base" {
            request.base = iter
                .next()
                .cloned()
                .ok_or_else(|| vec!["--base requires a reference argument".to_string()])?;
        } else if arg.starts_with('-') {
            return Err(vec![format!("unknown argument '{arg}'")]);
        } else {
            request.crates.push(arg.clone());
        }
    }
    Ok(request)
}

/// Reads the request into the set of manifests the census will probe.
fn scope(root: &Path, request: &Request) -> Result<Scope, Vec<String>> {
    if request.all {
        return Ok(Scope::Everything);
    }
    if !request.crates.is_empty() {
        let mut manifests = Vec::new();
        for name in &request.crates {
            let manifest = root.join(name).join(MANIFEST);
            if !manifest.is_file() {
                return Err(vec![format!("no crate at {}", manifest.display())]);
            }
            manifests.push(manifest);
        }
        return Ok(Scope::Touched(manifests));
    }
    Ok(Scope::Touched(touched_manifests(root, &request.base)?))
}

/// Every manifest the census knows how to probe, root first.
fn every_manifest(root: &Path) -> Vec<PathBuf> {
    let mut manifests = vec![root.join(MANIFEST)];
    let Ok(entries) = std::fs::read_dir(root) else {
        return manifests;
    };
    let mut members: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path().join(MANIFEST))
        .filter(|manifest| manifest.is_file())
        .collect();
    members.sort();
    manifests.extend(members);
    manifests
}

/// The manifests of the crates this branch touches, against `base`.
fn touched_manifests(root: &Path, base: &str) -> Result<Vec<PathBuf>, Vec<String>> {
    let reference = if git(&["rev-parse", "--verify", "--quiet", base]).is_ok() {
        base.to_string()
    } else {
        FALLBACK_BASE.to_string()
    };
    let changed = git(&["diff", "--name-only", &format!("{reference}...HEAD")])?;
    let mut manifests: Vec<PathBuf> = Vec::new();
    for line in changed.lines() {
        let owner = match line.split_once('/') {
            Some((directory, _)) => root.join(directory).join(MANIFEST),
            None => root.join(MANIFEST),
        };
        if owner.is_file() && !manifests.contains(&owner) {
            manifests.push(owner);
        }
    }
    manifests.sort();
    Ok(manifests)
}

/// Every declared dependency feature in `manifest` whose removal still compiles.
fn probe_manifest(root: &Path, manifest: &Path) -> Result<Vec<Candidate>, Vec<String>> {
    let krate = crate_name(root, manifest);
    let original = read(manifest)?;
    let mut unneeded = Vec::new();

    for candidate in declared(&krate, &original) {
        let Some(without) = manifest_without(&original, &candidate) else {
            continue;
        };
        write(manifest, &without)?;
        let compiled = check(root, manifest, &krate);
        write(manifest, &original)?;
        if compiled? {
            println!("  unneeded: {}", candidate.key());
            unneeded.push(candidate);
        }
    }
    Ok(unneeded)
}

/// Every dependency feature `text` declares, in declaration order.
fn declared(krate: &str, text: &str) -> Vec<Candidate> {
    let mut declared = Vec::new();
    let mut cursor = 0;
    while let Some(offset) = text[cursor..].find(FEATURES_KEY) {
        let start = cursor + offset;
        cursor = start + FEATURES_KEY.len();
        if !opens_a_key(text, start) {
            continue;
        }
        let Some(end) = text[cursor..].find(']') else {
            break;
        };
        let dependency = owner(text, start);
        let body_start = cursor;
        let body_end = cursor + end;
        for feature in quoted(&text[body_start..body_end]) {
            declared.push(Candidate {
                krate: krate.to_string(),
                dependency: dependency.clone(),
                feature,
                body_start,
                body_end,
            });
        }
        cursor = body_end;
    }
    declared
}

/// Whether the `features` at `start` opens its own key rather than ending another.
fn opens_a_key(text: &str, start: usize) -> bool {
    match text[..start].chars().next_back() {
        None => true,
        Some(previous) => !previous.is_alphanumeric() && previous != '-' && previous != '_',
    }
}

/// The dependency whose table encloses the feature list at `start`.
fn owner(text: &str, start: usize) -> String {
    for line in text[..start].lines().rev() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix('[') {
            let table = rest.trim_end_matches(']');
            return table
                .rsplit('.')
                .next()
                .unwrap_or(table)
                .trim_matches('"')
                .to_string();
        }
        if let Some((name, value)) = trimmed.split_once('=') {
            if value.trim_start().starts_with('{') {
                return name.trim().trim_matches('"').to_string();
            }
        }
    }
    String::new()
}

/// Every double-quoted item in one array body.
fn quoted(body: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut rest = body;
    while let Some(open) = rest.find('"') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('"') else { break };
        items.push(after[..close].to_string());
        rest = &after[close + 1..];
    }
    items
}

/// `text` with `candidate`'s one feature item removed, or `None` if absent.
///
/// The enclosing list is rewritten as one line of the surviving items, which
/// reformats the manifest for as long as the probe holds it. Every probe reads
/// the original text and restores it afterwards, so the reformatting never
/// outlives the one `cargo check` it was made for.
fn manifest_without(text: &str, candidate: &Candidate) -> Option<String> {
    if candidate.body_end > text.len() || candidate.body_start > candidate.body_end {
        return None;
    }
    let body = &text[candidate.body_start..candidate.body_end];
    let mut kept: Vec<String> = Vec::new();
    let mut removed = false;
    for item in quoted(body) {
        if !removed && item == candidate.feature {
            removed = true;
            continue;
        }
        kept.push(format!("\"{item}\""));
    }
    if !removed {
        return None;
    }
    let mut out = String::with_capacity(text.len());
    out.push_str(&text[..candidate.body_start]);
    out.push_str(&kept.join(", "));
    out.push_str(&text[candidate.body_end..]);
    Some(out)
}

/// Writes `text` to `path`, or a one-line diagnostic on failure.
fn write(path: &Path, text: &str) -> Result<(), Vec<String>> {
    std::fs::write(path, text).map_err(|e| vec![format!("cannot write {}: {e}", path.display())])
}

/// Whether the crate still compiles, with every target and the probe's features.
fn check(root: &Path, manifest: &Path, krate: &str) -> Result<bool, Vec<String>> {
    let mut command = Command::new("cargo");
    command
        .current_dir(root)
        .env("CARGO_TARGET_DIR", root.join(CENSUS_TARGET_DIR))
        .arg("check")
        .arg("--quiet")
        .arg("--all-targets");
    if manifest == root.join(MANIFEST) {
        command.arg("--workspace");
    } else {
        command.arg("--manifest-path").arg(manifest);
    }
    match PROBE_FEATURES.iter().find(|(name, _)| *name == krate) {
        Some((_, features)) => {
            command.arg("--features").arg(features);
        }
        None => {
            command.arg("--all-features");
        }
    }
    let status = command
        .status()
        .map_err(|e| vec![format!("failed to run cargo check: {e}")])?;
    Ok(status.success())
}

/// The crate directory a manifest belongs to, or `.` for the repository root.
fn crate_name(root: &Path, manifest: &Path) -> String {
    match manifest.parent() {
        Some(parent) if parent == root => ".".to_string(),
        Some(parent) => parent
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default(),
        None => ".".to_string(),
    }
}

/// A path as the repository sees it.
fn display_relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .to_string()
}

/// The blessed keys and the reasons they carry.
fn blessings(root: &Path) -> Result<BTreeMap<String, String>, Vec<String>> {
    let path = root.join(BLESSING_PATH);
    if !path.is_file() {
        return Ok(BTreeMap::new());
    }
    let mut blessed = BTreeMap::new();
    for line in read(&path)?.lines() {
        let entry = line.trim();
        if entry.is_empty() || entry.starts_with('#') {
            continue;
        }
        let (key, reason) = entry.split_once(BLESSING_SEPARATOR).unwrap_or((entry, ""));
        blessed.insert(key.trim().to_string(), reason.trim().to_string());
    }
    Ok(blessed)
}

/// Refuses any unneeded feature no blessing answers for.
fn judge(root: &Path, unneeded: &[Candidate]) -> Result<(), Vec<String>> {
    let blessed = blessings(root)?;
    let unanswered: Vec<&Candidate> = unneeded
        .iter()
        .filter(|candidate| !blessed.contains_key(&candidate.key()))
        .collect();

    if unanswered.is_empty() {
        println!("feature-census: every declared feature is needed or blessed");
        return Ok(());
    }

    let mut lines = vec![format!(
        "{} declared feature(s) compile away with nothing needing them:",
        unanswered.len()
    )];
    for candidate in unanswered {
        lines.push(format!("  {}", candidate.key()));
    }
    lines.push(String::new());
    lines.push("Remove each one, or bless it with the reason it must stay:".to_string());
    lines.push(format!("  {BLESSING_PATH}"));
    Err(lines)
}

/// Rewrites the blessing file to today's report, keeping the reasons already given.
fn bless(root: &Path, unneeded: &[Candidate]) -> Result<(), Vec<String>> {
    let existing = blessings(root)?;
    let mut out = String::new();
    out.push_str("# Dependency features that compile away but must stay.\n");
    out.push_str(
        "# One entry per line: <crate>::<dependency>::<feature>, two spaces, the reason.\n",
    );
    out.push_str("# Rewrite with `cargo run --bin feature-census -- --all --bless`.\n\n");
    for candidate in unneeded {
        let key = candidate.key();
        let reason = existing
            .get(&key)
            .filter(|reason| !reason.is_empty())
            .cloned()
            .unwrap_or_else(|| "TODO: say why this feature must stay".to_string());
        out.push_str(&key);
        out.push_str(BLESSING_SEPARATOR);
        out.push_str(&reason);
        out.push('\n');
    }
    write(&root.join(BLESSING_PATH), &out)?;
    println!(
        "feature-census: blessed {} feature(s) into {BLESSING_PATH}",
        unneeded.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A manifest's inline dependency tables yield one candidate per feature,
    /// each named by the dependency whose table encloses it.
    #[test]
    fn declared_names_each_feature_by_its_dependency() {
        let manifest = r#"
[dependencies]
reqwest = { workspace = true, default-features = false, features = [
    "json",
    "socks",
] }
serde = { workspace = true, features = ["derive"] }
"#;
        let found = declared("zingo-price", manifest);
        let keys: Vec<String> = found.iter().map(Candidate::key).collect();
        assert_eq!(
            keys,
            vec![
                "zingo-price::reqwest::json",
                "zingo-price::reqwest::socks",
                "zingo-price::serde::derive",
            ]
        );
    }

    /// `default-features` never reads as a feature list of its own, so a
    /// dependency that disables defaults contributes no phantom candidate.
    #[test]
    fn default_features_is_not_a_feature_list() {
        let manifest = "http = { version = \"1\", default-features = false }\n";
        assert_eq!(declared("zingolib", manifest), Vec::new());
    }

    /// Removal keeps every other item whether the list was written across
    /// lines or all on one, which is what a single-line list needs.
    #[test]
    fn removal_keeps_every_other_item() {
        for manifest in [
            "reqwest = { features = [\n    \"json\",\n    \"socks\",\n] }\n",
            "reqwest = { features = [\"json\", \"socks\"] }\n",
        ] {
            let candidate = declared("zingo-price", manifest)
                .into_iter()
                .find(|found| found.feature == "json")
                .expect("json is declared");
            let without = manifest_without(manifest, &candidate).expect("the feature is present");
            assert_eq!(
                without, "reqwest = { features = [\"socks\"] }\n",
                "only the named feature leaves, in {manifest:?}"
            );
        }
    }

    /// A feature the manifest does not declare cannot be removed from it.
    #[test]
    fn an_absent_feature_removes_nothing() {
        let candidate = Candidate {
            krate: "zingo-price".to_string(),
            dependency: "reqwest".to_string(),
            feature: "cookies".to_string(),
            body_start: 0,
            body_end: 0,
        };
        assert_eq!(manifest_without("reqwest = { }\n", &candidate), None);
    }
}
