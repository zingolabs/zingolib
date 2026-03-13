//! implementation of conduct chain for live chains

use http::Uri;
use zcash_protocol::consensus::BlockHeight;

use crate::{
    config::{DEFAULT_TESTNET_INDEXER_URI, some_infallible_uri},
    lightclient::LightClient,
};

use super::conduct_chain::ConductChain;

/// this is essentially a placeholder.
/// allows using existing `ChainGeneric` functions with `TestNet` wallets
pub struct NetworkedTestEnvironment {
    indexer_uri: Option<Uri>,
    latest_known_server_height: Option<BlockHeight>,
}

impl NetworkedTestEnvironment {
    async fn update_server_height(&mut self) {
        let latest = crate::grpc_connector::get_latest_block(self.indexer_uri().unwrap())
            .await
            .unwrap()
            .height as u32;
        self.latest_known_server_height = Some(BlockHeight::from(latest));
        crate::testutils::timestamped_test_log(
            format!("Networked Test Chain is now at height {latest}").as_str(),
        );
    }
}

impl ConductChain for NetworkedTestEnvironment {
    async fn setup() -> Self {
        Self {
            indexer_uri: some_infallible_uri(DEFAULT_TESTNET_INDEXER_URI),
            latest_known_server_height: None,
        }
    }

    async fn create_faucet(&mut self) -> LightClient {
        unimplemented!()
    }

    async fn zingo_config(&mut self) -> crate::config::ZingoConfig {
        unimplemented!()
    }

    async fn increase_chain_height(&mut self) {
        let before_height = self.latest_known_server_height;
        // loop until the server height increases
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            self.update_server_height().await;
            if self.latest_known_server_height != before_height {
                break;
            }
        }
    }

    fn indexer_uri(&self) -> Option<Uri> {
        self.indexer_uri.clone()
    }

    fn confirmation_patience_blocks(&self) -> usize {
        10
    }
}
