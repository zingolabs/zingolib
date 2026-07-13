//! TODO: Add Mod Description Here!

use std::convert::Infallible;

use nonempty::NonEmpty;

use zcash_client_backend::proposal::Proposal;
use zcash_client_backend::zip321::TransactionRequest;
use zcash_primitives::transaction::{TxId, fees::zip317};

use zingo_netutils::Indexer as _;
use zingo_netutils::lightwallet_protocol::RawTransaction;
use zingo_status::confirmation_status::ConfirmationStatus;

use crate::data::proposal::ZingoProposal;
use crate::lightclient::error::{LightClientError, SendError, TransmissionError};
use crate::lightclient::{DEFAULT_REQUEST_TIMEOUT, LightClient};
use crate::wallet::error::{CalculateTransactionError, WalletError};
use crate::wallet::output::OutputRef;

const MAX_RETRIES: u8 = 3;

/// A "queued for download" duplicate rejection proves delivery but not
/// minability: zebra is still verifying the earlier submission
/// (observed to lag it by seconds under load). Each resubmission is a
/// free probe of zebra's own state; wait up to this many probes, on
/// the retry loop's one-second cadence, for the verdict to become
/// storage-backed (issue #2450).
const MAX_QUEUED_PROBES: u8 = 30;

impl LightClient {
    /// Calculates transactions from a proposal and transmits them. The gate
    /// on a connected Indexer sits here, before calculation, so a doomed
    /// send fails without storing Calculated transactions it cannot
    /// transmit. `wrap_calculate_error` names the [`SendError`] variant the
    /// caller's proposal kind reports calculation failure through.
    async fn calculate_and_transmit<NoteRef>(
        &mut self,
        proposal: Proposal<zip317::FeeRule, NoteRef>,
        account: zip32::AccountId,
        wrap_calculate_error: impl FnOnce(CalculateTransactionError<NoteRef>) -> SendError,
    ) -> Result<NonEmpty<TxId>, LightClientError> {
        self.require_indexer()?;
        let calculated_txids = self
            .wallet()
            .write()
            .await
            .calculate_transactions(proposal, account)
            .await
            .map_err(wrap_calculate_error)?;

        self.transmit_transactions(calculated_txids).await
    }

    async fn send(
        &mut self,
        proposal: Proposal<zip317::FeeRule, OutputRef>,
        sending_account: zip32::AccountId,
    ) -> Result<NonEmpty<TxId>, LightClientError> {
        self.calculate_and_transmit(proposal, sending_account, SendError::CalculateSendError)
            .await
    }

    async fn shield(
        &mut self,
        proposal: Proposal<zip317::FeeRule, Infallible>,
        shielding_account: zip32::AccountId,
    ) -> Result<NonEmpty<TxId>, LightClientError> {
        self.calculate_and_transmit(proposal, shielding_account, SendError::CalculateShieldError)
            .await
    }

    /// Calculates and transmits transactions from a proposal value the
    /// caller holds — the pure-core two-phase send entry point (ADR 0006).
    ///
    /// The proposal is borrowed, so an Indexerless attempt fails with
    /// [`LightClientError::Offline`] while the caller's value remains
    /// intact by construction, ready to retry once an Indexer is
    /// configured.
    ///
    /// If sync was running prior to proposing, sync will have been paused.
    /// If `resume_sync` is `true`, sync will be resumed after transmission.
    pub async fn send_proposal(
        &mut self,
        proposal: &ZingoProposal,
        resume_sync: bool,
    ) -> Result<NonEmpty<TxId>, LightClientError> {
        let txids = match proposal {
            ZingoProposal::Send {
                proposal,
                sending_account,
            } => self.send(proposal.clone(), *sending_account).await,
            ZingoProposal::Shield {
                proposal,
                shielding_account,
            } => self.shield(proposal.clone(), *shielding_account).await,
        }?;

        if resume_sync {
            let _ignore_error = self.resume_sync();
        }

        Ok(txids)
    }

    /// Proposes and transmits transactions from a transaction request skipping proposal confirmation.
    ///
    /// If sync is running, sync will be paused before creating the send proposal. If `resume_sync` is `true`, sync will be resumed after send.
    pub async fn quick_send(
        &mut self,
        request: TransactionRequest,
        account_id: zip32::AccountId,
        resume_sync: bool,
    ) -> Result<NonEmpty<TxId>, LightClientError> {
        // Proposing is an Indexerless capability; only the calculate/transmit
        // stage below demands a connection.
        let _ignore_error = self.pause_sync();
        let proposal = self
            .wallet()
            .write()
            .await
            .create_send_proposal(request, account_id)
            .map_err(SendError::ProposeSendError)?;
        let txids = self.send(proposal, account_id).await?;
        if resume_sync {
            let _ignore_error = self.resume_sync();
        }

        Ok(txids)
    }

    /// Shields all transparent funds skipping proposal confirmation.
    pub async fn quick_shield(
        &mut self,
        account_id: zip32::AccountId,
    ) -> Result<NonEmpty<TxId>, LightClientError> {
        // Proposing is an Indexerless capability; only the calculate/transmit
        // stage below demands a connection.
        let proposal = self
            .wallet()
            .write()
            .await
            .create_shield_proposal(account_id)
            .map_err(SendError::ProposeShieldError)?;

        self.shield(proposal, account_id).await
    }

    /// Tranmits calculated transactions stored in the wallet matching txids of `calculated_txids` in the given order.
    /// Returns list of txids for successfully transmitted transactions.
    async fn transmit_transactions(
        &mut self,
        calculated_txids: NonEmpty<TxId>,
    ) -> Result<NonEmpty<TxId>, LightClientError> {
        let indexer = self.require_indexer()?.clone();
        let mut wallet = self.wallet().write().await;
        for txid in calculated_txids.iter() {
            let calculated_transaction = wallet
                .wallet_transactions
                .get(txid)
                .ok_or(WalletError::TransactionNotFound(*txid))?;
            let height = calculated_transaction.status().get_height();

            if !matches!(
                calculated_transaction.status(),
                ConfirmationStatus::Calculated(_)
            ) {
                return Err(SendError::TransmissionError(
                    TransmissionError::IncorrectTransactionStatus(*txid),
                )
                .into());
            }

            let mut transaction_bytes = vec![];
            calculated_transaction
                .transaction()
                .write(&mut transaction_bytes)
                .map_err(|e| {
                    pepper_sync::set_transactions_failed(
                        &mut wallet.wallet_transactions,
                        vec![*txid],
                    );
                    wallet.save_required = true;
                    WalletError::TransactionWrite(e)
                })?;

            let mut retry_count = 0;
            let mut queued_probes = 0;
            let txid_from_server = loop {
                let transmission_result = indexer
                    .clone()
                    .send_transaction(
                        RawTransaction {
                            data: transaction_bytes.clone(),
                            height: height.into(),
                        },
                        DEFAULT_REQUEST_TIMEOUT,
                    )
                    .await
                    .map_err(|e| {
                        SendError::TransmissionError(TransmissionError::TransmissionFailed(
                            format!("{e:?}"),
                        ))
                    });

                match transmission_result {
                    Ok(txid) => {
                        break Ok(txid);
                    }
                    Err(e) => {
                        // The node's rejections of resubmitted bytes
                        // are positive confirmation that an earlier
                        // submission was received (issue #2450).
                        // Substring matches because zainod surfaces
                        // the rejections untyped (zingolabs/zaino#1392);
                        // upgrade to typed checks when that lands.
                        if let SendError::TransmissionError(
                            TransmissionError::TransmissionFailed(message),
                        ) = &e
                        {
                            // Storage-backed duplicates: the earlier
                            // submission is minable (in the mempool)
                            // or already mined, so transmission is
                            // complete.
                            if message.contains("transaction already exists in mempool")
                                || message.contains("transaction already in block chain")
                            {
                                break Ok(txid.to_string());
                            }
                            // "Queued for download" proves delivery,
                            // not minability: zebra is still verifying
                            // the earlier submission. Hold success
                            // until the verdict is storage-backed, so
                            // send-Ok keeps meaning the transaction is
                            // minable now.
                            if message.contains("already queued for download") {
                                if queued_probes >= MAX_QUEUED_PROBES {
                                    pepper_sync::set_transactions_failed(
                                        &mut wallet.wallet_transactions,
                                        vec![*txid],
                                    );
                                    wallet.save_required = true;
                                    break Err(e);
                                }
                                queued_probes += 1;
                                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                                continue;
                            }
                        }
                        if retry_count >= MAX_RETRIES {
                            pepper_sync::set_transactions_failed(
                                &mut wallet.wallet_transactions,
                                vec![*txid],
                            );
                            wallet.save_required = true;
                            break Err(e);
                        } else {
                            retry_count += 1;
                            tokio::time::sleep(std::time::Duration::from_secs(1)).await;

                            continue;
                        }
                    }
                }
            }?;

            wallet
                .wallet_transactions
                .get_mut(txid)
                .ok_or(WalletError::TransactionNotFound(*txid))?
                .update_status(ConfirmationStatus::Transmitted(height), crate::utils::now());
            wallet.save_required = true;

            let txid_from_server =
                crate::utils::conversion::txid_from_hex_encoded_str(txid_from_server.as_str())
                    .map_err(WalletError::ConversionFailed)?;
            if txid_from_server != *txid {
                // during darkside tests, the server may report a different txid to the one calculated.
                #[cfg(not(feature = "darkside_tests"))]
                {
                    return Err(SendError::TransmissionError(
                        TransmissionError::IncorrectTxidFromServer(*txid, txid_from_server),
                    )
                    .into());
                }
            }
        }

        Ok(calculated_txids)
    }
}

/// Gap-4 cells of the protection audit's remediation plan
/// (docs/testing/test-protection-audit-dev-to-ironwood.md § Gap
/// remediation plan): the built transaction's expiry and consensus
/// branch id must derive from the wallet's synced height + 1.
/// `LightWallet::calculate_transactions` is the build-without-broadcast
/// seam — it proves and stores the transaction without transmitting —
/// so these cells run offline over a synthetic wallet.
#[cfg(test)]
mod built_transaction_shape {
    use zcash_protocol::consensus::{BlockHeight, BranchId};
    use zingo_common_components::protocol::ActivationHeights;
    use zingo_status::confirmation_status::ConfirmationStatus;

    use crate::lightclient::LightClient;
    use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;
    use crate::utils::conversion::address_from_str;
    use crate::wallet::LightWallet;
    use crate::wallet::keys::unified::ReceiverSelection;

    /// An orchard address of a different wallet, so the send is external.
    fn external_orchard_address() -> zcash_address::ZcashAddress {
        let mut external_wallet =
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::ABANDON_ART_SEED).build();
        let (_, unified_address) = external_wallet
            .generate_unified_address(ReceiverSelection::orchard_only(), zip32::AccountId::ZERO)
            .unwrap();
        address_from_str(&unified_address.encode(&external_wallet.chain_type())).unwrap()
    }

    /// Builds (without broadcasting) one send-all from the given wallet
    /// and returns the stored transaction's (target, expiry, branch id).
    async fn build_one_send(wallet: LightWallet) -> (u32, u32, BranchId) {
        let mut client = LightClient::new_for_test(wallet).await;
        let proposal = client
            .propose_send_all(
                external_orchard_address(),
                false,
                None,
                zip32::AccountId::ZERO,
            )
            .await
            .unwrap();
        let txids = client
            .wallet()
            .write()
            .await
            .calculate_transactions(proposal, zip32::AccountId::ZERO)
            .await
            .unwrap();
        assert_eq!(txids.len(), 1);
        let wallet = client.wallet();
        let wallet = wallet.read().await;
        let record = wallet.wallet_transactions.get(&txids[0]).unwrap();
        let ConfirmationStatus::Calculated(target) = record.status() else {
            panic!("a built, untransmitted transaction is stored as Calculated");
        };
        let transaction = record.transaction();
        (
            u32::from(target),
            u32::from(transaction.expiry_height()),
            transaction.consensus_branch_id(),
        )
    }

    /// The plain cell: synced to the default tip, the build targets
    /// tip + 1, expires the standard forty blocks later, and commits to
    /// the branch id in force at the target.
    #[tokio::test]
    async fn expiry_and_branch_id_derive_from_synced_height() {
        let tip = 20;
        let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .orchard_note(100_000)
            .tip(tip)
            .build();
        let chain = wallet.chain_type();

        let (target, expiry, branch_id) = build_one_send(wallet).await;

        assert_eq!(target, tip + 1);
        assert_eq!(expiry, target + 40, "standard tx expiry delta");
        assert_eq!(
            branch_id,
            BranchId::for_height(&chain, BlockHeight::from_u32(tip + 1))
        );
    }

    /// Offline twin of libtonode `fast::mine_to_transparent_and_shield`,
    /// which stays live as the pipeline control (its coinbase provenance
    /// and the documented shield-eligibility race are inexpressible
    /// offline): four transparent coins shield in one step, and the
    /// built transaction nets exactly their sum minus the 30_000
    /// four-input shield fee into orchard. The live assert is the
    /// post-confirmation orchard balance; the offline equivalent is the
    /// orchard bundle's value balance on the built transaction.
    #[tokio::test]
    async fn four_coin_shield_builds_and_nets_input_minus_fee() {
        let coin_value = 1_000_000u64;
        let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .transparent_coin(coin_value)
            .transparent_coin(coin_value)
            .transparent_coin(coin_value)
            .transparent_coin(coin_value)
            .build();
        let mut client = LightClient::new_for_test(wallet).await;

        let proposal = client.propose_shield(zip32::AccountId::ZERO).await.unwrap();
        let txids = client
            .wallet()
            .write()
            .await
            .calculate_transactions(proposal, zip32::AccountId::ZERO)
            .await
            .unwrap();
        assert_eq!(txids.len(), 1);

        let wallet = client.wallet();
        let wallet = wallet.read().await;
        let transaction = wallet
            .wallet_transactions
            .get(&txids[0])
            .unwrap()
            .transaction();
        let transparent = transaction
            .transparent_bundle()
            .expect("a shield spends transparent coins");
        assert_eq!(transparent.vin.len(), 4, "all four coins consumed");
        assert!(
            transparent.vout.is_empty(),
            "a shield pays no transparent outputs"
        );
        let orchard = transaction
            .orchard_bundle()
            .expect("a shield produces orchard change");
        // Negative value balance is value flowing INTO the orchard pool:
        // the four coins minus the 30_000 zip317 fee (four transparent
        // inputs plus the orchard action pair).
        assert_eq!(
            i64::from(orchard.value_balance()),
            -i64::try_from(4 * coin_value - 30_000).unwrap()
        );
    }

    /// Gap-1b cell of the remediation plan, mirroring the live
    /// multi_input_sapling_send_with_orchard_change_no_panic offline: a
    /// payment that no single sapling note covers builds (proves) a
    /// two-input sapling spend whose change crosses to orchard. The
    /// sapling proving parameters are embedded in the crate, so the
    /// plan's parameters precondition is satisfied in the unit
    /// environment.
    #[tokio::test]
    async fn two_input_sapling_spend_with_orchard_change_builds_offline() {
        use zcash_client_backend::zip321::{Payment, TransactionRequest};
        use zcash_protocol::value::Zatoshis;

        let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .sapling_note(20_000)
            .sapling_note(30_000)
            .build();
        let mut client = LightClient::new_for_test(wallet).await;

        // 25_000 plus the 20_000 ZIP-317 fee (two sapling spends, two
        // orchard actions) exceeds either note alone, so both are
        // gathered and 5_000 returns as orchard change.
        let request = TransactionRequest::new(vec![Payment::without_memo(
            external_orchard_address(),
            Zatoshis::const_from_u64(25_000),
        )])
        .unwrap();
        let proposal = client
            .propose_send(request, zip32::AccountId::ZERO)
            .await
            .unwrap();
        let step = proposal.steps().first();
        let change = step.balance().proposed_change();
        assert_eq!(change.len(), 1);
        assert_eq!(u64::from(change[0].value()), 5_000);
        assert_eq!(
            change[0].output_pool(),
            zcash_protocol::PoolType::ORCHARD,
            "the change crosses to orchard"
        );

        let txids = client
            .wallet()
            .write()
            .await
            .calculate_transactions(proposal, zip32::AccountId::ZERO)
            .await
            .unwrap();
        assert_eq!(txids.len(), 1);

        let wallet = client.wallet();
        let wallet = wallet.read().await;
        let transaction = wallet
            .wallet_transactions
            .get(&txids[0])
            .unwrap()
            .transaction();
        let sapling_bundle = transaction
            .sapling_bundle()
            .expect("spending sapling notes produces a sapling bundle");
        assert_eq!(
            sapling_bundle.shielded_spends().len(),
            2,
            "both fabricated sapling notes are spent"
        );
        // The sapling outputs are the builder's dummy padding; the real
        // change is the orchard action asserted above.
        let orchard_bundle = transaction
            .orchard_bundle()
            .expect("the orchard payment and change produce an orchard bundle");
        assert_eq!(
            orchard_bundle.actions().len(),
            2,
            "payment plus change, the orchard minimum"
        );
    }

    /// Gap-3 cell of the remediation plan: the entire ZIP-320 two-step
    /// builds offline behind the seam — `zcash_client_backend` chains
    /// step one's ephemeral transparent output into step two before
    /// anything touches a network. Step two's sole transparent input
    /// spends step one's ephemeral output, and the TEX-decoded P2PKH
    /// address receives the payment. The only class left live is zebra's
    /// mempool accepting the chained unmined pair.
    #[tokio::test]
    async fn tex_two_step_chains_ephemeral_output_offline() {
        use pepper_sync::keys::decode_address;
        use zcash_client_backend::address::Address;
        use zcash_client_backend::zip321::{Payment, TransactionRequest};
        use zcash_protocol::value::Zatoshis;
        use zcash_transparent::address::TransparentAddress;

        let payment_value = 100_000;
        let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .orchard_note(5_000_000)
            .build();
        let mut client = LightClient::new_for_test(wallet).await;

        // A TEX destination derived from an external wallet's first
        // transparent address, as ZIP 320 prescribes.
        let external_wallet =
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::ABANDON_ART_SEED).build();
        let taddr = external_wallet
            .transparent_addresses()
            .values()
            .next()
            .unwrap()
            .clone();
        let Address::Transparent(TransparentAddress::PublicKeyHash(taddr_bytes)) =
            decode_address(&external_wallet.chain_type(), &taddr).unwrap()
        else {
            panic!("a wallet-generated first taddr is p2pkh")
        };
        let tex_address = crate::testutils::interpret_taddr_as_tex_addr(
            taddr_bytes,
            &external_wallet.chain_type(),
        );

        let request = TransactionRequest::new(vec![Payment::without_memo(
            zcash_address::ZcashAddress::try_from_encoded(&tex_address).unwrap(),
            Zatoshis::from_u64(payment_value).unwrap(),
        )])
        .unwrap();
        let proposal = client
            .propose_send(request, zip32::AccountId::ZERO)
            .await
            .unwrap();
        let txids = client
            .wallet()
            .write()
            .await
            .calculate_transactions(proposal, zip32::AccountId::ZERO)
            .await
            .unwrap();
        assert_eq!(txids.len(), 2, "the ZIP-320 pair builds as two steps");

        let wallet = client.wallet();
        let wallet = wallet.read().await;
        let step_one = wallet.wallet_transactions.get(&txids[0]).unwrap();
        let step_two = wallet.wallet_transactions.get(&txids[1]).unwrap();

        let step_two_transparent = step_two
            .transaction()
            .transparent_bundle()
            .expect("the transparent leg carries a transparent bundle");
        assert_eq!(step_two_transparent.vin.len(), 1);
        let prevout = step_two_transparent.vin[0].prevout();
        assert_eq!(
            *prevout.txid(),
            txids[0],
            "step two's sole input spends step one"
        );
        let step_one_bundle = step_one
            .transaction()
            .transparent_bundle()
            .expect("the shield leg pays out an ephemeral transparent output");
        let ephemeral_output = step_one_bundle
            .vout
            .get(prevout.n() as usize)
            .expect("the spent index exists among step one's outputs");
        assert_eq!(
            step_two_transparent.vout.len(),
            1,
            "the transparent leg pays the TEX destination and nothing else"
        );
        // One transparent input, one transparent output: the ZIP-317 fee
        // is the two-action grace minimum, 10_000 zats.
        assert_eq!(
            u64::from(ephemeral_output.value()),
            payment_value + 10_000,
            "step one's ephemeral output funds step two's payment plus its fee exactly"
        );

        let expected_script: zcash_transparent::address::Script =
            TransparentAddress::PublicKeyHash(taddr_bytes)
                .script()
                .into();
        let tex_payment = step_two_transparent
            .vout
            .iter()
            .find(|out| *out.script_pubkey() == expected_script)
            .expect("one of step two's outputs pays the TEX-decoded p2pkh");
        assert_eq!(u64::from(tex_payment.value()), payment_value);
    }

    /// The boundary cell the tip_spend_rejection attribution isolated:
    /// a wallet synced to activation − 1 builds a transaction targeting
    /// the activation height, so it must commit to the POST-activation
    /// branch id. This is the permanent unit fence for the wallet-side
    /// wrong-branch-id failure observed live at the height-5 NU6.1/6.2
    /// co-activation.
    #[tokio::test]
    async fn boundary_adjacent_build_uses_post_activation_branch_id() {
        let boundary = 10;
        let heights = ActivationHeights::builder()
            .set_overwinter(Some(1))
            .set_sapling(Some(1))
            .set_blossom(Some(1))
            .set_heartwood(Some(1))
            .set_canopy(Some(1))
            .set_nu5(Some(1))
            .set_nu6(Some(1))
            .set_nu6_1(Some(1))
            .set_nu6_2(Some(boundary))
            .set_nu6_3(None)
            .set_nu7(None)
            .build();
        let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .orchard_note(100_000)
            .tip(boundary - 1)
            .activation_heights(heights)
            .build();
        let chain = wallet.chain_type();
        let pre_activation = BranchId::for_height(&chain, BlockHeight::from_u32(boundary - 1));
        let post_activation = BranchId::for_height(&chain, BlockHeight::from_u32(boundary));
        assert_ne!(
            pre_activation, post_activation,
            "the cell must sit on a real branch boundary"
        );

        let (target, _, branch_id) = build_one_send(wallet).await;

        assert_eq!(target, boundary);
        assert_eq!(branch_id, post_activation);
    }
}

#[cfg(test)]
mod test {
    //! all tests below (and in this mod) use example wallets, which describe real-world chains.

    use zingo_test_vectors::seeds;

    use crate::{
        config::{ClientConfig, WalletConfig},
        lightclient::{LightClient, error::LightClientError, sync::test::sync_example_wallet},
        mocks::proposal::ProposalBuilder,
        testutils::{
            chain_generics::{
                conduct_chain::ConductChain as _, networked::NetworkedTestEnvironment,
                with_assertions,
            },
            default_test_wallet_settings,
        },
        wallet::disk::testing::examples,
    };

    async fn create_basic_client() -> LightClient {
        let config = ClientConfig::builder()
            .set_wallet_config(WalletConfig::MnemonicPhrase {
                mnemonic_phrase: seeds::HOSPITAL_MUSEUM_SEED.to_string(),
                no_of_accounts: 1.try_into().unwrap(),
                birthday: 419200,
                wallet_settings: default_test_wallet_settings(),
            })
            .build();
        LightClient::new(config, true).await.unwrap()
    }

    #[tokio::test]
    async fn complete_and_broadcast_unconnected_error() {
        let mut lc = create_basic_client().await;
        let proposal = ProposalBuilder::default().build();
        let err = lc.send(proposal, zip32::AccountId::ZERO).await.unwrap_err();
        assert!(matches!(err, LightClientError::Offline));
    }

    /// live sync: execution time increases linearly until example wallet is upgraded
    /// live send TESTNET: these assume the wallet has on-chain TAZ.
    /// waits up to five blocks for confirmation per transaction. see [`zingolib/src/testutils/chain_generics/live_chain.rs`]
    /// as of now, average block time is supposedly about 75 seconds
    mod testnet {
        use zcash_protocol::{PoolType, ShieldedProtocol};

        use crate::testutils::lightclient::get_base_address;

        use super::*;

        #[ignore = "only one test can be run per testnet wallet at a time"]
        #[tokio::test]
        /// this is a networked sync test. its execution time scales linearly since last updated
        /// this is a networked send test. whether it can work depends on the state of live wallet on the blockchain
        async fn testnet_send_to_self_orchard_glory_goddess() {
            let case =
                examples::NetworkSeedVersion::Testnet(examples::TestnetSeedVersion::GloryGoddess);

            let mut client = sync_example_wallet(case).await;

            let client_addr =
                get_base_address(&client, PoolType::Shielded(ShieldedProtocol::Orchard)).await;

            with_assertions::assure_propose_send_bump_sync_all_recipients(
                &mut NetworkedTestEnvironment::setup().await,
                &mut client,
                vec![(&client_addr, 20_000, None)],
                vec![],
                true,
            )
            .await
            .unwrap();
        }
        #[ignore = "only one test can be run per testnet wallet at a time"]
        #[tokio::test]
        /// this is a networked sync test. its execution time scales linearly since last updated
        /// this is a networked send test. whether it can work depends on the state of live wallet on the blockchain
        async fn testnet_send_to_self_sapling_glory_goddess() {
            let case =
                examples::NetworkSeedVersion::Testnet(examples::TestnetSeedVersion::GloryGoddess);

            let mut client = sync_example_wallet(case).await;

            let client_addr =
                get_base_address(&client, PoolType::Shielded(ShieldedProtocol::Sapling)).await;

            with_assertions::assure_propose_send_bump_sync_all_recipients(
                &mut NetworkedTestEnvironment::setup().await,
                &mut client,
                vec![(&client_addr, 20_000, None)],
                vec![],
                true,
            )
            .await
            .unwrap();
        }
        #[ignore = "only one test can be run per testnet wallet at a time"]
        #[tokio::test]
        /// this is a networked sync test. its execution time scales linearly since last updated
        /// this is a networked send test. whether it can work depends on the state of live wallet on the blockchain
        /// about 273 seconds
        async fn testnet_send_to_self_transparent_and_then_shield_glory_goddess() {
            let case =
                examples::NetworkSeedVersion::Testnet(examples::TestnetSeedVersion::GloryGoddess);

            let mut client = sync_example_wallet(case).await;

            let client_addr = get_base_address(&client, PoolType::Transparent).await;

            let environment = &mut NetworkedTestEnvironment::setup().await;
            with_assertions::assure_propose_send_bump_sync_all_recipients(
                environment,
                &mut client,
                vec![(&client_addr, 100_001, None)],
                vec![],
                true,
            )
            .await
            .unwrap();

            let _ =
                with_assertions::assure_propose_shield_bump_sync(environment, &mut client, true)
                    .await
                    .unwrap();
        }
        #[ignore = "this needs to pass CI, but we arent there with testnet"]
        #[tokio::test]
        /// this is a networked sync test. its execution time scales linearly since last updated
        /// this is a networked send test. whether it can work depends on the state of live wallet on the blockchain
        async fn testnet_send_to_self_all_pools_glory_goddess() {
            let case =
                examples::NetworkSeedVersion::Testnet(examples::TestnetSeedVersion::GloryGoddess);

            let mut client = sync_example_wallet(case).await;
            let environment = &mut NetworkedTestEnvironment::setup().await;

            let client_addr =
                get_base_address(&client, PoolType::Shielded(ShieldedProtocol::Orchard)).await;
            with_assertions::assure_propose_send_bump_sync_all_recipients(
                &mut NetworkedTestEnvironment::setup().await,
                &mut client,
                vec![(&client_addr, 14_000, None)],
                vec![],
                true,
            )
            .await
            .unwrap();

            let client_addr =
                get_base_address(&client, PoolType::Shielded(ShieldedProtocol::Sapling)).await;
            with_assertions::assure_propose_send_bump_sync_all_recipients(
                &mut NetworkedTestEnvironment::setup().await,
                &mut client,
                vec![(&client_addr, 15_000, None)],
                vec![],
                true,
            )
            .await
            .unwrap();

            let client_addr = get_base_address(&client, PoolType::Transparent).await;
            with_assertions::assure_propose_send_bump_sync_all_recipients(
                environment,
                &mut client,
                vec![(&client_addr, 100_000, None)],
                vec![],
                true,
            )
            .await
            .unwrap();

            let _ =
                with_assertions::assure_propose_shield_bump_sync(environment, &mut client, true)
                    .await
                    .unwrap();
        }
    }
}

/// Migrated from libtonode `slow::t_incoming_t_outgoing_disallowed`: a
/// received transparent coin appears in the transaction summaries with its
/// height and value, and spending transparent funds through an ordinary
/// send is refused — the wallet demands a shield first, surfacing as an
/// insufficient-funds proposal error because transparent coins are not
/// send-spendable.
#[cfg(test)]
mod transparent_policy {
    use crate::{
        lightclient::LightClient,
        lightclient::error::{LightClientError, SendError},
        testutils::lightclient::from_inputs,
        testutils::synthetic_wallet::SyntheticWalletBuilder,
        wallet::error::ProposeSendError,
    };

    #[tokio::test]
    async fn t_incoming_t_outgoing_disallowed() {
        let value = 100_000;
        let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .transparent_coin(value)
            .build();
        let mut client = LightClient::new_for_test(wallet).await;

        let transaction = client
            .wallet()
            .read()
            .await
            .transaction_summaries(false)
            .await
            .unwrap()
            .0
            .first()
            .unwrap()
            .clone();
        // The builder confirms its first fabricated record at height 2.
        assert_eq!(transaction.blockheight, 2.into());
        assert_eq!(transaction.value, value);

        let sent_value = 20_000;
        let sent_transaction_error = from_inputs::quick_send(
            &mut client,
            vec![(zingo_test_vectors::EXT_TADDR, sent_value, None)],
        )
        .await
        .unwrap_err();
        assert!(matches!(
            sent_transaction_error,
            LightClientError::SendError(SendError::ProposeSendError(ProposeSendError::Proposal(
                zcash_client_backend::data_api::error::Error::InsufficientFunds {
                    available: _,
                    required: _
                }
            )))
        ));
    }
}
