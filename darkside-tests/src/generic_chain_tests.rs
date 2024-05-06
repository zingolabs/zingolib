use std::ops::Add;

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
        prepare_darksidewalletd, scenarios::DarksideScenario, update_tree_states_for_transaction,
        DarksideConnector, DarksideHandler,
    },
};

impl ChainTest for DarksideScenario {
    async fn setup() -> Self {
        let ds = DarksideScenario::default().await;
        prepare_darksidewalletd(ds.darkside_connector.0.clone(), true)
            .await
            .unwrap();
        ds
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

    async fn bump_chain(&mut self) {
        self.stage_blocks(u64::from(self.staged_blockheight) + 10, 0)
            .await;
        let mut streamed_raw_txns = self
            .darkside_connector
            .get_incoming_transactions()
            .await
            .unwrap();
        self.darkside_connector
            .clear_incoming_transactions()
            .await
            .unwrap();
        loop {
            let maybe_raw_tx = streamed_raw_txns.message().await.unwrap();
            match maybe_raw_tx {
                None => break,
                Some(raw_tx) => {
                    self.darkside_connector
                        .stage_transactions_stream(vec![(
                            dbg!(raw_tx.data.clone()),
                            u64::from(self.staged_blockheight),
                        )])
                        .await
                        .unwrap();
                    self.tree_state = update_tree_states_for_transaction(
                        &self.darkside_connector.0,
                        raw_tx.clone(),
                        u64::from(self.staged_blockheight),
                    )
                    .await;
                }
            }
        }
        self.apply_blocks(self.staged_blockheight.into()).await;
    }
}

#[tokio::test]
async fn chain_generic_send() {
    zingolib::test_framework::generic_chain_tests::simple_send::<DarksideScenario>(40_000).await;
}
