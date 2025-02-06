#![allow(unused_imports)] // used in tests

use proptest::proptest;
use tokio::runtime::Runtime;

use zcash_client_backend::PoolType::Shielded;
use zcash_client_backend::PoolType::Transparent;
use zcash_client_backend::ShieldedProtocol::Orchard;
use zcash_client_backend::ShieldedProtocol::Sapling;

use zingolib::testutils::chain_generics::fixtures;
use zingolib::testutils::int_to_pooltype;
use zingolib::testutils::int_to_shieldedprotocol;

use crate::utils::scenarios::DarksideEnvironment;

proptest! {
    #![proptest_config(proptest::test_runner::Config::with_cases(4))]
    #[test]
    fn single_sufficient_send_darkside(send_value in 0..50_000u64, change_value in 0..10_000u64, sender_protocol in 1..2, receiver_pool in 1..2) {
        // note: this darkside test does not check the mempool
        Runtime::new().unwrap().block_on(async {
            fixtures::single_sufficient_send::<DarksideEnvironment>(int_to_shieldedprotocol(sender_protocol), int_to_pooltype(receiver_pool), send_value, change_value, false).await;
        });
     }
    #[test]
    fn single_sufficient_send_0_change_darkside(send_value in 0..50_000u64, sender_protocol in 1..2, receiver_pool in 1..2) {
        Runtime::new().unwrap().block_on(async {
            fixtures::single_sufficient_send::<DarksideEnvironment>(int_to_shieldedprotocol(sender_protocol), int_to_pooltype(receiver_pool), send_value, 0, false).await;
        });
     }
}
pub(crate) mod conduct_chain {
    //! known issues include
    //! when a send includes a transparent note, a new txid is generated, replacing the originally sent txid.

    //!   - these tests cannot portray the full range of network weather.

    use orchard::tree::MerkleHashOrchard;
    use zingolib::lightclient::LightClient;
    use zingolib::testutils::chain_generics::conduct_chain::ConductChain;
    use zingolib::wallet::WalletBase;

    use crate::constants::ABANDON_TO_DARKSIDE_SAP_10_000_000_ZAT;
    use crate::constants::DARKSIDE_SEED;
    use crate::darkside_types::TreeState;
    use crate::utils::scenarios::DarksideEnvironment;
    use crate::utils::update_tree_states_for_transaction;

    /// doesnt use the full extent of DarksideEnvironment, preferring to rely on server truths when ever possible.
    impl ConductChain for DarksideEnvironment {
        async fn setup() -> Self {
            let elf = DarksideEnvironment::new(None).await;
            elf.darkside_connector
                .stage_blocks_create(1, 1, 0)
                .await
                .unwrap();
            elf.darkside_connector.apply_staged(1).await.unwrap();
            elf
        }

        /// the mock chain is fed to the Client via lightwalletd. where is that server to be found?
        fn lightserver_uri(&self) -> Option<http::Uri> {
            Some(self.client_builder.server_id.clone())
        }

        async fn create_faucet(&mut self) -> LightClient {
            self.stage_transaction(ABANDON_TO_DARKSIDE_SAP_10_000_000_ZAT)
                .await;
            let zingo_config = self
                .client_builder
                .make_unique_data_dir_and_load_config(self.regtest_network);
            LightClient::create_from_wallet_base_async(
                WalletBase::MnemonicPhrase(DARKSIDE_SEED.to_string()),
                &zingo_config,
                0,
                true,
            )
            .await
            .unwrap()
        }

        fn zingo_config(&mut self) -> zingolib::config::ZingoConfig {
            self.client_builder
                .make_unique_data_dir_and_load_config(self.regtest_network)
        }

        async fn bump_chain(&mut self) {
            let height_before =
                zingolib::grpc_connector::get_latest_block(self.lightserver_uri().unwrap())
                    .await
                    .unwrap()
                    .height;

            let blocks_to_add = 1;

            let mut streamed_raw_txns = self
                .darkside_connector
                .get_incoming_transactions()
                .await
                .unwrap();
            self.darkside_connector
                .clear_incoming_transactions()
                .await
                .unwrap();

            // trees
            let trees = zingolib::grpc_connector::get_trees(
                self.client_builder.server_id.clone(),
                height_before,
            )
            .await
            .unwrap();
            let mut sapling_tree: sapling_crypto::CommitmentTree = zcash_primitives::merkle_tree::read_commitment_tree(
                hex::decode(<sapling_crypto::note_encryption::SaplingDomain as zingolib::wallet::traits::DomainWalletExt>::get_tree(
                    &trees,
                ))
                .unwrap()
                .as_slice(),
            )
            .unwrap();
            let mut orchard_tree: zingolib::testutils::incrementalmerkletree::frontier::CommitmentTree<MerkleHashOrchard, 32> = zcash_primitives::merkle_tree::read_commitment_tree(
                hex::decode(<orchard::note_encryption::OrchardDomain as zingolib::wallet::traits::DomainWalletExt>::get_tree(&trees))
                    .unwrap()
                    .as_slice(),
            )
            .unwrap();

            self.darkside_connector
                .stage_blocks_create(height_before as i32 + 1, blocks_to_add, 0)
                .await
                .unwrap();

            let new_height = (height_before as i32 + blocks_to_add) as u64;

            loop {
                let maybe_raw_tx = streamed_raw_txns.message().await.unwrap();
                match maybe_raw_tx {
                    None => break,
                    Some(raw_tx) => {
                        // increase chain height
                        self.darkside_connector
                            .stage_transactions_stream(vec![(raw_tx.data.clone(), new_height)])
                            .await
                            .unwrap();

                        //trees
                        let transaction = zcash_primitives::transaction::Transaction::read(
                            raw_tx.data.as_slice(),
                            zcash_primitives::consensus::BranchId::Nu6,
                        )
                        .unwrap();
                        for output in transaction
                            .sapling_bundle()
                            .iter()
                            .flat_map(|bundle| bundle.shielded_outputs())
                        {
                            sapling_tree
                                .append(sapling_crypto::Node::from_cmu(output.cmu()))
                                .unwrap()
                        }
                        for action in transaction
                            .orchard_bundle()
                            .iter()
                            .flat_map(|bundle| bundle.actions())
                        {
                            orchard_tree
                                .append(MerkleHashOrchard::from_cmx(action.cmx()))
                                .unwrap()
                        }
                    }
                }
            }

            //trees
            let mut sapling_tree_bytes = vec![];
            zcash_primitives::merkle_tree::write_commitment_tree(
                &sapling_tree,
                &mut sapling_tree_bytes,
            )
            .unwrap();
            let mut orchard_tree_bytes = vec![];
            zcash_primitives::merkle_tree::write_commitment_tree(
                &orchard_tree,
                &mut orchard_tree_bytes,
            )
            .unwrap();
            let new_tree_state = TreeState {
                height: new_height as u64,
                sapling_tree: hex::encode(sapling_tree_bytes),
                orchard_tree: hex::encode(orchard_tree_bytes),
                network: crate::constants::first_tree_state().network,
                hash: "".to_string(),
                time: 0,
            };
            self.darkside_connector
                .add_tree_state(new_tree_state)
                .await
                .unwrap();

            self.darkside_connector
                .apply_staged(new_height as i32)
                .await
                .unwrap();
        }
    }
}
