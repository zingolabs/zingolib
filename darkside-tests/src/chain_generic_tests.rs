use zingolib::{
    lightclient::LightClient, test_framework::chain_generic_tests::ChainTest, wallet::WalletBase,
};

use crate::{
    constants::{ABANDON_TO_DARKSIDE_SAP_10_000_000_ZAT, DARKSIDE_SEED},
    utils::{scenarios::DarksideScenario, update_tree_states_for_transaction},
};

impl ChainTest for DarksideScenario {
    async fn setup() -> Self {
        DarksideScenario::new(None).await
    }

    async fn create_faucet(&mut self) -> LightClient {
        self.stage_transaction(ABANDON_TO_DARKSIDE_SAP_10_000_000_ZAT)
            .await;
        self.client_builder
            .build_client(DARKSIDE_SEED.to_string(), 0, true, self.regtest_network)
            .await
    }

    async fn create_client(&mut self) -> LightClient {
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
                            raw_tx.data.clone(),
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
                    self.darkside_connector
                        .add_tree_state(self.tree_state.clone())
                        .await
                        .unwrap();
                }
            }
        }
        self.darkside_connector
            .stage_blocks_create(u64::from(self.staged_blockheight) as i32, 1, 0)
            .await
            .unwrap();
        self.staged_blockheight = dbg!(self.staged_blockheight + 1);
        self.apply_blocks(u64::from(self.staged_blockheight)).await;
    }
}

#[tokio::test]
async fn chain_generic_darkside_send() {
    zingolib::test_framework::chain_generic_tests::simple_send::<DarksideScenario>(40_000).await;
}

use proptest::proptest;
use tokio::runtime::Runtime;
proptest! {
    #[test]
    fn chain_generic_darkside_send_proptest(value in 0..90_000u32) {
        Runtime::new().unwrap().block_on(async {
    zingolib::test_framework::chain_generic_tests::simple_send::<DarksideScenario>(value).await;
        });
     }
}
