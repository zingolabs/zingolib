use std::{
    net::SocketAddr,
    pin::Pin,
    sync::{atomic::AtomicBool, Arc},
};

use zingolib::{
    compact_formats::{
        compact_tx_streamer_server::{CompactTxStreamer, CompactTxStreamerServer},
        Address, AddressList, Balance, BlockId, BlockRange, ChainSpec, CompactBlock, CompactTx,
        Duration, Empty, Exclude, GetAddressUtxosArg, GetAddressUtxosReply,
        GetAddressUtxosReplyList, LightdInfo, PingResponse, RawTransaction, SendResponse,
        TransparentAddressBlockFilter, TreeState, TxFilter,
    },
    grpc_connector::GrpcConnector,
};

macro_rules! define_grpc_passthrough {
    (fn
        $name:ident(
            &$self:ident$(,$($arg:ident: $argty:ty,)*)?
        ) -> $ret:ty
    ) => {
        #[must_use]
        #[allow(clippy::type_complexity, clippy::type_repetition_in_bounds)]
        fn $name<'life0, 'async_trait>(&'life0 $self$($(, $arg: $argty)*)?) ->
           ::core::pin::Pin<Box<
                dyn ::core::future::Future<
                    Output = ::core::result::Result<
                        ::tonic::Response<$ret>,
                        ::tonic::Status
                >
            > + ::core::marker::Send + 'async_trait
        >>
        where
            'life0: 'async_trait,
            Self: 'async_trait,
        {
            Box::pin(async {
                while !$self.online.load(::core::sync::atomic::Ordering::Relaxed) {
                    ::tokio::time::sleep(::core::time::Duration::from_millis(50)).await;
                }
                println!("Proxy passing through {} call", stringify!($name));
                ::zingolib::grpc_connector::GrpcConnector::new($self.lightwalletd_uri.clone())
                    .get_client()
                    .await
                    .expect("Proxy server failed to create client")
                    .$name($($($arg),*)?)
                    .await
            })
        }
    };
}

pub struct ProxyServer {
    pub lightwalletd_uri: http::Uri,
    pub online: Arc<AtomicBool>,
}

impl ProxyServer {
    pub fn serve(
        self,
        listen_at: SocketAddr,
    ) -> tokio::task::JoinHandle<Result<(), tonic::transport::Error>> {
        tokio::task::spawn(async move {
            let svc = CompactTxStreamerServer::new(self);
            tonic::transport::Server::builder()
                .add_service(svc)
                .serve(listen_at)
                .await
        })
    }
}

impl CompactTxStreamer for ProxyServer {
    define_grpc_passthrough!(
        fn get_latest_block(
            &self,
            request: tonic::Request<ChainSpec>,
        ) -> BlockId
    );

    define_grpc_passthrough!(
        fn get_block(
            &self,
            request: tonic::Request<BlockId>,
        ) -> CompactBlock
    );

    #[doc = "Server streaming response type for the GetBlockRange method."]
    type GetBlockRangeStream = tonic::Streaming<CompactBlock>;

    define_grpc_passthrough!(
        fn get_block_range(
            &self,
            request: tonic::Request<BlockRange>,
        ) -> Self::GetBlockRangeStream
    );

    define_grpc_passthrough!(
        fn get_transaction(
            &self,
            request: tonic::Request<TxFilter>,
        ) -> RawTransaction
    );

    define_grpc_passthrough!(
        fn send_transaction(
            &self,
            request: tonic::Request<RawTransaction>,
        ) -> SendResponse
    );

    #[doc = "Server streaming response type for the GetTaddressTxids method."]
    type GetTaddressTxidsStream =
        Pin<Box<dyn futures::Stream<Item = Result<RawTransaction, tonic::Status>> + Send + Sync>>;

    #[doc = " Return the txids corresponding to the given t-address within the given block range"]
    #[must_use]
    #[allow(clippy::type_complexity, clippy::type_repetition_in_bounds)]
    fn get_taddress_txids<'life0, 'async_trait>(
        &'life0 self,
        request: tonic::Request<TransparentAddressBlockFilter>,
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
        request: tonic::Request<AddressList>,
    ) -> ::core::pin::Pin<
        Box<
            dyn ::core::future::Future<Output = Result<tonic::Response<Balance>, tonic::Status>>
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

    #[must_use]
    #[allow(clippy::type_complexity, clippy::type_repetition_in_bounds)]
    fn get_taddress_balance_stream<'life0, 'async_trait>(
        &'life0 self,
        request: tonic::Request<tonic::Streaming<Address>>,
    ) -> ::core::pin::Pin<
        Box<
            dyn ::core::future::Future<Output = Result<tonic::Response<Balance>, tonic::Status>>
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

    #[doc = "Server streaming response type for the GetMempoolTx method."]
    type GetMempoolTxStream =
        Pin<Box<dyn futures::Stream<Item = Result<CompactTx, tonic::Status>> + Send + Sync>>;

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
        request: tonic::Request<Exclude>,
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
    type GetMempoolStreamStream =
        Pin<Box<dyn futures::Stream<Item = Result<RawTransaction, tonic::Status>> + Send + Sync>>;

    #[doc = " Return a stream of current Mempool transactions. This will keep the output stream open while"]
    #[doc = " there are mempool transactions. It will close the returned stream when a new block is mined."]
    #[must_use]
    #[allow(clippy::type_complexity, clippy::type_repetition_in_bounds)]
    fn get_mempool_stream<'life0, 'async_trait>(
        &'life0 self,
        request: tonic::Request<Empty>,
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
        request: tonic::Request<BlockId>,
    ) -> ::core::pin::Pin<
        Box<
            dyn ::core::future::Future<Output = Result<tonic::Response<TreeState>, tonic::Status>>
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

    #[must_use]
    #[allow(clippy::type_complexity, clippy::type_repetition_in_bounds)]
    fn get_address_utxos<'life0, 'async_trait>(
        &'life0 self,
        request: tonic::Request<GetAddressUtxosArg>,
    ) -> ::core::pin::Pin<
        Box<
            dyn ::core::future::Future<
                    Output = Result<tonic::Response<GetAddressUtxosReplyList>, tonic::Status>,
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
    type GetAddressUtxosStreamStream = Pin<
        Box<dyn futures::Stream<Item = Result<GetAddressUtxosReply, tonic::Status>> + Send + Sync>,
    >;

    #[must_use]
    #[allow(clippy::type_complexity, clippy::type_repetition_in_bounds)]
    fn get_address_utxos_stream<'life0, 'async_trait>(
        &'life0 self,
        request: tonic::Request<GetAddressUtxosArg>,
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

    define_grpc_passthrough!(
        fn get_lightd_info(
            &self,
            request: tonic::Request<Empty>,
        ) -> LightdInfo
    );

    #[doc = " Testing-only, requires lightwalletd --ping-very-insecure (do not enable in production)"]
    #[must_use]
    #[allow(clippy::type_complexity, clippy::type_repetition_in_bounds)]
    fn ping<'life0, 'async_trait>(
        &'life0 self,
        request: tonic::Request<Duration>,
    ) -> ::core::pin::Pin<
        Box<
            dyn ::core::future::Future<
                    Output = Result<tonic::Response<PingResponse>, tonic::Status>,
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
