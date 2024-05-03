#![cfg(feature = "generic_chain_tests")]
use zingo_testutils::scenarios::setup;
use zingoconfig::RegtestNetwork;
use zingolib::test_framework::generic_chain_tests::ChainTest;

struct LibtonodeChain {
    regtest_network: RegtestNetwork,
}

impl ChainTest for LibtonodeChain {
    async fn setup() -> Self {
        let regtest_network = RegtestNetwork::all_upgrades_active();
        LibtonodeChain { regtest_network }
    }

    async fn build_client_and_fund(
        &self,
        funds: u32,
        pool: zcash_client_backend::PoolType,
    ) -> zingolib::lightclient::LightClient {
        let mut sb = setup::ScenarioBuilder::build_configure_launch(
            Some(pool.into()),
            None,
            None,
            &self.regtest_network,
        )
        .await;
        sb.client_builder
            .build_faucet(false, self.regtest_network)
            .await
    }

    async fn build_client(&self) -> zingolib::lightclient::LightClient {
        todo!()
    }

    async fn bump_chain(&self) {
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

#[tokio::test]
async fn chain_generic_send() {
    zingolib::test_framework::generic_chain_tests::simple_setup::<LibtonodeChain>(40_000).await;
}
