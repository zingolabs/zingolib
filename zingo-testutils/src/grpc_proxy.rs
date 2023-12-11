use zingolib::compact_formats::{
    compact_tx_streamer_server::CompactTxStreamer, BlockId, ChainSpec,
};

pub struct ProxyServer;

impl CompactTxStreamer for ProxyServer {
    #[doc = " Return the height of the tip of the best chain"]
    #[must_use]
    #[allow(clippy::type_complexity, clippy::type_repetition_in_bounds)]
    fn get_latest_block<'life0, 'async_trait>(
        &'life0 self,
        request: tonic::Request<ChainSpec>,
    ) -> ::core::pin::Pin<
        Box<
            dyn ::core::future::Future<Output = Result<tonic::Response<BlockId>, tonic::Status>>
                + ::core::marker::Send
                + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        todo!()
    }

    #[doc = " Return the compact block corresponding to the given block identifier"]
    #[must_use]
    #[allow(clippy::type_complexity, clippy::type_repetition_in_bounds)]
    fn get_block<'life0, 'async_trait>(
        &'life0 self,
        request: tonic::Request<super::BlockId>,
    ) -> ::core::pin::Pin<
        Box<
            dyn ::core::future::Future<
                    Output = Result<tonic::Response<super::CompactBlock>, tonic::Status>,
                > + ::core::marker::Send
                + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        todo!()
    }

    #[doc = "Server streaming response type for the GetBlockRange method."]
    type GetBlockRangeStream;

    #[doc = " Return a list of consecutive compact blocks"]
    #[must_use]
    #[allow(clippy::type_complexity, clippy::type_repetition_in_bounds)]
    fn get_block_range<'life0, 'async_trait>(
        &'life0 self,
        request: tonic::Request<super::BlockRange>,
    ) -> ::core::pin::Pin<
        Box<
            dyn ::core::future::Future<
                    Output = Result<tonic::Response<Self::GetBlockRangeStream>, tonic::Status>,
                > + ::core::marker::Send
                + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        todo!()
    }

    #[doc = " Return the requested full (not compact) transaction (as from zcashd)"]
    #[must_use]
    #[allow(clippy::type_complexity, clippy::type_repetition_in_bounds)]
    fn get_transaction<'life0, 'async_trait>(
        &'life0 self,
        request: tonic::Request<super::TxFilter>,
    ) -> ::core::pin::Pin<
        Box<
            dyn ::core::future::Future<
                    Output = Result<tonic::Response<super::RawTransaction>, tonic::Status>,
                > + ::core::marker::Send
                + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        todo!()
    }

    #[doc = " Submit the given transaction to the Zcash network"]
    #[must_use]
    #[allow(clippy::type_complexity, clippy::type_repetition_in_bounds)]
    fn send_transaction<'life0, 'async_trait>(
        &'life0 self,
        request: tonic::Request<super::RawTransaction>,
    ) -> ::core::pin::Pin<
        Box<
            dyn ::core::future::Future<
                    Output = Result<tonic::Response<super::SendResponse>, tonic::Status>,
                > + ::core::marker::Send
                + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        todo!()
    }

    #[doc = "Server streaming response type for the GetTaddressTxids method."]
    type GetTaddressTxidsStream;

    #[doc = " Return the txids corresponding to the given t-address within the given block range"]
    #[must_use]
    #[allow(clippy::type_complexity, clippy::type_repetition_in_bounds)]
    fn get_taddress_txids<'life0, 'async_trait>(
        &'life0 self,
        request: tonic::Request<super::TransparentAddressBlockFilter>,
    ) -> ::core::pin::Pin<
        Box<
            dyn ::core::future::Future<
                    Output = Result<tonic::Response<Self::GetTaddressTxidsStream>, tonic::Status>,
                > + ::core::marker::Send
                + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        todo!()
    }

    #[must_use]
    #[allow(clippy::type_complexity, clippy::type_repetition_in_bounds)]
    fn get_taddress_balance<'life0, 'async_trait>(
        &'life0 self,
        request: tonic::Request<super::AddressList>,
    ) -> ::core::pin::Pin<
        Box<
            dyn ::core::future::Future<
                    Output = Result<tonic::Response<super::Balance>, tonic::Status>,
                > + ::core::marker::Send
                + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        todo!()
    }

    #[must_use]
    #[allow(clippy::type_complexity, clippy::type_repetition_in_bounds)]
    fn get_taddress_balance_stream<'life0, 'async_trait>(
        &'life0 self,
        request: tonic::Request<tonic::Streaming<super::Address>>,
    ) -> ::core::pin::Pin<
        Box<
            dyn ::core::future::Future<
                    Output = Result<tonic::Response<super::Balance>, tonic::Status>,
                > + ::core::marker::Send
                + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        todo!()
    }

    #[doc = "Server streaming response type for the GetMempoolTx method."]
    type GetMempoolTxStream;

    #[doc = " Return the compact transactions currently in the mempool; the results"]
    #[doc = " can be a few seconds out of date. If the Exclude list is empty, return"]
    #[doc = " all transactions; otherwise return all *except* those in the Exclude list"]
    #[doc = " (if any); this allows the client to avoid receiving transactions that it"]
    #[doc = " already has (from an earlier call to this rpc). The transaction IDs in the"]
    #[doc = " Exclude list can be shortened to any number of bytes to make the request"]
    #[doc = " more bandwidth-efficient; if two or more transactions in the mempool"]
    #[doc = " match a shortened txid, they are all sent (none is excluded). Transactions"]
    #[doc = " in the exclude list that don\'t exist in the mempool are ignored."]
    #[must_use]
    #[allow(clippy::type_complexity, clippy::type_repetition_in_bounds)]
    fn get_mempool_tx<'life0, 'async_trait>(
        &'life0 self,
        request: tonic::Request<super::Exclude>,
    ) -> ::core::pin::Pin<
        Box<
            dyn ::core::future::Future<
                    Output = Result<tonic::Response<Self::GetMempoolTxStream>, tonic::Status>,
                > + ::core::marker::Send
                + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        todo!()
    }

    #[doc = "Server streaming response type for the GetMempoolStream method."]
    type GetMempoolStreamStream;

    #[doc = " Return a stream of current Mempool transactions. This will keep the output stream open while"]
    #[doc = " there are mempool transactions. It will close the returned stream when a new block is mined."]
    #[must_use]
    #[allow(clippy::type_complexity, clippy::type_repetition_in_bounds)]
    fn get_mempool_stream<'life0, 'async_trait>(
        &'life0 self,
        request: tonic::Request<super::Empty>,
    ) -> ::core::pin::Pin<
        Box<
            dyn ::core::future::Future<
                    Output = Result<tonic::Response<Self::GetMempoolStreamStream>, tonic::Status>,
                > + ::core::marker::Send
                + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        todo!()
    }

    #[doc = " GetTreeState returns the note commitment tree state corresponding to the given block."]
    #[doc = " See section 3.7 of the Zcash protocol specification. It returns several other useful"]
    #[doc = " values also (even though they can be obtained using GetBlock)."]
    #[doc = " The block can be specified by either height or hash."]
    #[must_use]
    #[allow(clippy::type_complexity, clippy::type_repetition_in_bounds)]
    fn get_tree_state<'life0, 'async_trait>(
        &'life0 self,
        request: tonic::Request<super::BlockId>,
    ) -> ::core::pin::Pin<
        Box<
            dyn ::core::future::Future<
                    Output = Result<tonic::Response<super::TreeState>, tonic::Status>,
                > + ::core::marker::Send
                + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        todo!()
    }

    #[must_use]
    #[allow(clippy::type_complexity, clippy::type_repetition_in_bounds)]
    fn get_address_utxos<'life0, 'async_trait>(
        &'life0 self,
        request: tonic::Request<super::GetAddressUtxosArg>,
    ) -> ::core::pin::Pin<
        Box<
            dyn ::core::future::Future<
                    Output = Result<
                        tonic::Response<super::GetAddressUtxosReplyList>,
                        tonic::Status,
                    >,
                > + ::core::marker::Send
                + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        todo!()
    }

    #[doc = "Server streaming response type for the GetAddressUtxosStream method."]
    type GetAddressUtxosStreamStream;

    #[must_use]
    #[allow(clippy::type_complexity, clippy::type_repetition_in_bounds)]
    fn get_address_utxos_stream<'life0, 'async_trait>(
        &'life0 self,
        request: tonic::Request<super::GetAddressUtxosArg>,
    ) -> ::core::pin::Pin<
        Box<
            dyn ::core::future::Future<
                    Output = Result<
                        tonic::Response<Self::GetAddressUtxosStreamStream>,
                        tonic::Status,
                    >,
                > + ::core::marker::Send
                + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        todo!()
    }

    #[doc = " Return information about this lightwalletd instance and the blockchain"]
    #[must_use]
    #[allow(clippy::type_complexity, clippy::type_repetition_in_bounds)]
    fn get_lightd_info<'life0, 'async_trait>(
        &'life0 self,
        request: tonic::Request<super::Empty>,
    ) -> ::core::pin::Pin<
        Box<
            dyn ::core::future::Future<
                    Output = Result<tonic::Response<super::LightdInfo>, tonic::Status>,
                > + ::core::marker::Send
                + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        todo!()
    }

    #[doc = " Testing-only, requires lightwalletd --ping-very-insecure (do not enable in production)"]
    #[must_use]
    #[allow(clippy::type_complexity, clippy::type_repetition_in_bounds)]
    fn ping<'life0, 'async_trait>(
        &'life0 self,
        request: tonic::Request<super::Duration>,
    ) -> ::core::pin::Pin<
        Box<
            dyn ::core::future::Future<
                    Output = Result<tonic::Response<super::PingResponse>, tonic::Status>,
                > + ::core::marker::Send
                + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        todo!()
    }
}
