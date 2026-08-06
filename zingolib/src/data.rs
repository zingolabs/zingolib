//! This is a mod for data structs that will be used across all sections of zingolib.

pub mod proposal;

/// Return type for fns that poll the status of task handles.
pub enum PollReport<T, E> {
    /// Task has not been launched.
    NoHandle,
    /// Task is not complete.
    NotReady,
    /// Task has completed successfully or failed.
    Ready(Result<T, E>),
}

/// The connected Indexer's server diagnostics, as reported by its
/// `get_lightd_info` RPC. Returned by `LightClient::info`.
#[derive(Debug, Clone)]
pub struct ServerInfo {
    /// The server's software version.
    pub version: String,
    /// The git commit the server was built from.
    pub git_commit: String,
    /// The URI this client is connected to.
    pub server_uri: http::Uri,
    /// The server's vendor string.
    pub vendor: String,
    /// Whether the server supports transparent addresses.
    pub taddr_support: bool,
    /// The chain name ("main", "test", "regtest").
    pub chain_name: String,
    /// The Sapling activation height of the chain.
    pub sapling_activation_height: u64,
    /// The consensus branch ID the server reports.
    pub consensus_branch_id: String,
    /// The server's view of the chain tip height.
    pub latest_block_height: u64,
}

impl From<ServerInfo> for json::JsonValue {
    fn from(info: ServerInfo) -> Self {
        json::object! {
            "version" => info.version,
            "git_commit" => info.git_commit,
            "server_uri" => info.server_uri.to_string(),
            "vendor" => info.vendor,
            "taddr_support" => info.taddr_support,
            "chain_name" => info.chain_name,
            "sapling_activation_height" => info.sapling_activation_height,
            "consensus_branch_id" => info.consensus_branch_id,
            "latest_block_height" => info.latest_block_height
        }
    }
}

/// transforming data related to the destination of a send.
pub mod receivers {
    use zcash_address::ZcashAddress;
    use zcash_client_backend::zip321::Payment;
    use zcash_client_backend::zip321::PaymentError;
    use zcash_client_backend::zip321::TransactionRequest;
    use zcash_client_backend::zip321::Zip321Error;
    use zcash_protocol::memo::MemoBytes;
    use zcash_protocol::value::Zatoshis;

    /// A list of Receivers
    pub type Receivers = Vec<Receiver>;

    /// The superficial representation of the the consumer's intended receiver
    #[derive(Clone, Debug, PartialEq)]
    pub struct Receiver {
        pub recipient_address: ZcashAddress,
        pub amount: Zatoshis,
        pub memo: Option<MemoBytes>,
    }
    impl Receiver {
        /// Create a new Receiver
        pub(crate) fn new(
            recipient_address: ZcashAddress,
            amount: Zatoshis,
            memo: Option<MemoBytes>,
        ) -> Self {
            Self {
                recipient_address,
                amount,
                memo,
            }
        }
    }
    impl TryFrom<Receiver> for Payment {
        type Error = PaymentError;

        fn try_from(receiver: Receiver) -> Result<Self, Self::Error> {
            Payment::new(
                receiver.recipient_address,
                Some(receiver.amount),
                receiver.memo,
                None,
                None,
                vec![],
            )
        }
    }

    /// Creates a [`zcash_client_backend::zip321::TransactionRequest`] from receivers.
    /// Note this fn is called to calculate the `spendable_shielded` balance
    /// shielding and TEX should be handled mutually exclusively
    pub fn transaction_request_from_receivers(
        receivers: Receivers,
    ) -> Result<TransactionRequest, Zip321Error> {
        let payments = receivers
            .into_iter()
            .enumerate()
            .map(|(i, receiver)| {
                Payment::try_from(receiver).map_err(|e| match e {
                    PaymentError::TransparentMemo => Zip321Error::TransparentMemo(i),
                    PaymentError::ZeroValuedTransparentOutput => {
                        Zip321Error::ZeroValuedTransparentOutput(i)
                    }
                })
            })
            .collect::<Result<Vec<_>, Zip321Error>>()?;

        TransactionRequest::new(payments)
    }
}
