//! libtonode tests use zcashd regtest mode to mock a chain

use zcash_protocol::{PoolType, ShieldedProtocol};
use zingo_infra_services::LocalNet;
use zingo_infra_services::indexer::{Indexer, Lightwalletd};
use zingo_infra_services::network::{ActivationHeights, localhost_uri};
use zingo_infra_services::validator::{Validator, Zcashd};

use crate::lightclient::LightClient;
use crate::testutils::chain_generics::conduct_chain::ConductChain;
use crate::testutils::scenarios::custom_clients_default;
use crate::testutils::scenarios::setup::{ClientBuilder, ScenarioBuilder};
use crate::testutils::timestamped_test_log;

/// includes utilities for connecting to zcashd regtest
pub struct LibtonodeEnvironment {
    /// Local network
    pub local_net: LocalNet<Lightwalletd, Zcashd>,
    /// Client builder
    pub client_builder: ClientBuilder,
}

/// known issues include --slow
/// these tests cannot portray the full range of network weather.
impl ConductChain for LibtonodeEnvironment {
    async fn setup() -> Self {
        timestamped_test_log("starting mock libtonode network");
        let (local_net, client_builder) = custom_clients_default().await;

        LibtonodeEnvironment {
            local_net,
            client_builder,
        }
    }

    async fn create_faucet(&mut self) -> LightClient {
        self.client_builder
            .build_faucet(false, self.local_net.validator().activation_heights())
    }

    fn zingo_config(&mut self) -> crate::config::ZingoConfig {
        self.client_builder
            .make_unique_data_dir_and_load_config(self.local_net.validator().activation_heights())
    }

    async fn bump_chain(&mut self) {
        self.local_net.validator().generate_blocks(1).await.unwrap();
    }

    fn lightserver_uri(&self) -> Option<http::Uri> {
        Some(localhost_uri(self.local_net.indexer().listen_port()))
    }
}
