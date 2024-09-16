//! this is an interface to access either a spend-capable and view-capable LightWallet or a viewing-only LightWatch
use std::sync::{atomic::AtomicU64, Arc};

use tokio::sync::RwLock;
use zingoconfig::ZingoConfig;

use crate::wallet::{
    data::{BlockData, WalletZecPriceInfo},
    keys::unified::WalletCapability,
    transaction_records_by_id::TransactionRecordsById,
    WalletOptions,
};

pub(crate) trait LightPocket {
    /// The block at which this wallet was born. Rescans
    /// will start from here.
    fn birthday(&mut self) -> &mut AtomicU64;

    /// The last 100 blocks, used if something gets re-orged
    fn blocks(&mut self) -> &mut Arc<RwLock<Vec<BlockData>>>;

    /// options
    fn wallet_options(&mut self) -> &mut Arc<RwLock<WalletOptions>>;

    /// The current price of ZEC. (time_fetched, price in USD)
    fn price(&mut self) -> &mut Arc<RwLock<WalletZecPriceInfo>>;

    /// configuration
    fn config(&mut self) -> &mut ZingoConfig;

    /// The cryptographic powers of the pocket.
    fn key(&mut self) -> &mut Arc<WalletCapability>;

    /// The map of transactions known in this pocket.
    async fn transaction_records_by_id(&mut self) -> &mut TransactionRecordsById;
}
