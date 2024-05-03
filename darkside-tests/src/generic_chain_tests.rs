use tokio::time::sleep;
use zingo_testutils::scenarios::setup::ClientBuilder;
use zingoconfig::RegtestNetwork;
use zingolib::{
    get_base_address, lightclient::LightClient, test_framework::generic_chain_tests::ChainTest,
    wallet::WalletBase,
};

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
    client_builder: ClientBuilder,
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

        let client_builder =
            ClientBuilder::new(server_id.clone(), darkside_handler.darkside_dir.clone());

        DarksideChain {
            server_id,
            darkside_handler,
            regtest_network,
            client_builder,
        }
    }

    async fn build_faucet(&mut self) -> LightClient {
        self.client_builder
            .build_client(DARKSIDE_SEED.to_string(), 0, true, self.regtest_network)
            .await
    }

    async fn build_client(&mut self) -> LightClient {
        let zingo_config = self
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

    async fn bump_chain(&self) {
        todo!()
    }
}

#[tokio::test]
async fn chain_generic_send() {
    zingolib::test_framework::generic_chain_tests::simple_setup::<DarksideChain>(40_000).await;
}
