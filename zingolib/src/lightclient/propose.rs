//! `LightClient` function `do_propose` generates a proposal to send to specified addresses.

use zcash_address::ZcashAddress;
use zcash_client_backend::zip321::TransactionRequest;
use zcash_protocol::value::Zatoshis;

use crate::ZENNIES_FOR_ZINGO_AMOUNT;
use crate::data::proposal::ProportionalFeeProposal;
use crate::data::proposal::ProportionalFeeShieldProposal;
use crate::data::proposal::ZingoProposal;
use crate::data::receivers::Receiver;
use crate::data::receivers::transaction_request_from_receivers;
use crate::get_zennies_for_zingo_address;
use crate::lightclient::LightClient;
use crate::wallet::error::ProposeSendError;
use crate::wallet::error::ProposeShieldError;

impl LightClient {
    fn append_zingo_zenny_receiver(&self, receivers: &mut Vec<Receiver>) {
        let zfz_address = get_zennies_for_zingo_address(self.chain_type());
        let dev_donation_receiver = Receiver::new(
            crate::utils::conversion::address_from_str(zfz_address).expect("Hard coded str"),
            Zatoshis::from_u64(ZENNIES_FOR_ZINGO_AMOUNT).expect("Hard coded u64."),
            None,
        );
        receivers.push(dev_donation_receiver);
    }

    /// Creates and stores a proposal from a transaction request.
    pub async fn propose_send(
        &mut self,
        request: TransactionRequest,
        account_id: zip32::AccountId,
    ) -> Result<ProportionalFeeProposal, ProposeSendError> {
        let _ignore_error = self.pause_sync();
        let mut wallet = self.wallet().write().await;
        let proposal = wallet.create_send_proposal(request, account_id)?;
        wallet.store_proposal(ZingoProposal::Send {
            proposal: proposal.clone(),
            sending_account: account_id,
        });

        Ok(proposal)
    }

    /// Creates and stores a proposal for sending all shielded funds from a specified account to a given `address`.
    pub async fn propose_send_all(
        &mut self,
        address: ZcashAddress,
        zennies_for_zingo: bool,
        memo: Option<zcash_protocol::memo::MemoBytes>,
        account_id: zip32::AccountId,
    ) -> Result<ProportionalFeeProposal, ProposeSendError> {
        let max_send_value = self
            .max_send_value(address.clone(), zennies_for_zingo, account_id)
            .await?;
        if max_send_value == Zatoshis::ZERO {
            return Err(ProposeSendError::ZeroValueSendAll);
        }
        let mut receivers = vec![Receiver::new(address, max_send_value, memo)];
        if zennies_for_zingo {
            self.append_zingo_zenny_receiver(&mut receivers);
        }
        let request = transaction_request_from_receivers(receivers)
            .map_err(ProposeSendError::TransactionRequestFailed)?;
        let _ignore_error = self.pause_sync();
        let mut wallet = self.wallet().write().await;
        let proposal = wallet.create_send_proposal(request, account_id)?;
        wallet.store_proposal(ZingoProposal::Send {
            proposal: proposal.clone(),
            sending_account: account_id,
        });

        Ok(proposal)
    }

    /// Creates and stores a proposal for shielding all transparent funds..
    pub async fn propose_shield(
        &mut self,
        account_id: zip32::AccountId,
    ) -> Result<ProportionalFeeShieldProposal, ProposeShieldError> {
        let mut wallet = self.wallet().write().await;
        let proposal = wallet.create_shield_proposal(account_id)?;
        wallet.store_proposal(ZingoProposal::Shield {
            proposal: proposal.clone(),
            shielding_account: account_id,
        });

        Ok(proposal)
    }

    /// Returns the maximum value that can be sent from the given `account_id`.
    ///
    /// This value is calculated from the shielded spendable balance minus any fees required to send those funds to
    /// the given `address`. If the wallet is still syncing, the spendable balance may be less than the confirmed
    /// balance - minus the fee - due to notes being above the minimum confirmation threshold or not being able to
    /// construct a witness from the current state of the wallet's note commitment tree.
    /// If `zennies_for_zingo` is set true, an additional payment of `1_000_000` ZAT to the `ZingoLabs` developer address
    /// will be taken into account.
    ///
    /// # Error
    ///
    /// Will return an error if this method fails to calculate the total wallet balance or create the
    /// proposal needed to calculate the fee
    pub async fn max_send_value(
        &self,
        address: ZcashAddress,
        zennies_for_zingo: bool,
        account_id: zip32::AccountId,
    ) -> Result<Zatoshis, ProposeSendError> {
        let mut wallet = self.wallet().write().await;
        let confirmed_balance = wallet.shielded_spendable_balance(account_id, false)?;
        let mut spendable_balance = confirmed_balance;

        loop {
            let mut receivers = vec![Receiver::new(address.clone(), spendable_balance, None)];
            if zennies_for_zingo {
                self.append_zingo_zenny_receiver(&mut receivers);
            }
            let request = transaction_request_from_receivers(receivers)?;
            let trial_proposal = wallet.create_send_proposal(request, account_id);

            match trial_proposal {
                Err(ProposeSendError::Proposal(
                    zcash_client_backend::data_api::error::Error::InsufficientFunds {
                        available,
                        required,
                    },
                )) => {
                    if let Some(shortfall) = required - confirmed_balance {
                        match spendable_balance - shortfall {
                            Some(updated_spendable) => {
                                spendable_balance = updated_spendable;
                            }
                            None => {
                                return Err(ProposeSendError::Proposal(
                                zcash_client_backend::data_api::error::Error::InsufficientFunds {
                                    available: confirmed_balance,
                                    required,
                                },
                            ));
                            }
                        }
                    } else {
                        // bugged underflow case, required should always be larger than confirmed shielded balance to cause
                        // insufficient funds error.
                        // returns insufficient funds error with same values from original error for debugging
                        return Err(ProposeSendError::Proposal(
                            zcash_client_backend::data_api::error::Error::InsufficientFunds {
                                available,
                                required,
                            },
                        ));
                    }
                }
                Err(e) => {
                    return Err(e);
                }
                Ok(_) => {
                    break;
                }
            }
        }

        Ok(spendable_balance)
    }
}

#[cfg(test)]
mod shielding {
    use zcash_protocol::consensus::Parameters;
    use zingo_test_vectors::seeds;

    use crate::{
        config::{ClientConfig, WalletConfig},
        lightclient::LightClient,
        testutils::default_test_wallet_settings,
        wallet::error::ProposeShieldError,
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
    async fn propose_shield_missing_scan_prerequisite() {
        let basic_client = create_basic_client().await;
        let propose_shield_result = basic_client
            .wallet()
            .write()
            .await
            .create_shield_proposal(zip32::AccountId::ZERO);
        match propose_shield_result {
            Err(ProposeShieldError::Component(
                zcash_client_backend::data_api::error::Error::ScanRequired,
            )) => true,
            _ => panic!("Unexpected error state!"),
        };
    }
    #[tokio::test]
    async fn get_transparent_addresses() {
        let basic_client = create_basic_client().await;
        let network = basic_client.chain_type();

        // TODO: store t addrs as concrete types instead of encoded
        let transparent_addresses = basic_client
            .wallet()
            .read()
            .await
            .transparent_addresses()
            .values()
            .map(|address| {
                Ok(zcash_address::ZcashAddress::try_from_encoded(address)?
                    .convert_if_network::<zcash_transparent::address::TransparentAddress>(
                        network.network_type(),
                    )
                    .expect("incorrect network should be checked on wallet load"))
            })
            .collect::<Result<Vec<_>, zcash_address::ParseError>>()
            .unwrap();

        assert_eq!(
            transparent_addresses,
            [
                zcash_transparent::address::TransparentAddress::PublicKeyHash([
                    161, 138, 222, 242, 254, 121, 71, 105, 93, 131, 177, 31, 59, 185, 120, 148,
                    255, 189, 198, 33
                ])
            ]
        );
    }
}

/// Migrated from libtonode's `send_all` suite: these guarantees are pure
/// proposal logic over wallet state, so a synthetic wallet replaces the
/// LocalNet round trip.
#[cfg(test)]
mod send_all {
    use zcash_protocol::PoolType;
    use zcash_protocol::value::Zatoshis;

    use crate::{
        lightclient::LightClient, testutils::synthetic_wallet::SyntheticWalletBuilder,
        utils::conversion::address_from_str, wallet::error::ProposeSendError,
        wallet::keys::unified::ReceiverSelection,
    };

    /// An address belonging to a different wallet, so the send is external.
    fn external_address(pool: PoolType) -> zcash_address::ZcashAddress {
        let mut external_wallet =
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::ABANDON_ART_SEED).build();
        let selection = match pool {
            PoolType::ORCHARD => ReceiverSelection::orchard_only(),
            PoolType::SAPLING => ReceiverSelection::sapling_only(),
            _ => unimplemented!("only shielded destinations are needed here"),
        };
        let (_, unified_address) = external_wallet
            .generate_unified_address(selection, zip32::AccountId::ZERO)
            .unwrap();
        address_from_str(&unified_address.encode(&external_wallet.chain_type())).unwrap()
    }

    /// Migrated from libtonode `send_all::ptfm_insufficient_funds`: a
    /// send-all whose only note cannot cover the fee of a cross-pool spend
    /// reports the exact shortfall.
    #[tokio::test]
    async fn ptfm_insufficient_funds() {
        let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .orchard_note(10_000)
            .build();
        let mut client = LightClient::new_for_test(wallet).await;

        let proposal_error = client
            .propose_send_all(
                external_address(PoolType::SAPLING),
                false,
                None,
                zip32::AccountId::ZERO,
            )
            .await;

        match proposal_error {
            Err(ProposeSendError::Proposal(
                zcash_client_backend::data_api::error::Error::InsufficientFunds {
                    available: a,
                    required: r,
                },
            )) => {
                assert_eq!(a, Zatoshis::const_from_u64(10_000));
                assert_eq!(r, Zatoshis::const_from_u64(30_000));
            }
            _ => panic!("expected an InsufficientFunds error"),
        }
    }

    /// Migrated from libtonode `send_all::ptfm_zero_value`: a send-all
    /// whose only note is entirely consumed by the fee is rejected as a
    /// zero-value send.
    #[tokio::test]
    async fn ptfm_zero_value() {
        let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .orchard_note(10_000)
            .build();
        let mut client = LightClient::new_for_test(wallet).await;

        let proposal_error = client
            .propose_send_all(
                external_address(PoolType::ORCHARD),
                false,
                None,
                zip32::AccountId::ZERO,
            )
            .await;

        assert!(matches!(
            proposal_error,
            Err(ProposeSendError::ZeroValueSendAll)
        ));
    }

    /// Migrated from libtonode `send_all::toggle_zennies_for_zingo`: with
    /// Zennies for Zingo enabled, the maximum sendable value deducts the
    /// zenny amount and the fee for one orchard note in, three outputs out.
    #[tokio::test]
    async fn toggle_zennies_for_zingo() {
        let initial_funds = 2_000_000;
        let zennies_magnitude = 1_000_000;
        let expected_fee = 15_000;

        let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .orchard_note(initial_funds)
            .build();
        let client = LightClient::new_for_test(wallet).await;

        assert_eq!(
            client
                .max_send_value(
                    external_address(PoolType::ORCHARD),
                    true,
                    zip32::AccountId::ZERO
                )
                .await
                .unwrap(),
            Zatoshis::from_u64(initial_funds - zennies_magnitude - expected_fee).unwrap()
        );
    }
}

/// Migrated from the libtonode chain_generics simpool instantiations
/// (`simpool_insufficient_{1,10_000}_orchard_to_*` and
/// `simpool_no_fund_1_000_000_to_*`): the insufficient-funds and unfunded
/// propose errors are pure proposal logic over wallet state, so a synthetic
/// wallet replaces the LocalNet environment and its multi-hop funding
/// chain.
#[cfg(test)]
mod simpool {
    use zcash_protocol::{PoolType, ShieldedProtocol};

    use crate::{
        lightclient::LightClient,
        testutils::{
            fee_tables, lightclient::from_inputs, synthetic_wallet::SyntheticWalletBuilder,
        },
        wallet::keys::unified::ReceiverSelection,
    };

    /// An encoded destination of the given pool type, belonging to a
    /// different wallet so the send is external.
    fn external_address(pool: PoolType) -> String {
        let mut external_wallet =
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::ABANDON_ART_SEED).build();
        let selection = match pool {
            PoolType::Shielded(ShieldedProtocol::Orchard) => ReceiverSelection::orchard_only(),
            PoolType::Shielded(ShieldedProtocol::Sapling) => ReceiverSelection::sapling_only(),
            PoolType::Transparent => return external_wallet.get_address(PoolType::Transparent),
        };
        let (_, unified_address) = external_wallet
            .generate_unified_address(selection, zip32::AccountId::ZERO)
            .unwrap();
        unified_address.encode(&external_wallet.chain_type())
    }

    /// A wallet holding one `source`-pool note `underflow_amount` short of
    /// a 100_000 send to `pool` reports the exact shortfall.
    async fn insufficient(source: ShieldedProtocol, underflow_amount: u64, pool: PoolType) {
        let expected_fee = fee_tables::one_to_one(Some(source), pool, true);
        let secondary_fund = 100_000 + expected_fee - underflow_amount;
        let builder = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED);
        let wallet = match source {
            ShieldedProtocol::Orchard => builder.orchard_note(secondary_fund),
            ShieldedProtocol::Sapling => builder.sapling_note(secondary_fund),
        }
        .build();
        let mut client = LightClient::new_for_test(wallet).await;

        let tertiary_fund = 100_000;
        assert_eq!(
            from_inputs::propose(
                &mut client,
                vec![(external_address(pool).as_str(), tertiary_fund, None)],
            )
            .await
            .unwrap_err()
            .to_string(),
            format!(
                "Insufficient balance (have {}, need {} including fee)",
                secondary_fund,
                tertiary_fund + expected_fee
            )
        );
    }

    /// A wallet with no funds at all reports the full amount-plus-fee need.
    async fn unfunded_to(try_amount: u64, pool: PoolType) {
        let expected_fee = fee_tables::one_to_one(None, pool, true);
        let wallet =
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED).build();
        let mut client = LightClient::new_for_test(wallet).await;

        assert_eq!(
            from_inputs::propose(
                &mut client,
                vec![(external_address(pool).as_str(), try_amount, None)],
            )
            .await
            .unwrap_err()
            .to_string(),
            format!(
                "Insufficient balance (have {}, need {} including fee)",
                0,
                try_amount + expected_fee
            )
        );
    }

    #[tokio::test]
    async fn insufficient_1_orchard_to_orchard() {
        insufficient(ShieldedProtocol::Orchard, 1, PoolType::ORCHARD).await;
    }
    #[tokio::test]
    async fn insufficient_1_orchard_to_sapling() {
        insufficient(ShieldedProtocol::Orchard, 1, PoolType::SAPLING).await;
    }
    #[tokio::test]
    async fn insufficient_1_orchard_to_transparent() {
        insufficient(ShieldedProtocol::Orchard, 1, PoolType::Transparent).await;
    }
    #[tokio::test]
    async fn insufficient_10_000_orchard_to_orchard() {
        insufficient(ShieldedProtocol::Orchard, 10_000, PoolType::ORCHARD).await;
    }
    #[tokio::test]
    async fn insufficient_10_000_orchard_to_sapling() {
        insufficient(ShieldedProtocol::Orchard, 10_000, PoolType::SAPLING).await;
    }
    #[tokio::test]
    async fn insufficient_10_000_orchard_to_transparent() {
        insufficient(ShieldedProtocol::Orchard, 10_000, PoolType::Transparent).await;
    }
    #[tokio::test]
    async fn no_fund_1_000_000_to_orchard() {
        unfunded_to(1_000_000, PoolType::ORCHARD).await;
    }
    #[tokio::test]
    async fn no_fund_1_000_000_to_sapling() {
        unfunded_to(1_000_000, PoolType::SAPLING).await;
    }
    #[tokio::test]
    async fn no_fund_1_000_000_to_transparent() {
        unfunded_to(1_000_000, PoolType::Transparent).await;
    }
    #[tokio::test]
    async fn insufficient_1_sapling_to_orchard() {
        insufficient(ShieldedProtocol::Sapling, 1, PoolType::ORCHARD).await;
    }
    #[tokio::test]
    async fn insufficient_1_sapling_to_sapling() {
        insufficient(ShieldedProtocol::Sapling, 1, PoolType::SAPLING).await;
    }
    #[tokio::test]
    async fn insufficient_1_sapling_to_transparent() {
        insufficient(ShieldedProtocol::Sapling, 1, PoolType::Transparent).await;
    }
    #[tokio::test]
    async fn insufficient_10_000_sapling_to_orchard() {
        insufficient(ShieldedProtocol::Sapling, 10_000, PoolType::ORCHARD).await;
    }
    #[tokio::test]
    async fn insufficient_10_000_sapling_to_sapling() {
        insufficient(ShieldedProtocol::Sapling, 10_000, PoolType::SAPLING).await;
    }
    #[tokio::test]
    async fn insufficient_10_000_sapling_to_transparent() {
        insufficient(ShieldedProtocol::Sapling, 10_000, PoolType::Transparent).await;
    }
}

/// Offline proposal-shape tests over synthetic wallets: shield proposals
/// and note-selection behavior, migrated from LocalNet tests whose
/// assertions were all propose-stage.
#[cfg(test)]
mod proposal_shape {
    use zcash_primitives::transaction::fees::zip317::MARGINAL_FEE;
    use zcash_protocol::{PoolType, ShieldedProtocol};

    use crate::lightclient::LightClient;
    use crate::testutils::lightclient::from_inputs;
    use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;
    use crate::wallet::keys::unified::ReceiverSelection;

    fn external_address(pool: PoolType) -> String {
        let mut external_wallet =
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::ABANDON_ART_SEED).build();
        let selection = match pool {
            PoolType::ORCHARD => ReceiverSelection::orchard_only(),
            PoolType::SAPLING => ReceiverSelection::sapling_only(),
            _ => unimplemented!("only shielded destinations are needed here"),
        };
        let (_, unified_address) = external_wallet
            .generate_unified_address(selection, zip32::AccountId::ZERO)
            .unwrap();
        unified_address.encode(&external_wallet.chain_type())
    }

    /// Migrated from libtonode `shield_transparent` (long `#[ignore]`d):
    /// shielding transparent funds proposes without error, consuming the
    /// coin into shielded change minus the fee. The original asserted
    /// nothing beyond the operations succeeding, so the offline proposal
    /// covers everything it protected.
    #[tokio::test]
    async fn shield_transparent() {
        let value = 100_000;
        let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .transparent_coin(value)
            .build();
        let mut client = LightClient::new_for_test(wallet).await;

        let proposal = client.propose_shield(zip32::AccountId::ZERO).await.unwrap();
        assert_eq!(proposal.steps().len(), 1);
        let step = proposal.steps().first();
        assert_eq!(step.transparent_inputs().len(), 1);
        let fee = u64::from(step.balance().fee_required());
        let change: u64 = step
            .balance()
            .proposed_change()
            .iter()
            .map(|change| u64::from(change.value()))
            .sum();
        assert_eq!(change + fee, value);
    }

    /// Migrated from libtonode `fast::mine_to_transparent_and_propose_shielding`:
    /// a four-coin shield proposes as a single step spending all four
    /// coins into one change output, with the zip317 fee for four
    /// transparent inputs plus the orchard action pair. The original
    /// mined the coins; the proposal shape is identical for fabricated
    /// ones (pepper-sync's TransparentCoin carries no coinbase marker,
    /// so propose logic cannot distinguish them).
    #[tokio::test]
    async fn four_coin_shield_proposal_shape() {
        let coin_value = 1_000_000;
        let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .transparent_coin(coin_value)
            .transparent_coin(coin_value)
            .transparent_coin(coin_value)
            .transparent_coin(coin_value)
            .build();
        let mut client = LightClient::new_for_test(wallet).await;

        let proposal = client.propose_shield(zip32::AccountId::ZERO).await.unwrap();
        assert_eq!(proposal.steps().len(), 1);
        let step = proposal.steps().first();
        assert_eq!(step.transparent_inputs().len(), 4);
        assert_eq!(u64::from(step.balance().fee_required()), 30_000);
        assert_eq!(step.balance().proposed_change().len(), 1);
        assert_eq!(
            u64::from(step.balance().proposed_change()[0].value()),
            4 * coin_value - 30_000
        );
    }

    /// Migrated from the chain_generics `ignore_dust_inputs` fixture's
    /// load-bearing half: note selection excludes dust inputs. From a
    /// wallet holding four 1_000-zat dust notes and one 15_000-zat note
    /// in each shielded pool, a 10_000-zat send selects exactly the two
    /// viable notes and none of the dust.
    #[tokio::test]
    async fn dust_inputs_are_ignored() {
        let builder = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED);
        let wallet = [1_000, 1_000, 1_000, 1_000, 15_000]
            .into_iter()
            .fold(builder, |builder, value| {
                builder.orchard_note(value).sapling_note(value)
            })
            .build();
        let mut client = LightClient::new_for_test(wallet).await;

        let destination = external_address(PoolType::Shielded(ShieldedProtocol::Orchard));
        let proposal =
            from_inputs::propose(&mut client, vec![(destination.as_str(), 10_000, None)])
                .await
                .unwrap();

        assert_eq!(proposal.steps().len(), 1);
        let step = proposal.steps().first();
        let selected_values: Vec<u64> = step
            .shielded_inputs()
            .expect("a shielded-funds send selects shielded inputs")
            .notes()
            .iter()
            .map(|note| u64::from(note.note().value()))
            .collect();
        assert!(
            selected_values.iter().all(|&value| value == 15_000),
            "dust notes were selected: {selected_values:?}"
        );
        assert_eq!(
            u64::from(step.balance().fee_required()),
            4 * u64::from(MARGINAL_FEE)
        );
    }

    /// Migrated from the chain_generics `note_selection_order` fixture's
    /// load-bearing half: from notes of 10/20/30/40 thousand zats, a
    /// 40_000-zat send selects the two-note covering set that leaves the
    /// least change — not more notes, and not a higher-value covering set.
    #[tokio::test]
    async fn note_selection_covers_target_with_minimal_change() {
        let builder = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED);
        let wallet = [10_000, 20_000, 30_000, 40_000]
            .into_iter()
            .fold(builder, |builder, value| builder.sapling_note(value))
            .build();
        let mut client = LightClient::new_for_test(wallet).await;

        let destination = external_address(PoolType::Shielded(ShieldedProtocol::Orchard));
        let proposal =
            from_inputs::propose(&mut client, vec![(destination.as_str(), 40_000, None)])
                .await
                .unwrap();

        assert_eq!(proposal.steps().len(), 1);
        let step = proposal.steps().first();
        let selected_values: Vec<u64> = step
            .shielded_inputs()
            .expect("a shielded-funds send selects shielded inputs")
            .notes()
            .iter()
            .map(|note| u64::from(note.note().value()))
            .collect();
        let selected_total: u64 = selected_values.iter().sum();
        let fee = u64::from(step.balance().fee_required());
        let change: u64 = step
            .balance()
            .proposed_change()
            .iter()
            .map(|change| u64::from(change.value()))
            .sum();
        assert_eq!(selected_values.len(), 2, "selected: {selected_values:?}");
        assert_eq!(selected_total, 40_000 + fee + change);
        // The fixture's guarantee: with 10_000-granular notes available,
        // any selection leaving 10_000 or more change used a bigger note
        // than necessary.
        assert!(change < 10_000, "change {change} implies oversized inputs");
    }
}
