//! `LightClient` function `do_propose` generates a proposal to send to specified addresses.

use zcash_address::ZcashAddress;
use zcash_protocol::value::Zatoshis;
use zip321::TransactionRequest;

use crate::ZENNIES_FOR_ZINGO_AMOUNT;
use crate::data::receivers::Receiver;
use crate::data::receivers::transaction_request_from_receivers;
use crate::get_zennies_for_zingo_address;
use crate::lightclient::LightClient;
use crate::wallet::error::ProposeSendError;
use crate::wallet::error::ProposeShieldError;
use crate::wallet::spend::op_return::OpReturnData;
use crate::wallet::spend::plan::PlanError;
use crate::wallet::spend::proposal::Proposal;

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
    /// OP_RETURN Data, if given, rides the final transaction of the send.
    ///
    /// Pauses the sync engine before the first wallet read and holds that
    /// pause beside the stored proposal, so the state the proposal
    /// selected against cannot shift before the send builds it. A proposal
    /// that fails to come into existence releases the pause on the way
    /// out, restoring the engine to the mode it was found in.
    ///
    /// `route_via_ephemeral` routes a transparent recipient through the
    /// ZIP 320 ephemeral hop, which a swap deposit needs so the vault can
    /// see an origin to refund to.
    pub async fn propose_send(
        &mut self,
        request: TransactionRequest,
        account_id: zip32::AccountId,
        op_return_data: Option<OpReturnData>,
        route_via_ephemeral: bool,
    ) -> Result<Proposal, ProposeSendError> {
        let minted = self.hold_proposal_pause();
        let result = {
            let mut wallet = self.wallet().write().await;
            wallet
                .create_send_proposal(request, account_id, op_return_data, route_via_ephemeral)
                .inspect(|proposal| {
                    wallet.store_proposal(proposal.clone());
                })
        };
        if result.is_err() && minted {
            self.release_proposal_pause(true);
        }
        result
    }

    /// Creates and stores a proposal for sending all shielded funds from a specified account to a given `address`.
    /// OP_RETURN Data, if given, rides the final transaction of the send.
    ///
    /// The pause a proposal holds (see [`Self::propose_send`]) is
    /// established before the send-all sizing: the [`Self::max_send_value`]
    /// trial proposals are wallet reads the sizing must not race a running
    /// engine for, and the value they compute must still hold when the real
    /// proposal is created.
    pub async fn propose_send_all(
        &mut self,
        address: ZcashAddress,
        zennies_for_zingo: bool,
        memo: Option<zcash_protocol::memo::MemoBytes>,
        account_id: zip32::AccountId,
        op_return_data: Option<OpReturnData>,
    ) -> Result<Proposal, ProposeSendError> {
        let minted = self.hold_proposal_pause();
        let result = self
            .propose_send_all_paused(address, zennies_for_zingo, memo, account_id, op_return_data)
            .await;
        if result.is_err() && minted {
            self.release_proposal_pause(true);
        }
        result
    }

    /// The [`Self::propose_send_all`] body, from sizing through storing,
    /// which its caller runs under the stored proposal's pause.
    async fn propose_send_all_paused(
        &mut self,
        address: ZcashAddress,
        zennies_for_zingo: bool,
        memo: Option<zcash_protocol::memo::MemoBytes>,
        account_id: zip32::AccountId,
        op_return_data: Option<OpReturnData>,
    ) -> Result<Proposal, ProposeSendError> {
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
        let mut wallet = self.wallet().write().await;
        let proposal = wallet.create_send_proposal(request, account_id, op_return_data, false)?;
        wallet.store_proposal(proposal.clone());

        Ok(proposal)
    }

    /// Creates and stores a proposal for shielding all transparent funds,
    /// under the same stored-proposal pause as [`Self::propose_send`].
    /// The shield path previously read spendable coins without pausing
    /// the engine at all.
    pub async fn propose_shield(
        &mut self,
        account_id: zip32::AccountId,
    ) -> Result<Proposal, ProposeShieldError> {
        let minted = self.hold_proposal_pause();
        let result = {
            let mut wallet = self.wallet().write().await;
            wallet
                .create_shield_proposal(account_id)
                .inspect(|proposal| {
                    wallet.store_proposal(proposal.clone());
                })
        };
        if result.is_err() && minted {
            self.release_proposal_pause(true);
        }
        result
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
        let wallet = self.wallet().write().await;
        let confirmed_balance = wallet.shielded_spendable_balance(account_id, false)?;
        let mut spendable_balance = confirmed_balance;

        loop {
            let mut receivers = vec![Receiver::new(address.clone(), spendable_balance, None)];
            if zennies_for_zingo {
                self.append_zingo_zenny_receiver(&mut receivers);
            }
            let request = transaction_request_from_receivers(receivers)?;
            let trial_proposal = wallet.create_send_proposal(request, account_id, None, false);

            match trial_proposal {
                Err(ProposeSendError::Plan(PlanError::InsufficientFunds {
                    available,
                    required,
                })) => {
                    if let Some(shortfall) = required - confirmed_balance {
                        match spendable_balance - shortfall {
                            Some(updated_spendable) => {
                                spendable_balance = updated_spendable;
                            }
                            None => {
                                return Err(ProposeSendError::Plan(PlanError::InsufficientFunds {
                                    available: confirmed_balance,
                                    required,
                                }));
                            }
                        }
                    } else {
                        // bugged underflow case, required should always be larger than confirmed shielded balance to cause
                        // insufficient funds error.
                        // returns insufficient funds error with same values from original error for debugging
                        return Err(ProposeSendError::Plan(PlanError::InsufficientFunds {
                            available,
                            required,
                        }));
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
            .build()
            .unwrap();
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
            Err(ProposeShieldError::Plan(crate::wallet::spend::plan::PlanError::SyncRequired)) => {
                true
            }
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
            PoolType::ORCHARD | PoolType::IRONWOOD => ReceiverSelection::orchard_only(),
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
                None,
            )
            .await;

        match proposal_error {
            Err(ProposeSendError::Plan(
                crate::wallet::spend::plan::PlanError::InsufficientFunds {
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
    /// zero-value send. The 20_000-zat note exactly covers the V6 fee
    /// (orchard input bundle plus ironwood payment bundle, two padded
    /// actions each), leaving zero to send.
    #[tokio::test]
    async fn ptfm_zero_value() {
        let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .orchard_note(20_000)
            .build();
        let mut client = LightClient::new_for_test(wallet).await;

        let proposal_error = client
            .propose_send_all(
                external_address(PoolType::ORCHARD),
                false,
                None,
                zip32::AccountId::ZERO,
                None,
            )
            .await;

        assert!(matches!(
            proposal_error,
            Err(ProposeSendError::ZeroValueSendAll)
        ));
    }

    /// Migrated from libtonode `send_all::ptfm_general`: a send-all from a
    /// wallet fragmented across both shielded pools drains every non-dust
    /// note and leaves the dust behind. The original assembled the
    /// fragmentation with five LocalNet sends and asserted zero
    /// balances-excluding-dust after mining, which is this same contract,
    /// one mined round trip later: send-all pays out the dust-excluded
    /// spendable balance minus the fee, so notes at or below the 5_000-zat
    /// `MARGINAL_FEE` dust line (which net nothing after paying for their
    /// own input) are never selected.
    #[tokio::test]
    async fn ptfm_general() {
        let viable_values = [100_000u64, 50_000];
        let orchard_values = [100_000, 5_000, 4_000];
        let sapling_values = [50_000, 4_000];

        let mut builder =
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED);
        for value in orchard_values {
            builder = builder.orchard_note(value);
        }
        for value in sapling_values {
            builder = builder.sapling_note(value);
        }
        let mut client = LightClient::new_for_test(builder.build()).await;

        let proposal = client
            .propose_send_all(
                external_address(PoolType::SAPLING),
                false,
                None,
                zip32::AccountId::ZERO,
                None,
            )
            .await
            .unwrap();

        assert_eq!(proposal.steps().len(), 1);
        let step = proposal.final_step();
        let mut selected: Vec<u64> = step
            .shielded_inputs()
            .iter()
            .map(|input| u64::from(input.value()))
            .collect();
        selected.sort_unstable();
        assert_eq!(
            selected,
            [50_000, 100_000],
            "send-all selects exactly the non-dust notes of both pools"
        );
        let fee = u64::from(step.fee());
        let payment: u64 = step
            .transaction_request()
            .payments()
            .values()
            .map(|payment| u64::from(payment.amount().expect("send-all payments carry amounts")))
            .sum();
        assert_eq!(payment + fee, viable_values.iter().sum::<u64>());
        let change: u64 = step
            .change()
            .iter()
            .map(|change| u64::from(change.value()))
            .sum();
        assert_eq!(change, 0, "send-all leaves no non-dust value behind");
    }

    /// Migrated from libtonode `fast::send_not_fully_synced` (renamed: no
    /// sync exists offline). The original proposed a send-all to the
    /// wallet's own sapling address while the validator ran five blocks
    /// ahead, asserting only that propose-and-send succeeded. Proposing
    /// never consults the server, so the offline half is exactly this
    /// self-destination send-all. The behind-tip send half remains covered
    /// by `load_wallet::verify_old_wallet_uses_server_height_in_send`.
    #[tokio::test]
    async fn send_all_to_own_sapling_proposes() {
        let note_value = 100_000;
        let mut wallet =
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
                .orchard_note(note_value)
                .build();
        let (_, own_sapling) = wallet
            .generate_unified_address(ReceiverSelection::sapling_only(), zip32::AccountId::ZERO)
            .unwrap();
        let destination = address_from_str(&own_sapling.encode(&wallet.chain_type())).unwrap();
        let mut client = LightClient::new_for_test(wallet).await;

        let proposal = client
            .propose_send_all(destination, false, None, zip32::AccountId::ZERO, None)
            .await
            .unwrap();

        assert_eq!(proposal.steps().len(), 1);
        let step = proposal.final_step();
        let fee = u64::from(step.fee());
        let payment: u64 = step
            .transaction_request()
            .payments()
            .values()
            .map(|payment| u64::from(payment.amount().expect("send-all payments carry amounts")))
            .sum();
        assert_eq!(payment + fee, note_value);
    }

    /// Gap-2 remediation from the protection audit (§ Gap remediation
    /// plan): with Zennies for Zingo enabled, the proposal itself carries
    /// the injected zenny payment. toggle_zennies_for_zingo pins the
    /// max-send arithmetic. This pins the injection mechanism: the
    /// request `propose_send_all` builds contains exactly one payment to
    /// the Zennies for Zingo address at `ZENNIES_FOR_ZINGO_AMOUNT`,
    /// alongside the send-all payment, and the step still balances.
    #[tokio::test]
    async fn send_all_with_zfz_injects_the_zennies_payment() {
        let initial_funds = 2_000_000;

        let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .orchard_note(initial_funds)
            .build();
        let zfz_address = crate::get_zennies_for_zingo_address(wallet.chain_type());
        let mut client = LightClient::new_for_test(wallet).await;

        let proposal = client
            .propose_send_all(
                external_address(PoolType::ORCHARD),
                true,
                None,
                zip32::AccountId::ZERO,
                None,
            )
            .await
            .unwrap();

        assert_eq!(proposal.steps().len(), 1);
        let step = proposal.final_step();
        let zennies_payments: Vec<u64> = step
            .transaction_request()
            .payments()
            .values()
            .filter(|payment| payment.recipient_address().encode() == zfz_address)
            .map(|payment| u64::from(payment.amount().expect("send-all payments carry amounts")))
            .collect();
        assert_eq!(
            zennies_payments,
            [crate::ZENNIES_FOR_ZINGO_AMOUNT],
            "exactly one zenny payment, at the fixed donation amount"
        );
        assert_eq!(step.transaction_request().payments().len(), 2);
        let fee = u64::from(step.fee());
        let payment: u64 = step
            .transaction_request()
            .payments()
            .values()
            .map(|payment| u64::from(payment.amount().expect("send-all payments carry amounts")))
            .sum();
        assert_eq!(payment + fee, initial_funds);
    }

    /// Migrated from libtonode `send_all::toggle_zennies_for_zingo`: with
    /// Zennies for Zingo enabled, the maximum sendable value deducts the
    /// zenny amount and the fee for one ironwood note in, three outputs
    /// out: the ironwood input bundle pads to two actions, and the payment,
    /// zenny, and change outputs land in the ironwood bundle as three.
    #[tokio::test]
    async fn toggle_zennies_for_zingo() {
        let initial_funds = 2_000_000;
        let zennies_magnitude = 1_000_000;
        let expected_fee = 15_000;

        let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .ironwood_note(initial_funds)
            .build();
        let client = LightClient::new_for_test(wallet).await;

        assert_eq!(
            client
                .max_send_value(
                    external_address(PoolType::IRONWOOD),
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
/// (`simpool_insufficient_{1,10_000}_{orchard,sapling}_to_*` and
/// `simpool_no_fund_1_000_000_to_*`): the insufficient-funds and unfunded
/// propose errors are pure proposal logic over wallet state, so a synthetic
/// wallet replaces the LocalNet environment and its multi-hop funding
/// chain.
#[cfg(test)]
mod simpool {
    use zcash_protocol::{PoolType, ShieldedPool};

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
            PoolType::Shielded(ShieldedPool::Orchard)
            | PoolType::Shielded(ShieldedPool::Ironwood) => ReceiverSelection::orchard_only(),
            PoolType::Shielded(ShieldedPool::Sapling) => ReceiverSelection::sapling_only(),
            PoolType::Transparent => return external_wallet.get_address(PoolType::Transparent),
        };
        let (_, unified_address) = external_wallet
            .generate_unified_address(selection, zip32::AccountId::ZERO)
            .unwrap();
        unified_address.encode(&external_wallet.chain_type())
    }

    /// A wallet holding one `source`-pool note `underflow_amount` short of
    /// a 100_000 send to `pool` reports the exact shortfall.
    async fn insufficient(source: ShieldedPool, underflow_amount: u64, pool: PoolType) {
        let expected_fee = fee_tables::one_to_one(Some(source), pool, true);
        let secondary_fund = 100_000 + expected_fee - underflow_amount;
        let builder = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED);
        let wallet = match source {
            ShieldedPool::Ironwood => builder.ironwood_note(secondary_fund),
            ShieldedPool::Orchard => builder.orchard_note(secondary_fund),
            ShieldedPool::Sapling => builder.sapling_note(secondary_fund),
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
                "insufficient funds: {} available of {} required",
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
                "insufficient funds: {} available of {} required",
                0,
                try_amount + expected_fee
            )
        );
    }

    #[tokio::test]
    async fn insufficient_1_ironwood_to_ironwood() {
        insufficient(ShieldedPool::Ironwood, 1, PoolType::IRONWOOD).await;
    }
    #[tokio::test]
    async fn insufficient_1_ironwood_to_sapling() {
        insufficient(ShieldedPool::Ironwood, 1, PoolType::SAPLING).await;
    }
    #[tokio::test]
    async fn insufficient_1_ironwood_to_transparent() {
        insufficient(ShieldedPool::Ironwood, 1, PoolType::Transparent).await;
    }
    #[tokio::test]
    async fn insufficient_10_000_ironwood_to_ironwood() {
        insufficient(ShieldedPool::Ironwood, 10_000, PoolType::IRONWOOD).await;
    }
    #[tokio::test]
    async fn insufficient_10_000_ironwood_to_sapling() {
        insufficient(ShieldedPool::Ironwood, 10_000, PoolType::SAPLING).await;
    }
    #[tokio::test]
    async fn insufficient_10_000_ironwood_to_transparent() {
        insufficient(ShieldedPool::Ironwood, 10_000, PoolType::Transparent).await;
    }
    #[tokio::test]
    async fn no_fund_1_000_000_to_ironwood() {
        unfunded_to(1_000_000, PoolType::IRONWOOD).await;
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
    async fn insufficient_1_sapling_to_ironwood() {
        insufficient(ShieldedPool::Sapling, 1, PoolType::IRONWOOD).await;
    }
    #[tokio::test]
    async fn insufficient_1_sapling_to_sapling() {
        insufficient(ShieldedPool::Sapling, 1, PoolType::SAPLING).await;
    }
    #[tokio::test]
    async fn insufficient_1_sapling_to_transparent() {
        insufficient(ShieldedPool::Sapling, 1, PoolType::Transparent).await;
    }
    #[tokio::test]
    async fn insufficient_10_000_sapling_to_ironwood() {
        insufficient(ShieldedPool::Sapling, 10_000, PoolType::IRONWOOD).await;
    }
    #[tokio::test]
    async fn insufficient_10_000_sapling_to_sapling() {
        insufficient(ShieldedPool::Sapling, 10_000, PoolType::SAPLING).await;
    }
    #[tokio::test]
    async fn insufficient_10_000_sapling_to_transparent() {
        insufficient(ShieldedPool::Sapling, 10_000, PoolType::Transparent).await;
    }
}

/// Migrated from the libtonode chain_generics pool matrix: every shielded
/// source paired with every receiver pool, with and without change, plus
/// the transparent minimum-value and boundary-value rows. The matrix's
/// fee, value, and change assertions are pure proposer arithmetic, so a
/// synthetic wallet funded with exactly `value + change + fee` replaces
/// the LocalNet environment and its two-hop funding chain. Each case
/// asserts the proposal pays `value`, charges the `fee_tables::one_to_one`
/// fee, and returns exactly `change`. The Transmitted-to-Confirmed round
/// trip the matrix also exercised remains covered by the two surviving
/// chain_generics fixtures, which drive the same follow_proposal machinery
/// (with test_mempool off, so the Mempool-status leg is not asserted
/// there).
#[cfg(test)]
mod pool_matrix {
    use zcash_protocol::{PoolType, ShieldedPool};

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
            PoolType::Shielded(ShieldedPool::Orchard)
            | PoolType::Shielded(ShieldedPool::Ironwood) => ReceiverSelection::orchard_only(),
            PoolType::Shielded(ShieldedPool::Sapling) => ReceiverSelection::sapling_only(),
            PoolType::Transparent => return external_wallet.get_address(PoolType::Transparent),
        };
        let (_, unified_address) = external_wallet
            .generate_unified_address(selection, zip32::AccountId::ZERO)
            .unwrap();
        unified_address.encode(&external_wallet.chain_type())
    }

    /// One matrix cell: a wallet holding a single `source` note of exactly
    /// `receiver_value + change + fee` proposes a `receiver_value` send to
    /// a `pool` destination, and the proposal's fee and change land on the
    /// fee table's prediction to the zatoshi.
    async fn matrix_case(source: ShieldedPool, pool: PoolType, receiver_value: u64, change: u64) {
        let expected_fee = fee_tables::one_to_one(
            Some(source),
            pool,
            !(source == ShieldedPool::Orchard && change == 0),
        );
        let funding = receiver_value + change + expected_fee;
        let builder = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED);
        let wallet = match source {
            ShieldedPool::Ironwood => builder.ironwood_note(funding),
            ShieldedPool::Orchard => builder.orchard_note(funding),
            ShieldedPool::Sapling => builder.sapling_note(funding),
        }
        .build();
        let mut client = LightClient::new_for_test(wallet).await;

        let destination = external_address(pool);
        let proposal = from_inputs::propose(
            &mut client,
            vec![(destination.as_str(), receiver_value, None)],
        )
        .await
        .unwrap();

        assert_eq!(proposal.steps().len(), 1);
        let step = proposal.final_step();
        assert_eq!(u64::from(step.fee()), expected_fee);
        let proposed_change: u64 = step
            .change()
            .iter()
            .map(|change| u64::from(change.value()))
            .sum();
        assert_eq!(proposed_change, change);
        let payment: u64 = step
            .transaction_request()
            .payments()
            .values()
            .map(|payment| u64::from(payment.amount().expect("matrix payments carry amounts")))
            .sum();
        assert_eq!(payment, receiver_value);
    }

    macro_rules! pool_matrix_case {
        ($name:ident, $source:expr, $receiver:expr, $send_value:expr, $change:expr) => {
            #[tokio::test]
            async fn $name() {
                matrix_case($source, $receiver, $send_value, $change).await;
            }
        };
    }

    pool_matrix_case!(
        sapling_sends_to_transparent,
        ShieldedPool::Sapling,
        PoolType::TRANSPARENT,
        10_000,
        1_000
    );
    pool_matrix_case!(
        sapling_sends_to_sapling,
        ShieldedPool::Sapling,
        PoolType::SAPLING,
        10_000,
        1_000
    );
    pool_matrix_case!(
        sapling_sends_to_ironwood,
        ShieldedPool::Sapling,
        PoolType::IRONWOOD,
        10_000,
        1_000
    );
    pool_matrix_case!(
        ironwood_sends_to_transparent,
        ShieldedPool::Ironwood,
        PoolType::TRANSPARENT,
        10_000,
        1_000
    );
    pool_matrix_case!(
        ironwood_sends_to_sapling,
        ShieldedPool::Ironwood,
        PoolType::SAPLING,
        10_000,
        1_000
    );
    pool_matrix_case!(
        ironwood_sends_to_ironwood,
        ShieldedPool::Ironwood,
        PoolType::IRONWOOD,
        10_000,
        1_000
    );
    pool_matrix_case!(
        sapling_sends_to_transparent_no_change,
        ShieldedPool::Sapling,
        PoolType::TRANSPARENT,
        10_000,
        0
    );
    pool_matrix_case!(
        sapling_sends_to_sapling_no_change,
        ShieldedPool::Sapling,
        PoolType::SAPLING,
        10_000,
        0
    );
    pool_matrix_case!(
        sapling_sends_to_ironwood_no_change,
        ShieldedPool::Sapling,
        PoolType::IRONWOOD,
        10_000,
        0
    );
    pool_matrix_case!(
        orchard_sends_to_transparent_no_change,
        ShieldedPool::Orchard,
        PoolType::TRANSPARENT,
        10_000,
        0
    );
    pool_matrix_case!(
        orchard_sends_to_sapling_no_change,
        ShieldedPool::Orchard,
        PoolType::SAPLING,
        10_000,
        0
    );
    pool_matrix_case!(
        orchard_sends_to_orchard_no_change,
        ShieldedPool::Orchard,
        PoolType::ORCHARD,
        10_000,
        0
    );
    pool_matrix_case!(
        orchard_sends_to_ironwood_no_change,
        ShieldedPool::Orchard,
        PoolType::IRONWOOD,
        10_000,
        0
    );
    pool_matrix_case!(
        ironwood_sends_to_transparent_no_change,
        ShieldedPool::Ironwood,
        PoolType::TRANSPARENT,
        10_000,
        0
    );
    pool_matrix_case!(
        ironwood_sends_to_sapling_no_change,
        ShieldedPool::Ironwood,
        PoolType::SAPLING,
        10_000,
        0
    );
    // 546 zatoshis is the canonical transparent dust threshold. The
    // proposer permits it; whether a relay accepts it is the network's
    // business, and zebra's mempool dust rule was the reason this row's
    // LocalNet ancestor sent 546 rather than 1.
    pool_matrix_case!(
        sapling_sends_to_transparent_minimum_value,
        ShieldedPool::Sapling,
        PoolType::TRANSPARENT,
        546,
        0
    );
    pool_matrix_case!(
        sapling_sends_to_transparent_boundary_values,
        ShieldedPool::Sapling,
        PoolType::TRANSPARENT,
        49_999,
        9_999
    );
}

/// Offline proposal-shape tests over synthetic wallets: shield proposals
/// and note-selection behavior, migrated from LocalNet tests whose
/// assertions were all propose-stage.
#[cfg(test)]
mod proposal_shape {
    use zcash_primitives::transaction::fees::zip317::MARGINAL_FEE;
    use zcash_protocol::{PoolType, ShieldedPool};

    use crate::lightclient::LightClient;
    use crate::testutils::fee_tables;
    use crate::testutils::lightclient::from_inputs;
    use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;
    use crate::wallet::keys::unified::ReceiverSelection;

    fn external_address(pool: PoolType) -> String {
        let mut external_wallet =
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::ABANDON_ART_SEED).build();
        let selection = match pool {
            PoolType::ORCHARD | PoolType::IRONWOOD => ReceiverSelection::orchard_only(),
            PoolType::SAPLING => ReceiverSelection::sapling_only(),
            _ => unimplemented!("only shielded destinations are needed here"),
        };
        let (_, unified_address) = external_wallet
            .generate_unified_address(selection, zip32::AccountId::ZERO)
            .unwrap();
        unified_address.encode(&external_wallet.chain_type())
    }

    /// A pool whose spendable list is exhausted mid-selection must still
    /// report the value it covered. The regression this pins: selecting the
    /// pool's last candidate note broke out of the selection loop before
    /// the remaining-needed figure was recomputed, so the caller saw a
    /// stale positive remainder and the next pool selected against a
    /// target that was already met. The wallet holds the covering note in
    /// Orchard and a larger note in Sapling (the two trailing pools of
    /// the selector's preference order for an Ironwood payment), so the
    /// assertion is independent of that order's head.
    #[tokio::test]
    async fn exhausted_pool_does_not_over_select() {
        let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .orchard_note(50_000)
            .sapling_note(100_000)
            .build();
        let mut client = LightClient::new_for_test(wallet).await;

        let destination = external_address(PoolType::Shielded(ShieldedPool::Ironwood));
        let proposal =
            from_inputs::propose(&mut client, vec![(destination.as_str(), 20_000, None)])
                .await
                .unwrap();
        let selected: Vec<u64> = proposal
            .final_step()
            .shielded_inputs()
            .iter()
            .map(|input| u64::from(input.value()))
            .collect();
        assert_eq!(
            selected,
            [50_000],
            "one note covers the send; an emptied pool must not spill selection into the next"
        );
    }

    /// A note bound to a pending migration part is withheld from ordinary
    /// input selection while a free note can satisfy the request (the
    /// ZIP 318 soft reservation). This is the unit-level twin of the
    /// libtonode `bound_note_reservation_and_external_spend_invalidation`
    /// scenario, and it fails alongside the exhausted-pool regression
    /// above: a stale remainder made the never-block fallback consume the
    /// bound note although the free note had already covered the target.
    #[tokio::test]
    async fn reservation_withholds_bound_note_while_a_free_note_suffices() {
        use pepper_sync::wallet::{NoteInterface as _, OrchardNote, OutputInterface as _};

        use crate::wallet::migration::{
            BoundNote, ConsentBinding, MigrationParams, MigrationPhase, MigrationState, PartId,
            PartRecord, SigningStrategy,
        };

        let mut wallet =
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
                .orchard_note(100_000)
                .orchard_note(50_000)
                .build();

        let (output_id, nullifier) = wallet
            .wallet_transactions
            .values()
            .flat_map(OrchardNote::transaction_outputs)
            .find(|note| note.value() == 100_000)
            .map(|note| {
                (
                    note.output_id(),
                    note.nullifier()
                        .expect("scanned notes carry nullifiers")
                        .to_bytes(),
                )
            })
            .expect("the wallet holds the 100_000 note");

        let params = MigrationParams::provisional(wallet.chain_type());
        wallet.migration = Some(MigrationState {
            consent: ConsentBinding {
                params_hash: params.params_hash(),
                plan_hash: [0; 32],
                consented_at: 0,
            },
            params,
            strategy: SigningStrategy::LazyAtBoundary,
            mode: crate::wallet::migration::MigrationMode::Scheduled,
            account: zip32::AccountId::ZERO,
            phase: MigrationPhase::PartsScheduled,
            parts: vec![PartRecord::new(
                PartId(0),
                100_000,
                BoundNote {
                    output_id,
                    nullifier,
                    commitment: [0; 32],
                },
            )],
        });
        let mut client = LightClient::new_for_test(wallet).await;

        let destination = external_address(PoolType::Shielded(ShieldedPool::Ironwood));
        let proposal =
            from_inputs::propose(&mut client, vec![(destination.as_str(), 20_000, None)])
                .await
                .unwrap();
        let selected: Vec<u64> = proposal
            .final_step()
            .shielded_inputs()
            .iter()
            .map(|input| u64::from(input.value()))
            .collect();
        assert_eq!(
            selected,
            [50_000],
            "the free note covers the send; the bound note stays reserved"
        );
    }

    /// Migrated from libtonode `slow::dust_sends_change_correctly`: a send
    /// of less than the fee still proposes, since the fee comes out of the
    /// selected note and the remainder returns as change. The original
    /// asserted nothing beyond the send call succeeding. The offline
    /// proposal now asserts the shape the name always promised.
    #[tokio::test]
    async fn dust_sends_change_correctly() {
        let note_value = 100_000;
        let sent_value = 1_000;
        let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .ironwood_note(note_value)
            .build();
        let mut client = LightClient::new_for_test(wallet).await;

        let destination = external_address(PoolType::IRONWOOD);
        let proposal =
            from_inputs::propose(&mut client, vec![(destination.as_str(), sent_value, None)])
                .await
                .unwrap();

        assert_eq!(proposal.steps().len(), 1);
        let step = proposal.final_step();
        let fee = u64::from(step.fee());
        assert_eq!(
            fee,
            fee_tables::one_to_one(Some(ShieldedPool::Ironwood), PoolType::IRONWOOD, true)
        );
        let change: u64 = step
            .change()
            .iter()
            .map(|change| u64::from(change.value()))
            .sum();
        assert_eq!(change, note_value - sent_value - fee);
    }

    /// Offline twin of libtonode
    /// `basic_transactions::send_and_sync_with_multiple_notes_no_panic`,
    /// which stays live as the pipeline control: a 50_000 payment from
    /// two 40_000 notes must gather both (either alone is consumed by
    /// the payment plus the 10_000 one-to-one fee) and return exactly
    /// 20_000 change.
    #[tokio::test]
    async fn payment_no_single_note_covers_gathers_both_and_changes() {
        let note_value = 40_000;
        let sent_value = 50_000;
        let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .ironwood_note(note_value)
            .ironwood_note(note_value)
            .build();
        let mut client = LightClient::new_for_test(wallet).await;

        let destination = external_address(PoolType::IRONWOOD);
        let proposal =
            from_inputs::propose(&mut client, vec![(destination.as_str(), sent_value, None)])
                .await
                .unwrap();

        assert_eq!(proposal.steps().len(), 1);
        let step = proposal.final_step();
        let selected: Vec<u64> = step
            .shielded_inputs()
            .iter()
            .map(|input| u64::from(input.value()))
            .collect();
        assert_eq!(selected, [note_value, note_value], "both notes gathered");
        let fee = u64::from(step.fee());
        assert_eq!(
            fee,
            fee_tables::one_to_one(Some(ShieldedPool::Ironwood), PoolType::IRONWOOD, true)
        );
        let change: u64 = step
            .change()
            .iter()
            .map(|change| u64::from(change.value()))
            .sum();
        assert_eq!(change, 2 * note_value - sent_value - fee);
        assert_eq!(change, 20_000);
    }

    /// Offline twin of libtonode `slow::sapling_dust_fee_collection`,
    /// which stays live as the pipeline control: from 100_000 orchard
    /// plus a 1_000-zat sapling dust note, a 50_000 send selects only
    /// the orchard note (dust cannot pay for its own input), charges the
    /// plain one-to-one fee, and leaves 40_000 change, the live test's
    /// closing balance, derived here at proposal time. The dust note
    /// stays behind untouched.
    #[tokio::test]
    async fn sapling_dust_is_not_collected_toward_fees() {
        let ironwood_value = 100_000;
        let sapling_dust = 1_000;
        let sent_value = 50_000;
        let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .ironwood_note(ironwood_value)
            .sapling_note(sapling_dust)
            .build();
        let mut client = LightClient::new_for_test(wallet).await;

        let destination = external_address(PoolType::IRONWOOD);
        let proposal =
            from_inputs::propose(&mut client, vec![(destination.as_str(), sent_value, None)])
                .await
                .unwrap();

        assert_eq!(proposal.steps().len(), 1);
        let step = proposal.final_step();
        let selected: Vec<u64> = step
            .shielded_inputs()
            .iter()
            .map(|input| u64::from(input.value()))
            .collect();
        assert_eq!(selected, [ironwood_value], "the dust note is not selected");
        let fee = u64::from(step.fee());
        assert_eq!(
            fee,
            fee_tables::one_to_one(Some(ShieldedPool::Ironwood), PoolType::IRONWOOD, true)
        );
        let change: u64 = step
            .change()
            .iter()
            .map(|change| u64::from(change.value()))
            .sum();
        assert_eq!(change, ironwood_value - sent_value - fee);
        assert_eq!(change, 40_000);
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
        let step = proposal.final_step();
        assert_eq!(step.transparent_inputs().len(), 1);
        let fee = u64::from(step.fee());
        let change: u64 = step
            .change()
            .iter()
            .map(|change| u64::from(change.value()))
            .sum();
        assert_eq!(change + fee, value);
    }

    /// Migrated from libtonode `fast::mine_to_transparent_and_propose_shielding`:
    /// a four-coin shield proposes as a single step spending all four
    /// coins into one change output, with the zip317 fee for four
    /// transparent inputs plus the orchard action pair. The original
    /// mined the coins and waited out coinbase maturity. Propose logic
    /// detects coinbase through the stored transaction's transparent
    /// bundle (`spendable_transparent_coins` demands 100 extra
    /// confirmations for it), and fabricated records carry no transparent
    /// bundle, so these coins present as ordinary mature ones and the
    /// proposal shape is identical.
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
        let step = proposal.final_step();
        assert_eq!(step.transparent_inputs().len(), 4);
        assert_eq!(u64::from(step.fee()), 30_000);
        assert_eq!(step.change().len(), 1);
        assert_eq!(u64::from(step.change()[0].value()), 4 * coin_value - 30_000);
    }

    /// Migrated from libtonode `slow::zero_value_change`: a send that
    /// leaves exactly zero after the fee still proposes an orchard change
    /// output of value zero. The original mined the transaction and then
    /// counted one unspent zero-value note and one confirmed-spent funding
    /// note. The note-state bookkeeping it exercised is covered by the
    /// pepper-sync spend-status rig, and the load-bearing claim (the
    /// builder emits the zero-value change) is decided at proposal time.
    #[tokio::test]
    async fn zero_value_change() {
        let note_value = 100_000;
        let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .ironwood_note(note_value)
            .build();
        let mut client = LightClient::new_for_test(wallet).await;

        let fee = fee_tables::one_to_one(Some(ShieldedPool::Ironwood), PoolType::IRONWOOD, true);
        let destination = external_address(PoolType::IRONWOOD);
        let proposal = from_inputs::propose(
            &mut client,
            vec![(destination.as_str(), note_value - fee, None)],
        )
        .await
        .unwrap();

        assert_eq!(proposal.steps().len(), 1);
        let step = proposal.final_step();
        assert_eq!(u64::from(step.fee()), fee);
        let change = step.change();
        assert_eq!(change.len(), 1);
        assert_eq!(u64::from(change[0].value()), 0);
        assert_eq!(change[0].pool(), PoolType::IRONWOOD);
    }

    /// Migrated from libtonode `slow::zero_value_change_to_orchard_created`:
    /// same zero-value-change arithmetic on a cross-pool send. An 80_000
    /// payment to an external sapling address out of a 100_000 orchard note
    /// costs exactly the 20_000 cross-pool fee, so the ironwood change
    /// output is proposed at value zero.
    #[tokio::test]
    async fn zero_value_change_to_ironwood_created() {
        let note_value = 100_000;
        let sent_value = 80_000;
        let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .ironwood_note(note_value)
            .build();
        let mut client = LightClient::new_for_test(wallet).await;

        let destination = external_address(PoolType::SAPLING);
        let proposal =
            from_inputs::propose(&mut client, vec![(destination.as_str(), sent_value, None)])
                .await
                .unwrap();

        assert_eq!(proposal.steps().len(), 1);
        let step = proposal.final_step();
        let fee = fee_tables::one_to_one(Some(ShieldedPool::Ironwood), PoolType::SAPLING, true);
        assert_eq!(u64::from(step.fee()), fee);
        assert_eq!(note_value - sent_value - fee, 0);
        let change = step.change();
        assert_eq!(change.len(), 1);
        assert_eq!(u64::from(change[0].value()), 0);
        assert_eq!(change[0].pool(), PoolType::IRONWOOD);
    }

    /// Spending a legacy Orchard note on a post-NU6.3 chain must leave the
    /// change in the **Orchard** pool, not route it to Ironwood. ZIP 318
    /// disables ordinary *payments* into the Orchard pool once NU6.3 activates
    /// "while still permitting change"; the turnstile (ZIP 2006) blocks value
    /// *entering* Orchard, and change funded by an Orchard input never leaves
    /// it, so an Orchard change output nets the pool down and stays
    /// consensus-valid. Keeping change in the source pool is the privacy policy
    /// for ordinary sends (a discussed decision): it reveals only the payment
    /// value crossing pools rather than migrating the whole note, and avoids
    /// stamping every send with an Orchard->Ironwood migration fingerprint.
    /// Deliberate Orchard->Ironwood movement is the migration engine's job, not
    /// an ordinary send's.
    #[tokio::test]
    async fn orchard_send_change_stays_orchard_post_nu6_3() {
        let note_value = 200_000;
        let sent_value = 50_000;
        let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .orchard_note(note_value)
            .build();
        let mut client = LightClient::new_for_test(wallet).await;

        // Post-NU6.3 the payment itself routes through Ironwood (ZIP 318), but
        // that must not drag the change out of Orchard.
        let destination = external_address(PoolType::IRONWOOD);
        let proposal =
            from_inputs::propose(&mut client, vec![(destination.as_str(), sent_value, None)])
                .await
                .unwrap();

        assert_eq!(proposal.steps().len(), 1);
        let step = proposal.final_step();
        let change = step.change();
        assert_eq!(change.len(), 1);
        assert!(
            u64::from(change[0].value()) > 0,
            "the send must leave real change for the pool assertion to bite"
        );
        assert_eq!(
            change[0].pool(),
            PoolType::ORCHARD,
            "change from spending a legacy Orchard note must stay in Orchard \
             post-NU6.3: ZIP 318 permits Orchard change, and the privacy policy \
             keeps it in the source pool instead of migrating on every send"
        );
    }

    /// Migrated from libtonode `fast::tex::send_to_tex`: a payment to a
    /// TEX address proposes as the ZIP-320 two-step: shield to an
    /// ephemeral transparent output, then the transparent leg to the TEX
    /// destination funded entirely by step one. The original's only
    /// proposal-level assertion was the step count. The offline version
    /// also pins the inter-step wiring. The transmission half (both steps
    /// mined, three wallet records) retires with the LocalNet original.
    #[tokio::test]
    async fn send_to_tex() {
        use pepper_sync::keys::decode_address;
        use zcash_keys::address::Address;
        use zcash_protocol::value::Zatoshis;
        use zcash_transparent::address::TransparentAddress;
        use zip321::{Payment, TransactionRequest};

        let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .ironwood_note(5_000_000)
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
            Zatoshis::from_u64(100_000).unwrap(),
        )])
        .unwrap();
        let proposal = client
            .propose_send(request, zip32::AccountId::ZERO, None, false)
            .await
            .unwrap();

        assert_eq!(proposal.steps().len(), 2);
        assert!(
            matches!(
                proposal,
                crate::wallet::spend::proposal::Proposal::TexTransfer(_)
            ),
            "the transparent leg spends step one's ephemeral output"
        );
    }

    /// Migrated from the chain_generics `ignore_dust_inputs` fixture's
    /// load-bearing half: note selection excludes dust inputs. From a
    /// wallet holding four 1_000-zat dust notes and one 50_000-zat note
    /// in each shielded pool, a 10_000-zat send selects exactly the one
    /// viable ironwood note and none of the dust.
    #[tokio::test]
    async fn dust_inputs_are_ignored() {
        let builder = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED);
        let wallet = [1_000, 1_000, 1_000, 1_000, 50_000]
            .into_iter()
            .fold(builder, |builder, value| {
                builder.ironwood_note(value).sapling_note(value)
            })
            .build();
        let mut client = LightClient::new_for_test(wallet).await;

        let destination = external_address(PoolType::Shielded(ShieldedPool::Ironwood));
        let proposal =
            from_inputs::propose(&mut client, vec![(destination.as_str(), 10_000, None)])
                .await
                .unwrap();

        assert_eq!(proposal.steps().len(), 1);
        let step = proposal.final_step();
        let selected_values: Vec<u64> = step
            .shielded_inputs()
            .iter()
            .map(|input| u64::from(input.value()))
            .collect();
        assert_eq!(
            selected_values,
            [50_000],
            "exactly the viable ironwood note, none of the dust"
        );
        assert_eq!(u64::from(step.fee()), 2 * u64::from(MARGINAL_FEE));
    }

    /// Migrated from the chain_generics `note_selection_order` fixture's
    /// load-bearing half: from notes of 10/20/30/40 thousand zats, a
    /// 40_000-zat send selects the two-note covering set that leaves the
    /// least change, not more notes and not a higher-value covering set.
    #[tokio::test]
    async fn note_selection_covers_target_with_minimal_change() {
        let builder = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED);
        let wallet = [10_000, 20_000, 30_000, 40_000]
            .into_iter()
            .fold(builder, |builder, value| builder.sapling_note(value))
            .build();
        let mut client = LightClient::new_for_test(wallet).await;

        let destination = external_address(PoolType::Shielded(ShieldedPool::Ironwood));
        let proposal =
            from_inputs::propose(&mut client, vec![(destination.as_str(), 40_000, None)])
                .await
                .unwrap();

        assert_eq!(proposal.steps().len(), 1);
        let step = proposal.final_step();
        let selected_values: Vec<u64> = step
            .shielded_inputs()
            .iter()
            .map(|input| u64::from(input.value()))
            .collect();
        let selected_total: u64 = selected_values.iter().sum();
        let fee = u64::from(step.fee());
        let change: u64 = step
            .change()
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

/// The propose/send family must never read wallet state a running engine
/// could be mutating, and a pause it takes must serve a pending proposal,
/// never outliving one. Each test here pins one way the imperative pause
/// discipline violates that contract. All four run red until the stored
/// pause discipline lands later on this branch.
#[cfg(test)]
mod sync_pause_contract {
    use std::sync::atomic;

    use pepper_sync::wallet::SyncMode;
    use zcash_protocol::PoolType;
    use zcash_protocol::value::Zatoshis;

    use crate::data::receivers::{Receiver, transaction_request_from_receivers};
    use crate::lightclient::LightClient;
    use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;
    use crate::wallet::LightWallet;
    use crate::wallet::keys::unified::ReceiverSelection;

    /// An address belonging to a different wallet, so the send is external.
    fn external_address(pool: PoolType) -> zcash_address::ZcashAddress {
        let mut external_wallet =
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::ABANDON_ART_SEED).build();
        let selection = match pool {
            PoolType::ORCHARD | PoolType::IRONWOOD => ReceiverSelection::orchard_only(),
            PoolType::SAPLING => ReceiverSelection::sapling_only(),
            _ => unimplemented!("only shielded destinations are needed here"),
        };
        let (_, unified_address) = external_wallet
            .generate_unified_address(selection, zip32::AccountId::ZERO)
            .unwrap();
        crate::utils::conversion::address_from_str(
            &unified_address.encode(&external_wallet.chain_type()),
        )
        .unwrap()
    }

    /// A synthetic-wallet client whose sync-mode atomic reads `Running`,
    /// simulating a consumer whose background sync engine is between
    /// batches. No engine task exists, so every mode transition observed
    /// (or leaked) is the code under test's own.
    async fn client_with_running_engine(wallet: LightWallet) -> LightClient {
        let client = LightClient::new_for_test(wallet).await;
        client
            .sync_mode
            .store(SyncMode::Running as u8, atomic::Ordering::Release);
        client
    }

    /// The pause a proposal takes exists for the proposal it stores. A
    /// proposing call that fails stores nothing, so it must restore the
    /// engine on its way out. The imperative discipline pauses first and
    /// error-returns past every resume, leaving a consumer's background
    /// sync silently suspended with nothing pending.
    #[tokio::test]
    async fn failed_proposal_leaves_no_leaked_pause() {
        let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .orchard_note(10_000)
            .build();
        let mut client = client_with_running_engine(wallet).await;
        let request = transaction_request_from_receivers(vec![Receiver::new(
            external_address(PoolType::ORCHARD),
            Zatoshis::const_from_u64(50_000),
            None,
        )])
        .unwrap();

        let result = client
            .propose_send(request, zip32::AccountId::ZERO, None, false)
            .await;

        assert!(result.is_err(), "a 50_000 send from 10_000 must fail");
        assert_eq!(
            client.sync_mode(),
            SyncMode::Running,
            "a failed proposal must leave the engine as it found it"
        );
    }

    /// A shield proposal must not come into existence while the engine is
    /// running: the proposal reads spendable coins the engine mutates.
    /// `propose_send` pauses for exactly this reason. The shield path
    /// never did.
    #[tokio::test]
    async fn shield_proposal_is_never_created_under_a_running_engine() {
        let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .transparent_coin(100_000)
            .build();
        let mut client = client_with_running_engine(wallet).await;

        let result = client.propose_shield(zip32::AccountId::ZERO).await;

        assert!(
            result.is_err() || client.sync_mode() != SyncMode::Running,
            "a shield proposal was created while the engine ran"
        );
    }

    /// `quick_shield` goes further than proposing: it builds and stores
    /// signed transactions, so its first wallet read must already run
    /// under a pause. Simulate the engine mid-batch by holding the
    /// wallet write lock: the call blocks on that lock, so if the mode
    /// still reads `Running` while the call is in flight, the build began
    /// without pausing the engine first.
    #[tokio::test]
    async fn quick_shield_never_builds_under_a_running_engine() {
        let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .transparent_coin(100_000)
            .build();
        let mut client = client_with_running_engine(wallet).await;
        let sync_mode = client.sync_mode.clone();
        let wallet_handle = client.wallet().clone();
        let engine_holds_wallet = wallet_handle.write().await;

        let call = tokio::spawn(async move {
            let _offline_transmission_failure = client.quick_shield(zip32::AccountId::ZERO).await;
        });
        // The current-thread runtime polls the spawned call until it
        // blocks on the held wallet lock.
        tokio::task::yield_now().await;

        assert_ne!(
            SyncMode::from_atomic_u8(sync_mode).unwrap(),
            SyncMode::Running,
            "quick_shield began its wallet reads while the engine ran"
        );

        drop(engine_holds_wallet);
        call.await.unwrap();
    }

    /// The send-all sizing (the `max_send_value` trial proposals) is
    /// `propose_send_all`'s first wallet read and must already run under
    /// a pause. Simulate the engine mid-batch by holding the wallet
    /// write lock: the sizing blocks on that lock, so if the engine's mode
    /// still reads `Running` while the call is in flight, the call began
    /// its reads without pausing the engine first.
    #[tokio::test]
    async fn send_all_sizing_runs_under_pause() {
        let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .orchard_note(100_000)
            .build();
        let mut client = client_with_running_engine(wallet).await;
        let sync_mode = client.sync_mode.clone();
        let wallet_handle = client.wallet().clone();
        let engine_holds_wallet = wallet_handle.write().await;

        let address = external_address(PoolType::ORCHARD);
        let call = tokio::spawn(async move {
            client
                .propose_send_all(address, false, None, zip32::AccountId::ZERO, None)
                .await
        });
        // The current-thread runtime polls the spawned call until it
        // blocks on the held wallet lock.
        tokio::task::yield_now().await;

        assert_ne!(
            SyncMode::from_atomic_u8(sync_mode).unwrap(),
            SyncMode::Running,
            "the send-all sizing began while the engine ran"
        );

        drop(engine_holds_wallet);
        call.await.unwrap().unwrap();
    }

    /// A request against a running-engine client that a proposal succeeds
    /// for, shared by the stored-pause protocol tests below.
    async fn client_with_stored_proposal() -> LightClient {
        let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .orchard_note(100_000)
            .build();
        let mut client = client_with_running_engine(wallet).await;
        let request = transaction_request_from_receivers(vec![Receiver::new(
            external_address(PoolType::ORCHARD),
            Zatoshis::const_from_u64(10_000),
            None,
        )])
        .unwrap();
        client
            .propose_send(request, zip32::AccountId::ZERO, None, false)
            .await
            .unwrap();
        client
    }

    /// A successful proposal holds its pause for as long as the
    /// proposal is stored: the engine stays paused, so the state the
    /// proposal selected against cannot shift before the send builds it.
    #[tokio::test]
    async fn proposing_holds_the_pause_for_the_stored_proposal() {
        let client = client_with_stored_proposal().await;

        assert_eq!(
            client.sync_mode(),
            SyncMode::Paused,
            "a stored proposal must hold the engine paused"
        );
    }

    /// Clearing a stored proposal (the decline path of the two-phase
    /// send) restores the engine to the mode it held before proposing.
    /// Previously the pause outlived the declined proposal until some
    /// later send opted into resuming.
    #[tokio::test]
    async fn clearing_the_proposal_restores_the_engine() {
        let mut client = client_with_stored_proposal().await;

        client.clear_proposal().await;

        assert_eq!(
            client.sync_mode(),
            SyncMode::Running,
            "declining a proposal must restore the engine"
        );
    }

    /// An Indexerless send attempt fails before consuming the stored
    /// proposal (ADR 0006), so the proposal and the pause guarding
    /// it survive for retry once an Indexer is configured.
    #[tokio::test]
    async fn offline_send_failure_keeps_the_proposal_guarded() {
        let mut client = client_with_stored_proposal().await;

        let result = client.send_stored_proposal(true).await;

        assert!(result.is_err(), "an Indexerless send must fail");
        assert_eq!(
            client.sync_mode(),
            SyncMode::Paused,
            "a send that consumed nothing must release nothing"
        );
    }
}
