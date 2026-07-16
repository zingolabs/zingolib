//! The curated Broadcast Indexer list.
//!
//! This list is kept deliberately separate from the sync-server list
//! (`zingo-cli`'s `most_up_indexer_uris`): broadcast targets are chosen for
//! reliable transaction relay, sync servers for low query latency, so tuning
//! one must not reshape the other. The separation is structural — this is its
//! own module with its own entries — even while the initial contents overlap
//! the sync list.
//!
//! PROVISIONAL: these entries are placeholders pending operational selection
//! of roughly ten reliable, low-latency mainnet indexers. They are not yet a
//! vetted broadcast set. See `docs/adr/0011-nym-mixnet-transmission.md`.
#![forbid(unsafe_code)]

use http::Uri;

/// Provisional curated broadcast targets (mainnet). See the module docs: this
/// is a placeholder pending an operationally vetted set.
pub const BROADCAST_INDEXERS: &[&str] = &[
    "https://zec.rocks:443",
    "https://zecnode.sarl:443",
    "https://zwallet.techly.fyi:443",
    "https://zw.run.place:443",
    "https://light.tracier.space:443",
    "https://webhighway.website:443",
    "https://zcash.johndo.men:443",
    "https://zecwal.sandycat.cc:443",
    "https://lw.chponks.site:443",
    "https://zec.rollrunner.info:443",
];

/// Parses [`BROADCAST_INDEXERS`] into `Uri`s, skipping any that fail to parse.
pub fn broadcast_indexers() -> Vec<Uri> {
    BROADCAST_INDEXERS
        .iter()
        .filter_map(|entry| entry.parse().ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_provisional_entry_parses() {
        assert_eq!(
            broadcast_indexers().len(),
            BROADCAST_INDEXERS.len(),
            "every provisional broadcast URI must parse"
        );
    }
}
