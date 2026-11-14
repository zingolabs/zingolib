use std::fmt;
use std::path::{Path, PathBuf};

use crate::binding_layer;
use crate::binding_layer::short_commit;
use crate::orasust;

pub const BINARY: &str = "binding-manifest";
pub const FILE: &str = "bindings/published.toml";
pub const REGISTRY: &str = "ghcr.io";
pub const REPOSITORY_OWNER_PATH: &str = "zingolabs/zingolib/";
pub const ANDROID: &str = "android";
pub const IOS: &str = "ios";
pub const PLATFORMS: [&str; 2] = [ANDROID, IOS];
pub const REQUIRED_PLATFORMS: [&str; 2] = PLATFORMS;
pub const REVISION_ANNOTATION: &str = "org.opencontainers.image.revision";
const PATH_SEPARATOR: &str = "/";
const TAG_SEPARATOR: &str = ":";
const REVISION_PATH_SEPARATOR: &str = ":";
const ENTRY_KEY: &str = "entry";
const COMMIT_KEY: &str = "commit";
const SINCE_KEY: &str = "since";
const AUDITED_KEY: &str = "audited";
const QUOTE: char = '"';
pub const COMMIT_LENGTH: usize = short_commit::FULL_LENGTH;
const COMMIT_EXPECTATION: &str = "a full lowercase commit hash";
const DIGEST_PREFIX: &str = "sha256:";
const DIGEST_HEX_LENGTH: usize = 64;
pub const COORDINATE_ANNOTATION: &str = "org.zingolabs.zingolib.coordinate";
pub const TITLE_ANNOTATION: &str = "org.opencontainers.image.title";
const CHECK_FLAG: &str = "--check";
const BASE_FLAG: &str = "--base";
const ALL_DIGESTS_FLAG: &str = "--all-digests";
const USAGE: &str =
    "usage: binding-manifest --check [--base <ref>] [--all-digests] | binding-manifest --orasust-version";
const CHECKOUTS_DIR: &str = "target/binding-manifest";
const PACKAGE_DIR_OPEN: &str = " (";
const PACKAGE_DIR_CLOSE: char = ')';
const FIRST_ORDINAL: usize = 1;

fn ordinal(index: usize) -> usize {
    index + FIRST_ORDINAL
}

pub fn platform(name: &str) -> Result<&'static str, Vec<String>> {
    PLATFORMS
        .into_iter()
        .find(|known| *known == name)
        .ok_or_else(|| {
            vec![format!(
                "unknown platform {name}; the platforms are {}",
                PLATFORMS.join(crate::LIST_SEPARATOR)
            )]
        })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuditedCrate {
    pub name: String,
    pub dir: PathBuf,
}

impl fmt::Display for AuditedCrate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}{}{}{}",
            self.name,
            PACKAGE_DIR_OPEN,
            self.dir.display(),
            PACKAGE_DIR_CLOSE
        )
    }
}

impl AuditedCrate {
    pub fn parse(text: &str) -> Option<Self> {
        let (name, rest) = text.split_once(PACKAGE_DIR_OPEN)?;
        let dir = rest.strip_suffix(PACKAGE_DIR_CLOSE)?;
        (!name.is_empty() && !name.contains(' ') && !dir.is_empty()).then(|| Self {
            name: name.to_string(),
            dir: PathBuf::from(dir),
        })
    }

    pub fn from_package(package: &cargo_metadata::Package, root: &Path) -> Option<Self> {
        let dir = package.manifest_path.parent()?.as_std_path();
        dir.strip_prefix(root).ok().map(|relative| Self {
            name: package.name.to_string(),
            dir: relative.to_path_buf(),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Entry {
    pub commit: String,
    pub since: Option<String>,
    pub digests: Vec<(String, String)>,
    pub audited: Vec<AuditedCrate>,
}

fn hex_value(
    key: &str,
    value: &str,
    hex: &str,
    length: usize,
    expected: &str,
) -> Result<String, Vec<String>> {
    if short_commit::is_hex(hex, length) {
        Ok(value.to_string())
    } else {
        Err(vec![format!(
            "{key} = {QUOTE}{value}{QUOTE} is not {expected}"
        )])
    }
}

pub fn commit_value(key: &str, value: &str) -> Result<String, Vec<String>> {
    hex_value(key, value, value, COMMIT_LENGTH, COMMIT_EXPECTATION)
}

fn digest_value(platform: &str, value: &str) -> Result<String, Vec<String>> {
    let hex = value.strip_prefix(DIGEST_PREFIX).unwrap_or_default();
    let expected = format!("a {DIGEST_PREFIX}<hex> digest");
    hex_value(platform, value, hex, DIGEST_HEX_LENGTH, &expected)
}

fn audited_value(items: &[&str]) -> Result<Vec<AuditedCrate>, Vec<String>> {
    items
        .iter()
        .map(|item| {
            AuditedCrate::parse(item).ok_or_else(|| {
                vec![format!(
                    "{AUDITED_KEY} item {QUOTE}{item}{QUOTE} is not `<package> (<directory>)`"
                )]
            })
        })
        .collect()
}

fn audited_array(crates: &[AuditedCrate]) -> toml_edit::Array {
    let mut array = toml_edit::Array::new();
    for found in crates {
        array.push(found.to_string());
    }
    array
}

fn line_of(text: &str, span: Option<std::ops::Range<usize>>) -> usize {
    span.map_or(FIRST_ORDINAL, |span| {
        text[..span.start].matches('\n').count() + FIRST_ORDINAL
    })
}

fn located(text: &str, key: Option<&toml_edit::Key>) -> impl Fn(Vec<String>) -> Vec<String> {
    let line = line_of(text, key.and_then(toml_edit::Key::span));
    move |diagnostics| {
        diagnostics
            .into_iter()
            .map(|diagnostic| format!("{FILE}:{line}: {diagnostic}"))
            .collect()
    }
}

fn string_value<'a>(key: &str, item: &'a toml_edit::Item) -> Result<&'a str, Vec<String>> {
    item.as_str().ok_or_else(|| {
        vec![format!(
            "{key} takes a quoted value, not {}",
            item.type_name()
        )]
    })
}

fn commit_item(key: &str, item: &toml_edit::Item) -> Result<String, Vec<String>> {
    commit_value(key, string_value(key, item)?)
}

fn audited_items(item: &toml_edit::Item) -> Result<Vec<&str>, Vec<String>> {
    let array = item
        .as_array()
        .ok_or_else(|| vec![format!("{AUDITED_KEY} takes an array")])?;
    if array.is_empty() {
        return Err(vec![format!("{AUDITED_KEY} names no crate")]);
    }
    array
        .iter()
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| vec![format!("{AUDITED_KEY} items are quoted")])
        })
        .collect()
}

fn entry_from(text: &str, table: &toml_edit::Table) -> Result<Entry, Vec<String>> {
    let mut entry = Entry::default();
    for (key, item) in table.iter() {
        let assigned = match key {
            COMMIT_KEY => commit_item(key, item).map(|commit| entry.commit = commit),
            SINCE_KEY => commit_item(key, item).map(|since| entry.since = Some(since)),
            AUDITED_KEY => audited_items(item)
                .and_then(|items| audited_value(&items))
                .map(|crates| entry.audited = crates),
            other => platform(other)
                .map_err(|_| vec![format!("unknown key {other}")])
                .and_then(|platform| {
                    string_value(platform, item)
                        .and_then(|value| digest_value(platform, value))
                        .map(|digest| entry.digests.push((platform.to_string(), digest)))
                }),
        };
        assigned.map_err(located(text, table.key(key)))?;
    }
    Ok(entry)
}

pub fn parse(text: &str) -> Result<Vec<Entry>, Vec<String>> {
    let document = toml_edit::Document::parse(text).map_err(|e| vec![format!("{FILE}: {e}")])?;
    let mut entries = Vec::new();
    for (key, item) in document.iter() {
        let at = located(text, document.key(key));
        if key != ENTRY_KEY {
            return Err(at(vec![format!("`{key}` lies outside an [[{ENTRY_KEY}]]")]));
        }
        let tables = item
            .as_array_of_tables()
            .ok_or_else(|| at(vec![format!("{ENTRY_KEY} is not an array of tables")]))?;
        for table in tables.iter() {
            entries.push(entry_from(text, table)?);
        }
    }
    Ok(entries)
}

pub fn missing_required(present: &[&str]) -> Vec<&'static str> {
    REQUIRED_PLATFORMS
        .into_iter()
        .filter(|platform| !present.contains(platform))
        .collect()
}

pub fn missing(entry: &Entry) -> Vec<String> {
    let present: Vec<&str> = entry
        .digests
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    missing_required(&present)
        .into_iter()
        .map(|platform| format!("{platform} digest"))
        .chain(
            entry
                .audited
                .is_empty()
                .then(|| format!("{AUDITED_KEY} crates")),
        )
        .collect()
}

pub fn validate(entries: &[Entry]) -> Result<(), Vec<String>> {
    let mut diagnostics = structure(entries).err().unwrap_or_default();
    for (index, entry) in entries.iter().enumerate() {
        for part in missing(entry) {
            diagnostics.push(format!(
                "entry {} ({}) names no {part}",
                ordinal(index),
                entry.commit
            ));
        }
    }
    crate::verdict(diagnostics)
}

pub fn structure(entries: &[Entry]) -> Result<(), Vec<String>> {
    let mut diagnostics = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let label = format!("entry {} ({})", ordinal(index), entry.commit);
        if entry.commit.is_empty() {
            diagnostics.push(format!("entry {} names no commit", ordinal(index)));
        }
        match (index, &entry.since) {
            (0, None) => diagnostics.push(format!("{label} is first and names no since commit")),
            (0, Some(_)) => {}
            (_, Some(_)) => diagnostics.push(format!(
                "{label} names a since commit, which only the first entry may"
            )),
            (_, None) => {}
        }
        if entries[..index]
            .iter()
            .any(|earlier| earlier.commit == entry.commit)
        {
            diagnostics.push(format!("{label} repeats an earlier entry's commit"));
        }
    }
    crate::verdict(diagnostics)
}

pub fn unchanged_since_base(base: &[Entry], head: &[Entry]) -> Result<(), Vec<String>> {
    crate::verdict(
        base.iter()
            .enumerate()
            .filter_map(|(index, kept)| match head.get(index) {
                Some(same) if same == kept => None,
                _ => Some(format!(
                    "entry {} ({}) changed or moved after its merge",
                    ordinal(index),
                    kept.commit
                )),
            })
            .collect(),
    )
}

/// The earlier entries whose commits are ancestors of `commit` and of no other such entry,
/// in record order. The newest by publication is not the nearest by ancestry: an older commit
/// published late is an ancestor of the entries published before it, and the first commit of
/// `dev` after a release line merges back has the line's candidate and the last entry of `dev`
/// among its ancestors, neither an ancestor of the other.
///
/// - Runs `git merge-base --is-ancestor` in `root` once per entry of `earlier`, then
///   `git merge-base --independent` once when more than one is an ancestor.
pub fn recorded_ancestors<'a>(
    root: &Path,
    earlier: &'a [Entry],
    commit: &str,
) -> Result<Vec<&'a Entry>, Vec<String>> {
    let mut ancestors = Vec::new();
    for entry in earlier {
        if crate::is_ancestor(root, &entry.commit, commit)? {
            ancestors.push(entry);
        }
    }
    if ancestors.len() < 2 {
        return Ok(ancestors);
    }
    let mut args = INDEPENDENT_ARGS.to_vec();
    args.extend(ancestors.iter().map(|entry| entry.commit.as_str()));
    let independent = crate::git_in(root, &args)?;
    let independent: Vec<&str> = independent.lines().map(str::trim).collect();
    ancestors.retain(|entry| independent.contains(&entry.commit.as_str()));
    Ok(ancestors)
}

const INDEPENDENT_ARGS: [&str; 2] = ["merge-base", "--independent"];

/// Each entry's commit with the commits its changelog section follows: the commits of its
/// [`recorded_ancestors`], or the first entry's own since commit.
///
/// - Runs `git merge-base` in `root` through [`recorded_ancestors`] for every entry.
pub fn publication_commits(
    root: &Path,
    entries: &[Entry],
) -> Result<Vec<(String, Vec<String>)>, Vec<String>> {
    entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let ancestors = recorded_ancestors(root, &entries[..index], &entry.commit)?;
            let since = if ancestors.is_empty() {
                vec![entry.since.clone().ok_or_else(|| {
                    vec![format!(
                        "no earlier entry is an ancestor of entry {} ({}), and it names no since commit",
                        ordinal(index),
                        entry.commit
                    )]
                })?]
            } else {
                ancestors
                    .into_iter()
                    .map(|ancestor| ancestor.commit.clone())
                    .collect()
            };
            Ok((entry.commit.clone(), since))
        })
        .collect()
}

pub fn repository(platform: &str) -> String {
    format!(
        "{REPOSITORY_OWNER_PATH}{}",
        binding_layer::artifact_name_without_commit(platform)
    )
}

pub fn reference(platform: &str, commit: &str) -> String {
    [
        REGISTRY,
        PATH_SEPARATOR,
        &repository(platform),
        TAG_SEPARATOR,
        commit,
    ]
    .concat()
}

fn parse_at(root: &Path, revision: Option<&str>) -> Result<Vec<Entry>, Vec<String>> {
    let text = match revision {
        None => crate::read(&root.join(FILE))?,
        Some(revision) if crate::listed_at(root, revision, FILE)? => crate::git_in(
            root,
            &["show", &[revision, FILE].join(REVISION_PATH_SEPARATOR)],
        )?,
        Some(_) => String::new(),
    };
    parse(&text).map_err(|diagnostics| located_at(revision, diagnostics))
}

fn located_at(revision: Option<&str>, diagnostics: Vec<String>) -> Vec<String> {
    match revision {
        None => diagnostics,
        Some(revision) => diagnostics
            .into_iter()
            .map(|diagnostic| [revision, &diagnostic].join(REVISION_PATH_SEPARATOR))
            .collect(),
    }
}

pub fn entries_at(root: &Path, revision: Option<&str>) -> Result<Vec<Entry>, Vec<String>> {
    let entries = parse_at(root, revision)?;
    validate(&entries).map_err(|diagnostics| located_at(revision, diagnostics))?;
    Ok(entries)
}

/// - Runs `cargo metadata` as a child process in `checkout`, once per Binding Layer crate.
pub fn audited_at(checkout: &Path) -> Result<Vec<AuditedCrate>, Vec<String>> {
    let checkout = &crate::canonical(checkout)?;
    let mut crates: Vec<AuditedCrate> = Vec::new();
    for dir in binding_layer::BINDING_CRATE_DIRS {
        let metadata = crate::metadata_of(&checkout.join(dir).join(crate::MANIFEST), true)?;
        for package in direct_normal_dependencies(&metadata, dir)? {
            if let Some(found) = AuditedCrate::from_package(package, checkout) {
                if !crates.contains(&found) {
                    crates.push(found);
                }
            }
        }
    }
    Ok(crates)
}

fn direct_normal_dependencies<'a>(
    metadata: &'a cargo_metadata::Metadata,
    dir: &str,
) -> Result<Vec<&'a cargo_metadata::Package>, Vec<String>> {
    let root = metadata
        .root_package()
        .ok_or_else(|| vec![format!("{dir} declares no package")])?;
    let node = metadata
        .resolve
        .as_ref()
        .and_then(|resolve| resolve.nodes.iter().find(|node| node.id == root.id))
        .ok_or_else(|| vec![format!("`cargo metadata` resolved no node for {dir}")])?;
    let mut direct: Vec<&cargo_metadata::Package> = node
        .deps
        .iter()
        .filter(|dep| {
            dep.dep_kinds
                .iter()
                .any(|info| info.kind == cargo_metadata::DependencyKind::Normal)
        })
        .filter_map(|dep| {
            metadata
                .packages
                .iter()
                .find(|package| package.id == dep.pkg)
        })
        .collect();
    direct.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(std::iter::once(root).chain(direct).collect())
}

/// - Runs `git worktree add --detach`, `git worktree remove --force`, and `git worktree prune` as child processes in `root`.
/// - Creates and removes the directory `target/binding-manifest/<commit>` under `root`.
pub fn with_checkout<T>(
    root: &Path,
    commit: &str,
    body: impl FnOnce(&Path) -> Result<T, Vec<String>>,
) -> Result<T, Vec<String>> {
    let dir = root.join(CHECKOUTS_DIR).join(commit);
    let dir_text = crate::utf8(&dir)?.to_string();
    let remove = || crate::git_in(root, &["worktree", "remove", "--force", &dir_text]).map(drop);
    if dir.exists() {
        remove().or_else(|_| {
            std::fs::remove_dir_all(&dir).map_err(|e| vec![format!("cannot clear {dir_text}: {e}")])
        })?;
        crate::git_in(root, &["worktree", "prune"])?;
    }
    crate::create_parent(&dir)?;
    crate::git_in(root, &["worktree", "add", "--detach", &dir_text, commit])?;
    let result = body(&dir);
    match (result, remove()) {
        (Ok(value), Ok(())) => Ok(value),
        (Ok(_), Err(removal)) => Err(removal),
        (Err(diagnostics), Ok(())) => Err(diagnostics),
        (Err(mut diagnostics), Err(removal)) => {
            diagnostics.extend(removal);
            Err(diagnostics)
        }
    }
}

/// - Creates and removes one detached worktree per entry through [`with_checkout`].
/// - Runs `cargo metadata` in each worktree through [`audited_at`].
pub fn audited_diagnostics(root: &Path, entries: &[Entry]) -> Result<Vec<String>, Vec<String>> {
    let mut diagnostics = Vec::new();
    for entry in entries {
        let found = with_checkout(root, &entry.commit, audited_at)?;
        if found != entry.audited {
            diagnostics.push(format!(
                "entry {} records {AUDITED_KEY} {} and `cargo metadata` at that commit reports {}",
                entry.commit,
                audited_array(&entry.audited),
                audited_array(&found)
            ));
        }
    }
    Ok(diagnostics)
}

fn commits_exist(root: &Path, entries: &[Entry]) -> Result<(), Vec<String>> {
    let commits: Vec<&String> = entries
        .iter()
        .flat_map(|entry| std::iter::once(&entry.commit).chain(entry.since.as_ref()))
        .collect();
    let specs: Vec<String> = commits
        .iter()
        .map(|commit| crate::commit_spec(commit))
        .collect();
    let args: Vec<&str> = std::iter::once("rev-parse")
        .chain(specs.iter().map(String::as_str))
        .collect();
    if crate::git_in(root, &args).is_ok() {
        return Ok(());
    }
    commits
        .iter()
        .try_for_each(|commit| crate::commit_of(root, commit).map(drop))
}

/// - Runs `git rev-parse` once, then `git merge-base --is-ancestor` in `root` once per commit the
///   manifest names.
fn commits_reachable(root: &Path, entries: &[Entry]) -> Result<(), Vec<String>> {
    commits_exist(root, entries)?;
    let mut diagnostics = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let label = format!("entry {} ({})", ordinal(index), entry.commit);
        if !crate::is_ancestor(root, &entry.commit, crate::BRANCH_TIP)? {
            diagnostics.push(format!(
                "{label} is not an ancestor of {}, so it is not a commit of this branch",
                crate::BRANCH_TIP
            ));
        }
        if let Some(since) = &entry.since {
            if !crate::is_ancestor(root, since, &entry.commit)? {
                diagnostics.push(format!(
                    "{label} names a since commit {since} that is not an ancestor of its commit"
                ));
            }
        }
    }
    crate::verdict(diagnostics)
}

fn merged_base<'a>(root: &Path, base: Option<&'a str>) -> Result<Option<&'a str>, Vec<String>> {
    let Some(base) = base else {
        return Ok(None);
    };
    let no_commit_before = base.len() == COMMIT_LENGTH && base.bytes().all(|byte| byte == b'0');
    if base.is_empty() || no_commit_before {
        return Ok(None);
    }
    if crate::commit_of(root, base).is_err() {
        crate::warn(&format!(
            "{base} is not a commit of this checkout, so every entry is checked"
        ));
        return Ok(None);
    }
    Ok(Some(base))
}

pub fn private_package_hint(name: &str) -> String {
    format!(
        "{name} cannot be read without a token; a package that a workflow's first push creates \
         is private, so set its visibility to public in the GitHub package settings, which \
         decision 1 of #2833 requires, and run the publish workflow again"
    )
}

pub fn revision_mismatch(name: &str, manifest: &str, commit: &str) -> Option<String> {
    let revision = crate::orasust::annotation(manifest, REVISION_ANNOTATION);
    (revision != Some(commit))
        .then(|| format!("{name} carries {REVISION_ANNOTATION} {revision:?}, not its commit"))
}

pub fn missing_platforms(digests: &[(&str, String)]) -> Vec<&'static str> {
    let present: Vec<&str> = digests.iter().map(|(platform, _)| *platform).collect();
    missing_required(&present)
}

/// - Runs `orasust resolve` as a child process, anonymously and then, when the registry refuses
///   the anonymous read and credentials are given, with them.
/// - Runs `orasust manifest fetch` as a child process when the registry serves the reference.
pub fn held_digest(
    platform: &str,
    commit: &str,
    credentials: Option<&orasust::Credentials>,
) -> Result<Option<String>, Vec<String>> {
    let name = reference(platform, commit);
    let digest = match orasust::resolve(&name)? {
        orasust::Resolution::Found(digest) => digest,
        orasust::Resolution::Absent => return Ok(None),
        orasust::Resolution::Private => {
            let with_token = credentials
                .map(|credentials| orasust::resolve_as(&name, credentials))
                .transpose()?;
            return match with_token {
                None | Some(orasust::Resolution::Found(_)) => {
                    Err(vec![private_package_hint(&name)])
                }
                Some(orasust::Resolution::Absent) => Ok(None),
                Some(orasust::Resolution::Private) => Err(vec![format!(
                    "{name} refuses the token as it refuses an anonymous read"
                )]),
            };
        }
    };
    let manifest = orasust::manifest(&name)?;
    match revision_mismatch(&name, &manifest, commit) {
        Some(mismatch) => Err(vec![mismatch]),
        None => Ok(Some(digest)),
    }
}

/// - Runs `orasust resolve` and `orasust manifest fetch` as child processes, once per recorded digest.
fn manifest_diagnostics(name: &str, commit: &str, digest: &str) -> Vec<String> {
    let found = match orasust::resolve(name) {
        Ok(orasust::Resolution::Found(found)) => found,
        Ok(orasust::Resolution::Absent) => {
            return vec![format!(
                "{name} is not in the registry, and the entry names {digest}"
            )]
        }
        Ok(orasust::Resolution::Private) => return vec![private_package_hint(name)],
        Err(diagnostics) => return diagnostics,
    };
    let mut diagnostics = Vec::new();
    if found != digest {
        diagnostics.push(format!(
            "{name} resolves to {found}, and the entry names {digest}"
        ));
    }
    match orasust::manifest(name) {
        Ok(manifest) => diagnostics.extend(revision_mismatch(name, &manifest, commit)),
        Err(mut failed) => diagnostics.append(&mut failed),
    }
    diagnostics
}

/// - Runs `orasust version` once, then `orasust` child processes through [`manifest_diagnostics`].
fn registry_diagnostics(entries: &[Entry]) -> Result<Vec<String>, Vec<String>> {
    if entries.iter().all(|entry| entry.digests.is_empty()) {
        return Ok(Vec::new());
    }
    crate::orasust::pinned()?;
    Ok(entries
        .iter()
        .flat_map(|entry| {
            entry.digests.iter().flat_map(|(platform, digest)| {
                manifest_diagnostics(&reference(platform, &entry.commit), &entry.commit, digest)
            })
        })
        .collect())
}

/// - Reads `bindings/published.toml` and, with `--base`, its copy at that git revision.
/// - Runs `git` child processes in `root`, and `cargo metadata` in a detached worktree of each
///   entry appended after the base's.
/// - Runs `orasust resolve` and `orasust manifest fetch` as child processes, in series, once per
///   recorded digest of the appended entries, or of every entry with `--all-digests`.
/// - Writes a warning to stderr, as a workflow command under `GITHUB_ACTIONS`, when `--base`
///   names no commit of the checkout, and then checks every entry.
pub fn dispatch(root: &Path, args: &[String]) -> Result<(), Vec<String>> {
    match args.first().map(String::as_str) {
        Some(CHECK_FLAG) => check(root, args),
        Some(crate::orasust::VERSION_FLAG) => {
            println!("{}", crate::orasust::version()?);
            Ok(())
        }
        _ => Err(vec![USAGE.to_string()]),
    }
}

fn check(root: &Path, args: &[String]) -> Result<(), Vec<String>> {
    let head = parse_at(root, None)?;
    validate(&head)?;
    commits_reachable(root, &head)?;
    let merged = match merged_base(root, crate::flag_value(args, BASE_FLAG)?)? {
        Some(base) => {
            let base = entries_at(root, Some(base))?;
            unchanged_since_base(&base, &head)?;
            base.len()
        }
        None => 0,
    };
    let (appended, digests) = verification_scopes(
        &head,
        merged,
        args.iter().any(|arg| arg == ALL_DIGESTS_FLAG),
    );
    let mut diagnostics = audited_diagnostics(root, appended)?;
    diagnostics.extend(registry_diagnostics(digests)?);
    crate::verdict(diagnostics)
}

pub fn verification_scopes(
    head: &[Entry],
    merged: usize,
    all_digests: bool,
) -> (&[Entry], &[Entry]) {
    let appended = &head[merged.min(head.len())..];
    (appended, if all_digests { head } else { appended })
}

pub fn appended(text: &str, entry: &Entry) -> Result<String, Vec<String>> {
    let parts = missing(entry);
    if !parts.is_empty() {
        return Err(vec![format!(
            "the entry for {} names no {}",
            entry.commit,
            parts.join(crate::LIST_SEPARATOR)
        )]);
    }
    let mut document: toml_edit::DocumentMut =
        text.parse().map_err(|e| vec![format!("{FILE}: {e}")])?;
    let mut table = toml_edit::Table::new();
    table.insert(
        COMMIT_KEY,
        toml_edit::value(commit_value(COMMIT_KEY, &entry.commit)?),
    );
    table.insert(AUDITED_KEY, toml_edit::value(audited_array(&entry.audited)));
    for (name, digest) in &entry.digests {
        let platform = platform(name)?;
        table.insert(platform, toml_edit::value(digest_value(platform, digest)?));
    }
    document
        .entry(ENTRY_KEY)
        .or_insert_with(|| toml_edit::Item::ArrayOfTables(toml_edit::ArrayOfTables::new()))
        .as_array_of_tables_mut()
        .ok_or_else(|| vec![format!("{FILE}: {ENTRY_KEY} is not an array of tables")])?
        .push(table);
    let appended = document.to_string();
    validate(&parse(&appended)?)?;
    Ok(appended)
}

/// - Reads the process arguments.
/// - Exits the process through [`crate::run`].
pub fn main() -> ! {
    crate::dispatch_from_root(BINARY, dispatch)
}

/// - Runs `git commit --allow-empty` and `git rev-parse` in `dir` under the hermetic config.
#[cfg(test)]
pub(crate) fn commit_on(dir: &Path, subject: &str) -> Result<String, Vec<String>> {
    crate::hermetic_git(
        dir,
        &["commit", "--quiet", "--allow-empty", "--message", subject],
    )?;
    crate::hermetic_git(dir, &["rev-parse", crate::BRANCH_TIP])
        .map(|commit| commit.trim().to_string())
}

#[cfg(test)]
mod tests;
