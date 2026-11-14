//! `LightClient` function `do_propose` generates a proposal to send to specified addresses.
//!
//! # A stored proposal holds sync paused
//!
//! Every `propose_*` method stores the proposal it creates and pauses a
//! running sync engine for as long as the proposal is stored, so the wallet
//! state it selected against cannot shift under it. The pause ends only when
//! the consumer settles the proposal, one of:
//!
//! - **confirm** it with [`LightClient::send_stored_proposal`] or
//!   [`LightClient::calculate_stored_proposal`], or
//! - **decline** it with [`LightClient::clear_proposal`].
//!
//! Nothing else resumes the engine: a continuous sync session under an
//! unsettled proposal stops scanning and stops publishing status until one
//! of those is called (issue #3104). A consumer that only wants to show a
//! fee must not propose. The previews [`LightClient::preview_send`],
//! [`LightClient::preview_send_all`] and [`LightClient::preview_shield`]
//! return the proposal without storing it, and [`LightClient::max_send_value`]
//! its sendable amount: none of them pauses anything.

use zcash_address::ZcashAddress;
use zcash_client_backend::data_api::error::Error as ProposalError;
use zcash_client_backend::zip321::TransactionRequest;
use zcash_protocol::value::Zatoshis;

use crate::data::proposal::OpReturnProposal;
use crate::data::proposal::ProportionalFeeProposal;
use crate::data::proposal::ProportionalFeeShieldProposal;
use crate::data::proposal::ZingoProposal;
use crate::data::receivers::Receiver;
use crate::data::receivers::transaction_request_from_receivers;
use crate::lightclient::LightClient;
use crate::lightclient::error::{LightClientError, SendError};
use crate::wallet::error::ProposeSendError;
use crate::wallet::error::ProposeShieldError;
use crate::wallet::error::WalletError;
use crate::wallet::propose::recipient_amount;
use crate::wallet::transparent::OpReturnData;

impl LightClient {
    /// Creates and stores a proposal from a transaction request.
    ///
    /// Pauses the sync engine before the first wallet read and holds that
    /// pause beside the stored proposal, so the state the proposal
    /// selected against cannot shift before the send builds it. A proposal
    /// that fails to come into existence releases the pause on the way
    /// out, restoring the engine to the mode it was found in.
    ///
    /// The caller must settle the stored proposal to let sync resume:
    /// confirm it with [`Self::send_stored_proposal`] or
    /// [`Self::calculate_stored_proposal`], or decline it with
    /// [`Self::clear_proposal`]. To show a fee without proposing, use
    /// [`Self::preview_send`] (see the module docs).
    pub async fn propose_send(
        &mut self,
        request: TransactionRequest,
        account_id: zip32::AccountId,
    ) -> Result<ProportionalFeeProposal, ProposeSendError> {
        let minted = self.hold_proposal_pause();
        let result = {
            let mut wallet = self.wallet().write().await;
            wallet
                .create_send_proposal(request, account_id)
                .inspect(|proposal| {
                    wallet.store_proposal(ZingoProposal::Send {
                        proposal: proposal.clone(),
                        sending_account: account_id,
                    });
                })
        };
        if result.is_err() && minted {
            self.release_proposal_pause(true);
        }
        result
    }

    /// Creates and stores a proposal that sends all shielded funds of
    /// `account_id` to `address`, under the same pause as
    /// [`Self::propose_send`], which the caller settles the same way. To
    /// show the fee and amount without proposing, use
    /// [`Self::preview_send_all`].
    pub async fn propose_send_all(
        &mut self,
        address: ZcashAddress,
        memo: Option<zcash_protocol::memo::MemoBytes>,
        account_id: zip32::AccountId,
    ) -> Result<ProportionalFeeProposal, ProposeSendError> {
        let minted = self.hold_proposal_pause();
        let result = {
            let mut wallet = self.wallet().write().await;
            wallet
                .create_send_all_proposal(address, memo, account_id)
                .inspect(|proposal| {
                    wallet.store_proposal(ZingoProposal::Send {
                        proposal: proposal.clone(),
                        sending_account: account_id,
                    });
                })
        };
        if result.is_err() && minted {
            self.release_proposal_pause(true);
        }
        result
    }

    /// Creates and stores a proposal for shielding all transparent funds,
    /// under the same stored-proposal pause as [`Self::propose_send`],
    /// which the caller settles the same way. To show the shield fee
    /// without proposing, use [`Self::preview_shield`]. The shield path
    /// previously read spendable coins without pausing the engine at all.
    pub async fn propose_shield(
        &mut self,
        account_id: zip32::AccountId,
    ) -> Result<ProportionalFeeShieldProposal, ProposeShieldError> {
        let minted = self.hold_proposal_pause();
        let result = {
            let mut wallet = self.wallet().write().await;
            wallet
                .create_shield_proposal(account_id)
                .inspect(|proposal| {
                    wallet.store_proposal(ZingoProposal::Shield {
                        proposal: proposal.clone(),
                        shielding_account: account_id,
                    });
                })
        };
        if result.is_err() && minted {
            self.release_proposal_pause(true);
        }
        result
    }

    /// Creates and stores a proposal to send `amount` to the transparent
    /// address `recipient` with `data` in an OP_RETURN (null-data) output.
    /// `recipient` is a P2PKH, P2SH, or TEX address. Holds the same
    /// stored-proposal pause as [`Self::propose_send`], which the caller
    /// settles the same way.
    ///
    /// The ephemeral source address is derived, not reserved. The wallet
    /// reserves it in [`Self::send_stored_proposal`], when the deshield
    /// is built. A dropped proposal leaves no trace in the address book.
    /// See [`OpReturnProposal`] for the two transactions the proposal
    /// describes and the fees it reports.
    pub async fn propose_send_with_op_return(
        &mut self,
        recipient: &str,
        amount: Zatoshis,
        data: OpReturnData,
        account_id: zip32::AccountId,
    ) -> Result<OpReturnProposal, LightClientError> {
        if let Some(deshield_txid) = self.wallet().read().await.pending_op_return_deshield() {
            return Err(SendError::OpReturnResumePending { deshield_txid }.into());
        }
        let minted = self.hold_proposal_pause();
        let result = self
            .create_op_return_proposal(recipient, amount, data, account_id)
            .await;
        match &result {
            Ok(proposal) => self
                .wallet()
                .write()
                .await
                .store_proposal(ZingoProposal::OpReturn(proposal.clone())),
            Err(_) if minted => self.release_proposal_pause(true),
            Err(_) => (),
        }
        result
    }

    /// Builds an [`OpReturnProposal`] without storing it. Derives the next
    /// ephemeral source address, sizes the OP_RETURN send fee, and creates
    /// the deshield proposal for `amount` plus that fee.
    pub(crate) async fn create_op_return_proposal(
        &mut self,
        recipient: &str,
        amount: Zatoshis,
        data: OpReturnData,
        account_id: zip32::AccountId,
    ) -> Result<OpReturnProposal, LightClientError> {
        use zcash_protocol::consensus::Parameters as _;

        let mut wallet = self.wallet().write().await;
        let chain_type = wallet.chain_type();

        let recipient = match ZcashAddress::try_from_encoded(recipient)
            .map_err(|e| SendError::OpReturn(WalletError::ParseError(e)))?
            .convert_if_network::<zcash_keys::address::Address>(chain_type.network_type())
        {
            Ok(zcash_keys::address::Address::Transparent(address)) => address,
            Ok(zcash_keys::address::Address::Tex(hash)) => {
                zcash_transparent::address::TransparentAddress::PublicKeyHash(hash)
            }
            _ => {
                return Err(
                    SendError::OpReturn(WalletError::OpReturnRecipientNotTransparent).into(),
                );
            }
        };

        let (source_address_id, source_address) = wallet
            .derive_refund_addresses(1, account_id)
            .map_err(|e| SendError::OpReturn(WalletError::from(e)))?
            .into_iter()
            .next()
            .expect("a request for one address yields one address");

        let target_height = wallet
            .get_migration_heights()
            .map_err(SendError::OpReturn)?
            .ok_or(SendError::OpReturn(WalletError::NoSyncData))?
            .0;

        let op_return_fee = wallet
            .op_return_send_fee(&recipient, &data, target_height)
            .map_err(SendError::OpReturn)?;

        let deshield_amount = (amount + op_return_fee).ok_or_else(|| {
            SendError::OpReturn(WalletError::TransparentBuild(
                "deshield amount overflows the zatoshi range".to_string(),
            ))
        })?;

        let source_zcash_address = ZcashAddress::try_from_encoded(
            &pepper_sync::keys::transparent::encode_address(&chain_type, source_address),
        )
        .map_err(|e| SendError::OpReturn(WalletError::ParseError(e)))?;

        let deshield_request = transaction_request_from_receivers(vec![Receiver::new(
            source_zcash_address,
            deshield_amount,
            None,
        )])
        .map_err(|e| {
            SendError::OpReturn(WalletError::TransparentBuild(format!(
                "deshield request: {e}"
            )))
        })?;

        let deshield = wallet
            .create_send_proposal(deshield_request, account_id)
            .map_err(SendError::ProposeSendError)?;

        Ok(OpReturnProposal::new(
            deshield,
            account_id,
            source_address_id,
            source_address,
            recipient,
            amount,
            data,
            op_return_fee,
        ))
    }

    /// Creates the proposal [`Self::propose_send`] would store, without
    /// storing it or pausing sync: a display-only read of its fee and
    /// pools for a consumer whose user is still deciding. The proposal
    /// cannot be sent; to send, call `propose_send` with the same request
    /// and settle it (see the module docs). The errors are those of
    /// `propose_send`: a wallet that cannot cover the request returns
    /// `InsufficientFunds`, one that still needs a scan `ScanRequired`.
    pub async fn preview_send(
        &self,
        request: TransactionRequest,
        account_id: zip32::AccountId,
    ) -> Result<ProportionalFeeProposal, ProposeSendError> {
        self.preview(|wallet| wallet.create_send_proposal(request, account_id))
            .await
    }

    /// Creates the proposal [`Self::propose_send_all`] would store, without
    /// storing it or pausing sync, the preview of a send-all: its fee, the
    /// amount the recipient receives and its pools.
    pub async fn preview_send_all(
        &self,
        address: ZcashAddress,
        memo: Option<zcash_protocol::memo::MemoBytes>,
        account_id: zip32::AccountId,
    ) -> Result<ProportionalFeeProposal, ProposeSendError> {
        self.preview(|wallet| wallet.create_send_all_proposal(address, memo, account_id))
            .await
    }

    /// Creates the proposal [`Self::propose_shield`] would store, without
    /// storing it or pausing sync, the preview of a shield: its fee and
    /// the value it shields. The errors are those of `propose_shield`.
    pub async fn preview_shield(
        &self,
        account_id: zip32::AccountId,
    ) -> Result<ProportionalFeeShieldProposal, ProposeShieldError> {
        self.preview(|wallet| wallet.create_shield_proposal(account_id))
            .await
    }

    /// Runs one proposal creation against the wallet and returns what it
    /// made, storing nothing. Creation takes the wallet write lock, as the
    /// input selection traits are `&mut`, and holds it only for the call.
    async fn preview<T, E>(
        &self,
        create: impl FnOnce(&mut crate::wallet::LightWallet) -> Result<T, E>,
    ) -> Result<T, E> {
        create(&mut *self.wallet().write().await)
    }

    /// Returns the maximum value that can be sent from `account_id` to
    /// `address`: the shielded spendable balance less the fee. A wallet
    /// that cannot cover the fee reports zero. A wallet that still needs
    /// a scan returns the `ScanRequired` error. To send that amount, use
    /// [`Self::propose_send_all`] rather than a `propose_send` request.
    /// Stores nothing and pauses nothing.
    pub async fn max_send_value(
        &self,
        address: ZcashAddress,
        account_id: zip32::AccountId,
    ) -> Result<Zatoshis, ProposeSendError> {
        match self.preview_send_all(address, None, account_id).await {
            Ok(proposal) => Ok(recipient_amount(&proposal)),
            Err(ProposeSendError::Proposal(ProposalError::InsufficientFunds { .. })) => {
                Ok(Zatoshis::ZERO)
            }
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod shielding;

/// Migrated from libtonode's `send_all` suite: these guarantees are pure
/// proposal logic over wallet state, so a synthetic wallet replaces the
/// LocalNet round trip.
#[cfg(test)]
mod send_all;

/// Migrated from the libtonode chain_generics simpool instantiations
/// (`simpool_insufficient_{1,10_000}_{orchard,sapling}_to_*` and
/// `simpool_no_fund_1_000_000_to_*`): the insufficient-funds and unfunded
/// propose errors are pure proposal logic over wallet state, so a synthetic
/// wallet replaces the LocalNet environment and its multi-hop funding
/// chain.
#[cfg(test)]
mod simpool;

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
mod pool_matrix;

/// Offline proposal-shape tests over synthetic wallets: shield proposals
/// and note-selection behavior, migrated from LocalNet tests whose
/// assertions were all propose-stage.
#[cfg(test)]
mod proposal_shape;

/// The propose/send family must never read wallet state a running engine
/// could be mutating, and a pause it takes must serve a pending proposal,
/// never outliving one. Each test here pins one way the imperative pause
/// discipline violates that contract. All four run red until the stored
/// pause discipline lands later on this branch.
#[cfg(test)]
mod sync_pause_contract;

#[cfg(test)]
mod op_return;
