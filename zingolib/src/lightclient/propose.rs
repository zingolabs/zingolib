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
