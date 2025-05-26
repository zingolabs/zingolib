//! LightClient function do_propose generates a proposal to send to specified addresses.

use zcash_address::ZcashAddress;
use zcash_client_backend::zip321::TransactionRequest;
use zcash_protocol::value::Zatoshis;

use crate::config::ChainType;
use crate::config::ZENNIES_FOR_ZINGO_AMOUNT;
use crate::config::ZENNIES_FOR_ZINGO_DONATION_ADDRESS;
use crate::config::ZENNIES_FOR_ZINGO_REGTEST_ADDRESS;
use crate::config::ZENNIES_FOR_ZINGO_TESTNET_ADDRESS;
use crate::data::proposal::ProportionalFeeProposal;
use crate::data::proposal::ProportionalFeeShieldProposal;
use crate::data::proposal::ZingoProposal;
use crate::data::receivers::Receiver;
use crate::data::receivers::transaction_request_from_receivers;
use crate::lightclient::LightClient;
use crate::wallet::error::ProposeSendError;
use crate::wallet::error::ProposeShieldError;

impl LightClient {
    fn append_zingo_zenny_receiver(&self, receivers: &mut Vec<Receiver>) {
        let zfz_address = match self.config().chain {
            ChainType::Mainnet => ZENNIES_FOR_ZINGO_DONATION_ADDRESS,
            ChainType::Testnet => ZENNIES_FOR_ZINGO_TESTNET_ADDRESS,
            ChainType::Regtest(_) => ZENNIES_FOR_ZINGO_REGTEST_ADDRESS,
        };
        let dev_donation_receiver = Receiver::new(
            crate::utils::conversion::address_from_str(zfz_address).expect("Hard coded str"),
            Zatoshis::from_u64(ZENNIES_FOR_ZINGO_AMOUNT).expect("Hard coded u64."),
            None,
        );
        receivers.push(dev_donation_receiver);
    }

    /// Stores a proposal in the `latest_proposal` field of the LightClient.
    /// This field must be populated in order to then create and transmit a transaction.
    async fn store_proposal(&mut self, proposal: ZingoProposal) {
        self.latest_proposal = Some(proposal);
    }

    /// Creates and stores a proposal from a transaction request.
    pub async fn propose_send(
        &mut self,
        request: TransactionRequest,
        account_id: zip32::AccountId,
    ) -> Result<ProportionalFeeProposal, ProposeSendError> {
        let proposal = self
            .wallet
            .lock()
            .await
            .create_send_proposal(request, account_id)
            .await?;
        self.store_proposal(ZingoProposal::Send {
            proposal: proposal.clone(),
            sending_account: account_id,
        })
        .await;

        Ok(proposal)
    }

    /// Creates and stores a proposal for sending all shielded funds from a specified account to a given `address`.
    pub async fn propose_send_all(
        &mut self,
        address: ZcashAddress,
        zennies_for_zingo: bool,
        memo: Option<zcash_primitives::memo::MemoBytes>,
        account_id: zip32::AccountId,
    ) -> Result<ProportionalFeeProposal, ProposeSendError> {
        let spendable_balance = self
            .get_spendable_shielded_balance(address.clone(), zennies_for_zingo, account_id)
            .await?;
        if spendable_balance == Zatoshis::ZERO {
            return Err(ProposeSendError::ZeroValueSendAll);
        }
        let mut receivers = vec![Receiver::new(address, spendable_balance, memo)];
        if zennies_for_zingo {
            self.append_zingo_zenny_receiver(&mut receivers);
        }
        let request = transaction_request_from_receivers(receivers)
            .map_err(ProposeSendError::TransactionRequestFailed)?;
        let proposal = self
            .wallet
            .lock()
            .await
            .create_send_proposal(request, account_id)
            .await?;
        self.store_proposal(ZingoProposal::Send {
            proposal: proposal.clone(),
            sending_account: account_id,
        })
        .await;

        Ok(proposal)
    }

    /// Returns the total confirmed shielded balance minus any fees required to send those funds to
    /// a given address
    /// Take zennies_for_zingo flag that if set true, will create a receiver of 1_000_000 ZAT at the
    /// ZingoLabs developer address.
    ///
    /// # Error
    ///
    /// Will return an error if this method fails to calculate the total wallet balance or create the
    /// proposal needed to calculate the fee
    // TODO: move spendable balance and create proposal to wallet layer
    pub async fn get_spendable_shielded_balance(
        &self,
        address: ZcashAddress,
        zennies_for_zingo: bool,
        account_id: zip32::AccountId,
    ) -> Result<Zatoshis, ProposeSendError> {
        let mut wallet = self.wallet.lock().await;
        let confirmed_balance = wallet
            .confirmed_shielded_balance_excluding_dust(account_id)
            .await?;
        let mut spendable_balance = confirmed_balance;

        loop {
            let mut receivers = vec![Receiver::new(address.clone(), spendable_balance, None)];
            if zennies_for_zingo {
                self.append_zingo_zenny_receiver(&mut receivers);
            }
            let request = transaction_request_from_receivers(receivers)?;
            let trial_proposal = wallet.create_send_proposal(request, account_id).await;

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

    /// Creates and stores a proposal for shielding all transparent funds..
    pub async fn propose_shield(
        &mut self,
        account_id: zip32::AccountId,
    ) -> Result<ProportionalFeeShieldProposal, ProposeShieldError> {
        let proposal = self
            .wallet
            .lock()
            .await
            .create_shield_proposal(account_id)
            .await?;
        self.store_proposal(ZingoProposal::Shield {
            proposal: proposal.clone(),
            shielding_account: account_id,
        })
        .await;

        Ok(proposal)
    }
}

#[cfg(test)]
mod shielding {
    use bip0039::Mnemonic;
    use pepper_sync::sync::SyncConfig;
    use zcash_protocol::consensus::Parameters;

    use crate::{
        config::ZingoConfigBuilder,
        lightclient::LightClient,
        wallet::{LightWallet, WalletBase, WalletSettings, error::ProposeShieldError},
    };

    fn create_basic_client() -> LightClient {
        let config = ZingoConfigBuilder::default().create();
        LightClient::create_from_wallet(
            LightWallet::new(
                config.chain,
                WalletBase::Mnemonic {
                    mnemonic: Mnemonic::from_phrase(
                        testvectors::seeds::HOSPITAL_MUSEUM_SEED.to_string(),
                    )
                    .unwrap(),
                    no_of_accounts: 1.try_into().unwrap(),
                },
                0.into(),
                WalletSettings {
                    sync_config: SyncConfig {
                        transparent_address_discovery:
                            pepper_sync::sync::TransparentAddressDiscovery::minimal(),
                    },
                },
            )
            .unwrap(),
            config,
            true,
        )
        .unwrap()
    }
    #[tokio::test]
    async fn propose_shield_missing_scan_prerequisite() {
        let basic_client = create_basic_client();
        let propose_shield_result = basic_client
            .wallet
            .lock()
            .await
            .create_shield_proposal(zip32::AccountId::ZERO)
            .await;
        match propose_shield_result {
            Err(ProposeShieldError::Component(
                zcash_client_backend::data_api::error::Error::ScanRequired,
            )) => true,
            _ => panic!("Unexpected error state!"),
        };
    }
    #[tokio::test]
    async fn get_transparent_addresses() {
        let basic_client = create_basic_client();
        let network = basic_client.wallet.lock().await.network;

        // TODO: store t addrs as concrete types instead of encoded
        let transparent_addresses = basic_client
            .wallet
            .lock()
            .await
            .transparent_addresses
            .values()
            .map(|address| {
                Ok(zcash_address::ZcashAddress::try_from_encoded(address)?
                    .convert_if_network::<zcash_primitives::legacy::TransparentAddress>(
                        network.network_type(),
                    )
                    .expect("incorrect network should be checked on wallet load"))
            })
            .collect::<Result<Vec<_>, zcash_address::ParseError>>()
            .unwrap();

        assert_eq!(
            transparent_addresses,
            [zcash_primitives::legacy::TransparentAddress::PublicKeyHash(
                [
                    161, 138, 222, 242, 254, 121, 71, 105, 93, 131, 177, 31, 59, 185, 120, 148,
                    255, 189, 198, 33
                ]
            )]
        );
    }
}
