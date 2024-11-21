//! a benchmark is a measurement of how quickly zingo can sync.
//! benchmarks need to take into account many factors

use zcash_primitives::consensus::BlockHeight;

/// all relevant data for a sync.
/// this struct will convert to json to be saved
/// todo: distinguish between raw json standard and zingo specific template, add Options
pub struct SyncBenchmark {
    network_info: NetworkInfo,
    system_info: (),
    account_info: AccountInfo,
    client_info: ClientInfo,
    /// /
    final_time: u64,
    /// /
    final_height: BlockHeight,
    /// /
    sync_time: u64,
    /// TODO should this be an alias?
    synced_blocks: u32,
}

/// the connected server, indexer, and chain
/// also, how fast is the network
pub struct NetworkInfo {
    url: Url,
    chain_type: ChainType,
    bandwidth: (),
    latency: (),
}
/// the hardware and underlying system
pub struct SystemInfo {}
/// the account (wallet) being synced
pub struct AccountInfo {
    wallet_id: NetworkSeedVersion,
    hdwallet_accounts: Option<u32>,
}
/// the software being used to sync
pub struct ClientInfo {
    client: String,
    version: String,
}
