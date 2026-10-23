use std::collections::{BTreeMap, BTreeSet};

use crate::{run_streaming, stdout_of_owned, GH};

const CACHE: &str = "cache";
pub const SAVING_REF: &str = "refs/heads/dev";
const LIST_LIMIT: &str = "500";
const LISTING: &str = r#".[] | "\(.id)\t\(.key)\t\(.createdAt)""#;
const RUST_CACHE_PREFIX: &str = "v0-rust-";
// The nightly saves the synced testnet faucet wallet as `testnet-faucet-<run
// id>`, one entry per night, restored by the family name alone.
const FAUCET_FAMILY: &str = "testnet-faucet";
const LOCKFILE_HASH_LEN: usize = 8;
pub const KEPT_PER_FAMILY: usize = 2;

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
    pub created_at: String,
}

// A family key is `<family>-<suffix>`: a run id after the faucet family, or
// a lockfile hash after a rust-cache family. Every other key is its own family.
pub fn family(key: &str) -> &str {
    let Some((stem, suffix)) = key.rsplit_once('-') else {
        return key;
    };
    let is_member = if stem == FAUCET_FAMILY {
        !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
    } else if key.starts_with(RUST_CACHE_PREFIX) {
        suffix.len() == LOCKFILE_HASH_LEN && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
    } else {
        false
    };
    if is_member {
        stem
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
            match (fields.next(), fields.next(), fields.next()) {
                (Some(id), Some(key), Some(created_at))
                    if !id.is_empty() && !key.is_empty() && !created_at.is_empty() =>
                {
                    Ok(Entry {
                        id: id.to_string(),
                        key: key.to_string(),
                        created_at: created_at.to_string(),
                    })
                }
                _ => Err(vec![format!(
                    "cache listing line {line:?} lacks an id, a key and a creation time"
                )]),
            }
        })
        .collect()
}

pub fn newest_first(mut entries: Vec<Entry>) -> Vec<Entry> {
    entries.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    entries
}

pub fn superseded(entries: Vec<Entry>) -> Vec<Entry> {
    let mut kept: BTreeMap<String, usize> = BTreeMap::new();
    newest_first(entries)
        .into_iter()
        .filter(|entry| {
            let count = kept.entry(family(&entry.key).to_string()).or_default();
            *count += 1;
            *count > KEPT_PER_FAMILY
        })
        .collect()
}

pub fn settle(failed: Vec<Entry>, remaining: &[Entry]) -> (Vec<Entry>, Vec<Entry>) {
    let listed: BTreeSet<&str> = remaining.iter().map(|entry| entry.id.as_str()).collect();
    failed
        .into_iter()
        .partition(|entry| !listed.contains(entry.id.as_str()))
}

#[derive(PartialEq, Eq, Debug, Default)]
pub struct Pruned {
    pub deleted: Vec<Entry>,
    pub gone: Vec<Entry>,
}

const DELETED: &str = "deleted";
const GONE: &str = "gone";

pub fn outcome_lines<'a>(
    deleted: &'a [Entry],
    gone: &'a [Entry],
) -> impl Iterator<Item = String> + 'a {
    std::iter::repeat(DELETED)
        .zip(deleted)
        .chain(std::iter::repeat(GONE).zip(gone))
        .map(|(outcome, entry)| format!("{outcome} {} {}", entry.id, entry.key))
}

fn progress(deleted: &[Entry], gone: &[Entry], unsettled: usize) -> Vec<String> {
    outcome_lines(deleted, gone)
        .chain(std::iter::once(format!(
            "deleted {}, found {} gone, left {} of {} superseded entries",
            deleted.len(),
            gone.len(),
            unsettled,
            deleted.len() + gone.len() + unsettled
        )))
        .collect()
}

pub fn report(
    deleted: Vec<Entry>,
    gone: Vec<Entry>,
    stuck: Vec<Entry>,
) -> Result<Pruned, Vec<String>> {
    if stuck.is_empty() {
        return Ok(Pruned { deleted, gone });
    }
    Err(stuck
        .iter()
        .map(|entry| {
            format!(
                "`gh cache delete {}` failed and the entry {} is still listed",
                entry.id, entry.key
            )
        })
        .chain(progress(&deleted, &gone, stuck.len()))
        .collect())
}

/// - Runs `gh cache list` as a child process and waits for it.
/// - Runs `gh cache delete` once per superseded entry, each as a child process.
/// - Runs `gh cache list` again when a delete failed, and waits for it.
pub fn prune() -> Result<Pruned, Vec<String>> {
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
        return Ok(Pruned {
            deleted,
            gone: Vec::new(),
        });
    }
    let remaining = match stdout_of_owned(GH, &list_args()).and_then(|listing| entries(&listing)) {
        Ok(remaining) => remaining,
        Err(mut diagnostics) => {
            diagnostics.extend(progress(&deleted, &[], failed.len()));
            return Err(diagnostics);
        }
    };
    let (gone, stuck) = settle(failed, &remaining);
    report(deleted, gone, stuck)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, key: &str) -> Entry {
        dated(id, key, "2026-10-16T03:00:00Z")
    }

    fn dated(id: &str, key: &str, created_at: &str) -> Entry {
        Entry {
            id: id.to_string(),
            key: key.to_string(),
            created_at: created_at.to_string(),
        }
    }

    fn ids(entries: Vec<Entry>) -> Vec<String> {
        entries.into_iter().map(|entry| entry.id).collect()
    }

    #[test]
    fn the_family_is_the_key_without_its_lockfile_hash() {
        assert_eq!(
            family("v0-rust-nym-feature-Linux-x64-26e84094-c5b5aea0"),
            "v0-rust-nym-feature-Linux-x64-26e84094"
        );
    }

    #[test]
    fn the_faucet_entries_share_one_family_keyed_by_run_id() {
        assert_eq!(family("testnet-faucet-39829487746"), "testnet-faucet");
        assert_eq!(family("testnet-faucet-1"), "testnet-faucet");
        assert_eq!(family("testnet-faucet-"), "testnet-faucet-");
        assert_eq!(family("testnet-faucet-notarun"), "testnet-faucet-notarun");
    }

    #[test]
    fn a_key_without_a_hash_suffix_is_its_own_family() {
        assert_eq!(family("cargo-dupes-Linux"), "cargo-dupes-Linux");
        assert_eq!(family("v0-rust-x-notahash"), "v0-rust-x-notahash");
        assert_eq!(family("nodash"), "nodash");
    }

    #[test]
    fn a_key_outside_rust_cache_is_its_own_family_whatever_its_suffix() {
        assert_eq!(
            family("foo-Linux-deadbeef"),
            "foo-Linux-deadbeef",
            "an exact-key cache may end in eight hex digits"
        );
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
    fn the_two_newest_entries_of_each_family_survive() {
        let listing = vec![
            entry("1", "v0-rust-a-26e84094-c5b5aea0"),
            entry("2", "v0-rust-b-26e84094-c5b5aea0"),
            entry("3", "v0-rust-a-26e84094-2afa9efd"),
            entry("4", "v0-rust-a-26e84094-15153fd1"),
            entry("5", "v0-rust-b-26e84094-2afa9efd"),
            entry("6", "v0-rust-a-26e84094-0badf00d"),
        ];
        assert_eq!(
            ids(superseded(listing)),
            ["4", "6"],
            "the listing is newest first and a family keeps {KEPT_PER_FAMILY}"
        );
    }

    #[test]
    fn the_creation_time_orders_the_entries_not_the_listing() {
        let listing = vec![
            dated("old", "v0-rust-a-26e84094-c5b5aea0", "2026-10-14T03:00:00Z"),
            dated("mid", "v0-rust-a-26e84094-2afa9efd", "2026-10-15T03:00:00Z"),
            dated("new", "v0-rust-a-26e84094-15153fd1", "2026-10-16T03:00:00Z"),
        ];
        assert_eq!(ids(newest_first(listing.clone())), ["new", "mid", "old"]);
        assert_eq!(ids(superseded(listing)), ["old"]);
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
    fn a_report_without_stuck_entries_keeps_the_gone_ones() {
        let pruned = report(vec![entry("1", "a")], vec![entry("2", "b")], Vec::new())
            .expect("no entry is stuck");
        assert_eq!(pruned.deleted, [entry("1", "a")]);
        assert_eq!(pruned.gone, [entry("2", "b")]);
    }

    #[test]
    fn a_report_with_a_stuck_entry_still_tells_the_progress() {
        let error = report(
            vec![entry("1", "a")],
            vec![entry("2", "b")],
            vec![entry("3", "c")],
        )
        .expect_err("one entry is stuck");
        assert_eq!(
            error,
            [
                "`gh cache delete 3` failed and the entry c is still listed",
                "deleted 1 a",
                "gone 2 b",
                "deleted 1, found 1 gone, left 1 of 3 superseded entries",
            ]
        );
    }

    #[test]
    fn a_listing_parses_into_ids_keys_and_creation_times() {
        let parsed = entries("11\tv0-rust-a-26e84094-c5b5aea0\t2026-10-16T03:00:00Z\n\n12\tk\tt\n")
            .expect("both lines carry an id, a key and a creation time");
        assert_eq!(
            parsed,
            [
                dated("11", "v0-rust-a-26e84094-c5b5aea0", "2026-10-16T03:00:00Z"),
                dated("12", "k", "t")
            ]
        );
    }

    #[test]
    fn a_line_without_a_creation_time_is_a_diagnostic() {
        let error = entries("11\tk\n").expect_err("the line lacks a creation time");
        assert!(
            error[0].contains("lacks an id, a key and a creation time"),
            "{error:?}"
        );
        let error = entries("11\n").expect_err("the line holds only an id");
        assert!(
            error[0].contains("lacks an id, a key and a creation time"),
            "{error:?}"
        );
    }

    #[test]
    fn the_listing_asks_for_the_saving_branch_newest_first() {
        let args = list_args();
        assert!(args.windows(2).any(|pair| pair == ["--ref", SAVING_REF]));
        assert!(args.windows(2).any(|pair| pair == ["--order", "desc"]));
    }
}
