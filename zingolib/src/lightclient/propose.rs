//! LightClient function do_propose generates a proposal to send to specified addresses.

use zcash_address::ZcashAddress;
use zcash_client_backend::zip321::TransactionRequest;
use zcash_primitives::transaction::components::amount::NonNegativeAmount;

use crate::config::ChainType;
use crate::config::ZENNIES_FOR_ZINGO_AMOUNT;
use crate::config::ZENNIES_FOR_ZINGO_DONATION_ADDRESS;
use crate::config::ZENNIES_FOR_ZINGO_REGTEST_ADDRESS;
use crate::config::ZENNIES_FOR_ZINGO_TESTNET_ADDRESS;
use crate::wallet::propose::{ProposeSendError, ProposeShieldError};

use crate::data::proposal::ProportionalFeeProposal;
use crate::data::proposal::ProportionalFeeShieldProposal;
use crate::data::proposal::ZingoProposal;
use crate::data::receivers::transaction_request_from_receivers;
use crate::data::receivers::Receiver;
use crate::lightclient::LightClient;

impl LightClient {
    fn append_zingo_zenny_receiver(&self, receivers: &mut Vec<Receiver>) {
        let zfz_address = match self.config().chain {
            ChainType::Mainnet => ZENNIES_FOR_ZINGO_DONATION_ADDRESS,
            ChainType::Testnet => ZENNIES_FOR_ZINGO_TESTNET_ADDRESS,
            ChainType::Regtest(_) => ZENNIES_FOR_ZINGO_REGTEST_ADDRESS,
        };
        let dev_donation_receiver = Receiver::new(
            crate::utils::conversion::address_from_str(zfz_address).expect("Hard coded str"),
            NonNegativeAmount::from_u64(ZENNIES_FOR_ZINGO_AMOUNT).expect("Hard coded u64."),
            None,
        );
        receivers.push(dev_donation_receiver);
    }

    /// Stores a proposal in the `latest_proposal` field of the LightClient.
    /// This field must be populated in order to then send a transaction.
    async fn store_proposal(&self, proposal: ZingoProposal) {
        let mut latest_proposal_lock = self.latest_proposal.write().await;
        *latest_proposal_lock = Some(proposal);
    }
    /// Creates and stores a proposal from a transaction request.
    pub async fn propose_send(
        &self,
        request: TransactionRequest,
    ) -> Result<ProportionalFeeProposal, crate::wallet::propose::ProposeSendError> {
        let proposal = self.wallet.create_send_proposal(request).await?;
        self.store_proposal(ZingoProposal::Transfer(proposal.clone()))
            .await;
        Ok(proposal)
    }

    /// Creates and stores a proposal for sending all shielded funds to a given address.
    pub async fn propose_send_all(
        &self,
        address: ZcashAddress,
        zennies_for_zingo: bool,
        memo: Option<zcash_primitives::memo::MemoBytes>,
    ) -> Result<ProportionalFeeProposal, ProposeSendError> {
        let spendable_balance = self
            .get_spendable_shielded_balance(address.clone(), zennies_for_zingo)
            .await?;
        if spendable_balance == NonNegativeAmount::ZERO {
            return Err(ProposeSendError::ZeroValueSendAll);
        }
        let mut receivers = vec![Receiver::new(address, spendable_balance, memo)];
        if zennies_for_zingo {
            self.append_zingo_zenny_receiver(&mut receivers);
        }
        let request = transaction_request_from_receivers(receivers)
            .map_err(ProposeSendError::TransactionRequestFailed)?;
        let proposal = self.wallet.create_send_proposal(request).await?;
        self.store_proposal(ZingoProposal::Transfer(proposal.clone()))
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
    ) -> Result<NonNegativeAmount, ProposeSendError> {
        let confirmed_shielded_balance = self
            .wallet
            .confirmed_shielded_balance_excluding_dust()
            .await?;
        let mut receivers = vec![Receiver::new(
            address.clone(),
            confirmed_shielded_balance,
            None,
        )];
        if zennies_for_zingo {
            self.append_zingo_zenny_receiver(&mut receivers);
        }
        let request = transaction_request_from_receivers(receivers)?;
        let failing_proposal = self.wallet.create_send_proposal(request).await;

        let shortfall = match failing_proposal {
            Err(ProposeSendError::Proposal(
                zcash_client_backend::data_api::error::Error::InsufficientFunds {
                    available,
                    required,
                },
            )) => {
                if let Some(shortfall) = required - confirmed_shielded_balance {
                    Ok(shortfall)
                } else {
                    // bugged underflow case, required should always be larger than available balance to cause
                    // insufficient funds error. would suggest discrepancy between `available` and `confirmed_shielded_balance`
                    // returns insufficient funds error with same values from original error for debugging
                    Err(ProposeSendError::Proposal(
                        zcash_client_backend::data_api::error::Error::InsufficientFunds {
                            available,
                            required,
                        },
                    ))
                }
            }
            Err(e) => Err(e),
            Ok(_) => Ok(NonNegativeAmount::ZERO), // in the case there is zero fee and the proposal is successful
        }?;

        (confirmed_shielded_balance - shortfall).ok_or(ProposeSendError::Proposal(
            zcash_client_backend::data_api::error::Error::InsufficientFunds {
                available: confirmed_shielded_balance,
                required: shortfall,
            },
        ))
    }

    /// Creates and stores a proposal for shielding all transparent funds..
    pub async fn propose_shield(
        &self,
    ) -> Result<ProportionalFeeShieldProposal, ProposeShieldError> {
        let proposal = self.wallet.create_shield_proposal().await?;
        self.store_proposal(ZingoProposal::Shield(proposal.clone()))
            .await;
        Ok(proposal)
    }
}

#[cfg(test)]
mod shielding {
    use crate::lightclient::propose::ProposeShieldError;

    async fn create_basic_client() -> crate::lightclient::LightClient {
        crate::lightclient::LightClient::create_unconnected(
            &crate::config::ZingoConfigBuilder::default().create(),
            crate::wallet::WalletBase::MnemonicPhrase(
                crate::testvectors::seeds::HOSPITAL_MUSEUM_SEED.to_string(),
            ),
            0,
        )
        .await
        .unwrap()
    }
    #[tokio::test]
    async fn propose_shield_missing_scan_prerequisite() {
        let basic_client = create_basic_client().await;
        let propose_shield_result = basic_client.wallet.create_shield_proposal().await;
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
        assert_eq!(
            basic_client.wallet.get_transparent_addresses(),
            [zcash_primitives::legacy::TransparentAddress::PublicKeyHash(
                [
                    161, 138, 222, 242, 254, 121, 71, 105, 93, 131, 177, 31, 59, 185, 120, 148,
                    255, 189, 198, 33
                ]
            )]
        );
    }
}
