#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use toml_edit::{Array, DocumentMut, Item, Value};
use workbench::{
    display_relative, merge_base, read, repo_root, run, touched_manifests, workspace_manifests,
    workspace_members, DEFAULT_BASE, MANIFEST,
};

/// The blessed entries, one per line, relative to the repository root.
const BLESSING_PATH: &str = "tools/workbench/feature-census-blessed.txt";

/// Separates a blessed entry's key from the reason it was blessed.
const BLESSING_SEPARATOR: &str = "  ";

/// Joins a candidate's crate, dependency, and feature into its one key.
const KEY_SEPARATOR: &str = "::";

/// The build directory the census keeps apart from an ordinary check.
const CENSUS_TARGET_DIR: &str = "target/census";

const DEPENDENCIES: &str = "dependencies";
const DEPENDENCY_TABLES: [&str; 3] = [DEPENDENCIES, "dev-dependencies", "build-dependencies"];
const WORKSPACE_TABLE: &str = "workspace";
const TARGET_TABLE: &str = "target";
const FEATURES: &str = "features";

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
    crate_dir: String,
    /// The dependency the feature is enabled on.
    dependency: String,
    /// The feature itself.
    feature: String,
    /// The keys from the manifest's root to the dependency's table.
    path: Vec<String>,
}

impl Candidate {
    /// The one key a blessing names this candidate by.
    fn key(&self) -> String {
        [
            self.crate_dir.as_str(),
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
        Scope::Everything => every_manifest(&root)?,
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
    Ok(Scope::Touched(touched_manifests(
        root,
        &merge_base(root, &request.base)?,
    )?))
}

/// - Runs `git ls-files` and `cargo tree` through the library, once per workspace.
fn every_manifest(root: &Path) -> Result<Vec<PathBuf>, Vec<String>> {
    let mut manifests = Vec::new();
    for workspace in workspace_manifests(root)? {
        manifests.extend(workspace_members(&workspace)?);
        manifests.push(workspace);
    }
    manifests.sort();
    manifests.dedup();
    Ok(manifests)
}

/// Every declared dependency feature in `manifest` whose removal still compiles.
fn probe_manifest(root: &Path, manifest: &Path) -> Result<Vec<Candidate>, Vec<String>> {
    let crate_dir = crate_name(root, manifest);
    let original = read(manifest)?;
    let mut unneeded = Vec::new();

    for candidate in declared(&crate_dir, &original)? {
        let Some(without) = manifest_without(&original, &candidate) else {
            continue;
        };
        write(manifest, &without)?;
        let compiled = check(root, manifest, &crate_dir);
        write(manifest, &original)?;
        if compiled? {
            println!("  unneeded: {}", candidate.key());
            unneeded.push(candidate);
        }
    }
    Ok(unneeded)
}

fn parsed(text: &str) -> Result<DocumentMut, Vec<String>> {
    text.parse()
        .map_err(|e| vec![format!("the manifest is not TOML: {e}")])
}

fn keys_of(item: &Item) -> Vec<String> {
    match item {
        Item::Table(table) => table.iter().map(|(key, _)| key.to_string()).collect(),
        Item::Value(Value::InlineTable(table)) => {
            table.iter().map(|(key, _)| key.to_string()).collect()
        }
        _ => Vec::new(),
    }
}

fn item_at<'a>(root: &'a mut Item, path: &[String]) -> Option<&'a mut Item> {
    path.iter().try_fold(root, |item, key| item.get_mut(key))
}

fn feature_array(dependency: &mut Item) -> Option<&mut Array> {
    match dependency {
        Item::Table(table) => table.get_mut(FEATURES)?.as_array_mut(),
        Item::Value(Value::InlineTable(table)) => table.get_mut(FEATURES)?.as_array_mut(),
        _ => None,
    }
}

/// The key paths of every table that holds dependencies: the three kinds at the
/// root, the workspace's, and the three kinds under each target.
fn dependency_tables(root: &Item) -> Vec<Vec<String>> {
    let mut tables: Vec<Vec<String>> = DEPENDENCY_TABLES
        .iter()
        .map(|table| vec![table.to_string()])
        .collect();
    tables.push(vec![WORKSPACE_TABLE.to_string(), DEPENDENCIES.to_string()]);
    if let Some(targets) = root.get(TARGET_TABLE) {
        for target in keys_of(targets) {
            for table in DEPENDENCY_TABLES {
                tables.push(vec![
                    TARGET_TABLE.to_string(),
                    target.clone(),
                    table.to_string(),
                ]);
            }
        }
    }
    tables
}

/// Every dependency feature `text` declares, in declaration order.
fn declared(crate_dir: &str, text: &str) -> Result<Vec<Candidate>, Vec<String>> {
    let mut document = parsed(text)?;
    let root = document.as_item_mut();
    let mut declared = Vec::new();
    for table in dependency_tables(root) {
        let dependencies = match item_at(root, &table) {
            Some(dependencies) => keys_of(dependencies),
            None => continue,
        };
        for dependency in dependencies {
            let mut path = table.clone();
            path.push(dependency.clone());
            let features: Vec<String> = match item_at(root, &path).and_then(feature_array) {
                Some(features) => features
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect(),
                None => continue,
            };
            for feature in features {
                declared.push(Candidate {
                    crate_dir: crate_dir.to_string(),
                    dependency: dependency.clone(),
                    feature,
                    path: path.clone(),
                });
            }
        }
    }
    Ok(declared)
}

/// `text` with `candidate`'s one feature item removed, or `None` if absent.
fn manifest_without(text: &str, candidate: &Candidate) -> Option<String> {
    let mut document = parsed(text).ok()?;
    let features = item_at(document.as_item_mut(), &candidate.path).and_then(feature_array)?;
    let position = features
        .iter()
        .position(|value| value.as_str() == Some(candidate.feature.as_str()))?;
    features.remove(position);
    Some(document.to_string())
}

/// Writes `text` to `path`, or a one-line diagnostic on failure.
fn write(path: &Path, text: &str) -> Result<(), Vec<String>> {
    std::fs::write(path, text).map_err(|e| vec![format!("cannot write {}: {e}", path.display())])
}

/// Whether the crate still compiles, with every target and the probe's features.
fn check(root: &Path, manifest: &Path, crate_dir: &str) -> Result<bool, Vec<String>> {
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
    match PROBE_FEATURES.iter().find(|(name, _)| *name == crate_dir) {
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

    #[test]
    fn every_manifest_reaches_the_nested_workspace_members() {
        let root = repo_root().unwrap();
        let manifests = every_manifest(&root).unwrap();
        for member in ["zingo-ffi/lib", "zingo-ffi/uniffi-bindgen", "zingo-cli"] {
            assert!(
                manifests.contains(&root.join(member).join(MANIFEST)),
                "{member} is a workspace member the census must probe"
            );
        }
        assert!(manifests.contains(&root.join(MANIFEST)));
    }

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
        let found = declared("zingo-price", manifest).unwrap();
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

    #[test]
    fn a_commented_out_feature_list_declares_nothing() {
        let manifest =
            "[dependencies]\n# reqwest = { features = [\"json\"] }\nserde = { workspace = true }\n";
        assert_eq!(declared("zingo-price", manifest).unwrap(), Vec::new());
    }

    #[test]
    fn workspace_and_target_dependencies_declare_features_too() {
        let manifest = "[workspace.dependencies]\nhttp = { version = \"1\", features = [\"std\"] }\n\n\
                        [target.'cfg(unix)'.dependencies]\nlibc = { version = \"0.2\", features = [\"extra_traits\"] }\n\n\
                        [dependencies.reqwest]\nversion = \"0.12\"\nfeatures = [\"json\"]\n";
        let keys: Vec<String> = declared(".", manifest)
            .unwrap()
            .iter()
            .map(Candidate::key)
            .collect();
        assert_eq!(
            keys,
            vec![".::reqwest::json", ".::http::std", ".::libc::extra_traits"]
        );
    }

    #[test]
    fn a_bracket_inside_a_comment_does_not_end_the_list() {
        let manifest =
            "[dependencies]\nreqwest = { features = [\n    \"json\", # ]\n    \"socks\",\n] }\n";
        let features: Vec<String> = declared("zingo-price", manifest)
            .unwrap()
            .into_iter()
            .map(|candidate| candidate.feature)
            .collect();
        assert_eq!(features, vec!["json".to_string(), "socks".to_string()]);
    }

    /// `default-features` never reads as a feature list of its own, so a
    /// dependency that disables defaults contributes no phantom candidate.
    #[test]
    fn default_features_is_not_a_feature_list() {
        let manifest = "[dependencies]\nhttp = { version = \"1\", default-features = false }\n";
        assert_eq!(declared("zingolib", manifest).unwrap(), Vec::new());
    }

    /// Removal keeps every other item and every other line of the manifest,
    /// whether the list was written across lines or all on one.
    #[test]
    fn removal_keeps_every_other_item() {
        for manifest in [
            "[dependencies]\nreqwest = { features = [\n    \"json\",\n    \"socks\",\n] }\nserde = \"1\"\n",
            "[dependencies]\nreqwest = { features = [\"json\", \"socks\"] }\nserde = \"1\"\n",
        ] {
            let candidate = declared("zingo-price", manifest)
                .unwrap()
                .into_iter()
                .find(|found| found.feature == "json")
                .expect("json is declared");
            let without = manifest_without(manifest, &candidate).expect("the feature is present");
            let remaining: Vec<String> = declared("zingo-price", &without)
                .unwrap()
                .into_iter()
                .map(|found| found.feature)
                .collect();
            assert_eq!(remaining, vec!["socks".to_string()], "in {manifest:?}");
            assert!(without.ends_with("serde = \"1\"\n"), "the rest of {manifest:?} stays");
        }
    }

    /// A feature the manifest does not declare cannot be removed from it.
    #[test]
    fn an_absent_feature_removes_nothing() {
        let candidate = Candidate {
            crate_dir: "zingo-price".to_string(),
            dependency: "reqwest".to_string(),
            feature: "cookies".to_string(),
            path: vec![DEPENDENCIES.to_string(), "reqwest".to_string()],
        };
        assert_eq!(
            manifest_without("[dependencies]\nreqwest = { }\n", &candidate),
            None
        );
    }
}
