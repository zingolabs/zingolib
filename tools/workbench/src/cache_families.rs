use crate::{run_streaming, stdout_of};

const GH: &str = "gh";
const CACHE: &str = "cache";

/// The branch whose entries the saves target; pull requests only restore.
pub const SAVING_REF: &str = "refs/heads/dev";

/// The store holds a few hundred entries at most; this bound lists them all.
const LIST_LIMIT: &str = "500";

/// One line per entry, newest first: the id, the key and the creation time.
const LISTING: &str = r#".[] | "\(.id)\t\(.key)\t\(.createdAt)""#;

/// rust-cache appends the lockfile hash as the last eight hex digits of a
/// key; the key without it is the restore prefix every job restores by.
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

/// The entries to delete from a newest-first listing: every entry whose
/// family already has a newer one.
pub fn superseded(entries: &[Entry]) -> Vec<&Entry> {
    let mut seen = std::collections::BTreeSet::new();
    entries
        .iter()
        .filter(|entry| !seen.insert(family(&entry.key)))
        .collect()
}

/// - Runs `gh cache list` as a child process and waits for it.
/// - Runs `gh cache delete` once per superseded entry, each as a child process.
pub fn prune() -> Result<Vec<Entry>, Vec<String>> {
    let args = list_args();
    let listing = stdout_of(GH, &args.iter().map(String::as_str).collect::<Vec<_>>())?;
    let entries = entries(&listing)?;
    let doomed: Vec<Entry> = superseded(&entries).into_iter().cloned().collect();
    for entry in &doomed {
        run_streaming(GH, &[CACHE, "delete", &entry.id], &[])?;
    }
    Ok(doomed)
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
    fn the_newest_entry_of_each_family_survives() {
        let listing = [
            entry("1", "v0-rust-a-26e84094-c5b5aea0"),
            entry("2", "v0-rust-b-26e84094-c5b5aea0"),
            entry("3", "v0-rust-a-26e84094-2afa9efd"),
            entry("4", "v0-rust-a-26e84094-15153fd1"),
            entry("5", "v0-rust-b-26e84094-2afa9efd"),
        ];
        let doomed: Vec<&str> = superseded(&listing)
            .into_iter()
            .map(|entry| entry.id.as_str())
            .collect();
        assert_eq!(doomed, ["3", "4", "5"], "the listing is newest first");
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
