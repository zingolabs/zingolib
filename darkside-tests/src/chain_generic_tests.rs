use proptest::proptest;
use tokio::runtime::Runtime;

use zcash_client_backend::PoolType::Shielded;
use zcash_client_backend::PoolType::Transparent;
use zcash_client_backend::ShieldedProtocol::Orchard;
use zcash_client_backend::ShieldedProtocol::Sapling;

use zingo_testutils::chain_generic_tests::fixtures::send_value_to_pool;

use crate::utils::scenarios::DarksideEnvironment;

#[tokio::test]
#[ignore] // darkside cant handle transparent?
async fn darkside_send_40_000_to_transparent() {
    send_value_to_pool::<DarksideEnvironment>(40_000, Transparent).await;
}

proptest! {
    #![proptest_config(proptest::test_runner::Config::with_cases(4))]
    #[test]
    fn darkside_send_pvalue_to_orchard(value in 0..90u64) {
        Runtime::new().unwrap().block_on(async {
    send_value_to_pool::<DarksideEnvironment>(value * 1_000, Shielded(Orchard)).await;
        });
     }
    #[test]
    fn darkside_send_pvalue_to_sapling(value in 0..90u64) {
        Runtime::new().unwrap().block_on(async {
    send_value_to_pool::<DarksideEnvironment>(value * 1_000, Shielded(Sapling)).await;
        });
     }
}

pub(crate) mod impl_conduct_chain_for_darkside_environment {
    //! known issues include
    //!   - transparent sends do not work
    //!   - txids are regenerated randomly. zingo can optionally accept_server_txid
    //! these tests cannot portray the full range of network weather.

    use zingo_testutils::chain_generic_tests::conduct_chain::ConductChain;
    use zingolib::lightclient::LightClient;
    use zingolib::wallet::WalletBase;

    use crate::constants::ABANDON_TO_DARKSIDE_SAP_10_000_000_ZAT;
    use crate::constants::DARKSIDE_SEED;
    use crate::utils::scenarios::DarksideEnvironment;
    use crate::utils::update_tree_states_for_transaction;
    impl ConductChain for DarksideEnvironment {
        async fn setup() -> Self {
            DarksideEnvironment::new(None).await
        }

        async fn create_faucet(&mut self) -> LightClient {
            self.stage_transaction(ABANDON_TO_DARKSIDE_SAP_10_000_000_ZAT)
                .await;
            let mut zingo_config = self
                .client_builder
                .make_unique_data_dir_and_load_config(self.regtest_network);
            zingo_config.accept_server_txids = true;
            LightClient::create_from_wallet_base_async(
                WalletBase::MnemonicPhrase(DARKSIDE_SEED.to_string()),
                &zingo_config,
                0,
                true,
            )
            .await
            .unwrap()
        }

        async fn create_client(&mut self) -> LightClient {
            let mut zingo_config = self
                .client_builder
                .make_unique_data_dir_and_load_config(self.regtest_network);
            zingo_config.accept_server_txids = true;
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
            self.staged_blockheight = self.staged_blockheight + 1;
            self.apply_blocks(u64::from(self.staged_blockheight)).await;
        }
    }
}
