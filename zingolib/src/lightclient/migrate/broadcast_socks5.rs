//! The Nym-mixnet [`BroadcastClient`]: migration parts fan out over the
//! Broadcast Indexers through the local SOCKS5 proxy.
//!
//! ZIP 318 requires that when a network-privacy layer is enabled, every
//! migration broadcast is routed through it. This client satisfies that for
//! Nym by reusing the exact transport a regular send already uses —
//! [`crate::lightclient::send::mixnet_fanout_transmit`], the escalating,
//! serially gated witness-rotation fan-out over the curated Broadcast Indexer
//! list (ADR 0011) — so a migration part is submitted with the same
//! IP-obfuscation, witness rotation, and per-arm resilience as an ordinary
//! transaction rather than over clearnet.
//!
//! Like [`super::broadcast_grpc`] it lives outside `wallet::migration`, so the
//! migration modules stay free of the network stack; the broadcast-only
//! capability that keeps a broadcast session from synchronizing is the
//! [`BroadcastClient`] trait itself.
#![forbid(unsafe_code)]

use zcash_primitives::transaction::TxId;
use zcash_protocol::consensus::BlockHeight;

use crate::lightclient::indexer_history::IndexerHistoryHandle;
use crate::lightclient::transmit::TransmitProgressHandle;
use crate::wallet::migration::broadcast::{BroadcastClient, BroadcastError};

/// Submits migration parts over the Nym mixnet and can do nothing else. Holds
/// the local SOCKS5 proxy address the wallet's Mixnet Mode resolved and the
/// cross-session indexer history the fan-out records each witness attempt to.
pub struct Socks5BroadcastClient {
    socks5_addr: String,
    history: IndexerHistoryHandle,
    progress: TransmitProgressHandle,
}

impl Socks5BroadcastClient {
    /// A client fanning out through the local SOCKS5 proxy at `socks5_addr`.
    /// The caller has already resolved the fail-closed Mixnet Mode route, so
    /// reaching this constructor means the mixnet is ready.
    pub fn new(socks5_addr: String, history: IndexerHistoryHandle) -> Self {
        Socks5BroadcastClient {
            socks5_addr,
            history,
            progress: TransmitProgressHandle::default(),
        }
    }
}

impl BroadcastClient for Socks5BroadcastClient {
    async fn submit(
        &self,
        raw_tx: Vec<u8>,
        txid: TxId,
        expiry_height: BlockHeight,
    ) -> Result<TxId, BroadcastError> {
        let height = u64::from(u32::from(expiry_height));
        crate::lightclient::send::mixnet_fanout_transmit(
            &self.socks5_addr,
            &raw_tx,
            height,
            &txid,
            &self.progress,
            &self.history,
        )
        .await
        // The fan-out reports the server-echoed id; the wallet's local txid is
        // authoritative, so return that. A fan-out that exhausts every witness
        // leaves the part unconsumed and retriable next window — Transport, not
        // Rejected.
        .map(|_server_txid| txid)
        .map_err(BroadcastError::Transport)
    }
}
