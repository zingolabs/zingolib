//! Transparent address operations for the [`Indexer`] trait.
//!
//! These methods expose transparent (t-address) balance queries, transaction
//! history, and UTXO lookups. They are gated behind the
//! `globally-public-transparent` feature because transparent data is
//! publicly visible on-chain and using these methods leaks which
//! addresses belong to the caller.

use std::{future::Future, time::Duration};

use lightwallet_protocol::{
    Address, AddressList, Balance, GetAddressUtxosArg, GetAddressUtxosReply,
    GetAddressUtxosReplyList, RawTransaction, TransparentAddressBlockFilter,
};
use tonic::Request;

use super::{GrpcIndexer, Indexer};

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
    ) -> impl Future<Output = Result<tonic::Streaming<RawTransaction>, tonic::Status>> + Send;

    /// Return a stream of transactions for a transparent address in a block range.
    ///
    /// Results are sorted by block height. Mempool transactions are not included.
    fn get_taddress_transactions(
        &mut self,
        filter: TransparentAddressBlockFilter,
        timeout: Duration,
    ) -> impl Future<Output = Result<tonic::Streaming<RawTransaction>, tonic::Status>> + Send;

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
    ) -> impl Future<Output = Result<GetAddressUtxosReplyList, tonic::Status>> + Send;

    /// Return a stream of UTXOs for the given addresses.
    ///
    /// Prefer this over [`get_address_utxos`](TransparentIndexer::get_address_utxos)
    /// when the result set may be large.
    fn get_address_utxos_stream(
        &mut self,
        arg: GetAddressUtxosArg,
        timeout: Duration,
    ) -> impl Future<Output = Result<tonic::Streaming<GetAddressUtxosReply>, tonic::Status>> + Send;
}

impl TransparentIndexer for GrpcIndexer {
    #[allow(deprecated)]
    async fn get_taddress_txids(
        &mut self,
        filter: TransparentAddressBlockFilter,
        timeout: Duration,
    ) -> Result<tonic::Streaming<RawTransaction>, tonic::Status> {
        let mut request = Request::new(filter);
        request.set_timeout(timeout);
        Ok(self
            .clear_net_client
            .get_taddress_txids(request)
            .await?
            .into_inner())
    }

    async fn get_taddress_transactions(
        &mut self,
        filter: TransparentAddressBlockFilter,
        timeout: Duration,
    ) -> Result<tonic::Streaming<RawTransaction>, tonic::Status> {
        let mut request = Request::new(filter);
        request.set_timeout(timeout);
        Ok(self
            .clear_net_client
            .get_taddress_transactions(request)
            .await?
            .into_inner())
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
    ) -> Result<GetAddressUtxosReplyList, tonic::Status> {
        let mut request = Request::new(arg);
        request.set_timeout(timeout);
        Ok(self
            .clear_net_client
            .get_address_utxos(request)
            .await?
            .into_inner())
    }

    async fn get_address_utxos_stream(
        &mut self,
        arg: GetAddressUtxosArg,
        timeout: Duration,
    ) -> Result<tonic::Streaming<GetAddressUtxosReply>, tonic::Status> {
        let mut request = Request::new(arg);
        request.set_timeout(timeout);
        Ok(self
            .clear_net_client
            .get_address_utxos_stream(request)
            .await?
            .into_inner())
    }
}
