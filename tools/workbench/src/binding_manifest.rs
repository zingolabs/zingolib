use std::path::Path;

pub const BINARY: &str = "binding-manifest";
pub const FILE: &str = "bindings/published.toml";
pub const REGISTRY: &str = "ghcr.io";
pub const REPOSITORY_PREFIX: &str = "zingolabs/zingolib/binding-layer-";
pub const PLATFORMS: [&str; 2] = ["android", "ios"];
pub const REQUIRED_PLATFORMS: [&str; 1] = ["android"];
pub const REVISION_ANNOTATION: &str = "org.opencontainers.image.revision";
const PATH_SEPARATOR: &str = "/";
const TAG_SEPARATOR: &str = ":";
const ENTRY_HEADER: &str = "[[entry]]";
const COMMIT_KEY: &str = "commit";
const SINCE_KEY: &str = "since";
const COMMENT_MARK: char = '#';
const ASSIGNMENT: char = '=';
const QUOTE: char = '"';
const COMMIT_LENGTH: usize = 40;
const DIGEST_PREFIX: &str = "sha256:";
const DIGEST_HEX_LENGTH: usize = 64;
pub const DESCRIPTOR_ANNOTATION: &str = "org.zingolabs.zingolib.descriptor";
pub const TITLE_ANNOTATION: &str = "org.opencontainers.image.title";
const CHECK_FLAG: &str = "--check";
const BASE_FLAG: &str = "--base";
const NEWEST_FLAG: &str = "--newest";
const RECORD_FLAG: &str = "--record";
const COMMIT_FLAG: &str = "--commit";
const PLATFORM_FLAG: &str = "--platform";
const DIGEST_FLAG: &str = "--digest";
const USAGE: &str = "usage: binding-manifest --check [--base <ref>] \
    | binding-manifest --newest \
    | binding-manifest --record --commit <commit> --platform <platform> --digest <digest>";

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Entry {
    pub commit: String,
    pub since: Option<String>,
    pub digests: Vec<(String, String)>,
}

fn is_hex(text: &str, length: usize) -> bool {
    text.len() == length
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn commit_value(key: &str, value: &str) -> Result<String, Vec<String>> {
    if is_hex(value, COMMIT_LENGTH) {
        Ok(value.to_string())
    } else {
        Err(vec![format!(
            "{key} = {QUOTE}{value}{QUOTE} is not a full lowercase commit hash"
        )])
    }
}

fn digest_value(platform: &str, value: &str) -> Result<String, Vec<String>> {
    match value.strip_prefix(DIGEST_PREFIX) {
        Some(hex) if is_hex(hex, DIGEST_HEX_LENGTH) => Ok(value.to_string()),
        _ => Err(vec![format!(
            "{platform} = {QUOTE}{value}{QUOTE} is not a {DIGEST_PREFIX}<hex> digest"
        )]),
    }
}

fn assignment(line: &str) -> Option<(&str, &str)> {
    let (key, rest) = line.split_once(ASSIGNMENT)?;
    let value = rest.trim().strip_prefix(QUOTE)?.strip_suffix(QUOTE)?;
    (!value.contains(QUOTE)).then_some((key.trim(), value))
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
        platform if PLATFORMS.contains(&platform) => {
            taken(key, entry.digests.iter().any(|(name, _)| name == platform))?;
            entry
                .digests
                .push((platform.to_string(), digest_value(platform, value)?));
        }
        other => return Err(vec![format!("unknown key {other}")]),
    }
    Ok(())
}

pub fn parse(text: &str) -> Result<Vec<Entry>, Vec<String>> {
    let mut entries: Vec<Entry> = Vec::new();
    for (index, raw) in text.lines().enumerate() {
        let line = raw.trim();
        let located = |lines: Vec<String>| {
            lines
                .into_iter()
                .map(|diagnostic| format!("{FILE}:{}: {diagnostic}", index + 1))
                .collect::<Vec<_>>()
        };
        if line.is_empty() || line.starts_with(COMMENT_MARK) {
            continue;
        }
        if line == ENTRY_HEADER {
            entries.push(Entry::default());
            continue;
        }
        let (key, value) =
            assignment(line).ok_or_else(|| located(vec![format!("cannot read `{line}`")]))?;
        let entry = entries.last_mut().ok_or_else(|| {
            located(vec![format!(
                "`{line}` comes before the first {ENTRY_HEADER}"
            )])
        })?;
        assign(entry, key, value).map_err(located)?;
    }
    Ok(entries)
}

pub fn validate(entries: &[Entry]) -> Result<(), Vec<String>> {
    let mut diagnostics = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let label = format!("entry {} ({})", index + 1, entry.commit);
        if entry.commit.is_empty() {
            diagnostics.push(format!("entry {} names no commit", index + 1));
        }
        for platform in REQUIRED_PLATFORMS {
            if !entry.digests.iter().any(|(name, _)| name == platform) {
                diagnostics.push(format!("{label} names no {platform} digest"));
            }
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
    if diagnostics.is_empty() {
        Ok(())
    } else {
        Err(diagnostics)
    }
}

pub fn unchanged_since_base(base: &[Entry], head: &[Entry]) -> Result<(), Vec<String>> {
    let diagnostics: Vec<String> = base
        .iter()
        .enumerate()
        .filter_map(|(index, kept)| match head.get(index) {
            Some(same) if same == kept => None,
            _ => Some(format!(
                "entry {} ({}) changed or moved after its merge",
                index + 1,
                kept.commit
            )),
        })
        .collect();
    if diagnostics.is_empty() {
        Ok(())
    } else {
        Err(diagnostics)
    }
}

pub fn publication_commits(entries: &[Entry]) -> Vec<(String, String)> {
    entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let since = index
                .checked_sub(1)
                .map(|previous| entries[previous].commit.clone())
                .or_else(|| entry.since.clone())
                .unwrap_or_default();
            (entry.commit.clone(), since)
        })
        .collect()
}

pub fn reference(platform: &str, commit: &str) -> String {
    [
        REGISTRY,
        PATH_SEPARATOR,
        REPOSITORY_PREFIX,
        platform,
        TAG_SEPARATOR,
        commit,
    ]
    .concat()
}

pub fn entries_at(root: &Path, revision: Option<&str>) -> Result<Vec<Entry>, Vec<String>> {
    let text = match revision {
        None => crate::read(&root.join(FILE))?,
        Some(revision) if crate::listed_at(root, revision, FILE)? => {
            crate::git_in(root, &["show", &[revision, FILE].join(TAG_SEPARATOR)])?
        }
        Some(_) => String::new(),
    };
    let entries = parse(&text)?;
    validate(&entries)?;
    Ok(entries)
}

fn commits_exist(root: &Path, entries: &[Entry]) -> Result<(), Vec<String>> {
    entries
        .iter()
        .flat_map(|entry| std::iter::once(&entry.commit).chain(entry.since.as_ref()))
        .try_for_each(|commit| crate::commit_of(root, commit).map(drop))
}

#[cfg(feature = "registry")]
fn registry_diagnostics(entries: &[Entry]) -> Result<Vec<String>, Vec<String>> {
    use oci_client::secrets::RegistryAuth;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| vec![format!("cannot start the runtime: {e}")])?;
    let client = oci_client::Client::new(oci_client::client::ClientConfig::default());
    runtime.block_on(async {
        let mut diagnostics = Vec::new();
        for entry in entries {
            for (platform, digest) in &entry.digests {
                let name = reference(platform, &entry.commit);
                let image = oci_client::Reference::try_from(name.as_str())
                    .map_err(|e| vec![format!("{name}: {e}")])?;
                match client
                    .pull_image_manifest(&image, &RegistryAuth::Anonymous)
                    .await
                {
                    Err(e) => diagnostics.push(format!("{name}: {e}")),
                    Ok((manifest, found)) => {
                        if found != *digest {
                            diagnostics.push(format!(
                                "{name} resolves to {found}, and the entry names {digest}"
                            ));
                        }
                        let revision = manifest
                            .annotations
                            .as_ref()
                            .and_then(|annotations| annotations.get(REVISION_ANNOTATION));
                        if revision != Some(&entry.commit) {
                            diagnostics.push(format!(
                                "{name} carries {REVISION_ANNOTATION} {revision:?}, not its commit"
                            ));
                        }
                    }
                }
            }
        }
        Ok(diagnostics)
    })
}

#[cfg(not(feature = "registry"))]
fn registry_diagnostics(_entries: &[Entry]) -> Result<Vec<String>, Vec<String>> {
    Err(vec![format!(
        "{BINARY} was built without the registry feature and cannot ask the registry"
    )])
}

/// - Reads `bindings/published.toml` and, with `--base`, its copy at that git revision.
/// - Runs `git` child processes in `root`.
/// - Fetches one manifest per entry and platform from the registry, anonymously.
pub fn dispatch(root: &Path, args: &[String]) -> Result<(), Vec<String>> {
    match args.first().map(String::as_str) {
        Some(CHECK_FLAG) => check(root, args),
        Some(NEWEST_FLAG) => newest(root),
        Some(RECORD_FLAG) => record(root, args),
        _ => Err(vec![USAGE.to_string()]),
    }
}

fn check(root: &Path, args: &[String]) -> Result<(), Vec<String>> {
    let head = entries_at(root, None)?;
    commits_exist(root, &head)?;
    if let Some(base) = crate::flag_value(args, BASE_FLAG)? {
        unchanged_since_base(&entries_at(root, Some(base))?, &head)?;
    }
    let diagnostics = registry_diagnostics(&head)?;
    if diagnostics.is_empty() {
        Ok(())
    } else {
        Err(diagnostics)
    }
}

fn newest(root: &Path) -> Result<(), Vec<String>> {
    let entries = entries_at(root, None)?;
    let last = entries
        .last()
        .ok_or_else(|| vec![format!("{FILE} holds no entry")])?;
    println!("{}", last.commit);
    Ok(())
}

fn record(root: &Path, args: &[String]) -> Result<(), Vec<String>> {
    let required = |flag: &str| crate::required_flag(args, flag, USAGE);
    let commit = required(COMMIT_FLAG)?;
    let platform = required(PLATFORM_FLAG)?;
    let digest = required(DIGEST_FLAG)?;
    let file = root.join(FILE);
    let text = recorded(&crate::read(&file)?, commit, platform, digest)?;
    std::fs::write(&file, text).map_err(|e| vec![format!("cannot write {}: {e}", file.display())])
}

pub fn recorded(
    text: &str,
    commit: &str,
    platform: &str,
    digest: &str,
) -> Result<String, Vec<String>> {
    if !PLATFORMS.contains(&platform) {
        return Err(vec![format!("unknown platform {platform}")]);
    }
    let line = format!(
        "{platform} = {QUOTE}{}{QUOTE}",
        digest_value(platform, digest)?
    );
    let mut lines: Vec<String> = Vec::new();
    let mut in_target = false;
    let mut written = false;
    for raw in text.lines() {
        let trimmed = raw.trim();
        if trimmed == ENTRY_HEADER {
            written |= in_target && !written && insert_after_text(&mut lines, &line);
            in_target = false;
        }
        match assignment(trimmed) {
            Some((COMMIT_KEY, value)) => in_target = value == commit,
            Some((key, _)) if in_target && key == platform => {
                lines.push(line.clone());
                written = true;
                continue;
            }
            _ => {}
        }
        lines.push(raw.to_string());
    }
    written |= in_target && !written && insert_after_text(&mut lines, &line);
    if !written {
        return Err(vec![format!("{FILE} holds no entry for {commit}")]);
    }
    let mut joined = lines.join("\n");
    joined.push('\n');
    validate(&parse(&joined)?)?;
    Ok(joined)
}

fn insert_after_text(lines: &mut Vec<String>, line: &str) -> bool {
    let at = lines
        .iter()
        .rposition(|existing| !existing.trim().is_empty())
        .map_or(lines.len(), |index| index + 1);
    lines.insert(at, line.to_string());
    true
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

    fn entry(commit: &str, since: Option<&str>, platforms: &[&str]) -> Entry {
        Entry {
            commit: commit.to_string(),
            since: since.map(str::to_string),
            digests: platforms
                .iter()
                .map(|platform| (platform.to_string(), DIGEST.to_string()))
                .collect(),
        }
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
                format!(
                    "# a comment\n\n[[entry]]\ncommit = \"{}\"\n{since}{digests}",
                    entry.commit
                )
            })
            .collect()
    }

    #[test]
    fn the_manifest_round_trips_through_its_text_form() {
        let entries = vec![
            entry(FIRST, Some(ORIGIN), &["android", "ios"]),
            entry(SECOND, None, &["android"]),
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
        ];
        for text in rejected {
            let diagnostic = parse(text).unwrap_err().concat();
            assert!(diagnostic.starts_with(FILE), "{text}: {diagnostic}");
        }
        assert!(parse("[[entry]]\nandroid = \"x\"\nandroid = \"y\"\n").is_err());
    }

    #[test]
    fn validation_pins_the_since_rule_the_digest_rule_and_uniqueness() {
        let diagnostics = validate(&[
            entry(FIRST, None, &[]),
            entry(SECOND, Some(ORIGIN), &["ios"]),
            entry(SECOND, None, &["android"]),
        ])
        .unwrap_err()
        .concat();
        assert!(diagnostics.contains("first and names no since"));
        assert!(diagnostics.contains("names no android digest"));
        assert_eq!(
            validate(&[entry(FIRST, Some(ORIGIN), &["android"])]),
            Ok(())
        );
        assert!(validate(&[entry(FIRST, Some(ORIGIN), &["ios"])]).is_err());
        assert!(diagnostics.contains("only the first entry may"));
        assert!(diagnostics.contains("repeats an earlier entry"));
    }

    #[test]
    fn a_merged_entry_never_changes_and_the_head_may_only_append() {
        let base = vec![entry(FIRST, Some(ORIGIN), &["android"])];
        let appended = vec![base[0].clone(), entry(SECOND, None, &["android"])];
        assert_eq!(unchanged_since_base(&base, &appended), Ok(()));
        let edited = vec![entry(FIRST, Some(ORIGIN), &["android", "ios"])];
        assert!(unchanged_since_base(&base, &edited).is_err());
        assert!(unchanged_since_base(&base, &[]).is_err());
        let reordered = vec![entry(SECOND, Some(ORIGIN), &["android"]), base[0].clone()];
        assert!(unchanged_since_base(&base, &reordered).is_err());
    }

    #[test]
    fn each_publication_follows_the_previous_entry_and_the_first_its_since() {
        let entries = vec![
            entry(FIRST, Some(ORIGIN), &["android"]),
            entry(SECOND, None, &["android"]),
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
            reference("android", FIRST),
            format!("ghcr.io/zingolabs/zingolib/binding-layer-android:{FIRST}")
        );
    }

    #[test]
    fn recording_a_digest_replaces_the_platform_line_or_adds_one_to_the_named_entry() {
        let text = manifest(&[
            entry(FIRST, Some(ORIGIN), &["android"]),
            entry(SECOND, None, &["android"]),
        ]);
        let replaced = DIGEST.replace('0', "f");
        let with_ios = recorded(&text, SECOND, "ios", &replaced).unwrap();
        assert_eq!(
            parse(&with_ios).unwrap()[1].digests,
            [
                ("android".to_string(), DIGEST.to_string()),
                ("ios".to_string(), replaced.clone())
            ]
        );
        let re_android = recorded(&with_ios, FIRST, "android", &replaced).unwrap();
        assert_eq!(
            parse(&re_android).unwrap()[0].digests,
            [("android".to_string(), replaced)]
        );
        assert!(recorded(&text, ORIGIN, "android", DIGEST).is_err());
        assert!(recorded(&text, SECOND, "linux", DIGEST).is_err());
        assert!(recorded(&text, SECOND, "android", "sha256:short").is_err());
    }

    #[test]
    fn the_committed_manifest_parses_and_validates() {
        let root = crate::repo_root().unwrap();
        assert!(entries_at(&root, None).is_ok());
    }
}
