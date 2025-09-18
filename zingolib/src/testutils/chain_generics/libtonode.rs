//! libtonode tests use zcashd regtest mode to mock a chain

use zingo_infra_services::LocalNet;
use zingo_infra_services::indexer::Indexer;
use zingo_infra_services::network::localhost_uri;
use zingo_infra_services::validator::Validator;

use crate::lightclient::LightClient;
use crate::testutils::chain_generics::conduct_chain::ConductChain;
use crate::testutils::scenarios::ClientBuilder;
use crate::testutils::scenarios::custom_clients_default;
use crate::testutils::scenarios::network_combo::{DefaultIndexer, DefaultValidator};
use crate::testutils::timestamped_test_log;

/// includes utilities for connecting to zcashd regtest
pub struct LibtonodeEnvironment {
    /// Local network
    pub local_net: LocalNet<DefaultIndexer, DefaultValidator>,
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
        self.client_builder.build_faucet(
            false,
            self.local_net.validator().activation_heights().inner(),
        )
    }

    fn zingo_config(&mut self) -> crate::config::ZingoConfig {
        self.client_builder.make_unique_data_dir_and_load_config(
            self.local_net.validator().activation_heights().inner(),
        )
    }

    async fn increase_chain_height(&mut self) {
        let start_height = self.local_net.validator().get_chain_height().await;
        self.local_net
            .validator()
            .generate_blocks(1)
            .await
            .expect("Called for side effect, failed!");
        assert_eq!(
            self.local_net.validator().get_chain_height().await,
            start_height + 1
        );
    }

    fn lightserver_uri(&self) -> Option<http::Uri> {
        Some(localhost_uri(self.local_net.indexer().listen_port()))
    }

    fn confirmation_patience_blocks(&self) -> usize {
        1
    }
}
