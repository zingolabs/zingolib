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
//! observable — the entries resolve to unrelated IPs — but operator identity
//! is ultimately self-asserted, and a sybil operator running several entries
//! would weaken rotation. Operational vetting of this list (liveness over
//! time, relay honesty, operator diversity) is a tracked follow-up; see
//! `docs/adr/0011-nym-mixnet-transmission.md`.
//!
//! # Port-443 restriction
//!
//! This list carries only Transmission targets reached over the mixnet, so an
//! endpoint the mixnet cannot reach is worse than useless — it wastes fan-out
//! rounds on a certain failure. A 2026-07-21 paired clearnet/mixnet probe (the
//! `nym probe` diagnostic) found a clean split: every port-443 witness answered
//! over the mixnet, while all three port-9067 witnesses
//! (`lwd.zcashexplorer.app`, `zec.alexxiy.top`, `carover0.xyz`) completed the
//! SOCKS5 tunnel but then failed the TLS handshake with an EOF — the mixnet
//! exit gateways relay the standard 443 but mishandle the non-standard
//! lightwalletd port. Their clearnet TLS works, so this is a mixnet-reachability
//! property, not a dead host; because the list is mixnet-only, they are
//! excluded. Any future entry MUST be port 443 for the same reason, a rule the
//! `every_entry_is_port_443` test enforces.

#![forbid(unsafe_code)]

use http::Uri;

/// Curated broadcast targets (mainnet): the publicly reachable indexers found
/// by the 2026-07-21 discovery sweep, one endpoint per operator, restricted to
/// those the mixnet can actually reach. See the module docs for provenance,
/// the operator-diversity rationale, and the port-443 restriction.
pub const BROADCAST_INDEXERS: &[&str] = &[
    "https://zec.rocks:443",
    "https://us.zec.stardust.rest:443",
    "https://zec-node.cakewallet.com:443",
    "https://lightwalletd.mainnet.cipherscan.app:443",
    "https://lwd.z0n.jp:443",
    "https://l.ombie.cash:443",
    "https://zec.0xrpc.io:443",
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
    fn every_entry_is_https() {
        // Mixnet transmission is TLS-only end to end, so the exit gateway that
        // terminates the tunnel cannot read or tamper with the traffic. Every
        // broadcast target must be https; the transmit path refuses anything
        // else at dial time.
        for entry in broadcast_indexers() {
            assert_eq!(
                entry.scheme_str(),
                Some("https"),
                "broadcast entry {entry} must be https"
            );
        }
    }

    #[test]
    fn every_entry_is_port_443() {
        // The mixnet exit gateways relay 443 but mishandle non-standard ports
        // (2026-07-21 paired probe: every port-9067 witness failed the
        // TLS handshake over the tunnel). Since this list is mixnet-only, a
        // non-443 entry is a guaranteed fan-out failure. See the module docs.
        for entry in broadcast_indexers() {
            assert_eq!(
                entry.port_u16(),
                Some(443),
                "broadcast entry {entry} must be port 443 to traverse the mixnet"
            );
        }
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
