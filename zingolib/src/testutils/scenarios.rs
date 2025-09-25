//! In practice there are several common scenarios for which helpers are provided.
//! These scenarios vary in the configuration of clients in use.  Most scenarios
//! require some funds, the simplest way to access funds is to use a "faucet".
//! A "faucet" is a client that receives mining rewards (because its spend capability
//! generated the address registered as the `minetoaddress` in the zcash.conf that's
//! used by the 'regetst mode' zcashs backing these tests.).
//! HELPERS:
//! If you just need a faucet, use the "faucet" helper.
//! If you need a faucet, and a single recipient, use 'faucet_recipient`
//! For less common client configurations use the client builder directly with
//! custom_clients
//! All scenarios have a default (i.e. faucet_default) which take minimal parameters and
//! build the scenario with the most common settings. This simplifies test writing in
//! most cases by removing the need for configuration.

use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::LazyLock;

use bip0039::Mnemonic;

use portpicker::Port;
use tempfile::TempDir;
use zcash_protocol::PoolType;

use zebra_chain::parameters::{NetworkKind, testnet};
use zingo_full_stack_tests::indexer::{
    Indexer, Lightwalletd, LightwalletdConfig, Zainod, ZainodConfig,
};
use zingo_full_stack_tests::network::localhost_uri;
use zingo_full_stack_tests::utils::ExecutableLocation;
use zingo_full_stack_tests::validator::{Validator, Zcashd, ZcashdConfig, Zebrad, ZebradConfig};
use zingo_full_stack_tests::{LocalNet, Process};
use zingo_test_vectors::{
    FUND_OFFLOAD_ORCHARD_ONLY, REG_O_ADDR_FROM_ABANDONART, REG_T_ADDR_FROM_ABANDONART,
    REG_Z_ADDR_FROM_ABANDONART, seeds,
};

use pepper_sync::config::{PerformanceLevel, SyncConfig, TransparentAddressDiscovery};

use crate::config::{ChainType, ZingoConfig, load_clientconfig};
use crate::get_base_address_macro;
use crate::lightclient::LightClient;
use crate::testutils::increase_height_and_wait_for_client;
use crate::testutils::lightclient::from_inputs::quick_send;
use crate::wallet::WalletBase;
use crate::wallet::keys::unified::ReceiverSelection;
use crate::wallet::{LightWallet, WalletSettings};
use network_combo::DefaultIndexer;
use network_combo::DefaultValidator;

/// Default regtest network processes for testing and zingo-cli regtest mode
#[cfg(feature = "test_zainod_zcashd")]
#[allow(missing_docs)]
pub mod network_combo {
    use zingo_full_stack_tests::{indexer::Zainod, validator::Zcashd};

    pub type DefaultIndexer = Zainod;
    pub type DefaultValidator = Zcashd;
}
/// Default regtest network processes for testing and zingo-cli regtest mode
#[cfg(all(not(feature = "test_zainod_zcashd"), feature = "test_lwd_zebrad"))]
#[allow(missing_docs)]
pub mod network_combo {
    use zingo_full_stack_tests::{indexer::Lightwalletd, validator::Zebrad};

    pub type DefaultIndexer = Lightwalletd;
    pub type DefaultValidator = Zebrad;
}
/// Default regtest network processes for testing and zingo-cli regtest mode
#[cfg(all(
    not(feature = "test_zainod_zcashd"),
    not(feature = "test_lwd_zebrad"),
    feature = "test_lwd_zcashd"
))]
#[allow(missing_docs)]
pub mod network_combo {
    use zingo_full_stack_tests::{indexer::Lightwalletd, validator::Zcashd};

    pub type DefaultIndexer = Lightwalletd;
    pub type DefaultValidator = Zcashd;
}
/// Default regtest network processes for testing and zingo-cli regtest mode
#[cfg(not(any(
    feature = "test_zainod_zcashd",
    feature = "test_lwd_zebrad",
    feature = "test_lwd_zcashd"
)))]
#[allow(missing_docs)]
pub mod network_combo {
    use zingo_full_stack_tests::{indexer::Zainod, validator::Zebrad};

    pub type DefaultIndexer = Zainod;
    pub type DefaultValidator = Zebrad;
}

/// Trait for generalising local network functionality across any combination of zcash processes.
pub trait ZingoTestLocalNetwork<I: Indexer, V: Validator> {
    /// Launch local network.
    fn launch(
        indexer_listen_port: Option<Port>,
        mine_to_pool: PoolType,
        activation_heights: testnet::ConfiguredActivationHeights,
        chain_cache: Option<PathBuf>,
    ) -> impl Future<Output = LocalNet<I, V>>;
}

impl ZingoTestLocalNetwork<Zainod, Zebrad> for (Zainod, Zebrad) {
    async fn launch(
        indexer_listen_port: Option<Port>,
        _mine_to_pool: PoolType,
        configured_activation_heights: testnet::ConfiguredActivationHeights,
        chain_cache: Option<PathBuf>,
    ) -> LocalNet<Zainod, Zebrad> {
        LocalNet::<Zainod, Zebrad>::launch(
            ZainodConfig {
                zainod_bin: ZAINOD_BIN.clone(),
                listen_port: indexer_listen_port,
                validator_port: 0,
                chain_cache: None,
                network: NetworkKind::Regtest,
            },
            ZebradConfig {
                zebrad_bin: ZEBRAD_BIN.clone(),
                network_listen_port: None,
                rpc_listen_port: None,
                indexer_listen_port: None,
                configured_activation_heights,
                miner_address: REG_T_ADDR_FROM_ABANDONART,
                chain_cache,
                network: NetworkKind::Regtest,
            },
        )
        .await
    }
}

impl ZingoTestLocalNetwork<Zainod, Zcashd> for (Zainod, Zcashd) {
    async fn launch(
        indexer_listen_port: Option<Port>,
        mine_to_pool: PoolType,
        configured_activation_heights: testnet::ConfiguredActivationHeights,
        chain_cache: Option<PathBuf>,
    ) -> LocalNet<Zainod, Zcashd> {
        let miner_address = match mine_to_pool {
            PoolType::ORCHARD => REG_O_ADDR_FROM_ABANDONART,
            PoolType::SAPLING => REG_Z_ADDR_FROM_ABANDONART,
            PoolType::Transparent => REG_T_ADDR_FROM_ABANDONART,
        };

        LocalNet::<Zainod, Zcashd>::launch(
            ZainodConfig {
                zainod_bin: ZAINOD_BIN.clone(),
                listen_port: indexer_listen_port,
                validator_port: 0,
                chain_cache: None,
                network: NetworkKind::Regtest,
            },
            ZcashdConfig {
                zcashd_bin: ZCASHD_BIN.clone(),
                zcash_cli_bin: ZCASH_CLI_BIN.clone(),
                rpc_listen_port: None,
                configured_activation_heights,
                miner_address: Some(miner_address),
                chain_cache,
            },
        )
        .await
    }
}

impl ZingoTestLocalNetwork<Lightwalletd, Zcashd> for (Lightwalletd, Zcashd) {
    async fn launch(
        indexer_listen_port: Option<Port>,
        mine_to_pool: PoolType,
        configured_activation_heights: testnet::ConfiguredActivationHeights,
        chain_cache: Option<PathBuf>,
    ) -> LocalNet<Lightwalletd, Zcashd> {
        let miner_address = match mine_to_pool {
            PoolType::ORCHARD => REG_O_ADDR_FROM_ABANDONART,
            PoolType::SAPLING => REG_Z_ADDR_FROM_ABANDONART,
            PoolType::Transparent => REG_T_ADDR_FROM_ABANDONART,
        };

        LocalNet::<Lightwalletd, Zcashd>::launch(
            LightwalletdConfig {
                lightwalletd_bin: LIGHTWALLETD_BIN.clone(),
                listen_port: indexer_listen_port,
                zcashd_conf: PathBuf::new(),
                darkside: false,
            },
            ZcashdConfig {
                zcashd_bin: ZCASHD_BIN.clone(),
                zcash_cli_bin: ZCASH_CLI_BIN.clone(),
                rpc_listen_port: None,
                configured_activation_heights,
                miner_address: Some(miner_address),
                chain_cache,
            },
        )
        .await
    }
}

impl ZingoTestLocalNetwork<Lightwalletd, Zebrad> for (Lightwalletd, Zebrad) {
    async fn launch(
        indexer_listen_port: Option<Port>,
        _mine_to_pool: PoolType,
        configured_activation_heights: testnet::ConfiguredActivationHeights,
        chain_cache: Option<PathBuf>,
    ) -> LocalNet<Lightwalletd, Zebrad> {
        LocalNet::<Lightwalletd, Zebrad>::launch(
            LightwalletdConfig {
                lightwalletd_bin: LIGHTWALLETD_BIN.clone(),
                listen_port: indexer_listen_port,
                zcashd_conf: PathBuf::new(),
                darkside: false,
            },
            ZebradConfig {
                zebrad_bin: ZEBRAD_BIN.clone(),
                network_listen_port: None,
                rpc_listen_port: None,
                indexer_listen_port: None,
                configured_activation_heights,
                miner_address: REG_T_ADDR_FROM_ABANDONART,
                chain_cache,
                network: NetworkKind::Regtest,
            },
        )
        .await
    }
}

/// Generate 100 blocks and shield the faucet if attempting to mine to a shielded pool as Zebrad does not currently
/// support this. Also generates an additional block to confirm the shield, dumps the excess funds and generates a
/// final block to confirm the send.
async fn zebrad_shielded_funds<I: Indexer, V: Validator>(
    local_net: &LocalNet<I, V>,
    mine_to_pool: PoolType,
    faucet: &mut LightClient,
) {
    if !matches!(mine_to_pool, PoolType::Transparent) {
        local_net.validator().generate_blocks(100).await.unwrap();
        faucet.sync_and_await().await.unwrap();
        faucet.quick_shield(zip32::AccountId::ZERO).await.unwrap();
        local_net.validator().generate_blocks(1).await.unwrap();
        faucet.sync_and_await().await.unwrap();
        quick_send(faucet, vec![(FUND_OFFLOAD_ORCHARD_ONLY, 624_960_000, None)])
            .await
            .unwrap();
        local_net.validator().generate_blocks(1).await.unwrap();
    }
}

/// Helper function to get the test binary path
fn get_test_binary_path(binary_name: &str) -> ExecutableLocation {
    // Try CARGO_WORKSPACE_DIR first (available in newer cargo versions)
    // Otherwise fall back to CARGO_MANIFEST_DIR and go up one level
    let workspace_dir = std::env::var("CARGO_WORKSPACE_DIR")
        .map(PathBuf::from)
        .or_else(|_| {
            std::env::var("CARGO_MANIFEST_DIR")
                .map(|manifest_dir| PathBuf::from(manifest_dir).parent().unwrap().to_path_buf())
        })
        .unwrap_or_else(|_| PathBuf::from("."));

    let path = workspace_dir
        .join("test_binaries")
        .join("bins")
        .join(binary_name);
    if path.exists() {
        ExecutableLocation::Specific(path.canonicalize().unwrap())
    } else {
        ExecutableLocation::Global(binary_name.to_string())
    }
}

/// Zcashd binary location. First checks test_binaries/bins, then $PATH if not found.
pub static ZCASHD_BIN: LazyLock<ExecutableLocation> =
    LazyLock::new(|| get_test_binary_path("zcashd"));

/// Zcash CLI binary location. First checks test_binaries/bins, then $PATH if not found.
pub static ZCASH_CLI_BIN: LazyLock<ExecutableLocation> =
    LazyLock::new(|| get_test_binary_path("zcash-cli"));

/// Zebrad binary location. First checks test_binaries/bins, then $PATH if not found.
pub static ZEBRAD_BIN: LazyLock<ExecutableLocation> =
    LazyLock::new(|| get_test_binary_path("zebrad"));

/// Lightwalletd binary location. First checks test_binaries/bins, then $PATH if not found.
pub static LIGHTWALLETD_BIN: LazyLock<ExecutableLocation> =
    LazyLock::new(|| get_test_binary_path("lightwalletd"));

/// Zainod binary location. First checks test_binaries/bins, then $PATH if not found.
pub static ZAINOD_BIN: LazyLock<ExecutableLocation> =
    LazyLock::new(|| get_test_binary_path("zainod"));

/// Struct for building lightclients for integration testing
pub struct ClientBuilder {
    /// Indexer URI
    pub server_id: http::Uri,
    /// Directory for wallet files
    pub zingo_datadir: TempDir,
    client_number: u8,
}

impl ClientBuilder {
    /// TODO: Add Doc Comment Here!
    pub fn new(server_id: http::Uri, zingo_datadir: TempDir) -> Self {
        let client_number = 0;
        ClientBuilder {
            server_id,
            zingo_datadir,
            client_number,
        }
    }

    pub fn make_unique_data_dir_and_load_config(
        &mut self,
        configured_activation_heights: testnet::ConfiguredActivationHeights,
    ) -> ZingoConfig {
        //! Each client requires a unique data_dir, we use the
        //! client_number counter for this.
        self.client_number += 1;
        let conf_path = format!(
            "{}_client_{}",
            self.zingo_datadir.path().to_string_lossy(),
            self.client_number
        );
        self.create_clientconfig(PathBuf::from(conf_path), configured_activation_heights)
    }

    /// TODO: Add Doc Comment Here!
    pub fn create_clientconfig(
        &self,
        conf_path: PathBuf,
        configured_activation_heights: testnet::ConfiguredActivationHeights,
    ) -> ZingoConfig {
        std::fs::create_dir(&conf_path).unwrap();
        load_clientconfig(
            self.server_id.clone(),
            Some(conf_path),
            ChainType::Regtest(configured_activation_heights),
            WalletSettings {
                sync_config: SyncConfig {
                    transparent_address_discovery: TransparentAddressDiscovery::minimal(),
                    performance_level: PerformanceLevel::High,
                },
                min_confirmations: NonZeroU32::try_from(1).unwrap(),
            },
            1.try_into().unwrap(),
        )
        .unwrap()
    }

    /// TODO: Add Doc Comment Here!
    pub fn build_faucet(
        &mut self,
        overwrite: bool,
        configured_activation_heights: testnet::ConfiguredActivationHeights,
    ) -> LightClient {
        //! A "faucet" is a lightclient that receives mining rewards
        self.build_client(
            seeds::ABANDON_ART_SEED.to_string(),
            0,
            overwrite,
            configured_activation_heights,
        )
    }

    /// TODO: Add Doc Comment Here!
    pub fn build_client(
        &mut self,
        mnemonic_phrase: String,
        birthday: u64,
        overwrite: bool,
        configured_activation_heights: testnet::ConfiguredActivationHeights,
    ) -> LightClient {
        let config = self.make_unique_data_dir_and_load_config(configured_activation_heights);
        let mut wallet = LightWallet::new(
            config.chain.clone(),
            WalletBase::Mnemonic {
                mnemonic: Mnemonic::from_phrase(mnemonic_phrase).unwrap(),
                no_of_accounts: 1.try_into().unwrap(),
            },
            (birthday as u32).into(),
            config.wallet_settings.clone(),
        )
        .unwrap();
        wallet
            .generate_unified_address(ReceiverSelection::sapling_only(), zip32::AccountId::ZERO)
            .unwrap();

        LightClient::create_from_wallet(wallet, config, overwrite).unwrap()
    }
}

/// TODO: Add Doc Comment Here!
pub async fn unfunded_client(
    activation_heights: LocalNetwork,
    chain_cache: Option<PathBuf>,
) -> (LocalNet<DefaultIndexer, DefaultValidator>, LightClient) {
    let (local_net, mut client_builder) =
        custom_clients(PoolType::ORCHARD, activation_heights, chain_cache).await;

    let mut lightclient = client_builder.build_client(
        seeds::HOSPITAL_MUSEUM_SEED.to_string(),
        1,
        true,
        activation_heights,
    );
    lightclient.sync_and_await().await.unwrap();

    (local_net, lightclient)
}

/// TODO: Add Doc Comment Here!
pub async fn unfunded_client_default() -> (LocalNet<DefaultIndexer, DefaultValidator>, LightClient)
{
    unfunded_client(LocalNetwork::default_regtest_heights(), None).await
}

/// Many scenarios need to start with spendable funds.  This setup provides
/// 3 blocks worth of coinbase to a preregistered spend capability.
///
/// This key is registered to receive block rewards by corresponding to the
/// address registered as the "mineraddress" field in zcash.conf
///
/// The general scenario framework requires instances of zingo-cli, lightwalletd,
/// and zcashd (in regtest mode). This setup is intended to produce the most basic
/// of scenarios.  As scenarios with even less requirements
/// become interesting (e.g. without experimental features, or txindices) we'll create more setups.
pub async fn faucet(
    mine_to_pool: PoolType,
    activation_heights: LocalNetwork,
    chain_cache: Option<PathBuf>,
) -> (LocalNet<DefaultIndexer, DefaultValidator>, LightClient) {
    let (local_net, mut client_builder) =
        custom_clients(mine_to_pool, activation_heights, chain_cache).await;

    let mut faucet = client_builder.build_faucet(true, activation_heights);

    if matches!(DefaultValidator::PROCESS, Process::Zebrad) {
        zebrad_shielded_funds(&local_net, mine_to_pool, &mut faucet).await;
    }

    faucet.sync_and_await().await.unwrap();

    (local_net, faucet)
}

/// TODO: Add Doc Comment Here!
pub async fn faucet_default() -> (LocalNet<DefaultIndexer, DefaultValidator>, LightClient) {
    faucet(
        PoolType::ORCHARD,
        LocalNetwork::default_regtest_heights(),
        None,
    )
    .await
}

/// TODO: Add Doc Comment Here!
pub async fn faucet_recipient(
    mine_to_pool: PoolType,
    activation_heights: LocalNetwork,
    chain_cache: Option<PathBuf>,
) -> (
    LocalNet<DefaultIndexer, DefaultValidator>,
    LightClient,
    LightClient,
) {
    let (local_net, mut client_builder) =
        custom_clients(mine_to_pool, activation_heights, chain_cache).await;

    let mut faucet = client_builder.build_faucet(true, activation_heights);
    let mut recipient = client_builder.build_client(
        seeds::HOSPITAL_MUSEUM_SEED.to_string(),
        1,
        true,
        activation_heights,
    );

    if matches!(DefaultValidator::PROCESS, Process::Zebrad) {
        zebrad_shielded_funds(&local_net, mine_to_pool, &mut faucet).await;
    }

    faucet.sync_and_await().await.unwrap();
    recipient.sync_and_await().await.unwrap();

    (local_net, faucet, recipient)
}

/// TODO: Add Doc Comment Here!
pub async fn faucet_recipient_default() -> (
    LocalNet<DefaultIndexer, DefaultValidator>,
    LightClient,
    LightClient,
) {
    faucet_recipient(
        PoolType::ORCHARD,
        LocalNetwork::default_regtest_heights(),
        None,
    )
    .await
}

/// TODO: Add Doc Comment Here!
pub async fn faucet_funded_recipient(
    orchard_funds: Option<u64>,
    sapling_funds: Option<u64>,
    transparent_funds: Option<u64>,
    mine_to_pool: PoolType,
    activation_heights: LocalNetwork,
    chain_cache: Option<PathBuf>,
) -> (
    LocalNet<DefaultIndexer, DefaultValidator>,
    LightClient,
    LightClient,
    Option<String>,
    Option<String>,
    Option<String>,
) {
    let (local_net, mut faucet, mut recipient) =
        faucet_recipient(mine_to_pool, activation_heights, chain_cache).await;
    increase_height_and_wait_for_client(&local_net, &mut faucet, 1)
        .await
        .unwrap();

    let orchard_txid = if let Some(funds) = orchard_funds {
        Some(
            super::lightclient::from_inputs::quick_send(
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
            super::lightclient::from_inputs::quick_send(
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
            super::lightclient::from_inputs::quick_send(
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
    faucet.sync_and_await().await.unwrap();

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
) -> (
    LocalNet<DefaultIndexer, DefaultValidator>,
    LightClient,
    LightClient,
    String,
) {
    let (local_net, faucet, recipient, orchard_txid, _sapling_txid, _transparent_txid) =
        faucet_funded_recipient(
            Some(orchard_funds),
            None,
            None,
            PoolType::ORCHARD,
            LocalNetwork::default_regtest_heights(),
            None,
        )
        .await;

    (local_net, faucet, recipient, orchard_txid.unwrap())
}

/// TODO: Add Doc Comment Here!
pub async fn custom_clients(
    mine_to_pool: PoolType,
    activation_heights: LocalNetwork,
    chain_cache: Option<PathBuf>,
) -> (LocalNet<DefaultIndexer, DefaultValidator>, ClientBuilder) {
    let local_net = <(DefaultIndexer, DefaultValidator) as ZingoTestLocalNetwork<
        DefaultIndexer,
        DefaultValidator,
    >>::launch(None, mine_to_pool, activation_heights, chain_cache)
    .await;

    local_net.validator().generate_blocks(2).await.unwrap();

    let client_builder = ClientBuilder::new(
        localhost_uri(local_net.indexer().listen_port()),
        tempfile::tempdir().unwrap(),
    );

    (local_net, client_builder)
}

/// TODO: Add Doc Comment Here!
pub async fn custom_clients_default() -> (LocalNet<DefaultIndexer, DefaultValidator>, ClientBuilder)
{
    let (local_net, client_builder) = custom_clients(
        PoolType::ORCHARD,
        LocalNetwork::default_regtest_heights(),
        None,
    )
    .await;

    (local_net, client_builder)
}

/// TODO: Add Doc Comment Here!
pub async fn unfunded_mobileclient() -> LocalNet<DefaultIndexer, DefaultValidator> {
    <(DefaultIndexer, DefaultValidator) as ZingoTestLocalNetwork<
        DefaultIndexer,
        DefaultValidator,
    >>::launch(
        Some(20_000),
        PoolType::SAPLING,
        LocalNetwork::default_regtest_heights(),
        None,
    )
    .await
}

/// TODO: Add Doc Comment Here!
pub async fn funded_orchard_mobileclient(value: u64) -> LocalNet<DefaultIndexer, DefaultValidator> {
    let local_net = unfunded_mobileclient().await;
    let mut client_builder = ClientBuilder::new(
        localhost_uri(local_net.indexer().port()),
        tempfile::tempdir().unwrap(),
    );
    let mut faucet =
        client_builder.build_faucet(true, local_net.validator().get_activation_heights());
    let recipient = client_builder.build_client(
        seeds::HOSPITAL_MUSEUM_SEED.to_string(),
        1,
        true,
        local_net.validator().get_activation_heights(),
    );
    faucet.sync_and_await().await.unwrap();
    super::lightclient::from_inputs::quick_send(
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
) -> LocalNet<DefaultIndexer, DefaultValidator> {
    let local_net = unfunded_mobileclient().await;
    let mut client_builder = ClientBuilder::new(
        localhost_uri(local_net.indexer().port()),
        tempfile::tempdir().unwrap(),
    );
    let mut faucet =
        client_builder.build_faucet(true, local_net.validator().get_activation_heights());
    let mut recipient = client_builder.build_client(
        seeds::HOSPITAL_MUSEUM_SEED.to_string(),
        1,
        true,
        local_net.validator().get_activation_heights(),
    );
    increase_height_and_wait_for_client(&local_net, &mut faucet, 1)
        .await
        .unwrap();
    super::lightclient::from_inputs::quick_send(
        &mut faucet,
        vec![(&get_base_address_macro!(recipient, "unified"), value, None)],
    )
    .await
    .unwrap();
    increase_height_and_wait_for_client(&local_net, &mut recipient, 1)
        .await
        .unwrap();
    super::lightclient::from_inputs::quick_send(
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
    super::lightclient::from_inputs::quick_send(
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
) -> LocalNet<DefaultIndexer, DefaultValidator> {
    let local_net = unfunded_mobileclient().await;
    let mut client_builder = ClientBuilder::new(
        localhost_uri(local_net.indexer().port()),
        tempfile::tempdir().unwrap(),
    );
    let mut faucet =
        client_builder.build_faucet(true, local_net.validator().get_activation_heights());
    let mut recipient = client_builder.build_client(
        seeds::HOSPITAL_MUSEUM_SEED.to_string(),
        1,
        true,
        local_net.validator().get_activation_heights(),
    );
    increase_height_and_wait_for_client(&local_net, &mut faucet, 1)
        .await
        .unwrap();

    // // received from a faucet to transparent
    super::lightclient::from_inputs::quick_send(
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
) -> LocalNet<DefaultIndexer, DefaultValidator> {
    let local_net = unfunded_mobileclient().await;
    let mut client_builder = ClientBuilder::new(
        localhost_uri(local_net.indexer().port()),
        tempfile::tempdir().unwrap(),
    );
    let mut faucet =
        client_builder.build_faucet(true, local_net.validator().get_activation_heights());
    let mut recipient = client_builder.build_client(
        seeds::HOSPITAL_MUSEUM_SEED.to_string(),
        1,
        true,
        local_net.validator().get_activation_heights(),
    );
    increase_height_and_wait_for_client(&local_net, &mut faucet, 1)
        .await
        .unwrap();

    // // received from a faucet to orchard
    super::lightclient::from_inputs::quick_send(
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
    super::lightclient::from_inputs::quick_send(
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
    super::lightclient::from_inputs::quick_send(
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
    super::lightclient::from_inputs::quick_send(
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
    super::lightclient::from_inputs::quick_send(
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
    super::lightclient::from_inputs::quick_send(
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
    super::lightclient::from_inputs::quick_send(
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
