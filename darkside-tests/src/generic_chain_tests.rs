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
    // async fn setup() -> Self {
    //     todo!()
    // }

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
        todo!()
    }
}

#[tokio::test]
async fn chain_generic_send() {
    let darkside_handler = DarksideHandler::new(None);

    let server_id = zingoconfig::construct_lightwalletd_uri(Some(format!(
        "http://127.0.0.1:{}",
        darkside_handler.grpc_port
    )));
    prepare_darksidewalletd(server_id.clone(), true)
        .await
        .unwrap();
    let regtest_network = RegtestNetwork::all_upgrades_active();

    let darksidechain = DarksideChain {
        server_id,
        darkside_handler,
        regtest_network,
    };

    zingolib::test_framework::generic_chain_tests::simple(darksidechain, 40_000).await;
}
