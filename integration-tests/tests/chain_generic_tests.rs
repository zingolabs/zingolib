use zcash_client_backend::PoolType::Shielded;
use zcash_client_backend::PoolType::Transparent;
use zcash_client_backend::ShieldedProtocol::Orchard;
use zcash_client_backend::ShieldedProtocol::Sapling;

use zingo_testutils::chain_generic_tests::fixtures::change_required;
use zingo_testutils::chain_generic_tests::fixtures::ignore_dust_inputs;
use zingo_testutils::chain_generic_tests::fixtures::propose_and_broadcast_value_to_pool;
use zingo_testutils::chain_generic_tests::fixtures::send_grace_dust;
use zingo_testutils::chain_generic_tests::fixtures::send_required_dust;
use zingo_testutils::chain_generic_tests::fixtures::send_shield_cycle;
use zingo_testutils::chain_generic_tests::fixtures::send_value_to_pool;

use libtonode_environment::LibtonodeEnvironment;

#[tokio::test]
async fn libtonode_send_40_000_to_transparent() {
    send_value_to_pool::<LibtonodeEnvironment>(40_000, Transparent).await;
}
#[tokio::test]
async fn libtonode_send_40_000_to_sapling() {
    send_value_to_pool::<LibtonodeEnvironment>(40_000, Shielded(Sapling)).await;
}
#[tokio::test]
async fn libtonode_send_40_000_to_orchard() {
    send_value_to_pool::<LibtonodeEnvironment>(40_000, Shielded(Orchard)).await;
}

#[tokio::test]
async fn libtonode_propose_and_broadcast_40_000_to_transparent() {
    propose_and_broadcast_value_to_pool::<LibtonodeEnvironment>(40_000, Transparent).await;
}
#[tokio::test]
async fn libtonode_propose_and_broadcast_40_000_to_sapling() {
    propose_and_broadcast_value_to_pool::<LibtonodeEnvironment>(40_000, Shielded(Sapling)).await;
}
#[tokio::test]
async fn libtonode_propose_and_broadcast_40_000_to_orchard() {
    propose_and_broadcast_value_to_pool::<LibtonodeEnvironment>(40_000, Shielded(Orchard)).await;
}
#[tokio::test]
async fn libtonode_change_required() {
    change_required::<LibtonodeEnvironment>().await;
}
#[tokio::test]
async fn libtonode_send_shield_cycle() {
    send_shield_cycle::<LibtonodeEnvironment>(4).await;
}
#[tokio::test]
async fn libtonode_send_required_dust() {
    send_required_dust::<LibtonodeEnvironment>().await;
}
#[tokio::test]
async fn libtonode_send_grace_dust() {
    send_grace_dust::<LibtonodeEnvironment>().await;
}
#[tokio::test]
async fn libtonode_ignore_dust_inputs() {
    ignore_dust_inputs::<LibtonodeEnvironment>().await;
}

pub(crate) mod libtonode_environment {
    use zcash_client_backend::PoolType;

    use zcash_client_backend::ShieldedProtocol::Sapling;

    use zingo_testutils::chain_generic_tests::conduct_chain::ConductChain;
    use zingo_testutils::scenarios::setup::ScenarioBuilder;
    use zingoconfig::RegtestNetwork;
    use zingolib::lightclient::LightClient;
    use zingolib::wallet::WalletBase;
    pub(crate) struct LibtonodeEnvironment {
        regtest_network: RegtestNetwork,
        scenario_builder: ScenarioBuilder,
    }

    /// known issues include --slow
    /// these tests cannot portray the full range of network weather.
    impl ConductChain for LibtonodeEnvironment {
        async fn setup() -> Self {
            let regtest_network = RegtestNetwork::all_upgrades_active();
            let scenario_builder = ScenarioBuilder::build_configure_launch(
                Some(PoolType::Shielded(Sapling)),
                None,
                None,
                &regtest_network,
            )
            .await;
            LibtonodeEnvironment {
                regtest_network,
                scenario_builder,
            }
        }

        async fn create_faucet(&mut self) -> LightClient {
            self.scenario_builder
                .client_builder
                .build_faucet(false, self.regtest_network)
                .await
        }

        async fn create_client(&mut self) -> LightClient {
            let zingo_config = self
                .scenario_builder
                .client_builder
                .make_unique_data_dir_and_load_config(self.regtest_network);
            LightClient::create_from_wallet_base_async(
                WalletBase::FreshEntropy,
                &zingo_config,
                0,
                false,
            )
            .await
            .unwrap()
        }

        async fn bump_chain(&mut self) {
            let start_height = self
                .scenario_builder
                .regtest_manager
                .get_current_height()
                .unwrap();
            let target = start_height + 1;
            self.scenario_builder
                .regtest_manager
                .generate_n_blocks(1)
                .expect("Called for side effect, failed!");
            assert_eq!(
                self.scenario_builder
                    .regtest_manager
                    .get_current_height()
                    .unwrap(),
                target
            );
        }
    }
}
