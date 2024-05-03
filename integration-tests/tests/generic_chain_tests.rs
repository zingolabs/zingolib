#![cfg(feature = "generic_chain_tests")]
use zcash_client_backend::{PoolType, ShieldedProtocol::Sapling};
use zingo_testutils::scenarios::setup::{self, ScenarioBuilder};
use zingoconfig::RegtestNetwork;
use zingolib::{test_framework::generic_chain_tests::ChainTest, wallet::notes::SaplingNote};

struct LibtonodeChain {
    regtest_network: RegtestNetwork,
    scenario_builder: ScenarioBuilder,
}

impl ChainTest for LibtonodeChain {
    async fn setup() -> Self {
        let regtest_network = RegtestNetwork::all_upgrades_active();
        let scenario_builder = setup::ScenarioBuilder::build_configure_launch(
            Some(PoolType::Shielded(Sapling).into()),
            None,
            None,
            &regtest_network,
        )
        .await;
        LibtonodeChain {
            regtest_network,
            scenario_builder,
        }
    }

    async fn build_faucet(&mut self) -> zingolib::lightclient::LightClient {
        self.scenario_builder
            .client_builder
            .build_faucet(false, self.regtest_network)
            .await
    }

    async fn build_client(&self) -> zingolib::lightclient::LightClient {
        todo!()
    }

    async fn bump_chain(&self) {
        todo!()
    }
}

#[tokio::test]
async fn chain_generic_send() {
    zingolib::test_framework::generic_chain_tests::simple_setup::<LibtonodeChain>(40_000).await;
}
