//! In practice there are several common scenarios for which helpers are provided.
//! These scenarios vary in the configuration of clients in use.  Most scenarios
//! require some funds, the simplest way to access funds is to use a "faucet".
//! A "faucet" is a client that receives mining rewards (because its spend capability
//! generated the address the backing regtest Validator mines to).
//! HELPERS:
//! If you just need a faucet, use the "faucet" helper.
//! If you need a faucet, and a single recipient, use 'faucet_recipient`
//! For less common client configurations use the client builder directly with
//! custom_clients
//! All scenarios have a default (i.e. faucet_default) which take minimal parameters and
//! build the scenario with the most common settings. This simplifies test writing in
//! most cases by removing the need for configuration.

use std::path::PathBuf;

use portpicker::Port;
use tempfile::TempDir;
use zcash_local_net::ProcessId;
use zcash_protocol::{PoolType, ShieldedPool};

use zcash_local_net::LocalNet;
use zcash_local_net::MinerPool;
use zcash_local_net::indexer::{Indexer, IndexerConfig};
use zcash_local_net::logs::LogsToStdoutAndStderr;
use zcash_local_net::process::Process;
use zcash_local_net::validator::{Validator, ValidatorConfig};

use network_combo::DefaultIndexer;
use network_combo::DefaultValidator;
use zingo_test_vectors::FUND_OFFLOAD_ORCHARD_ONLY;

use crate::chain_cache::{self, CacheManifest, CachedStage, Disposition};
use crate::observability::FrontRecord;
use crate::setup_metrics::MeteredNet;

pub use crate::chain_cache::ChainCachePolicy;
use zingo_common_components::protocol::ActivationHeights;
use zingo_test_vectors::{block_rewards, seeds};
use zingolib::config::WalletConfig;
use zingolib::config::{ChainType, ClientConfig};
use zingolib::get_base_address_macro;
use zingolib::lightclient::LightClient;
use zingolib::lightclient::error::LightClientError;
use zingolib::testutils::default_test_wallet_settings;
use zingolib::testutils::lightclient::from_inputs::{self, quick_send};
use zingolib::testutils::lightclient::get_base_address;
use zingolib::testutils::port_to_localhost_uri;
use zingolib::testutils::sync_to_target_height;
use zingolib::testutils::timed;
use zingolib::wallet::keys::unified::ReceiverSelection;

/// Default regtest network processes for testing and zingo-cli regtest mode:
/// the Core stack, zainod in front of zebrad.
#[allow(missing_docs)]
pub mod network_combo {
    pub type DefaultIndexer = zcash_local_net::indexer::zainod::Zainod;
    pub type DefaultValidator = zcash_local_net::validator::zebrad::Zebrad;
}

/// Indexer-convergence dispatch. zcash_local_net's barrier
/// (`await_indexer_convergence`, added in infra commit fa2da0b) is defined
/// only for zainod, whose "Syncing block" log line is the observation
/// channel. This trait lets the generic scenario helpers use the barrier
/// where the indexer has one and fall back to a no-op where it does not.
pub trait IndexerConvergence {
    /// Blocks until the indexer's own chain index reports `target`, where
    /// observable. A no-op otherwise.
    fn converge(&self, target: u32) -> impl std::future::Future<Output = ()> + Send;
}

impl<V> IndexerConvergence for LocalNet<V, zcash_local_net::indexer::zainod::Zainod>
where
    V: Validator + LogsToStdoutAndStderr + Send,
    <V as Process>::Config: Send,
{
    async fn converge(&self, target: u32) {
        self.await_indexer_convergence(target)
            .await
            .expect("indexer convergence barrier failed");
    }
}

/// Map the wallet-domain pool selection onto the infrastructure miner pool at
/// the `zcash_local_net` boundary.
fn miner_pool(mine_to_pool: PoolType) -> MinerPool {
    match mine_to_pool {
        PoolType::Transparent => MinerPool::Transparent,
        PoolType::Shielded(ShieldedPool::Sapling) => MinerPool::Sapling,
        PoolType::Shielded(ShieldedPool::Orchard) | PoolType::Shielded(ShieldedPool::Ironwood) => {
            MinerPool::Orchard
        }
    }
}

/// Rebuild the wallet-domain activation heights as the `zingo_consensus` type
/// that `zcash_local_net` validators consume.
fn net_activation_heights(
    heights: &ActivationHeights,
) -> zcash_local_net::protocol::ActivationHeights {
    zcash_local_net::protocol::ActivationHeightsBuilder::new()
        .set_overwinter(heights.overwinter())
        .set_sapling(heights.sapling())
        .set_blossom(heights.blossom())
        .set_heartwood(heights.heartwood())
        .set_canopy(heights.canopy())
        .set_nu5(heights.nu5())
        .set_nu6(heights.nu6())
        .set_nu6_1(heights.nu6_1())
        .set_nu6_2(heights.nu6_2())
        .set_nu6_3(heights.nu6_3())
        .set_nu7(heights.nu7())
        .build()
}

/// The default test activation heights in the infrastructure's own
/// vocabulary, for diagnostics that drive `zcash_local_net` processes
/// directly (e.g. the launch-block falsification tests).
pub fn default_net_activation_heights() -> zcash_local_net::protocol::ActivationHeights {
    net_activation_heights(&default_test_activation_heights())
}

/// Rebuild the infrastructure activation heights as the wallet-domain type
/// that zingolib configuration consumes. Inverse of the private
/// `net_activation_heights`, for values coming back across the
/// `zcash_local_net` boundary (e.g. `Validator::get_activation_heights`).
pub fn wallet_activation_heights(
    heights: &zcash_local_net::protocol::ActivationHeights,
) -> ActivationHeights {
    ActivationHeights::builder()
        .set_overwinter(heights.overwinter())
        .set_sapling(heights.sapling())
        .set_blossom(heights.blossom())
        .set_heartwood(heights.heartwood())
        .set_canopy(heights.canopy())
        .set_nu5(heights.nu5())
        .set_nu6(heights.nu6())
        .set_nu6_1(heights.nu6_1())
        .set_nu6_2(heights.nu6_2())
        .set_nu6_3(heights.nu6_3())
        .set_nu7(heights.nu7())
        .build()
}

/// The default activation heights for scenario tests: the harness's regtest
/// fixture shape (the only shape its default lockbox-disbursement and
/// funding-stream fixtures pair with, since zebrad rejects NU6.x activation
/// blocks whose subsidy config doesn't match, so e.g. the all-at-1
/// `ActivationHeights::default()` stalls the chain at genesis), with one
/// amendment: NU7 stays off. NU6.3 activates at the fixture's height 5,
/// so wallet activity is Ironwood-era by default (ADR 0009) while
/// coinbase blocks 2..=4 still yield legacy Orchard notes, a mixed
/// chain by design, since behavior *relative to* Ironwood is a primary
/// test subject during the migration window.
pub fn default_test_activation_heights() -> ActivationHeights {
    let fixture =
        wallet_activation_heights(&zcash_local_net::validator::regtest_test_activation_heights());
    ActivationHeights::builder()
        .set_overwinter(fixture.overwinter())
        .set_sapling(fixture.sapling())
        .set_blossom(fixture.blossom())
        .set_heartwood(fixture.heartwood())
        .set_canopy(fixture.canopy())
        .set_nu5(fixture.nu5())
        .set_nu6(fixture.nu6())
        .set_nu6_1(fixture.nu6_1())
        .set_nu6_2(fixture.nu6_2())
        .set_nu6_3(fixture.nu6_3())
        .set_nu7(None)
        .build()
}

/// The deferred (lockbox) funding stream in the harness's regtest fixture
/// skims 1/100 of the block subsidy from its start height (2) onward.
pub const DEFERRED_STREAM_SKIM: u64 = block_rewards::CANOPY / 100;

/// Miner reward for blocks at or above the funding-stream start height (2).
pub const POST_STREAM_BLOCK_REWARD: u64 = block_rewards::CANOPY - DEFERRED_STREAM_SKIM;

/// Amount `normalize_shielded_faucet_balance` offloads from the faucet to keep its
/// spendable balance predictable for test assertions.
pub const FUND_OFFLOAD_AMOUNT: u64 = 624_960_000;

/// Total miner rewards for blocks 1..=`count` under
/// [`default_test_activation_heights`]: block 1 pays the full subsidy, and
/// every later block pays the post-funding-stream reward.
pub const fn mined_block_rewards_total(count: u64) -> u64 {
    block_rewards::CANOPY + POST_STREAM_BLOCK_REWARD * (count - 1)
}

/// Chain height after a shielded-pool faucet scenario finishes setting up:
/// 1 launch block + 2 setup blocks + 2 ladder-clearing blocks + 1 block
/// confirming the offload send. (No maturity blocks: shielded coinbase is
/// exempt from the transparent `COINBASE_MATURITY_BLOCKS` rule.)
pub const FUNDED_FAUCET_SETUP_HEIGHT: u32 = 6;

/// HYPOTHESIS (server-run adjudicated): with an Orchard miner pool the
/// coinbase pays the orchard receiver from the NU5 activation block (height
/// 2 under [`default_test_activation_heights`]) onward. If orchard coinbase
/// actually starts one block later, every orchard expectation derived from
/// this constant fails high by exactly one [`POST_STREAM_BLOCK_REWARD`].
/// Flip this to 3 and nothing else.
pub const ORCHARD_COINBASE_START_HEIGHT: u32 = 2;

/// HYPOTHESIS (server-run adjudicated): with an Ironwood miner pool the
/// coinbase pays the orchard receiver from the NU6_3 activation block (height
/// 5 under [`default_test_activation_heights`]) onward.
pub const IRONWOOD_COINBASE_START_HEIGHT: u32 = 5;

/// HYPOTHESIS (server-run adjudicated): block 1 predates NU5, so an Orchard
/// miner pool pays block 1's full pre-funding-stream subsidy to the miner's
/// SAPLING receiver, observed as `s_balance: 625000000` in orchard-mined
/// scenarios. If refuted, s-balance expectations fail by exactly this value.
pub const BLOCK_ONE_SAPLING_COINBASE: u64 = block_rewards::CANOPY;

/// Total orchard coinbase received by the faucet at `tip` under an Orchard
/// miner pool: one post-funding-stream reward per block from
/// [`ORCHARD_COINBASE_START_HEIGHT`] through `tip`.
pub const fn orchard_coinbase_total(tip: u32) -> u64 {
    (tip - ORCHARD_COINBASE_START_HEIGHT + 1) as u64 * POST_STREAM_BLOCK_REWARD
}

/// The faucet's ironwood balance right after a `PoolType::IRONWOOD` scenario
/// finishes setting up ironwood coinbase through
/// [`FUNDED_FAUCET_SETUP_HEIGHT`], less the offload amount. The offload's
/// fee cancels out: the faucet pays it, then collects it right back in the
/// coinbase of the confirming block it mines.
pub const fn funded_faucet_ironwood_balance() -> u64 {
    orchard_coinbase_total(FUNDED_FAUCET_SETUP_HEIGHT) - FUND_OFFLOAD_AMOUNT
}

/// To launch a `LocalNet` with darkside settings.
pub async fn launch_test<V, I>(
    indexer_listen_port: Option<Port>,
    mine_to_pool: PoolType,
    configured_activation_heights: ActivationHeights,
    chain_cache: Option<PathBuf>,
) -> LocalNet<V, I>
where
    V: Validator + LogsToStdoutAndStderr + Send,
    <V as Process>::Config: Send + ValidatorConfig + Default,
    I: Indexer + LogsToStdoutAndStderr,
    <I as Process>::Config: Send + IndexerConfig + Default,
{
    // The harness probes the Validator's RPC over reqwest/rustls before any
    // LightClient (the usual installer) exists, so install the provider here.
    zingolib::ensure_default_crypto_provider();
    let mut validator_config = <V as Process>::Config::default();
    validator_config.set_test_parameters(
        miner_pool(mine_to_pool),
        net_activation_heights(&configured_activation_heights),
        chain_cache,
    );
    let mut indexer_config = <I as Process>::Config::default();
    indexer_config.set_listen_port(indexer_listen_port);
    LocalNet::launch_from_two_configs(validator_config, indexer_config)
        .await
        .expect("ing to launch a LocalNetwork with testconfiguration.")
}

/// Sync `client` until it has fully scanned the Validator's current chain
/// tip. A bare `sync_and_await` only reaches whatever the Indexer has
/// ingested at that instant, which races behind the Validator right after
/// `generate_blocks`, the cause of nondeterministic stale-balance test
/// failures.
pub async fn sync_client_to_validator_tip<V, I>(
    local_net: &LocalNet<V, I>,
    client: &mut LightClient,
) where
    I: Indexer + LogsToStdoutAndStderr,
    V: Validator + LogsToStdoutAndStderr + Send,
    <I as Process>::Config: Send,
    <V as Process>::Config: Send,
    LocalNet<V, I>: IndexerConvergence,
{
    let tip = timed(
        "sync_to_validator_tip::get_chain_height",
        local_net.validator().get_chain_height(),
    )
    .await;
    timed(
        "sync_to_validator_tip::indexer_converge",
        local_net.converge(tip),
    )
    .await;
    timed(
        "sync_to_validator_tip::wallet_sync",
        sync_to_target_height(client, tip),
    )
    .await
    .unwrap();
}

/// Zebra's mempool rejection text for a spend of, or anchored at, the tip
/// block; the leaf link of the send failure's cause chain carries it.
const TIP_BLOCK_REJECTION: &str = "until the next chain tip block";

/// Reports whether any link of `error`'s cause chain carries zebra's tip-block rejection text.
fn rejects_tip_block_spend(error: &(dyn std::error::Error + 'static)) -> bool {
    let mut link: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(current) = link {
        if current.to_string().contains(TIP_BLOCK_REJECTION) {
            return true;
        }
        link = current.source();
    }
    false
}

/// The single lag-safe send primitive: sends `receivers` from `sender` in
/// one transaction, then mines one block at a time (waiting for `sender`'s
/// wallet to reach each new height) until the transaction is confirmed.
///
/// Owns both send hazards discovered on the zainod+zebrad stack, so callers
/// need no choreography:
///
/// - Indexer tip-lag: `generate_blocks` returns when the *validator* has the
///   block, but wallet sync talks to the *indexer*, whose poll-based
///   ingestion lags by 100-500ms (see
///   `indexer_tip_lags_validator_after_block_generation`). Waiting on the
///   wallet's own height, as `increase_height_and_wait_for_client` does,
///   closes that window.
/// - Tip-block spends: zebra's mempool rejects a transaction spending a
///   note from (or anchored at) the tip block itself ("rejected from the
///   mempool until the next chain tip block"). When zebra says exactly
///   that, one separation block is mined and the send retried once,
///   mirroring the fix in `normalize_shielded_faucet_balance`.
///
/// Confirmation is asserted (up to a few blocks of tolerance), so a
/// transaction that misses its block for any new reason fails HERE with the
/// txids, not downstream at some unrelated balance assert.
pub async fn send_and_bump<V, I>(
    local_net: &LocalNet<V, I>,
    sender: &mut LightClient,
    receivers: Vec<(&str, u64, Option<&str>)>,
) -> nonempty::NonEmpty<zcash_primitives::transaction::TxId>
where
    V: Validator + LogsToStdoutAndStderr + Send,
    <V as Process>::Config: Send,
    I: Indexer + LogsToStdoutAndStderr,
    <I as Process>::Config: Send,
    LocalNet<V, I>: IndexerConvergence,
{
    let txids = match from_inputs::quick_send(sender, receivers.clone()).await {
        Ok(txids) => txids,
        Err(e) if rejects_tip_block_spend(&e) => {
            // Tip-block spend rejected: separate from the tip and retry once.
            increase_height_and_wait_for_client(local_net, sender, 1)
                .await
                .unwrap();
            from_inputs::quick_send(sender, receivers)
                .await
                .expect("send must succeed after tip separation")
        }
        Err(e) => panic!("send failed: {e}"),
    };

    // The transaction can miss the immediately-following block; allow a few
    // blocks before declaring it lost.
    const MAX_BLOCKS_TO_CONFIRMATION: u32 = 5;
    for _ in 0..MAX_BLOCKS_TO_CONFIRMATION {
        increase_height_and_wait_for_client(local_net, sender, 1)
            .await
            .unwrap();
        let wallet = sender.wallet();
        let wallet = wallet.read().await;
        if txids.iter().all(|txid| {
            wallet
                .wallet_transactions
                .get(txid)
                .is_some_and(|tx| tx.status().is_confirmed())
        }) {
            drop(wallet);
            return txids;
        }
    }
    panic!(
        "transaction(s) {txids:?} still unconfirmed after mining \
         {MAX_BLOCKS_TO_CONFIRMATION} blocks"
    );
}

/// When mining to a shielded pool, dump the excess faucet funds and generate
/// a block to confirm the send. Coinbase lands directly in the mined-to pool
/// (zebrad mines to Orchard natively), and shielded coinbase has no
/// `COINBASE_MATURITY_BLOCKS` rule, so the wallet spends blocks-old orchard coinbase
/// fine (server-verified by `value_transfers`).
///
/// Two constraints govern when the offload can be sent (both observed as
/// zebra mempool rejections):
///
/// 1. The wallet must be synced to the REAL tip when it builds the send.
///    It signs for tip+1's consensus branch id, and the fixture ladder
///    activates NU6.1/NU6.2 at height 5, right where this setup operates
///    ("transaction uses an incorrect consensus branch id" when the wallet
///    was held a block behind).
/// 2. It must not spend the tip block's own note ("could not validate
///    orchard proof" when it did).
///
/// Mining two extra blocks first clears the upgrade ladder (tip 5, so
/// tip+1 and the inclusion block share the NU6.2 branch id) and accumulates
/// four pre-tip notes so oldest-first selection never reaches the tip note.
///
/// The orchard-to-ironwood normalization uses the immediate migration rather
/// than a self-send: 731b2b761 taught note selection to lead with the
/// payment's own pool, so a send to an ironwood receiver no longer drains
/// orchard (it left one orchard note un-migrated, observed as balance
/// assertions failing exactly one block reward short on freshly mined
/// chains while cached replays of pre-731b2b761 wallet-built blocks kept
/// passing). The immediate migration spends only orchard by construction, which also
/// satisfies constraint 2 structurally. Its fee cancels the same way the
/// offload's does (the faucet collects it back in the confirming block's
/// coinbase), so [`funded_faucet_ironwood_balance`] is fee-invariant.
async fn normalize_shielded_faucet_balance<V, I>(
    local_net: &LocalNet<V, I>,
    mine_to_pool: PoolType,
    faucet: &mut LightClient,
) where
    I: Indexer + LogsToStdoutAndStderr,
    V: Validator + LogsToStdoutAndStderr + Send,
    <I as Process>::Config: Send,
    <V as Process>::Config: Send,
    LocalNet<V, I>: IndexerConvergence,
{
    let chain_height = local_net.validator().get_chain_height().await;
    let activation_heights = local_net.validator().get_activation_heights().await;
    if !matches!(mine_to_pool, PoolType::Transparent)
        && activation_heights
            .nu6_3()
            .is_some_and(|ironwood_height| chain_height >= ironwood_height - 2)
    {
        local_net.validator().generate_blocks(1).await.unwrap();
        sync_client_to_validator_tip(local_net, faucet).await;
        quick_send(
            faucet,
            vec![(FUND_OFFLOAD_ORCHARD_ONLY, FUND_OFFLOAD_AMOUNT, None)],
        )
        .await
        .unwrap();
        local_net.validator().generate_blocks(1).await.unwrap();
        sync_client_to_validator_tip(local_net, faucet).await;
        let migration_summary = faucet
            .migrate_immediately(zip32::AccountId::ZERO)
            .await
            .unwrap();
        assert_eq!(
            migration_summary.residual, 0,
            "normalization must leave no orchard value behind"
        );
        local_net.validator().generate_blocks(1).await.unwrap();
    }
}

/// Struct for building lightclients for integration testing
pub struct ClientBuilder {
    /// Indexer URI
    pub server_id: http::Uri,
    /// Directory for wallet files
    pub zingo_datadir: TempDir,
    /// The activation-height schedule every built wallet is configured
    /// with. On a managed stack this is derived from the running validator
    /// (the sole source of activation-height truth, infras ADR 0003). An
    /// unmanaged stack (darkside) asserts its own schedule here, once.
    activation_heights: ActivationHeights,
    client_number: u8,
}

impl ClientBuilder {
    /// TODO: Add Doc Comment Here!
    pub fn new(
        server_id: http::Uri,
        zingo_datadir: TempDir,
        activation_heights: ActivationHeights,
    ) -> Self {
        let client_number = 0;
        ClientBuilder {
            server_id,
            zingo_datadir,
            activation_heights,
            client_number,
        }
    }

    pub fn make_unique_data_dir_and_create_config(
        &mut self,
        wallet_config: WalletConfig,
    ) -> ClientConfig {
        //! Each client requires a unique `data_dir`, we use the
        //! `client_number` counter for this.
        self.client_number += 1;
        let conf_path = format!(
            "{}_client_{}",
            self.zingo_datadir.path().to_string_lossy(),
            self.client_number
        );
        self.create_clientconfig(
            PathBuf::from(conf_path),
            self.activation_heights,
            wallet_config,
        )
    }

    /// TODO: Add Doc Comment Here!
    pub fn create_clientconfig(
        &self,
        conf_path: PathBuf,
        configured_activation_heights: ActivationHeights,
        wallet_config: WalletConfig,
    ) -> ClientConfig {
        std::fs::create_dir(&conf_path).unwrap();
        ClientConfig::builder()
            .set_indexer_uri(self.server_id.clone())
            .set_chain_type(ChainType::Regtest(configured_activation_heights))
            .set_wallet_dir(conf_path)
            .set_wallet_config(wallet_config)
            .build()
            .unwrap()
    }

    /// TODO: Add Doc Comment Here!
    pub async fn build_faucet(&mut self, overwrite: bool) -> LightClient {
        //! A "faucet" is a lightclient that receives mining rewards
        self.build_client(
            WalletConfig::MnemonicPhrase {
                mnemonic_phrase: seeds::ABANDON_ART_SEED.to_string(),
                no_of_accounts: 1.try_into().unwrap(),
                birthday: 1,
                wallet_settings: default_test_wallet_settings(),
            },
            overwrite,
        )
        .await
    }

    /// TODO: Add Doc Comment Here!
    pub async fn build_client(
        &mut self,
        wallet_config: WalletConfig,
        overwrite: bool,
    ) -> LightClient {
        let config = self.make_unique_data_dir_and_create_config(wallet_config);
        let mut lightclient = timed(
            "build_client::new_clearnet_consented",
            LightClient::new_clearnet_consented(config, overwrite),
        )
        .await
        .unwrap();
        timed(
            "build_client::generate_unified_address",
            lightclient.generate_unified_address(
                ReceiverSelection::sapling_only(),
                zip32::AccountId::ZERO,
            ),
        )
        .await
        .unwrap();

        lightclient
    }
}

/// TODO: Add Doc Comment Here!
pub async fn unfunded_client(
    configured_activation_heights: ActivationHeights,
    cache: ChainCachePolicy,
) -> (MeteredNet, LightClient) {
    let (replay, export) = resolve_cache(
        PoolType::IRONWOOD,
        &configured_activation_heights,
        cache,
        CachedStage::Bare,
    );
    let (mut local_net, mut client_builder) =
        custom_clients_raw(PoolType::IRONWOOD, configured_activation_heights, replay).await;

    let mut lightclient = client_builder
        .build_client(
            WalletConfig::MnemonicPhrase {
                mnemonic_phrase: seeds::HOSPITAL_MUSEUM_SEED.to_string(),
                no_of_accounts: 1.try_into().unwrap(),
                birthday: 1,
                wallet_settings: default_test_wallet_settings(),
            },
            true,
        )
        .await;
    sync_client_to_validator_tip(&local_net, &mut lightclient).await;

    if let Some((dir, manifest)) = export {
        chain_cache::export(&local_net, &dir, &manifest).await;
    }
    local_net.mark_setup_complete("unfunded_client");
    (local_net, lightclient)
}

/// TODO: Add Doc Comment Here!
pub async fn unfunded_client_default() -> (MeteredNet, LightClient) {
    unfunded_client(default_test_activation_heights(), ChainCachePolicy::PerTest).await
}

/// Many scenarios need to start with spendable funds.  This setup provides
/// 3 blocks worth of coinbase to a preregistered spend capability (5 blocks for zebrad).
///
/// This key is registered to receive block rewards by corresponding to the
/// address the regtest Validator mines to.
///
/// The general scenario framework requires instances of zingo-cli, an Indexer,
/// and a Validator (in regtest mode). This setup is intended to produce the most basic
/// of scenarios.  As scenarios with even less requirements
/// become interesting (e.g. without experimental features, or txindices) we'll create more setups.
pub async fn faucet(
    mine_to_pool: PoolType,
    configured_activation_heights: ActivationHeights,
    cache: ChainCachePolicy,
) -> (MeteredNet, LightClient) {
    let (replay, export) = resolve_cache(
        mine_to_pool,
        &configured_activation_heights,
        cache,
        CachedStage::Funded,
    );
    let replayed = replay.is_some();
    let (mut local_net, mut client_builder) =
        custom_clients_raw(mine_to_pool, configured_activation_heights, replay).await;

    let mut faucet = client_builder.build_faucet(true).await;

    // A replayed chain already contains the balance-normalization offload;
    // the freshly built faucet wallet recovers it by sync below.
    if !replayed && matches!(DefaultValidator::PROCESS, ProcessId::Zebrad) {
        normalize_shielded_faucet_balance(&local_net, mine_to_pool, &mut faucet).await;
    }

    sync_client_to_validator_tip(&local_net, &mut faucet).await;

    // Replay-time twin of the fresh-build invariant inside the
    // normalization: a cache whose chain shape predates the current setup
    // semantics otherwise replays cleanly and fails blocks later at some
    // unrelated balance assert (2026-07-17: pre-drain caches did exactly
    // that for a day). The manifest's setup_semantics key should discard
    // such caches before this fires; this assert is the backstop that
    // names the cache instead of the downstream test.
    if replayed && matches!(mine_to_pool, PoolType::Shielded(ShieldedPool::Ironwood)) {
        let ironwood_confirmed = faucet
            .account_balance(zip32::AccountId::ZERO)
            .await
            .expect("the freshly synced faucet reports a balance")
            .confirmed_ironwood_balance
            .map(zcash_protocol::value::Zatoshis::into_u64)
            .unwrap_or(0);
        assert_eq!(
            ironwood_confirmed,
            funded_faucet_ironwood_balance(),
            "replayed chain does not satisfy current setup semantics. Stale chain cache? \
             (the manifest's schema/setup_semantics should have discarded it)"
        );
    }

    if let Some((dir, manifest)) = export {
        chain_cache::export(&local_net, &dir, &manifest).await;
    }
    local_net.mark_setup_complete("faucet");
    (local_net, faucet)
}

/// TODO: Add Doc Comment Here!
pub async fn faucet_default() -> (MeteredNet, LightClient) {
    faucet(
        PoolType::IRONWOOD,
        default_test_activation_heights(),
        ChainCachePolicy::PerTest,
    )
    .await
}

/// TODO: Add Doc Comment Here!
pub async fn faucet_recipient(
    mine_to_pool: PoolType,
    configured_activation_heights: ActivationHeights,
    cache: ChainCachePolicy,
) -> (MeteredNet, LightClient, LightClient) {
    let (replay, export) = resolve_cache(
        mine_to_pool,
        &configured_activation_heights,
        cache,
        CachedStage::Funded,
    );
    let replayed = replay.is_some();
    let (mut local_net, mut client_builder) =
        custom_clients_raw(mine_to_pool, configured_activation_heights, replay).await;

    let mut faucet = client_builder.build_faucet(true).await;
    let mut recipient = client_builder
        .build_client(
            WalletConfig::MnemonicPhrase {
                mnemonic_phrase: seeds::HOSPITAL_MUSEUM_SEED.to_string(),
                no_of_accounts: 1.try_into().unwrap(),
                birthday: 1,
                wallet_settings: default_test_wallet_settings(),
            },
            true,
        )
        .await;

    // A replayed chain already contains the balance-normalization offload;
    // the freshly built faucet wallet recovers it by sync below.
    if !replayed && matches!(DefaultValidator::PROCESS, ProcessId::Zebrad) {
        normalize_shielded_faucet_balance(&local_net, mine_to_pool, &mut faucet).await;
    }

    sync_client_to_validator_tip(&local_net, &mut faucet).await;
    sync_client_to_validator_tip(&local_net, &mut recipient).await;

    if let Some((dir, manifest)) = export {
        chain_cache::export(&local_net, &dir, &manifest).await;
    }
    local_net.mark_setup_complete("faucet_recipient");
    (local_net, faucet, recipient)
}

/// TODO: Add Doc Comment Here!
pub async fn faucet_recipient_default() -> (MeteredNet, LightClient, LightClient) {
    faucet_recipient(
        PoolType::IRONWOOD,
        default_test_activation_heights(),
        ChainCachePolicy::PerTest,
    )
    .await
}

/// Like every other scenario, snapshots at full setup completion
/// (ADR 0003): the funding sends are embedded in the cache, and the
/// txids they minted at build time are recorded in the cache's
/// `outputs.json`, so a warm run replays the chain and returns the
/// recorded identifiers, which name transactions that are literally in
/// the replayed blocks.
///
/// If nu6.3 is activated, `orchard_funds` will be sent to the ironwood pool.
pub async fn faucet_funded_recipient(
    orchard_funds: Option<u64>,
    sapling_funds: Option<u64>,
    transparent_funds: Option<u64>,
    mine_to_pool: PoolType,
    configured_activation_heights: ActivationHeights,
    cache: ChainCachePolicy,
) -> (
    MeteredNet,
    LightClient,
    LightClient,
    Option<String>,
    Option<String>,
    Option<String>,
) {
    let manifest = CacheManifest::describe(
        mine_to_pool,
        &configured_activation_heights,
        CachedStage::RecipientFunded {
            orchard_funds,
            sapling_funds,
            transparent_funds,
        },
    );
    let disposition = chain_cache::resolve(cache, &manifest);

    if let Disposition::Replay(blocks_file) = disposition {
        // The warm path is exactly a faucet_recipient over the cached
        // chain, since the funding transactions are in the replayed blocks,
        // and the freshly built wallets recover them by sync.
        let (mut local_net, faucet, recipient) = faucet_recipient(
            mine_to_pool,
            configured_activation_heights,
            ChainCachePolicy::LoadRaw(blocks_file.clone()),
        )
        .await;
        let outputs = chain_cache::load_outputs(&blocks_file).unwrap_or_else(|| {
            panic!(
                "cache for this test lacks outputs.json ({}); a PerTest cache writes it \
                 atomically with the blocks, so this cache is corrupt or hand-rolled. \
                 Discard it (or craft the outputs) and rerun",
                blocks_file.display()
            )
        });
        let recorded = |key: &str| {
            outputs
                .get(key)
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        };
        let (orchard_txid, sapling_txid, transparent_txid) = (
            recorded("orchard_txid"),
            recorded("sapling_txid"),
            recorded("transparent_txid"),
        );
        local_net.mark_setup_complete("faucet_funded_recipient");
        return (
            local_net,
            faucet,
            recipient,
            orchard_txid,
            sapling_txid,
            transparent_txid,
        );
    }

    // Live run or cache build: generate everything, then export the
    // completed setup (blocks and minted txids together) if building.
    let (mut local_net, mut faucet, mut recipient) = faucet_recipient(
        mine_to_pool,
        configured_activation_heights,
        ChainCachePolicy::Disabled,
    )
    .await;
    increase_height_and_wait_for_client(&local_net, &mut faucet, 1)
        .await
        .unwrap();

    let orchard_txid = if let Some(funds) = orchard_funds {
        Some(
            quick_send(
                &mut faucet,
                vec![(&get_base_address_macro!(recipient, "unified"), funds, None)],
            )
            .await
            .unwrap()
            .first()
            .to_string(),
        )
    } else {
        None
    };
    let sapling_txid = if let Some(funds) = sapling_funds {
        Some(
            quick_send(
                &mut faucet,
                vec![(&get_base_address_macro!(recipient, "sapling"), funds, None)],
            )
            .await
            .unwrap()
            .first()
            .to_string(),
        )
    } else {
        None
    };
    let transparent_txid = if let Some(funds) = transparent_funds {
        Some(
            quick_send(
                &mut faucet,
                vec![(
                    &get_base_address_macro!(recipient, "transparent"),
                    funds,
                    None,
                )],
            )
            .await
            .unwrap()
            .first()
            .to_string(),
        )
    } else {
        None
    };
    increase_height_and_wait_for_client(&local_net, &mut recipient, 1)
        .await
        .unwrap();
    sync_client_to_validator_tip(&local_net, &mut faucet).await;

    if let Disposition::Export(dir) = disposition {
        chain_cache::export_with_outputs(
            &local_net,
            &dir,
            &manifest,
            serde_json::json!({
                "orchard_txid": orchard_txid,
                "sapling_txid": sapling_txid,
                "transparent_txid": transparent_txid,
            }),
        )
        .await;
    }
    local_net.mark_setup_complete("faucet_funded_recipient");
    (
        local_net,
        faucet,
        recipient,
        orchard_txid,
        sapling_txid,
        transparent_txid,
    )
}

/// TODO: Add Doc Comment Here!
pub async fn faucet_funded_recipient_default(
    orchard_funds: u64,
) -> (MeteredNet, LightClient, LightClient, String) {
    let (local_net, faucet, recipient, orchard_txid, _sapling_txid, _transparent_txid) =
        faucet_funded_recipient(
            Some(orchard_funds),
            None,
            None,
            PoolType::IRONWOOD,
            default_test_activation_heights(),
            ChainCachePolicy::PerTest,
        )
        .await;

    (local_net, faucet, recipient, orchard_txid.unwrap())
}

/// Resolve the cache policy for a scenario whose chain the given stage
/// determines. On a miss, live-build that stage's chain, snapshot it
/// Resolve the cache policy into what the launch needs: a blocks file
/// to replay instead of live generation, and/or a pending export for
/// the constructor to perform at its snapshot point. A build run is a
/// live run plus the export: it keeps its net and continues, and the
/// warm runs exercise the replay path (ADR 0003).
fn resolve_cache(
    mine_to_pool: PoolType,
    configured_activation_heights: &ActivationHeights,
    cache: ChainCachePolicy,
    stage: CachedStage,
) -> (
    Option<PathBuf>,
    Option<(chain_cache::CacheDir, CacheManifest)>,
) {
    let manifest = CacheManifest::describe(mine_to_pool, configured_activation_heights, stage);
    match chain_cache::resolve(cache, &manifest) {
        Disposition::Live => (None, None),
        Disposition::Replay(blocks_file) => (Some(blocks_file), None),
        Disposition::Export(dir) => (None, Some((dir, manifest))),
    }
}

/// Launch the network combo with the zainod→zebrad hop interposed: the
/// Validator first, then a recording link tap in front of its JSON-RPC
/// port, then the Indexer dialing the tap, assembled via
/// `LocalNet::from_parts`. `launch_from_two_configs` wires the two
/// processes directly and leaves no seam, which is why the seam was
/// added upstream (infrastructure commit 63b31a0).
async fn launch_observed(
    mine_to_pool: PoolType,
    configured_activation_heights: ActivationHeights,
) -> (
    LocalNet<DefaultValidator, DefaultIndexer>,
    std::sync::Arc<FrontRecord>,
    std::sync::Arc<FrontRecord>,
) {
    // The harness probes the Validator's RPC over reqwest/rustls before
    // any LightClient (the usual installer) exists.
    zingolib::ensure_default_crypto_provider();

    // Connected before launch, the fronts see every client of each
    // process, including the launch-mine, which no post-launch tap
    // could reach.
    let zebrad_front = FrontRecord::prime("zebrad front");
    let zainod_front = FrontRecord::prime("zainod front");

    let mut validator_config = <DefaultValidator as Process>::Config::default();
    validator_config.set_test_parameters(
        miner_pool(mine_to_pool),
        net_activation_heights(&configured_activation_heights),
        None,
    );
    validator_config.rpc_front_observer = Some(zebrad_front.clone());

    let mut indexer_config = <DefaultIndexer as Process>::Config::default();
    indexer_config.set_listen_port(None);
    indexer_config.grpc_front_observer = Some(zainod_front.clone());

    let local_net = LocalNet::launch_from_two_configs(validator_config, indexer_config)
        .await
        .expect("network combo must launch");

    (local_net, zebrad_front, zainod_front)
}

/// The uncached launch primitive under every scenario constructor:
/// start the network combo, establish the initial chain (by replaying
/// `replay_from` when given, by mining otherwise), and hand back a
/// client builder.
async fn custom_clients_raw(
    mine_to_pool: PoolType,
    configured_activation_heights: ActivationHeights,
    replay_from: Option<PathBuf>,
) -> (MeteredNet, ClientBuilder) {
    let setup_started = std::time::Instant::now();
    let (local_net, zebrad_front, zainod_front) = timed(
        "custom_clients_raw::launch_observed",
        launch_observed(mine_to_pool, configured_activation_heights),
    )
    .await;
    let mut local_net = MeteredNet::new(local_net, zebrad_front, zainod_front, setup_started);

    match replay_from {
        Some(blocks_file) => {
            let tip = timed(
                "custom_clients_raw::chain_cache_replay",
                chain_cache::replay(&local_net, &blocks_file),
            )
            .await;
            // The replay reorgs the launch block away; the Indexer may
            // have already ingested it (it starts syncing within
            // milliseconds of launch). Barrier on convergence to the
            // replayed tip so no wallet ever syncs the orphaned branch.
            timed(
                "custom_clients_raw::replay_converge",
                local_net.converge(tip),
            )
            .await;
        }
        None => {
            timed(
                "custom_clients_raw::generate_initial_blocks",
                local_net.validator().generate_blocks(2),
            )
            .await
            .unwrap();
        }
    }

    let client_builder = ClientBuilder::new(
        // The Indexer's listen port IS its observing front since the
        // front-proxy inversion, so wallet traffic lands in the record.
        port_to_localhost_uri(local_net.indexer().listen_port()),
        tempfile::tempdir().unwrap(),
        // The validator is the sole source of activation-height truth
        // (infras ADR 0003): wallets take their schedule from the running
        // validator, never from a caller-supplied vector.
        wallet_activation_heights(
            &timed(
                "custom_clients_raw::get_activation_heights",
                local_net.validator().get_activation_heights(),
            )
            .await,
        ),
    );

    local_net.mark_setup_complete("custom_clients");
    (local_net, client_builder)
}

/// TODO: Add Doc Comment Here!
pub async fn custom_clients(
    mine_to_pool: PoolType,
    configured_activation_heights: ActivationHeights,
    cache: ChainCachePolicy,
) -> (MeteredNet, ClientBuilder) {
    let (replay, export) = resolve_cache(
        mine_to_pool,
        &configured_activation_heights,
        cache,
        CachedStage::Bare,
    );
    let (local_net, client_builder) =
        custom_clients_raw(mine_to_pool, configured_activation_heights, replay).await;
    if let Some((dir, manifest)) = export {
        chain_cache::export(&local_net, &dir, &manifest).await;
    }
    (local_net, client_builder)
}

/// TODO: Add Doc Comment Here!
pub async fn custom_clients_default() -> (MeteredNet, ClientBuilder) {
    let (local_net, client_builder) = custom_clients(
        PoolType::IRONWOOD,
        default_test_activation_heights(),
        ChainCachePolicy::PerTest,
    )
    .await;

    (local_net, client_builder)
}

/// TODO: Add Doc Comment Here!
pub async fn unfunded_mobileclient() -> LocalNet<DefaultValidator, DefaultIndexer> {
    launch_test::<DefaultValidator, DefaultIndexer>(
        Some(20_000),
        // The funded_* derivatives below build a faucet (ABANDON_ART, the
        // miner-key holder) that must SPEND this coinbase. Mine to Orchard,
        // not Transparent: zebrad mines Orchard natively, and shielded
        // coinbase is exempt from the transparent COINBASE_MATURITY_BLOCKS
        // rule, so the faucet can spend it immediately. A transparent
        // coinbase would be unspendable until ~100 maturity blocks, and the
        // faucet would report `available: 0` and every funded scenario
        // would panic at its first send. (zebrad cannot mine to Sapling,
        // so Orchard is the only immediately-spendable pool.)
        PoolType::ORCHARD,
        // The ironwood-era workspace default (ADR 0009): zingo-mobile's
        // integration tests assert ironwood-activated behavior (shield
        // and change outputs land in the Ironwood pool), so the
        // mobileclient scenarios track the same schedule.
        default_test_activation_heights(),
        None,
    )
    .await
}

/// TODO: Add Doc Comment Here!
pub async fn funded_orchard_mobileclient(value: u64) -> LocalNet<DefaultValidator, DefaultIndexer> {
    let local_net = unfunded_mobileclient().await;
    let mut client_builder = ClientBuilder::new(
        port_to_localhost_uri(local_net.indexer().port()),
        tempfile::tempdir().unwrap(),
        wallet_activation_heights(&local_net.validator().get_activation_heights().await),
    );
    let mut faucet = client_builder.build_faucet(true).await;
    let recipient = client_builder
        .build_client(
            WalletConfig::MnemonicPhrase {
                mnemonic_phrase: seeds::HOSPITAL_MUSEUM_SEED.to_string(),
                no_of_accounts: 1.try_into().unwrap(),
                birthday: 1,
                wallet_settings: default_test_wallet_settings(),
            },
            true,
        )
        .await;
    normalize_shielded_faucet_balance(&local_net, PoolType::ORCHARD, &mut faucet).await;
    increase_height_and_wait_for_client(&local_net, &mut faucet, 1)
        .await
        .unwrap();
    quick_send(
        &mut faucet,
        vec![(&get_base_address_macro!(recipient, "unified"), value, None)],
    )
    .await
    .unwrap();
    local_net.validator().generate_blocks(1).await.unwrap();

    local_net
}

/// TODO: Add Doc Comment Here!
pub async fn funded_orchard_with_3_txs_mobileclient(
    value: u64,
) -> LocalNet<DefaultValidator, DefaultIndexer> {
    let local_net = unfunded_mobileclient().await;
    let mut client_builder = ClientBuilder::new(
        port_to_localhost_uri(local_net.indexer().port()),
        tempfile::tempdir().unwrap(),
        wallet_activation_heights(&local_net.validator().get_activation_heights().await),
    );
    let mut faucet = client_builder.build_faucet(true).await;
    let mut recipient = client_builder
        .build_client(
            WalletConfig::MnemonicPhrase {
                mnemonic_phrase: seeds::HOSPITAL_MUSEUM_SEED.to_string(),
                no_of_accounts: 1.try_into().unwrap(),
                birthday: 1,
                wallet_settings: default_test_wallet_settings(),
            },
            true,
        )
        .await;
    // Fund the faucet with spendable Orchard coinbase (see
    // funded_orchard_mobileclient / faucet()).
    normalize_shielded_faucet_balance(&local_net, PoolType::ORCHARD, &mut faucet).await;
    increase_height_and_wait_for_client(&local_net, &mut faucet, 1)
        .await
        .unwrap();
    quick_send(
        &mut faucet,
        vec![(&get_base_address_macro!(recipient, "unified"), value, None)],
    )
    .await
    .unwrap();
    increase_height_and_wait_for_client(&local_net, &mut recipient, 1)
        .await
        .unwrap();
    quick_send(
        &mut recipient,
        vec![(
            &get_base_address_macro!(faucet, "unified"),
            value.checked_div(10).unwrap(),
            None,
        )],
    )
    .await
    .unwrap();
    increase_height_and_wait_for_client(&local_net, &mut recipient, 1)
        .await
        .unwrap();
    let recipient_sapling_address = get_base_address_macro!(recipient, "sapling");
    quick_send(
        &mut recipient,
        vec![(
            &recipient_sapling_address,
            value.checked_div(10).unwrap(),
            Some("note-to-self test memo"),
        )],
    )
    .await
    .unwrap();
    local_net.validator().generate_blocks(1).await.unwrap();

    local_net
}

/// This scenario funds a client with transparent funds.
pub async fn funded_transparent_mobileclient(
    value: u64,
) -> LocalNet<DefaultValidator, DefaultIndexer> {
    let local_net = unfunded_mobileclient().await;
    let mut client_builder = ClientBuilder::new(
        port_to_localhost_uri(local_net.indexer().port()),
        tempfile::tempdir().unwrap(),
        wallet_activation_heights(&local_net.validator().get_activation_heights().await),
    );
    let mut faucet = client_builder.build_faucet(true).await;
    let mut recipient = client_builder
        .build_client(
            WalletConfig::MnemonicPhrase {
                mnemonic_phrase: seeds::HOSPITAL_MUSEUM_SEED.to_string(),
                no_of_accounts: 1.try_into().unwrap(),
                birthday: 1,
                wallet_settings: default_test_wallet_settings(),
            },
            true,
        )
        .await;
    // Fund the faucet with spendable Orchard coinbase (see
    // funded_orchard_mobileclient / faucet()).
    normalize_shielded_faucet_balance(&local_net, PoolType::ORCHARD, &mut faucet).await;
    increase_height_and_wait_for_client(&local_net, &mut faucet, 1)
        .await
        .unwrap();

    // // received from a faucet to transparent
    quick_send(
        &mut faucet,
        vec![(
            &get_base_address_macro!(recipient, "transparent"),
            value.checked_div(4).unwrap(),
            None,
        )],
    )
    .await
    .unwrap();
    increase_height_and_wait_for_client(&local_net, &mut recipient, 1)
        .await
        .unwrap();

    local_net.validator().generate_blocks(1).await.unwrap();

    local_net
}

/// TODO: Add Doc Comment Here!
pub async fn funded_orchard_sapling_transparent_shielded_mobileclient(
    value: u64,
) -> LocalNet<DefaultValidator, DefaultIndexer> {
    let local_net = unfunded_mobileclient().await;
    let mut client_builder = ClientBuilder::new(
        port_to_localhost_uri(local_net.indexer().port()),
        tempfile::tempdir().unwrap(),
        wallet_activation_heights(&local_net.validator().get_activation_heights().await),
    );
    let mut faucet = client_builder.build_faucet(true).await;
    let mut recipient = client_builder
        .build_client(
            WalletConfig::MnemonicPhrase {
                mnemonic_phrase: seeds::HOSPITAL_MUSEUM_SEED.to_string(),
                no_of_accounts: 1.try_into().unwrap(),
                birthday: 1,
                wallet_settings: default_test_wallet_settings(),
            },
            true,
        )
        .await;
    // Fund the faucet with spendable Orchard coinbase (see
    // funded_orchard_mobileclient / faucet()).
    normalize_shielded_faucet_balance(&local_net, PoolType::ORCHARD, &mut faucet).await;
    increase_height_and_wait_for_client(&local_net, &mut faucet, 1)
        .await
        .unwrap();

    // // received from a faucet to orchard
    quick_send(
        &mut faucet,
        vec![(
            &get_base_address_macro!(recipient, "unified"),
            value.checked_div(2).unwrap(),
            None,
        )],
    )
    .await
    .unwrap();
    increase_height_and_wait_for_client(&local_net, &mut recipient, 1)
        .await
        .unwrap();

    // // received from a faucet to sapling
    quick_send(
        &mut faucet,
        vec![(
            &get_base_address_macro!(recipient, "sapling"),
            value.checked_div(4).unwrap(),
            None,
        )],
    )
    .await
    .unwrap();
    increase_height_and_wait_for_client(&local_net, &mut faucet, 1)
        .await
        .unwrap();

    // // received from a faucet to transparent
    quick_send(
        &mut faucet,
        vec![(
            &get_base_address_macro!(recipient, "transparent"),
            value.checked_div(4).unwrap(),
            None,
        )],
    )
    .await
    .unwrap();
    increase_height_and_wait_for_client(&local_net, &mut recipient, 1)
        .await
        .unwrap();

    // // send to a faucet
    quick_send(
        &mut recipient,
        vec![(
            &get_base_address_macro!(faucet, "unified"),
            value.checked_div(10).unwrap(),
            None,
        )],
    )
    .await
    .unwrap();
    increase_height_and_wait_for_client(&local_net, &mut recipient, 1)
        .await
        .unwrap();

    // // send to self orchard
    let recipient_unified_address = get_base_address_macro!(recipient, "unified");
    quick_send(
        &mut recipient,
        vec![(
            &recipient_unified_address,
            value.checked_div(10).unwrap(),
            None,
        )],
    )
    .await
    .unwrap();
    increase_height_and_wait_for_client(&local_net, &mut recipient, 1)
        .await
        .unwrap();

    // // send to self sapling
    let recipient_sapling_address = get_base_address_macro!(recipient, "sapling");
    quick_send(
        &mut recipient,
        vec![(
            &recipient_sapling_address,
            value.checked_div(10).unwrap(),
            None,
        )],
    )
    .await
    .unwrap();
    increase_height_and_wait_for_client(&local_net, &mut recipient, 1)
        .await
        .unwrap();

    // // send to self transparent
    let recipient_transparent_address = get_base_address_macro!(recipient, "transparent");
    quick_send(
        &mut recipient,
        vec![(
            &recipient_transparent_address,
            value.checked_div(10).unwrap(),
            None,
        )],
    )
    .await
    .unwrap();
    increase_height_and_wait_for_client(&local_net, &mut recipient, 1)
        .await
        .unwrap();

    // // shield transparent
    recipient
        .quick_shield(zip32::AccountId::ZERO)
        .await
        .unwrap();
    increase_height_and_wait_for_client(&local_net, &mut recipient, 1)
        .await
        .unwrap();

    local_net.validator().generate_blocks(1).await.unwrap();

    local_net
}

/// Send from sender to recipient and then bump chain and sync both lightclients
pub async fn send_value_between_clients_and_sync<V, I>(
    local_net: &LocalNet<V, I>,
    sender: &mut LightClient,
    recipient: &mut LightClient,
    value: u64,
    address_pool: PoolType,
) -> Result<String, LightClientError>
where
    V: Validator + LogsToStdoutAndStderr + Send,
    <V as Process>::Config: Send,
    I: Indexer + LogsToStdoutAndStderr,
    <I as Process>::Config: Send,
    LocalNet<V, I>: IndexerConvergence,
{
    let txid = from_inputs::quick_send(
        sender,
        vec![(
            &get_base_address(recipient, address_pool).await,
            value,
            None,
        )],
    )
    .await
    .unwrap();
    increase_height_and_wait_for_client(local_net, sender, 1).await?;
    recipient.sync_and_await().await?;
    Ok(txid.first().to_string())
}

/// This function increases the chain height reliably (with polling) but
/// it _also_ ensures that the client state is synced.
/// Unsynced clients are very interesting to us.  See `increase_server_height`
/// to reliably increase the server without syncing the client
pub async fn increase_height_and_wait_for_client<V, I>(
    local_net: &LocalNet<V, I>,
    client: &mut LightClient,
    n: u32,
) -> Result<(), LightClientError>
where
    V: Validator + LogsToStdoutAndStderr + Send,
    <V as Process>::Config: Send,
    I: Indexer + LogsToStdoutAndStderr,
    <I as Process>::Config: Send,
    LocalNet<V, I>: IndexerConvergence,
{
    let target = generate_n_blocks_return_new_height(local_net, n).await;
    // Barrier first: with the indexer's index at the validator tip, the
    // wallet-side height poll below completes on its first pass instead
    // of spinning against a lagging indexer.
    local_net.converge(target).await;
    sync_to_target_height(client, target).await
}

/// TODO: Add Doc Comment Here!
pub async fn generate_n_blocks_return_new_height<V, I>(local_net: &LocalNet<V, I>, n: u32) -> u32
where
    V: Validator + LogsToStdoutAndStderr + Send,
    <V as Process>::Config: Send,
    I: Indexer + LogsToStdoutAndStderr,
    <I as Process>::Config: Send,
{
    let start_height = local_net.validator().get_chain_height().await;
    let target = start_height + n;
    local_net.validator().generate_blocks(n).await.unwrap();
    assert_eq!(local_net.validator().get_chain_height().await, target);

    target
}
