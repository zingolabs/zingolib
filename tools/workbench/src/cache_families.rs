use std::collections::{BTreeMap, BTreeSet};

use crate::{run_streaming, stdout_of_owned, GH};

const CACHE: &str = "cache";
pub const SAVING_REF: &str = "refs/heads/dev";
const LIST_LIMIT: &str = "500";
const LISTING: &str = r#".[] | "\(.id)\t\(.key)\t\(.createdAt)""#;
const RUST_CACHE_PREFIX: &str = "v0-rust-";
// The nightly saves the synced testnet faucet wallet under this prefix and
// the run id, one entry per night, restored by the prefix alone.
const FAUCET_CACHE_PREFIX: &str = "testnet-faucet-";
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

pub fn family(key: &str) -> &str {
    if let Some(run_id) = key.strip_prefix(FAUCET_CACHE_PREFIX) {
        if !run_id.is_empty() && run_id.bytes().all(|byte| byte.is_ascii_digit()) {
            return FAUCET_CACHE_PREFIX.trim_end_matches('-');
        }
        return key;
    }
    if !key.starts_with(RUST_CACHE_PREFIX) {
        return key;
    }
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
mod tests;
