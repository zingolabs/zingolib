use std::fmt;
use std::path::{Path, PathBuf};

use crate::binding_layer;

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
const ENTRY_HEADER: &str = "[[entry]]";
const COMMIT_KEY: &str = "commit";
const SINCE_KEY: &str = "since";
const AUDITED_KEY: &str = "audited";
const COMMENT_MARK: char = '#';
const ASSIGNMENT: char = '=';
const QUOTE: char = '"';
const COMMIT_LENGTH: usize = 40;
const COMMIT_EXPECTATION: &str = "a full lowercase commit hash";
const DIGEST_PREFIX: &str = "sha256:";
const DIGEST_HEX_LENGTH: usize = 64;
pub const DESCRIPTOR_ANNOTATION: &str = "org.zingolabs.zingolib.descriptor";
pub const TITLE_ANNOTATION: &str = "org.opencontainers.image.title";
const CHECK_FLAG: &str = "--check";
const BASE_FLAG: &str = "--base";
const ALL_DIGESTS_FLAG: &str = "--all-digests";
const NEWEST_FLAG: &str = "--newest";
const USAGE: &str =
    "usage: binding-manifest --check [--base <ref>] [--all-digests] | binding-manifest --newest | binding-manifest --orasust-version";
pub const PUBLISH_COMMAND: &str = "/publish";
const CHECKOUTS_DIR: &str = "target/binding-manifest";
const BRANCH_TIP: &str = "HEAD";
const TREE_ARGS: [&str; 8] = [
    "tree", "--locked", "--edges", "normal", "--depth", "1", "--prefix", "none",
];
const TREE_FORMAT: [&str; 2] = ["--format", crate::PACKAGE_ID_FORMAT];
const ARRAY_OPEN: char = '[';
const ARRAY_CLOSE: char = ']';
const ITEM_SEPARATOR: char = ',';
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
            crate::PACKAGE_DIR_OPEN,
            self.dir.display(),
            crate::PACKAGE_DIR_CLOSE
        )
    }
}

impl AuditedCrate {
    pub fn parse(text: &str) -> Option<Self> {
        let (name, rest) = text.split_once(crate::PACKAGE_DIR_OPEN)?;
        let dir = rest.strip_suffix(crate::PACKAGE_DIR_CLOSE)?;
        (!name.is_empty() && !name.contains(' ') && !dir.is_empty()).then(|| Self {
            name: name.to_string(),
            dir: PathBuf::from(dir),
        })
    }

    pub fn from_tree_line(line: &str, root: &Path) -> Option<Self> {
        let (name, dir) = crate::package_location(line)?;
        dir.strip_prefix(root).ok().map(|relative| Self {
            name: name.to_string(),
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

struct LocatedEntry {
    entry: Entry,
    last_line: usize,
    keys: Vec<(String, usize)>,
}

fn is_hex(text: &str, length: usize) -> bool {
    text.len() == length
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn hex_value(
    key: &str,
    value: &str,
    hex: &str,
    length: usize,
    expected: &str,
) -> Result<String, Vec<String>> {
    if is_hex(hex, length) {
        Ok(value.to_string())
    } else {
        Err(vec![format!(
            "{key} = {QUOTE}{value}{QUOTE} is not {expected}"
        )])
    }
}

fn commit_value(key: &str, value: &str) -> Result<String, Vec<String>> {
    hex_value(key, value, value, COMMIT_LENGTH, COMMIT_EXPECTATION)
}

fn digest_value(platform: &str, value: &str) -> Result<String, Vec<String>> {
    let hex = value.strip_prefix(DIGEST_PREFIX).unwrap_or_default();
    let expected = format!("a {DIGEST_PREFIX}<hex> digest");
    hex_value(platform, value, hex, DIGEST_HEX_LENGTH, &expected)
}

fn quoted(text: &str) -> Option<&str> {
    let value = text.trim().strip_prefix(QUOTE)?.strip_suffix(QUOTE)?;
    (!value.contains(QUOTE)).then_some(value)
}

fn assignment(line: &str) -> Option<(&str, &str)> {
    let (key, rest) = line.split_once(ASSIGNMENT)?;
    quoted(rest).map(|value| (key.trim(), value))
}

fn array_assignment(line: &str) -> Option<(&str, Vec<&str>)> {
    let (key, rest) = line.split_once(ASSIGNMENT)?;
    let inner = rest
        .trim()
        .strip_prefix(ARRAY_OPEN)?
        .strip_suffix(ARRAY_CLOSE)?;
    let items = inner
        .split(ITEM_SEPARATOR)
        .map(quoted)
        .collect::<Option<Vec<_>>>()?;
    Some((key.trim(), items))
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

fn render_audited(crates: &[AuditedCrate]) -> String {
    let items = crates
        .iter()
        .map(|found| format!("{QUOTE}{found}{QUOTE}"))
        .collect::<Vec<_>>()
        .join(crate::LIST_SEPARATOR);
    format!("{ARRAY_OPEN}{items}{ARRAY_CLOSE}")
}

fn assign(entry: &mut Entry, key: &str, value: &str) -> Result<(), Vec<String>> {
    let taken = |name: &str, present: bool| {
        if present {
            Err(vec![format!("{name} is set twice in one entry")])
        } else {
            Ok(())
        }
    };
    match key {
        COMMIT_KEY => {
            taken(key, !entry.commit.is_empty())?;
            entry.commit = commit_value(key, value)?;
        }
        SINCE_KEY => {
            taken(key, entry.since.is_some())?;
            entry.since = Some(commit_value(key, value)?);
        }
        AUDITED_KEY => return Err(vec![format!("{AUDITED_KEY} takes an array")]),
        other => {
            let platform = platform(other).map_err(|_| vec![format!("unknown key {other}")])?;
            taken(key, entry.digests.iter().any(|(name, _)| name == platform))?;
            entry
                .digests
                .push((platform.to_string(), digest_value(platform, value)?));
        }
    }
    Ok(())
}

fn assign_array(entry: &mut Entry, key: &str, items: &[&str]) -> Result<(), Vec<String>> {
    match key {
        AUDITED_KEY if !entry.audited.is_empty() => {
            Err(vec![format!("{AUDITED_KEY} is set twice in one entry")])
        }
        AUDITED_KEY => {
            entry.audited = audited_value(items)?;
            Ok(())
        }
        other => Err(vec![format!("{other} takes a quoted value, not an array")]),
    }
}

fn parse_located(text: &str) -> Result<Vec<LocatedEntry>, Vec<String>> {
    let mut entries: Vec<LocatedEntry> = Vec::new();
    for (index, raw) in text.lines().enumerate() {
        let line = raw.trim();
        let located = |lines: Vec<String>| {
            lines
                .into_iter()
                .map(|diagnostic| format!("{FILE}:{}: {diagnostic}", ordinal(index)))
                .collect::<Vec<_>>()
        };
        if line.is_empty() || line.starts_with(COMMENT_MARK) {
            continue;
        }
        if line == ENTRY_HEADER {
            entries.push(LocatedEntry {
                entry: Entry::default(),
                last_line: index,
                keys: Vec::new(),
            });
            continue;
        }
        let current = entries.last_mut().ok_or_else(|| {
            located(vec![format!(
                "`{line}` comes before the first {ENTRY_HEADER}"
            )])
        })?;
        let key = if let Some((key, value)) = assignment(line) {
            assign(&mut current.entry, key, value).map_err(located)?;
            key
        } else if let Some((key, items)) = array_assignment(line) {
            assign_array(&mut current.entry, key, &items).map_err(located)?;
            key
        } else {
            return Err(located(vec![format!("cannot read `{line}`")]));
        };
        current.last_line = index;
        current.keys.push((key.to_string(), index));
    }
    Ok(entries)
}

pub fn parse(text: &str) -> Result<Vec<Entry>, Vec<String>> {
    Ok(parse_located(text)?
        .into_iter()
        .map(|located| located.entry)
        .collect())
}

pub fn missing_required(present: &[&str]) -> Vec<&'static str> {
    REQUIRED_PLATFORMS
        .into_iter()
        .filter(|platform| !present.contains(platform))
        .collect()
}

pub fn awaited(entry: &Entry) -> Vec<String> {
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
        for part in awaited(entry) {
            diagnostics.push(format!(
                "entry {} ({}) names no {part}; it awaits `{PUBLISH_COMMAND}`, which records it",
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

pub fn publication_commits(entries: &[Entry]) -> Vec<(String, String)> {
    entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let since = entries[..index]
                .last()
                .map(|previous| previous.commit.clone())
                .or_else(|| entry.since.clone())
                .unwrap_or_default();
            (entry.commit.clone(), since)
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

/// - Runs `cargo tree` as a child process in `checkout`, once per Binding Layer crate.
pub fn audited_at(checkout: &Path) -> Result<Vec<AuditedCrate>, Vec<String>> {
    let checkout = &std::fs::canonicalize(checkout)
        .map_err(|e| vec![format!("cannot resolve {}: {e}", checkout.display())])?;
    let mut crates: Vec<AuditedCrate> = Vec::new();
    for dir in binding_layer::BINDING_CRATE_DIRS {
        let manifest = checkout.join(dir).join(crate::MANIFEST);
        let args = [
            TREE_ARGS.as_slice(),
            &["--manifest-path", crate::utf8(&manifest)?],
            TREE_FORMAT.as_slice(),
        ]
        .concat();
        let output = crate::stdout_in(checkout, crate::CARGO, &args, &[])?;
        let found: Vec<AuditedCrate> = output
            .lines()
            .filter_map(|line| AuditedCrate::from_tree_line(line, checkout))
            .collect();
        if found.is_empty() {
            return Err(vec![format!(
                "no crate of the `cargo tree` output for {dir} lies under {}: {}",
                checkout.display(),
                output.lines().next().unwrap_or_default()
            )]);
        }
        for each in found {
            if !crates.contains(&each) {
                crates.push(each);
            }
        }
    }
    Ok(crates)
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
/// - Runs `cargo tree` in each worktree through [`audited_at`].
pub fn audited_diagnostics(root: &Path, entries: &[Entry]) -> Result<Vec<String>, Vec<String>> {
    let mut diagnostics = Vec::new();
    for entry in entries {
        let found = with_checkout(root, &entry.commit, audited_at)?;
        if found != entry.audited {
            diagnostics.push(format!(
                "entry {} records {AUDITED_KEY} {} and `cargo tree` at that commit reports {}",
                entry.commit,
                render_audited(&entry.audited),
                render_audited(&found)
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
        if !crate::is_ancestor(root, &entry.commit, BRANCH_TIP)? {
            diagnostics.push(format!(
                "{label} is not an ancestor of {BRANCH_TIP}, so it is not a commit of this branch"
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
        eprintln!("warning: {base} is not a commit of this checkout, so every entry is checked");
        return Ok(None);
    }
    Ok(Some(base))
}

pub fn private_package_hint(name: &str) -> String {
    format!(
        "{name} cannot be read without a token; a package that a workflow's first push creates \
         is private, so set its visibility to public in the GitHub package settings, which \
         decision 1 of #2833 requires, and comment `{PUBLISH_COMMAND}` again"
    )
}

/// - Runs `orasust resolve` and `orasust manifest fetch` as child processes, once per recorded digest.
fn manifest_diagnostics(name: &str, commit: &str, digest: &str) -> Vec<String> {
    let found = match crate::orasust::resolve(name) {
        Ok(found) => found,
        Err(mut diagnostics) => {
            diagnostics.push(private_package_hint(name));
            return diagnostics;
        }
    };
    let mut diagnostics = Vec::new();
    if found != digest {
        diagnostics.push(format!(
            "{name} resolves to {found}, and the entry names {digest}"
        ));
    }
    let manifest = match crate::orasust::manifest(name) {
        Ok(manifest) => manifest,
        Err(mut failed) => {
            diagnostics.append(&mut failed);
            return diagnostics;
        }
    };
    let revision = crate::orasust::annotation(&manifest, REVISION_ANNOTATION);
    if revision != Some(commit) {
        diagnostics.push(format!(
            "{name} carries {REVISION_ANNOTATION} {revision:?}, not its commit"
        ));
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
/// - Runs `git` child processes in `root`, and `cargo tree` in a detached worktree of each
///   entry appended after the base's.
/// - Runs `orasust resolve` and `orasust manifest fetch` as child processes, in series, once per
///   recorded digest of the appended entries, or of every entry with `--all-digests`.
/// - Writes a warning to stderr when `--base` names no commit of the checkout, and then checks
///   every entry.
pub fn dispatch(root: &Path, args: &[String]) -> Result<(), Vec<String>> {
    match args.first().map(String::as_str) {
        Some(CHECK_FLAG) => check(root, args),
        Some(NEWEST_FLAG) => {
            println!("{}", newest_awaiting(root)?);
            Ok(())
        }
        Some(crate::orasust::VERSION_FLAG) => {
            println!("{}", crate::orasust::version()?);
            Ok(())
        }
        _ => Err(vec![USAGE.to_string()]),
    }
}

fn check(root: &Path, args: &[String]) -> Result<(), Vec<String>> {
    let head = entries_at(root, None)?;
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

pub fn newest_commit(entries: &[Entry]) -> Result<&str, Vec<String>> {
    let last = entries
        .last()
        .ok_or_else(|| vec![format!("{FILE} holds no entry")])?;
    if last.commit.is_empty() {
        return Err(vec![format!("the newest entry of {FILE} names no commit")]);
    }
    if awaited(last).is_empty() {
        return Err(vec![format!(
            "the newest entry of {FILE} ({}) is published; append an entry to publish another commit",
            last.commit
        )]);
    }
    Ok(&last.commit)
}

pub fn newest_publishable(entries: &[Entry]) -> Result<&str, Vec<String>> {
    structure(entries)?;
    newest_commit(entries)
}

/// - Reads `bindings/published.toml` under `root`.
/// - Runs `git rev-parse` and `git merge-base --is-ancestor` in `root` over every commit the
///   manifest names.
pub fn newest_awaiting(root: &Path) -> Result<String, Vec<String>> {
    let entries = parse_at(root, None)?;
    commits_reachable(root, &entries)?;
    newest_publishable(&entries).map(str::to_string)
}

pub fn recorded(
    text: &str,
    commit: &str,
    platform_name: &str,
    digest: &str,
) -> Result<String, Vec<String>> {
    let platform = platform(platform_name)?;
    let value = format!("{QUOTE}{}{QUOTE}", digest_value(platform, digest)?);
    recorded_line(text, commit, platform, &value)
}

pub fn recorded_audited(
    text: &str,
    commit: &str,
    crates: &[AuditedCrate],
) -> Result<String, Vec<String>> {
    if crates.is_empty() {
        return Err(vec![format!("{AUDITED_KEY} names no crate")]);
    }
    recorded_line(text, commit, AUDITED_KEY, &render_audited(crates))
}

fn recorded_line(text: &str, commit: &str, key: &str, value: &str) -> Result<String, Vec<String>> {
    let target = parse_located(text)?
        .into_iter()
        .find(|located| located.entry.commit == commit)
        .ok_or_else(|| vec![format!("{FILE} holds no entry for {commit}")])?;
    let line = format!("{key} = {value}");
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    match target.keys.iter().find(|(found, _)| found == key) {
        Some((_, index)) => lines[*index] = line,
        None => lines.insert(target.last_line + 1, line),
    }
    let mut joined = lines.join("\n");
    joined.push('\n');
    parse(&joined)?;
    Ok(joined)
}

/// - Reads the process arguments.
/// - Exits the process through [`crate::run`].
pub fn main() -> ! {
    crate::dispatch_from_root(BINARY, dispatch)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIRST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const SECOND: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const ORIGIN: &str = "cccccccccccccccccccccccccccccccccccccccc";
    const DIGEST: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const ROOT: &str = "/host/zingolib";
    const EXPECTED_AUDITED: [&str; 5] = [
        "zingo (zingo-ffi/lib)",
        "pepper-sync (pepper-sync)",
        "zingolib (zingolib)",
        "zingo-nym-proxy-ffi (zingo-netutils/nym-proxy-ffi)",
        "zingo-netutils (zingo-netutils)",
    ];

    fn audited(items: &[&str]) -> Vec<AuditedCrate> {
        items
            .iter()
            .map(|item| AuditedCrate::parse(item).unwrap())
            .collect()
    }

    fn entry(commit: &str, since: Option<&str>, platforms: &[&str]) -> Entry {
        Entry {
            commit: commit.to_string(),
            since: since.map(str::to_string),
            digests: platforms
                .iter()
                .map(|platform| (platform.to_string(), DIGEST.to_string()))
                .collect(),
            audited: audited(&EXPECTED_AUDITED[..2]),
        }
    }

    fn rendered(crates: &[AuditedCrate]) -> Vec<String> {
        crates.iter().map(ToString::to_string).collect()
    }

    fn manifest(entries: &[Entry]) -> String {
        entries
            .iter()
            .map(|entry| {
                let since = entry
                    .since
                    .as_ref()
                    .map(|since| format!("since = \"{since}\"\n"))
                    .unwrap_or_default();
                let digests: String = entry
                    .digests
                    .iter()
                    .map(|(platform, digest)| format!("{platform} = \"{digest}\"\n"))
                    .collect();
                let audited = if entry.audited.is_empty() {
                    String::new()
                } else {
                    format!("audited = {}\n", render_audited(&entry.audited))
                };
                format!(
                    "# a comment\n\n[[entry]]\ncommit = \"{}\"\n{since}{digests}{audited}",
                    entry.commit
                )
            })
            .collect()
    }

    #[test]
    fn the_manifest_round_trips_through_its_text_form() {
        let entries = vec![
            entry(FIRST, Some(ORIGIN), &[ANDROID, IOS]),
            entry(SECOND, None, &PLATFORMS),
        ];
        let parsed = parse(&manifest(&entries)).unwrap();
        assert_eq!(parsed, entries);
        assert_eq!(validate(&parsed), Ok(()));
    }

    #[test]
    fn the_grammar_rejects_what_it_does_not_name() {
        let rejected = [
            "[[entry]]\ncommit = \"short\"\n",
            "[[entry]]\nandroid = \"sha256:short\"\n",
            "[[entry]]\ncommit = \"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"\ncolour = \"red\"\n",
            "commit = \"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"\n",
            "[[entry]]\ncommit = aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n",
            "[[entry]]\n[entry.digests]\n",
            "[[entry]]\naudited = \"zingo (zingo-ffi/lib)\"\n",
            "[[entry]]\naudited = []\n",
            "[[entry]]\naudited = [\"zingo\"]\n",
            "[[entry]]\naudited = [\"zingo (zingo-ffi/lib)\", \"pepper-sync pepper-sync\"]\n",
            "[[entry]]\nandroid = [\"sha256:0\"]\n",
        ];
        for text in rejected {
            let diagnostic = parse(text).unwrap_err().concat();
            assert!(diagnostic.starts_with(FILE), "{text}: {diagnostic}");
        }
        assert!(parse("[[entry]]\nandroid = \"x\"\nandroid = \"y\"\n").is_err());
        assert!(parse(
            "[[entry]]\naudited = [\"zingo (zingo-ffi/lib)\"]\naudited = [\"zingo (zingo-ffi/lib)\"]\n"
        )
        .is_err());
    }

    #[test]
    fn a_platform_outside_the_set_is_refused_by_name() {
        assert_eq!(platform(ANDROID), Ok(ANDROID));
        assert_eq!(platform(IOS), Ok(IOS));
        let diagnostic = platform("linux").unwrap_err().concat();
        assert!(diagnostic.contains("linux") && diagnostic.contains(ANDROID));
    }

    #[test]
    fn an_audited_crate_round_trips_through_its_text_form_and_a_tree_line_is_made_relative() {
        let found = AuditedCrate::parse("zingo (zingo-ffi/lib)").unwrap();
        assert_eq!(found.to_string(), "zingo (zingo-ffi/lib)");
        assert_eq!(found.dir, PathBuf::from("zingo-ffi/lib"));
        assert_eq!(AuditedCrate::parse("zingo"), None);
        assert_eq!(AuditedCrate::parse("zingo v2.0.0 (zingo-ffi/lib)"), None);
        let root = Path::new(ROOT);
        assert_eq!(
            AuditedCrate::from_tree_line("zingolib v6.0.0 (/host/zingolib/zingolib)", root),
            AuditedCrate::parse("zingolib (zingolib)")
        );
        assert_eq!(
            AuditedCrate::from_tree_line("android_logger v0.11.3", root),
            None
        );
        assert_eq!(
            AuditedCrate::from_tree_line("serde_derive v1.0.0 (proc-macro)", root),
            None
        );
        assert_eq!(
            AuditedCrate::from_tree_line("other v1.0.0 (/host/elsewhere/other)", root),
            None
        );
    }

    #[test]
    fn the_checked_out_commit_audits_the_five_crates_the_plan_names() {
        let root = crate::repo_root().unwrap();
        assert_eq!(rendered(&audited_at(&root).unwrap()), EXPECTED_AUDITED);
    }

    #[test]
    fn the_recorded_audited_crates_are_checked_against_cargo_tree_at_the_commit() {
        let root = crate::repo_root().unwrap();
        let head = crate::commit_of(&root, "HEAD").unwrap();
        let mut recorded = entry(&head, Some(ORIGIN), &[ANDROID]);
        recorded.audited = audited(&EXPECTED_AUDITED);
        assert_eq!(
            audited_diagnostics(&root, std::slice::from_ref(&recorded)).unwrap(),
            Vec::<String>::new()
        );
        recorded.audited.pop();
        let diagnostic = audited_diagnostics(&root, &[recorded]).unwrap().concat();
        assert!(diagnostic.contains(&head), "{diagnostic}");
        assert!(!root.join(CHECKOUTS_DIR).join(&head).exists());
    }

    #[test]
    fn a_push_re_derives_only_the_appended_audited_sets_and_may_verify_every_digest() {
        let head = vec![
            entry(FIRST, Some(ORIGIN), &["android"]),
            entry(SECOND, None, &["android"]),
        ];
        let (appended, digests) = verification_scopes(&head, 1, true);
        assert_eq!(appended, &head[1..]);
        assert_eq!(digests, &head[..]);
        let (appended, digests) = verification_scopes(&head, 1, false);
        assert_eq!(appended, &head[1..]);
        assert_eq!(digests, &head[1..]);
        let (appended, _) = verification_scopes(&head, 5, false);
        assert!(appended.is_empty());
    }

    #[test]
    fn the_required_set_difference_has_one_home() {
        assert_eq!(REQUIRED_PLATFORMS, PLATFORMS);
        assert_eq!(missing_required(&[]), REQUIRED_PLATFORMS);
        assert_eq!(missing_required(&PLATFORMS), Vec::<&str>::new());
        assert_eq!(missing_required(&[IOS]), [ANDROID]);
        assert_eq!(missing_required(&[ANDROID]), [IOS]);
        assert_eq!(
            repository(ANDROID),
            "zingolabs/zingolib/binding-layer-android"
        );
    }

    #[test]
    fn an_incomplete_entry_is_named_as_awaiting_publish() {
        let mut incomplete = entry(FIRST, Some(ORIGIN), &[]);
        incomplete.audited.clear();
        let awaited_parts = REQUIRED_PLATFORMS.len() + 1;
        assert_eq!(awaited(&incomplete).len(), awaited_parts);
        let diagnostics = validate(&[incomplete]).unwrap_err();
        assert_eq!(diagnostics.len(), awaited_parts, "{diagnostics:?}");
        assert!(diagnostics
            .iter()
            .all(|diagnostic| diagnostic.contains(PUBLISH_COMMAND)));
    }

    #[test]
    fn the_newest_commit_is_the_last_entry_only_while_it_awaits_publish() {
        let published = entry(FIRST, Some(ORIGIN), &PLATFORMS);
        let android_only = entry(FIRST, Some(ORIGIN), &[ANDROID]);
        assert_eq!(
            newest_commit(std::slice::from_ref(&android_only)),
            Ok(FIRST)
        );
        let diagnostic = newest_commit(std::slice::from_ref(&published))
            .unwrap_err()
            .concat();
        assert!(diagnostic.contains(FIRST) && diagnostic.contains("published"));
        let awaiting = entry(SECOND, None, &[]);
        assert_eq!(
            newest_commit(&[published.clone(), awaiting.clone()]),
            Ok(SECOND)
        );
        let mut nameless = awaiting;
        nameless.commit.clear();
        assert!(newest_commit(&[published, nameless]).is_err());
        assert!(newest_commit(&[]).is_err());
    }

    #[test]
    fn recording_the_audited_crates_adds_the_array_line_or_replaces_it() {
        let text = manifest(&[entry(FIRST, Some(ORIGIN), &PLATFORMS)]);
        let mut bare = entry(SECOND, None, &[]);
        bare.audited.clear();
        let appended = format!("{text}{}", manifest(&[bare]));
        let crates = audited(&EXPECTED_AUDITED);
        let with_audited = recorded_audited(&appended, SECOND, &crates).unwrap();
        let parsed = parse(&with_audited).unwrap();
        assert_eq!(parsed[1].audited, crates);
        assert_eq!(
            validate(&parsed),
            Err(PLATFORMS
                .map(|platform| format!(
                    "entry 2 ({SECOND}) names no {platform} digest; it awaits `/publish`, which records it"
                ))
                .to_vec())
        );
        let replaced = recorded_audited(&with_audited, SECOND, &crates[..1]).unwrap();
        assert_eq!(parse(&replaced).unwrap()[1].audited, crates[..1]);
        assert!(recorded_audited(&text, SECOND, &[]).is_err());
    }

    #[test]
    fn a_recorded_line_joins_its_entry_before_a_following_comment() {
        let text = format!(
            "[[entry]]\ncommit = \"{FIRST}\"\nsince = \"{ORIGIN}\"\n\n# trailing\n\n[[entry]]\ncommit = \"{SECOND}\"\n"
        );
        let recorded = recorded(&text, FIRST, ANDROID, DIGEST).unwrap();
        let lines: Vec<&str> = recorded.lines().collect();
        assert_eq!(lines[3], format!("android = \"{DIGEST}\""));
        assert_eq!(lines[5], "# trailing");
        assert_eq!(parse(&recorded).unwrap()[1].digests, []);
    }

    #[test]
    fn validation_pins_the_since_rule_the_digest_rule_and_uniqueness() {
        let diagnostics = validate(&[
            entry(FIRST, None, &[]),
            entry(SECOND, Some(ORIGIN), &[IOS]),
            entry(SECOND, None, &[ANDROID]),
        ])
        .unwrap_err()
        .concat();
        assert!(diagnostics.contains("first and names no since"));
        assert!(diagnostics.contains("names no android digest"));
        assert!(diagnostics.contains("names no ios digest"));
        assert_eq!(validate(&[entry(FIRST, Some(ORIGIN), &PLATFORMS)]), Ok(()));
        assert!(validate(&[entry(FIRST, Some(ORIGIN), &[ANDROID])]).is_err());
        assert!(validate(&[entry(FIRST, Some(ORIGIN), &[IOS])]).is_err());
        assert!(diagnostics.contains("only the first entry may"));
        assert!(diagnostics.contains("repeats an earlier entry"));
    }

    #[test]
    fn an_unreadable_package_is_explained_by_its_visibility() {
        let hint = private_package_hint("ghcr.io/zingolabs/zingolib/binding-layer-android:abc");
        assert!(hint.contains("binding-layer-android:abc"));
        assert!(hint.contains("public"));
        assert!(hint.contains(PUBLISH_COMMAND));
    }

    #[test]
    fn a_structural_defect_refuses_publication_before_any_push() {
        let awaiting_first = [entry(FIRST, Some(ORIGIN), &[])];
        assert_eq!(structure(&awaiting_first), Ok(()));
        assert_eq!(newest_publishable(&awaiting_first), Ok(FIRST));
        let no_since = [entry(FIRST, None, &[])];
        assert!(structure(&no_since)
            .unwrap_err()
            .concat()
            .contains("names no since"));
        assert!(newest_publishable(&no_since).is_err());
        let repeated = [
            entry(FIRST, Some(ORIGIN), &PLATFORMS),
            entry(FIRST, None, &[]),
        ];
        assert!(newest_publishable(&repeated)
            .unwrap_err()
            .concat()
            .contains("repeats"));
        let published = [entry(FIRST, Some(ORIGIN), &PLATFORMS)];
        assert!(newest_publishable(&published)
            .unwrap_err()
            .concat()
            .contains("is published"));
    }

    #[test]
    fn a_merged_entry_never_changes_and_the_head_may_only_append() {
        let base = vec![entry(FIRST, Some(ORIGIN), &[ANDROID])];
        let appended = vec![base[0].clone(), entry(SECOND, None, &[ANDROID])];
        assert_eq!(unchanged_since_base(&base, &appended), Ok(()));
        let edited = vec![entry(FIRST, Some(ORIGIN), &[ANDROID, IOS])];
        assert!(unchanged_since_base(&base, &edited).is_err());
        assert!(unchanged_since_base(&base, &[]).is_err());
        let reordered = vec![entry(SECOND, Some(ORIGIN), &[ANDROID]), base[0].clone()];
        assert!(unchanged_since_base(&base, &reordered).is_err());
    }

    #[test]
    fn each_publication_follows_the_previous_entry_and_the_first_its_since() {
        let entries = vec![
            entry(FIRST, Some(ORIGIN), &[ANDROID]),
            entry(SECOND, None, &[ANDROID]),
        ];
        assert_eq!(
            publication_commits(&entries),
            [
                (FIRST.to_string(), ORIGIN.to_string()),
                (SECOND.to_string(), FIRST.to_string())
            ]
        );
    }

    #[test]
    fn a_reference_names_the_platform_repository_and_the_commit_tag() {
        assert_eq!(
            reference(ANDROID, FIRST),
            format!("ghcr.io/zingolabs/zingolib/binding-layer-android:{FIRST}")
        );
    }

    #[test]
    fn recording_a_digest_replaces_the_platform_line_or_adds_one_to_the_named_entry() {
        let text = manifest(&[
            entry(FIRST, Some(ORIGIN), &[ANDROID]),
            entry(SECOND, None, &[ANDROID]),
        ]);
        let replaced = DIGEST.replace('0', "f");
        let with_ios = recorded(&text, SECOND, IOS, &replaced).unwrap();
        assert_eq!(
            parse(&with_ios).unwrap()[1].digests,
            [
                (ANDROID.to_string(), DIGEST.to_string()),
                (IOS.to_string(), replaced.clone())
            ]
        );
        let re_android = recorded(&with_ios, FIRST, ANDROID, &replaced).unwrap();
        assert_eq!(
            parse(&re_android).unwrap()[0].digests,
            [(ANDROID.to_string(), replaced)]
        );
        assert!(recorded(&text, ORIGIN, ANDROID, DIGEST).is_err());
        assert!(recorded(&text, SECOND, "linux", DIGEST).is_err());
        assert!(recorded(&text, SECOND, ANDROID, "sha256:short").is_err());
    }

    #[test]
    fn commits_are_resolved_in_one_pass_and_a_missing_one_is_named() {
        let root = crate::repo_root().unwrap();
        let head = crate::commit_of(&root, "HEAD").unwrap();
        assert_eq!(
            commits_exist(&root, &[entry(&head, None, &[ANDROID])]),
            Ok(())
        );
        let diagnostic = commits_exist(&root, &[entry(&head, Some(FIRST), &[ANDROID])])
            .unwrap_err()
            .concat();
        assert!(diagnostic.contains(FIRST), "{diagnostic}");
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        let identity = [
            "-c",
            "user.name=workbench",
            "-c",
            "user.email=workbench@example.invalid",
        ];
        crate::git_in(dir, &[identity.as_slice(), args].concat())
            .unwrap()
            .trim()
            .to_string()
    }

    fn commit_on(dir: &Path, subject: &str) -> String {
        git(
            dir,
            &["commit", "--quiet", "--allow-empty", "--message", subject],
        );
        git(dir, &["rev-parse", BRANCH_TIP])
    }

    #[test]
    fn a_manifest_commit_must_lie_on_the_branch_and_after_its_since() {
        let repo = std::env::temp_dir().join(format!(
            "workbench-commits-reachable-{}",
            std::process::id()
        ));
        crate::fresh_dir(&repo).unwrap();
        git(&repo, &["init", "--quiet", "--initial-branch", "main"]);
        let root_commit = commit_on(&repo, "root");
        git(&repo, &["switch", "--quiet", "--create", "side"]);
        let side = commit_on(&repo, "side");
        git(&repo, &["switch", "--quiet", "main"]);
        let tip = commit_on(&repo, "tip");

        assert_eq!(
            commits_reachable(&repo, &[entry(&tip, Some(&root_commit), &[ANDROID])]),
            Ok(())
        );
        let off_branch = commits_reachable(&repo, &[entry(&side, Some(&root_commit), &[ANDROID])])
            .unwrap_err()
            .concat();
        assert!(
            off_branch.contains(&side) && off_branch.contains(BRANCH_TIP),
            "{off_branch}"
        );
        let later_since = commits_reachable(&repo, &[entry(&tip, Some(&side), &[ANDROID])])
            .unwrap_err()
            .concat();
        assert!(later_since.contains(&side), "{later_since}");
        std::fs::remove_dir_all(&repo).unwrap();
    }

    #[test]
    fn an_absent_base_checks_every_entry_and_a_present_one_is_kept() {
        let root = crate::repo_root().unwrap();
        let no_commit_before = "0".repeat(COMMIT_LENGTH);
        assert_eq!(merged_base(&root, None), Ok(None));
        assert_eq!(merged_base(&root, Some("")), Ok(None));
        assert_eq!(merged_base(&root, Some(&no_commit_before)), Ok(None));
        assert_eq!(merged_base(&root, Some("no-such-ref")), Ok(None));
        assert_eq!(merged_base(&root, Some(BRANCH_TIP)), Ok(Some(BRANCH_TIP)));
    }

    #[test]
    fn a_checkout_body_failure_survives_the_worktree_removal() {
        let repo =
            std::env::temp_dir().join(format!("workbench-checkout-failure-{}", std::process::id()));
        crate::fresh_dir(&repo).unwrap();
        git(&repo, &["init", "--quiet", "--initial-branch", "main"]);
        let tip = commit_on(&repo, "tip");
        let failure = vec!["the body failed".to_string()];
        let result: Result<(), Vec<String>> = with_checkout(&repo, &tip, |_| Err(failure.clone()));
        assert_eq!(result, Err(failure));
        assert!(!repo.join(CHECKOUTS_DIR).join(&tip).exists());
        std::fs::remove_dir_all(&repo).unwrap();
    }

    #[test]
    fn the_committed_manifest_parses_and_validates() {
        let root = crate::repo_root().unwrap();
        assert!(entries_at(&root, None).is_ok());
    }
}
