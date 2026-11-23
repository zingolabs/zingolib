//! Transparent address operations for the [`Indexer`] trait.
//!
//! These methods expose transparent (t-address) balance queries, transaction
//! history, and UTXO lookups. They are gated behind the
//! `globally-public-transparent` feature because transparent data is
//! publicly visible on-chain and using these methods leaks which
//! addresses belong to the caller.

use std::{future::Future, time::Duration};

use lightwallet_protocol::{
    Address, AddressList, Balance, GetAddressUtxosArg as WireGetAddressUtxosArg,
    GetAddressUtxosReply as WireGetAddressUtxosReply,
    TransparentAddressBlockFilter as WireTransparentAddressBlockFilter,
};
use tonic::Request;
use zl_common::chain::{GetAddressUtxosArg, GetAddressUtxosReply, TransparentAddressBlockFilter};

use super::{
    GrpcIndexer, Indexer, RawTransactionStream, block_range_to_wire, parse_raw_transaction,
};

/// Converts a domain [`GetAddressUtxosArg`] into its wire representation.
fn get_address_utxos_arg_to_wire(arg: GetAddressUtxosArg) -> WireGetAddressUtxosArg {
    WireGetAddressUtxosArg {
        addresses: arg.addresses,
        start_height: arg.start_height.into(),
        max_entries: arg.max_entries,
    }
}

/// Converts a domain [`TransparentAddressBlockFilter`] into its wire representation.
fn transparent_address_block_filter_to_wire(
    filter: TransparentAddressBlockFilter,
) -> WireTransparentAddressBlockFilter {
    WireTransparentAddressBlockFilter {
        address: filter.address,
        range: filter.range.map(block_range_to_wire),
    }
}

/// A server's address-UTXO stream, each item already parsed into the validated domain
/// [`GetAddressUtxosReply`]. Boxed for the same reason as [`super::SubtreeRootStream`].
pub type GetAddressUtxosReplyStream =
    std::pin::Pin<Box<dyn futures::Stream<Item = Result<GetAddressUtxosReply, tonic::Status>> + Send>>;

/// Parses a wire [`GetAddressUtxosReply`](WireGetAddressUtxosReply) into the validated domain
/// [`GetAddressUtxosReply`], mapping a parse failure into the same [`tonic::Status`] channel
/// every other `Indexer` method answers through.
fn parse_get_address_utxos_reply(
    wire: WireGetAddressUtxosReply,
) -> Result<GetAddressUtxosReply, tonic::Status> {
    GetAddressUtxosReply::from_wire_parts(
        wire.address,
        wire.txid,
        wire.index,
        wire.script,
        wire.value_zat,
        wire.height,
    )
    .map_err(|error| tonic::Status::new(tonic::Code::Internal, error.to_string()))
}

/// Extension of [`Indexer`] for transparent address operations.
///
/// All methods in this trait expose globally-public transparent data.
/// Callers can depend on the following:
///
/// - Balance and UTXO results reflect only confirmed (mined) state.
/// - Streaming results are sorted by block height.
pub trait TransparentIndexer: Indexer {
    /// Return a stream of transactions for a transparent address in a block range.
    ///
    /// Same behavior as
    /// [`get_taddress_transactions`](TransparentIndexer::get_taddress_transactions).
    /// This method is a legacy alias; callers should migrate.
    #[deprecated(note = "use get_taddress_transactions instead")]
    fn get_taddress_txids(
        &mut self,
        filter: TransparentAddressBlockFilter,
        timeout: Duration,
    ) -> impl Future<Output = Result<RawTransactionStream, tonic::Status>> + Send;

    /// Return a stream of transactions for a transparent address in a block range.
    ///
    /// Results are sorted by block height. Mempool transactions are not included.
    fn get_taddress_transactions(
        &mut self,
        filter: TransparentAddressBlockFilter,
        timeout: Duration,
    ) -> impl Future<Output = Result<RawTransactionStream, tonic::Status>> + Send;

    /// Return the total confirmed balance for the given transparent addresses.
    ///
    /// The returned [`Balance`] contains the sum in zatoshis. Only confirmed
    /// (mined) outputs are included; mempool UTXOs are not counted.
    fn get_taddress_balance(
        &mut self,
        addresses: AddressList,
        timeout: Duration,
    ) -> impl Future<Output = Result<Balance, tonic::Status>> + Send;

    /// Return the total confirmed balance by streaming addresses to the server.
    ///
    /// Client-streaming variant of
    /// [`get_taddress_balance`](TransparentIndexer::get_taddress_balance).
    /// The addresses are streamed individually, avoiding message size limits
    /// for large address sets. Returns the same [`Balance`] sum.
    fn get_taddress_balance_stream(
        &mut self,
        addresses: Vec<Address>,
    ) -> impl Future<Output = Result<Balance, tonic::Status>> + Send;

    /// Return UTXOs for the given addresses as a single response.
    ///
    /// Results are sorted by block height. Pass `max_entries = 0` for
    /// unlimited results.
    fn get_address_utxos(
        &mut self,
        arg: GetAddressUtxosArg,
        timeout: Duration,
    ) -> impl Future<Output = Result<Vec<GetAddressUtxosReply>, tonic::Status>> + Send;

    /// Return a stream of UTXOs for the given addresses.
    ///
    /// Prefer this over [`get_address_utxos`](TransparentIndexer::get_address_utxos)
    /// when the result set may be large.
    fn get_address_utxos_stream(
        &mut self,
        arg: GetAddressUtxosArg,
        timeout: Duration,
    ) -> impl Future<Output = Result<GetAddressUtxosReplyStream, tonic::Status>> + Send;
}

impl TransparentIndexer for GrpcIndexer {
    #[allow(deprecated)]
    async fn get_taddress_txids(
        &mut self,
        filter: TransparentAddressBlockFilter,
        timeout: Duration,
    ) -> Result<RawTransactionStream, tonic::Status> {
        let mut request = Request::new(transparent_address_block_filter_to_wire(filter));
        request.set_timeout(timeout);
        let wire_stream = self
            .clear_net_client
            .get_taddress_txids(request)
            .await?
            .into_inner();
        Ok(Box::pin(futures::StreamExt::map(wire_stream, |item| {
            item.and_then(parse_raw_transaction)
        })))
    }

    async fn get_taddress_transactions(
        &mut self,
        filter: TransparentAddressBlockFilter,
        timeout: Duration,
    ) -> Result<RawTransactionStream, tonic::Status> {
        let mut request = Request::new(transparent_address_block_filter_to_wire(filter));
        request.set_timeout(timeout);
        let wire_stream = self
            .clear_net_client
            .get_taddress_transactions(request)
            .await?
            .into_inner();
        Ok(Box::pin(futures::StreamExt::map(wire_stream, |item| {
            item.and_then(parse_raw_transaction)
        })))
    }

    async fn get_taddress_balance(
        &mut self,
        addresses: AddressList,
        timeout: Duration,
    ) -> Result<Balance, tonic::Status> {
        let mut request = Request::new(addresses);
        request.set_timeout(timeout);
        Ok(self
            .clear_net_client
            .get_taddress_balance(request)
            .await?
            .into_inner())
    }

    async fn get_taddress_balance_stream(
        &mut self,
        addresses: Vec<Address>,
    ) -> Result<Balance, tonic::Status> {
        let stream = tokio_stream::iter(addresses);
        Ok(self
            .clear_net_client
            .get_taddress_balance_stream(stream)
            .await?
            .into_inner())
    }

    async fn get_address_utxos(
        &mut self,
        arg: GetAddressUtxosArg,
        timeout: Duration,
    ) -> Result<Vec<GetAddressUtxosReply>, tonic::Status> {
        let mut request = Request::new(get_address_utxos_arg_to_wire(arg));
        request.set_timeout(timeout);
        self.clear_net_client
            .get_address_utxos(request)
            .await?
            .into_inner()
            .address_utxos
            .into_iter()
            .map(parse_get_address_utxos_reply)
            .collect()
    }

    async fn get_address_utxos_stream(
        &mut self,
        arg: GetAddressUtxosArg,
        timeout: Duration,
    ) -> Result<GetAddressUtxosReplyStream, tonic::Status> {
        let mut request = Request::new(get_address_utxos_arg_to_wire(arg));
        request.set_timeout(timeout);
        let wire_stream = self
            .clear_net_client
            .get_address_utxos_stream(request)
            .await?
            .into_inner();
        Ok(Box::pin(futures::StreamExt::map(wire_stream, |item| {
            item.and_then(parse_get_address_utxos_reply)
        })))
    }
}
