//! libtonode tests use zcashd regtest mode to mock a chain

use zcash_protocol::{PoolType, ShieldedProtocol};
use zingo_infra_services::network::ActivationHeights;

use crate::lightclient::LightClient;
use crate::testutils::chain_generics::conduct_chain::ConductChain;
use crate::testutils::scenarios::setup::ScenarioBuilder;
use crate::testutils::timestamped_test_log;

/// includes utilities for connecting to zcashd regtest
pub struct LibtonodeEnvironment {
    /// internal RegtestNetwork
    pub activation_heights: ActivationHeights,
    /// internal ScenarioBuilder
    pub scenario_builder: ScenarioBuilder,
}

/// known issues include --slow
/// these tests cannot portray the full range of network weather.
impl ConductChain for LibtonodeEnvironment {
    async fn setup() -> Self {
        timestamped_test_log("starting mock libtonode network");
        let activation_heights = ActivationHeights::default();
        // let scenario_builder = ScenarioBuilder::build_configure_launch(
        //     Some(PoolType::Shielded(ShieldedProtocol::Sapling)),
        //     None,
        //     None,
        //     &regtest_network,
        //     true,
        // )
        // .await;
        // timestamped_test_log("started mock libtonode network");
        // LibtonodeEnvironment {
        //     activation_heights: regtest_network,
        //     scenario_builder,
        // }
        todo!()
    }

    async fn create_faucet(&mut self) -> LightClient {
        self.scenario_builder
            .client_builder
            .build_faucet(false, self.activation_heights)
    }

    fn zingo_config(&mut self) -> crate::config::ZingoConfig {
        self.scenario_builder
            .client_builder
            .make_unique_data_dir_and_load_config(self.activation_heights)
    }

    async fn bump_chain(&mut self) {
        let start_height = self
            .scenario_builder
            .regtest_manager
            .get_current_height()
            .unwrap();
        self.scenario_builder
            .regtest_manager
            .generate_n_blocks(1)
            .expect("Called for side effect, failed!");
        assert_eq!(
            self.scenario_builder
                .regtest_manager
                .get_current_height()
                .unwrap(),
            start_height + 1
        );
    }

    fn lightserver_uri(&self) -> Option<http::Uri> {
        Some(self.scenario_builder.client_builder.server_id.clone())
    }
}
