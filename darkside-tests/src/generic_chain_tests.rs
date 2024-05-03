use tokio::time::sleep;
use zingo_testutils::scenarios::setup::ClientBuilder;
use zingoconfig::RegtestNetwork;
use zingolib::{get_base_address, test_framework::generic_chain_tests::ChainTest};

use crate::{
    constants::DARKSIDE_SEED,
    utils::{
        prepare_darksidewalletd, update_tree_states_for_transaction, DarksideConnector,
        DarksideHandler,
    },
};

struct DarksideChain {
    server_id: http::Uri,
    darkside_handler: DarksideHandler,
    regtest_network: RegtestNetwork,
}

impl ChainTest for DarksideChain {
    async fn setup() -> Self {
        let darkside_handler = DarksideHandler::new(None);

        let server_id = zingoconfig::construct_lightwalletd_uri(Some(format!(
            "http://127.0.0.1:{}",
            darkside_handler.grpc_port
        )));
        prepare_darksidewalletd(server_id.clone(), true)
            .await
            .unwrap();
        let regtest_network = RegtestNetwork::all_upgrades_active();

        DarksideChain {
            server_id,
            darkside_handler,
            regtest_network,
        }
    }

    async fn build_client_and_fund(
        &self,
        funds: u32,
        pool: zcash_client_backend::PoolType,
    ) -> zingolib::lightclient::LightClient {
        ClientBuilder::new(
            self.server_id.clone(),
            self.darkside_handler.darkside_dir.clone(),
        )
        .build_client(DARKSIDE_SEED.to_string(), 0, true, self.regtest_network)
        .await
    }

    async fn build_client(&self) -> zingolib::lightclient::LightClient {
        todo!()
    }

    async fn bump_chain(&self) {
        let connector = DarksideConnector(self.server_id.clone());
        let mut streamed_raw_txns = connector.get_incoming_transactions().await.unwrap();
        let raw_tx = streamed_raw_txns.message().await.unwrap().unwrap();
        // There should only be one transaction incoming
        assert!(streamed_raw_txns.message().await.unwrap().is_none());
        connector
            .stage_transactions_stream(vec![(raw_tx.data.clone(), 4)])
            .await
            .unwrap();
        connector.stage_blocks_create(4, 1, 0).await.unwrap();
        update_tree_states_for_transaction(&self.server_id, raw_tx.clone(), 4).await;
        connector.apply_staged(4).await.unwrap();
        sleep(std::time::Duration::from_secs(1)).await;
    }
}

#[tokio::test]
async fn chain_generic_send() {
    zingolib::test_framework::generic_chain_tests::simple_setup::<DarksideChain>(40_000).await;
}
