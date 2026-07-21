//! The curated Broadcast Indexer list.
//!
//! This list is kept deliberately separate from the sync-server list
//! (`zingo-cli`'s `most_up_indexer_uris`): broadcast targets are chosen for
//! reliable transaction relay, sync servers for low query latency, so tuning
//! one must not reshape the other.
//!
//! # Provenance
//!
//! Populated 2026-07-21 from a three-way discovery sweep — the hosh.zec.rocks
//! tracker (via its 2026-04-18 Internet Archive snapshot; the live site was
//! down), the hardcoded server lists of open-source Zcash wallets (Ywallet,
//! Zashi, zingo-mobile, zingo-pc, Cake, Unstoppable, Nerdbank, zecwallet
//! lineage), and a Zcash community-forum / ZecHub web sweep — yielding 130
//! candidate endpoints. Every candidate was then probed live with a
//! `GetLightdInfo` gRPC call; exactly 19 answered on mainnet, all lightwalletd
//! instances synced to the same chain tip. Every zaino deployment and the
//! entire `lightwalletd.com` and `zcash-infra.com` fleets were dead.
//!
//! The entries below are those 19 survivors deduplicated to ONE endpoint per
//! operator, because the party Witness Rotation defends against is the
//! operator, not the DNS name: a uniform pick over an operator-diverse pool
//! spreads sends across accumulating parties, where a pool listing one
//! operator's many regional endpoints would overweight that operator.
//! Live regional variants folded into their operator's single entry:
//! - `zec.rocks:443` also answers as `na.`, `sa.`, `eu.`, `ap.zec.rocks:443`
//!   (`me.`, `zcashd.`, and `zaino.unsafe.` variants were dead).
//! - `us.zec.stardust.rest:443` also answers as `eu.zec.stardust.rest:443`
//!   (`eu2.` and `jp.` were dead).
//!
//! Distinct-operator status is inferred from domains and confirmed as far as
//! observable — the 14 entries resolve to 14 unrelated IPs — but operator
//! identity is ultimately self-asserted, and a sybil operator running several
//! entries would weaken rotation. Operational vetting of this list (liveness
//! over time, relay honesty, operator diversity) is a tracked follow-up; see
//! `docs/adr/0011-nym-mixnet-transmission.md`.

#![forbid(unsafe_code)]

use http::Uri;

/// Curated broadcast targets (mainnet): every publicly reachable indexer
/// found by the 2026-07-21 discovery sweep, one endpoint per operator. See
/// the module docs for provenance and the operator-diversity rationale.
pub const BROADCAST_INDEXERS: &[&str] = &[
    "https://zec.rocks:443",
    "https://us.zec.stardust.rest:443",
    "https://zec-node.cakewallet.com:443",
    "https://lwd.zcashexplorer.app:9067",
    "https://lightwalletd.mainnet.cipherscan.app:443",
    "https://lwd.z0n.jp:443",
    "https://l.ombie.cash:443",
    "https://zec.0xrpc.io:443",
    "https://zec.alexxiy.top:9067",
    "https://carover0.xyz:9067",
    "https://myzec.cryptover.site:443",
    "https://zcashlw.devshore.ovh:443",
    "https://znode.roamerx.win:443",
    "https://webhighway.website:443",
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
    fn every_entry_parses() {
        assert_eq!(
            broadcast_indexers().len(),
            BROADCAST_INDEXERS.len(),
            "every broadcast URI must parse"
        );
    }

    #[test]
    fn one_endpoint_per_operator() {
        // Witness Rotation defends against the operator, not the DNS name:
        // no two entries may share a registrable parent domain.
        let operator_domains: Vec<String> = BROADCAST_INDEXERS
            .iter()
            .map(|entry| {
                let host = entry
                    .parse::<Uri>()
                    .expect("checked by every_entry_parses")
                    .host()
                    .expect("every broadcast URI has a host")
                    .to_string();
                let labels: Vec<&str> = host.rsplit('.').collect();
                labels
                    .iter()
                    .take(2)
                    .rev()
                    .copied()
                    .collect::<Vec<_>>()
                    .join(".")
            })
            .collect();
        let mut deduped = operator_domains.clone();
        deduped.sort();
        deduped.dedup();
        assert_eq!(
            deduped.len(),
            operator_domains.len(),
            "two broadcast entries share an operator domain: {operator_domains:?}"
        );
    }
}
