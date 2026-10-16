use std::collections::BTreeSet;

use crate::{run_streaming, stdout_of_owned};

const GH: &str = "gh";
const CACHE: &str = "cache";
pub const SAVING_REF: &str = "refs/heads/dev";
const LIST_LIMIT: &str = "500";
const LISTING: &str = r#".[] | "\(.id)\t\(.key)\t\(.createdAt)""#;
const LOCKFILE_HASH_LEN: usize = 8;

pub fn list_args() -> Vec<String> {
    [
        CACHE,
        "list",
        "--ref",
        SAVING_REF,
        "--limit",
        LIST_LIMIT,
        "--sort",
        "created_at",
        "--order",
        "desc",
        "--json",
        "id,key,createdAt",
        "--jq",
        LISTING,
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Entry {
    pub id: String,
    pub key: String,
}

pub fn family(key: &str) -> &str {
    let Some((prefix, suffix)) = key.rsplit_once('-') else {
        return key;
    };
    let is_hash =
        suffix.len() == LOCKFILE_HASH_LEN && suffix.bytes().all(|byte| byte.is_ascii_hexdigit());
    if is_hash {
        prefix
    } else {
        key
    }
}

pub fn entries(listing: &str) -> Result<Vec<Entry>, Vec<String>> {
    listing
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let mut fields = line.split('\t');
            match (fields.next(), fields.next()) {
                (Some(id), Some(key)) if !id.is_empty() && !key.is_empty() => Ok(Entry {
                    id: id.to_string(),
                    key: key.to_string(),
                }),
                _ => Err(vec![format!(
                    "cache listing line {line:?} lacks an id and a key"
                )]),
            }
        })
        .collect()
}

pub fn superseded(entries: Vec<Entry>) -> Vec<Entry> {
    let mut seen = BTreeSet::new();
    entries
        .into_iter()
        .filter(|entry| !seen.insert(family(&entry.key).to_string()))
        .collect()
}

pub fn settle(failed: Vec<Entry>, remaining: &[Entry]) -> (Vec<Entry>, Vec<Entry>) {
    let listed: BTreeSet<&str> = remaining.iter().map(|entry| entry.id.as_str()).collect();
    failed
        .into_iter()
        .partition(|entry| !listed.contains(entry.id.as_str()))
}

/// - Runs `gh cache list` as a child process and waits for it.
/// - Runs `gh cache delete` once per superseded entry, each as a child process.
/// - Runs `gh cache list` again when a delete failed, and waits for it.
pub fn prune() -> Result<Vec<Entry>, Vec<String>> {
    let doomed = superseded(entries(&stdout_of_owned(GH, &list_args())?)?);
    let mut deleted = Vec::new();
    let mut failed = Vec::new();
    for entry in doomed {
        match run_streaming(GH, &[CACHE, "delete", &entry.id], &[]) {
            Ok(()) => deleted.push(entry),
            Err(_) => failed.push(entry),
        }
    }
    if failed.is_empty() {
        return Ok(deleted);
    }
    let remaining = entries(&stdout_of_owned(GH, &list_args())?)?;
    let (_gone, stuck) = settle(failed, &remaining);
    if stuck.is_empty() {
        return Ok(deleted);
    }
    Err(stuck
        .iter()
        .map(|entry| {
            format!(
                "`gh cache delete {}` failed and the entry {} is still listed",
                entry.id, entry.key
            )
        })
        .chain(std::iter::once(format!(
            "deleted {} of {} superseded entries",
            deleted.len(),
            deleted.len() + stuck.len()
        )))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, key: &str) -> Entry {
        Entry {
            id: id.to_string(),
            key: key.to_string(),
        }
    }

    #[test]
    fn the_family_is_the_key_without_its_lockfile_hash() {
        assert_eq!(
            family("v0-rust-nym-feature-Linux-x64-26e84094-c5b5aea0"),
            "v0-rust-nym-feature-Linux-x64-26e84094"
        );
    }

    #[test]
    fn a_key_without_a_hash_suffix_is_its_own_family() {
        assert_eq!(family("cargo-dupes-Linux"), "cargo-dupes-Linux");
        assert_eq!(family("v0-rust-x-notahash"), "v0-rust-x-notahash");
        assert_eq!(family("nodash"), "nodash");
    }

    #[test]
    fn a_setup_gradle_key_is_its_own_family() {
        let home = "gradle-home-v1|Linux-X64|package[b8a35c2f917259958c9abe8f2ff1b5ff]-5d859920e0f6ab6fbc6e0d4c1a1b2c3d4e5f6071";
        assert_eq!(family(home), home, "the commit sha is not a lockfile hash");
        let bundle = "gradle-dependencies-v1-b8a35c2f917259958c9abe8f2ff1b5ff";
        assert_eq!(
            family(bundle),
            bundle,
            "a bundle is restored by its exact key"
        );
    }

    #[test]
    fn the_newest_entry_of_each_family_survives() {
        let listing = vec![
            entry("1", "v0-rust-a-26e84094-c5b5aea0"),
            entry("2", "v0-rust-b-26e84094-c5b5aea0"),
            entry("3", "v0-rust-a-26e84094-2afa9efd"),
            entry("4", "v0-rust-a-26e84094-15153fd1"),
            entry("5", "v0-rust-b-26e84094-2afa9efd"),
        ];
        let doomed: Vec<String> = superseded(listing)
            .into_iter()
            .map(|entry| entry.id)
            .collect();
        assert_eq!(doomed, ["3", "4", "5"], "the listing is newest first");
    }

    #[test]
    fn a_failed_delete_of_an_entry_no_longer_listed_is_done() {
        let failed = vec![entry("1", "a"), entry("2", "b")];
        let remaining = [entry("2", "b"), entry("9", "z")];
        let (gone, stuck) = settle(failed, &remaining);
        assert_eq!(gone, [entry("1", "a")]);
        assert_eq!(stuck, [entry("2", "b")]);
    }

    #[test]
    fn a_listing_parses_into_ids_and_keys() {
        let parsed = entries("11\tv0-rust-a-26e84094-c5b5aea0\t2026-10-16T03:00:00Z\n\n12\tk\tt\n")
            .expect("both lines carry an id and a key");
        assert_eq!(
            parsed,
            [entry("11", "v0-rust-a-26e84094-c5b5aea0"), entry("12", "k")]
        );
    }

    #[test]
    fn a_line_without_a_key_is_a_diagnostic() {
        let error = entries("11\n").expect_err("the line holds only an id");
        assert!(error[0].contains("lacks an id and a key"), "{error:?}");
    }

    #[test]
    fn the_listing_asks_for_the_saving_branch_newest_first() {
        let args = list_args();
        assert!(args.windows(2).any(|pair| pair == ["--ref", SAVING_REF]));
        assert!(args.windows(2).any(|pair| pair == ["--order", "desc"]));
    }
}
