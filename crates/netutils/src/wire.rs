//! The wire boundary: every conversion between a `lightwallet_protocol` proto message and its
//! owned `zl_common::chain` domain counterpart, so no other module in this crate (or any
//! consumer crate) needs to name a proto type directly.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use futures::Stream;
use lightwallet_protocol::{
    BlockId as WireBlockId, BlockRange as WireBlockRange, CompactBlock as WireCompactBlock,
    CompactTx as WireCompactTx, GetSubtreeRootsArg as WireGetSubtreeRootsArg,
    LightdInfo as WireLightdInfo, RawTransaction as WireRawTransaction,
    SubtreeRoot as WireSubtreeRoot, TreeState as WireTreeState, TxFilter as WireTxFilter,
};
use zl_common::chain::{
    BlockId, BlockRange, CompactBlock, CompactTx, GetSubtreeRootsArg, LightdInfo, RawTransaction,
    SubtreeRoot, TreeState, TxFilter,
};

/// A domain type that can be parsed from its wire representation, failing exactly when the
/// wire value's shape violates this type's own invariants.
///
/// Lives here, not in `zl_common`, because the orphan rule blocks implementing any trait on a
/// `zl_common::chain` type for a `lightwallet_protocol` wire type from a third crate: at least
/// one of the trait or the type being converted from has to be local. `zl_common` itself can't
/// define this trait with a `lightwallet_protocol`-typed `Wire`, since it is forbidden from
/// depending on proto crates at all.
pub(crate) trait ParseWire: Sized {
    /// The wire-format type carrying this value over the gRPC boundary.
    type Wire;
    /// This type's own parse failure. Each primitive already defines its own `Invalid*` error;
    /// this just gives every one of them a common name to compose `?` over.
    type Error: std::fmt::Display;

    fn parse_wire(wire: Self::Wire) -> Result<Self, Self::Error>;
}

/// Maps a [`ParseWire`] failure into the same [`tonic::Status`] channel every `Indexer` method
/// answers through.
pub(crate) fn status_of(error: impl std::fmt::Display) -> tonic::Status {
    tonic::Status::new(tonic::Code::Internal, error.to_string())
}

/// Parses `wire` via [`ParseWire`], mapping a failure through [`status_of`].
pub(crate) fn parse_wire_as_status<T: ParseWire>(wire: T::Wire) -> Result<T, tonic::Status> {
    T::parse_wire(wire).map_err(status_of)
}

/// A domain type that can be converted into its wire representation. Always infallible: a
/// value built locally (never received from a server) is valid by construction, unlike
/// [`ParseWire`]'s inbound direction.
pub(crate) trait ToWire {
    /// The wire-format type this value crosses the gRPC boundary as.
    type Wire;

    fn to_wire(self) -> Self::Wire;
}

/// Runs one unary RPC whose request is a prebuilt wire value (there is nothing to convert:
/// `ChainSpec`/`Empty` carry no data), parsing the response via [`ParseWire`].
pub(crate) async fn unary_rpc_raw<Req, T, F, Fut>(
    wire_request: Req,
    timeout: Duration,
    call: F,
) -> Result<T, tonic::Status>
where
    T: ParseWire,
    F: FnOnce(tonic::Request<Req>) -> Fut,
    Fut: Future<Output = Result<tonic::Response<T::Wire>, tonic::Status>>,
{
    let mut request = tonic::Request::new(wire_request);
    request.set_timeout(timeout);
    parse_wire_as_status(call(request).await?.into_inner())
}

/// Runs one unary RPC, converting `arg` to its wire form and parsing the response via
/// [`ParseWire`]. Every unary `Indexer`/`TransparentIndexer` method taking one domain argument
/// shares this exact shape; factored out so each stays one line instead of repeating it
/// (`cargo-dupes` otherwise flags the repeated shape as duplication, AGENTS.md).
pub(crate) async fn unary_rpc<Arg, T, F, Fut>(
    arg: Arg,
    timeout: Duration,
    call: F,
) -> Result<T, tonic::Status>
where
    Arg: ToWire,
    T: ParseWire,
    F: FnOnce(tonic::Request<Arg::Wire>) -> Fut,
    Fut: Future<Output = Result<tonic::Response<T::Wire>, tonic::Status>>,
{
    unary_rpc_raw(arg.to_wire(), timeout, call).await
}

/// Runs one streaming RPC, converting `arg` to its wire form and parsing each response item
/// via [`ParseWire`]. Shared shape for every streaming method taking one domain argument; see
/// [`unary_rpc`].
pub(crate) async fn stream_rpc<Arg, T, F, Fut, S>(
    arg: Arg,
    timeout: Duration,
    call: F,
) -> Result<Pin<Box<dyn Stream<Item = Result<T, tonic::Status>> + Send>>, tonic::Status>
where
    Arg: ToWire,
    T: ParseWire + Send + 'static,
    F: FnOnce(tonic::Request<Arg::Wire>) -> Fut,
    Fut: Future<Output = Result<tonic::Response<S>, tonic::Status>>,
    S: Stream<Item = Result<T::Wire, tonic::Status>> + Send + 'static,
{
    let mut request = tonic::Request::new(arg.to_wire());
    request.set_timeout(timeout);
    let wire_stream = call(request).await?.into_inner();
    Ok(Box::pin(futures::StreamExt::map(wire_stream, |item| {
        item.and_then(parse_wire_as_status)
    })))
}

impl ParseWire for RawTransaction {
    type Wire = WireRawTransaction;
    type Error = zl_common::chain::InvalidRawTransaction;

    fn parse_wire(wire: Self::Wire) -> Result<Self, Self::Error> {
        Self::from_wire_parts(wire.data, wire.height)
    }
}

impl ParseWire for zl_common::chain::CompactSaplingSpend {
    type Wire = lightwallet_protocol::CompactSaplingSpend;
    type Error = zl_common::chain::InvalidCompactBlock;

    fn parse_wire(wire: Self::Wire) -> Result<Self, Self::Error> {
        Self::from_wire(wire.nf)
    }
}

impl ParseWire for zl_common::chain::CompactOrchardAction {
    type Wire = lightwallet_protocol::CompactOrchardAction;
    type Error = zl_common::chain::InvalidCompactBlock;

    fn parse_wire(wire: Self::Wire) -> Result<Self, Self::Error> {
        Self::from_wire(
            wire.nullifier,
            wire.cmx,
            wire.ephemeral_key,
            wire.ciphertext,
        )
    }
}

impl ParseWire for zl_common::chain::CompactTxIn {
    type Wire = lightwallet_protocol::CompactTxIn;
    type Error = zl_common::chain::InvalidCompactBlock;

    fn parse_wire(wire: Self::Wire) -> Result<Self, Self::Error> {
        Self::from_wire(wire.prevout_txid, wire.prevout_index)
    }
}

impl ParseWire for CompactTx {
    type Wire = WireCompactTx;
    type Error = zl_common::chain::InvalidCompactBlock;

    /// Parses every nested Sapling, Orchard, Ironwood, and transparent component first. The
    /// Sapling output and transparent-output copies are plain field projections (nothing in
    /// them is validated, so they skip [`ParseWire`] rather than wrap a result that can never
    /// be an error).
    fn parse_wire(wire: Self::Wire) -> Result<Self, Self::Error> {
        let spends = wire
            .spends
            .into_iter()
            .map(zl_common::chain::CompactSaplingSpend::parse_wire)
            .collect::<Result<Vec<_>, _>>()?;
        let outputs = wire
            .outputs
            .into_iter()
            .map(|out| zl_common::chain::CompactSaplingOutput {
                cmu: out.cmu,
                ephemeral_key: out.ephemeral_key,
                ciphertext: out.ciphertext,
            })
            .collect();
        let actions = wire
            .actions
            .into_iter()
            .map(zl_common::chain::CompactOrchardAction::parse_wire)
            .collect::<Result<Vec<_>, _>>()?;
        let ironwood_actions = wire
            .ironwood_actions
            .into_iter()
            .map(zl_common::chain::CompactOrchardAction::parse_wire)
            .collect::<Result<Vec<_>, _>>()?;
        let vin = wire
            .vin
            .into_iter()
            .map(zl_common::chain::CompactTxIn::parse_wire)
            .collect::<Result<Vec<_>, _>>()?;
        let vout = wire
            .vout
            .into_iter()
            .map(|out| zl_common::chain::TxOut {
                value: out.value,
                script_pub_key: out.script_pub_key,
            })
            .collect();
        Self::from_wire_parts(
            wire.index,
            wire.txid,
            wire.fee,
            spends,
            outputs,
            actions,
            ironwood_actions,
            vin,
            vout,
        )
    }
}

impl ParseWire for CompactBlock {
    type Wire = WireCompactBlock;
    type Error = zl_common::chain::InvalidCompactBlock;

    /// Parses every transaction first.
    fn parse_wire(wire: Self::Wire) -> Result<Self, Self::Error> {
        let vtx = wire
            .vtx
            .into_iter()
            .map(CompactTx::parse_wire)
            .collect::<Result<Vec<_>, _>>()?;
        let chain_metadata = wire
            .chain_metadata
            .map(|metadata| zl_common::chain::ChainMetadata {
                sapling_commitment_tree_size: metadata.sapling_commitment_tree_size,
                orchard_commitment_tree_size: metadata.orchard_commitment_tree_size,
                ironwood_commitment_tree_size: metadata.ironwood_commitment_tree_size,
            });
        Self::from_wire_parts(
            wire.height,
            wire.hash,
            wire.prev_hash,
            wire.time,
            wire.header,
            vtx,
            chain_metadata,
        )
    }
}

/// Converts a wire [`LightdInfo`](WireLightdInfo) into the domain [`LightdInfo`]. Every field is
/// already a plain owned value, so this is a lossless, infallible projection onto the subset
/// callers use — it skips [`ParseWire`] for the same reason [`CompactSaplingOutput`'s
/// (`zl_common::chain::CompactSaplingOutput`) copy inside [`CompactTx::parse_wire`] does.
pub(crate) fn parse_lightd_info(wire: WireLightdInfo) -> LightdInfo {
    LightdInfo {
        version: wire.version,
        vendor: wire.vendor,
        taddr_support: wire.taddr_support,
        chain_name: wire.chain_name,
        sapling_activation_height: wire.sapling_activation_height,
        consensus_branch_id: wire.consensus_branch_id,
        block_height: wire.block_height,
        git_commit: wire.git_commit,
        lightwallet_protocol_version: wire.lightwallet_protocol_version,
    }
}

impl ParseWire for BlockId {
    type Wire = WireBlockId;
    type Error = zl_common::chain::InvalidBlockId;

    /// The one direction this bidirectional type needs validation: [`Indexer::get_latest_block`]
    /// receives one from the server.
    fn parse_wire(wire: Self::Wire) -> Result<Self, Self::Error> {
        Self::from_wire_parts(wire.height, wire.hash)
    }
}

impl ToWire for BlockId {
    type Wire = WireBlockId;

    fn to_wire(self) -> Self::Wire {
        WireBlockId {
            height: self.height.into(),
            hash: self.hash.map(|hash| hash.0.to_vec()).unwrap_or_default(),
        }
    }
}

impl ToWire for BlockRange {
    type Wire = WireBlockRange;

    fn to_wire(self) -> Self::Wire {
        WireBlockRange {
            start: Some(self.start.to_wire()),
            end: Some(self.end.to_wire()),
            pool_types: self
                .pool_types
                .into_iter()
                .map(zl_common::chain::pool_type_wire_value)
                .collect(),
        }
    }
}

impl ToWire for TxFilter {
    type Wire = WireTxFilter;

    fn to_wire(self) -> Self::Wire {
        WireTxFilter {
            block: self.block.map(ToWire::to_wire),
            index: self.index,
            hash: self
                .hash
                .map(|txid| txid.as_ref().to_vec())
                .unwrap_or_default(),
        }
    }
}

impl ToWire for GetSubtreeRootsArg {
    type Wire = WireGetSubtreeRootsArg;

    fn to_wire(self) -> Self::Wire {
        WireGetSubtreeRootsArg {
            start_index: self.start_index,
            shielded_protocol: self.shielded_protocol,
            max_entries: self.max_entries,
        }
    }
}

impl ParseWire for SubtreeRoot {
    type Wire = WireSubtreeRoot;
    type Error = zl_common::chain::InvalidSubtreeRoot;

    fn parse_wire(wire: Self::Wire) -> Result<Self, Self::Error> {
        Self::from_wire_parts(
            wire.root_hash,
            wire.completing_block_hash,
            wire.completing_block_height,
        )
    }
}

impl ParseWire for TreeState {
    type Wire = WireTreeState;
    type Error = zl_common::chain::InvalidTreeState;

    fn parse_wire(wire: Self::Wire) -> Result<Self, Self::Error> {
        Self::from_wire_parts(
            wire.height,
            &wire.hash,
            &wire.sapling_tree,
            &wire.orchard_tree,
            &wire.ironwood_tree,
        )
    }
}

/// An invariant that only emerges once primitives are bundled into a [`CompactTx`] or
/// [`CompactBlock`]: nothing in [`zl_common::chain`] itself enforces it (each primitive only
/// validates its own fields), it falls out of how their [`ParseWire`] impls compose their
/// nested `?`-propagating parses.
#[cfg(test)]
mod compact_parsing {
    use super::*;

    fn valid_spend() -> lightwallet_protocol::CompactSaplingSpend {
        lightwallet_protocol::CompactSaplingSpend { nf: vec![0xaa; 32] }
    }

    fn invalid_spend() -> lightwallet_protocol::CompactSaplingSpend {
        lightwallet_protocol::CompactSaplingSpend { nf: vec![0xaa; 31] }
    }

    fn tx_with_spends(spends: Vec<lightwallet_protocol::CompactSaplingSpend>) -> WireCompactTx {
        WireCompactTx {
            txid: vec![0xaa; 32],
            spends,
            ..Default::default()
        }
    }

    #[test]
    fn a_tx_with_only_valid_spends_parses() {
        assert!(CompactTx::parse_wire(tx_with_spends(vec![valid_spend(), valid_spend()])).is_ok());
    }

    /// One invalid nested spend fails the whole transaction's parse, not just that spend: the
    /// bundling step has no way to accept a transaction with a partially-invalid component.
    #[test]
    fn one_invalid_nested_spend_fails_the_whole_transaction() {
        assert!(
            CompactTx::parse_wire(tx_with_spends(vec![valid_spend(), invalid_spend()])).is_err()
        );
    }

    /// The same short-circuit holds one level up: a block with one bad transaction fails to
    /// parse at all, even when every other transaction (before or after it) is valid.
    #[test]
    fn one_invalid_nested_transaction_fails_the_whole_block() {
        let good = tx_with_spends(vec![valid_spend()]);
        let bad = tx_with_spends(vec![invalid_spend()]);
        let block = WireCompactBlock {
            hash: vec![0xaa; 32],
            prev_hash: vec![0xbb; 32],
            vtx: vec![good.clone(), bad, good],
            ..Default::default()
        };
        assert!(CompactBlock::parse_wire(block).is_err());
    }

    #[test]
    fn a_block_with_only_valid_transactions_parses() {
        let good = tx_with_spends(vec![valid_spend()]);
        let block = WireCompactBlock {
            hash: vec![0xaa; 32],
            prev_hash: vec![0xbb; 32],
            vtx: vec![good.clone(), good],
            ..Default::default()
        };
        assert!(CompactBlock::parse_wire(block).is_ok());
    }
}
